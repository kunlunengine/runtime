//! Tokio-backed JavaScriptCore host primitives for Kunlun Runtime.

mod builtins;
mod host;

pub use builtins::{
    BUILTIN_MODULES, BuiltinModuleDescriptor, TYPESCRIPT_DECLARATIONS, is_builtin_specifier,
};
pub use host::HostPermissions;

use kunlun_jsc::{DeferredPromise, JscError, JscVm};
use std::cell::RefCell;
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::time::Instant;

static NEXT_EVALUATION_ID: AtomicU64 = AtomicU64::new(1);

pub const EVENT_LOOP_BACKEND: &str = "caller-provided Tokio runtime";

#[derive(Debug)]
pub enum RuntimeError {
    Jsc(JscError),
    HostInitialization(String),
}

impl Display for RuntimeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Jsc(error) => Display::fmt(error, formatter),
            Self::HostInitialization(error) => {
                write!(formatter, "could not initialize host services: {error}")
            }
        }
    }
}

impl Error for RuntimeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Jsc(error) => Some(error),
            Self::HostInitialization(_) => None,
        }
    }
}

impl From<JscError> for RuntimeError {
    fn from(error: JscError) -> Self {
        Self::Jsc(error)
    }
}

/// A thread-affine JSC isolate driven by the caller's Tokio runtime.
///
/// Promise handles are kept in dispatcher fields declared before the VM so
/// they are dropped before the JSC context.
pub struct TokioIsolate {
    timers: TimerDispatcher,
    host: host::HostDispatcher,
    vm: JscVm,
}

impl TokioIsolate {
    pub fn new(name: &str) -> Result<Self, RuntimeError> {
        Self::new_with_permissions(name, HostPermissions::none())
    }

    pub fn new_with_permissions(
        name: &str,
        permissions: HostPermissions,
    ) -> Result<Self, RuntimeError> {
        let timers = TimerDispatcher::new();
        let mut vm = JscVm::new(name)?;
        let host = host::HostDispatcher::new(permissions)
            .map_err(|error| RuntimeError::HostInitialization(error.to_string()))?;
        let pending_timers = Rc::clone(&timers.pending);

        vm.install_sleep_scheduler(move |duration, promise| {
            pending_timers.borrow_mut().push(PendingTimer {
                deadline: Instant::now().checked_add(duration),
                promise,
            });
        })?;
        host.install(&vm)?;
        builtins::install_builtin_modules(&mut vm)?;

        Ok(Self { timers, host, vm })
    }

    pub fn evaluate(&mut self, source: &str, source_url: &str) -> Result<String, RuntimeError> {
        self.vm.evaluate(source, source_url).map_err(Into::into)
    }

    /// Evaluates JavaScript as the body of an async function.
    ///
    /// The body may use `await`, call the Promise-returning `sleep(ms)` host
    /// function, and return a value. ESM/top-level-await support is a separate
    /// module-loader milestone. This bootstrap API has no execution deadline:
    /// cancelling the Rust future cannot preempt synchronous JavaScript.
    pub async fn evaluate_async_body(
        &mut self,
        source: &str,
        source_url: &str,
    ) -> Result<String, RuntimeError> {
        let Self { timers, host, vm } = self;
        run_async_body(vm, timers, host, source, source_url).await
    }
}

struct TimerDispatcher {
    pending: Rc<RefCell<Vec<PendingTimer>>>,
}

struct PendingTimer {
    deadline: Option<Instant>,
    promise: DeferredPromise,
}

impl TimerDispatcher {
    fn new() -> Self {
        Self {
            pending: Rc::new(RefCell::new(Vec::new())),
        }
    }

    fn settle_expired(&self) -> Result<(), JscError> {
        let now = Instant::now();
        let ready = {
            let mut pending = self.pending.borrow_mut();
            let mut ready = Vec::new();
            let mut index = 0;
            while index < pending.len() {
                if pending[index]
                    .deadline
                    .is_none_or(|deadline| deadline <= now)
                {
                    ready.push(pending.swap_remove(index));
                } else {
                    index += 1;
                }
            }
            ready
        };

        for timer in ready {
            if timer.deadline.is_some() {
                timer.promise.resolve_undefined()?;
            } else {
                timer
                    .promise
                    .reject_message("sleep duration exceeds the host clock range")?;
            }
        }
        Ok(())
    }

    fn next_deadline(&self) -> Option<Instant> {
        self.pending
            .borrow()
            .iter()
            .filter_map(|timer| timer.deadline)
            .min()
    }
}

struct AsyncStateCleanup<'a> {
    vm: &'a mut JscVm,
    state: String,
    armed: bool,
}

impl<'a> AsyncStateCleanup<'a> {
    fn new(vm: &'a mut JscVm, state: String) -> Self {
        Self {
            vm,
            state,
            armed: true,
        }
    }

    fn vm(&mut self) -> &mut JscVm {
        self.vm
    }

    fn cleanup(&mut self) {
        cleanup_state(self.vm, &self.state);
        self.armed = false;
    }
}

impl Drop for AsyncStateCleanup<'_> {
    fn drop(&mut self) {
        if self.armed {
            cleanup_state(self.vm, &self.state);
        }
    }
}

async fn run_async_body(
    vm: &mut JscVm,
    timers: &TimerDispatcher,
    host: &mut host::HostDispatcher,
    source: &str,
    source_url: &str,
) -> Result<String, RuntimeError> {
    let id = NEXT_EVALUATION_ID.fetch_add(1, Ordering::Relaxed);
    let state = format!("__kunlunAsyncState{id}");
    let wrapper = format!(
        "(() => {{\n\
           const __state = globalThis.{state} = {{ done: false, value: undefined, error: undefined }};\n\
           (async () => {{\n\
             try {{\n\
               const __value = await (async () => {{\n{source}\n}})();\n\
               __state.value = String(__value);\n\
             }} catch (__error) {{\n\
               __state.error = String(__error) + '\\n' + String(__error?.stack ?? '');\n\
             }} finally {{\n\
               __state.done = true;\n\
             }}\n\
           }})();\n\
         }})();"
    );

    let mut cleanup = AsyncStateCleanup::new(vm, state);
    if let Err(error) = cleanup.vm().evaluate(&wrapper, source_url) {
        return Err(error.into());
    }

    let result = async {
        loop {
            timers.settle_expired()?;
            host.settle_completions()?;

            let poll_source = format!("globalThis.{}.done", cleanup.state);
            let done = cleanup.vm().evaluate(&poll_source, "kunlun:async-poll")?;
            if done == "true" {
                break;
            }
            if let Some(deadline) = timers.next_deadline() {
                let _ = tokio::time::timeout_at(deadline, host.wait_for_completion()).await;
            } else {
                host.wait_for_completion().await;
            }
        }
        let error_check = format!("globalThis.{}.error !== undefined", cleanup.state);
        let has_error = cleanup.vm().evaluate(&error_check, "kunlun:async-result")? == "true";
        if has_error {
            let error_source = format!("globalThis.{}.error", cleanup.state);
            cleanup
                .vm()
                .evaluate(&error_source, "kunlun:async-result")
                .map_err(RuntimeError::Jsc)
                .and_then(|message| Err(RuntimeError::Jsc(JscError::Exception(message))))
        } else {
            let value_source = format!("globalThis.{}.value", cleanup.state);
            cleanup
                .vm()
                .evaluate(&value_source, "kunlun:async-result")
                .map_err(RuntimeError::Jsc)
        }
    }
    .await;
    cleanup.cleanup();
    result
}

fn cleanup_state(vm: &mut JscVm, state: &str) {
    let _ = vm.evaluate(
        &format!("delete globalThis.{state}"),
        "kunlun:async-cleanup",
    );
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::Duration;
    use tokio::runtime::{Builder, Runtime};

    fn test_runtime() -> Runtime {
        Builder::new_current_thread().enable_all().build().unwrap()
    }

    #[test]
    fn awaits_native_promise_jobs() {
        let runtime = test_runtime();
        let mut isolate = TokioIsolate::new("promise-test").unwrap();
        let value = runtime
            .block_on(isolate.evaluate_async_body(
                "const value = await Promise.resolve(21); return value * 2;",
                "test:///promise.js",
            ))
            .unwrap();
        assert_eq!(value, "42");
    }

    #[test]
    fn evaluates_multiple_async_bodies_in_one_isolate() {
        let runtime = test_runtime();
        let mut isolate = TokioIsolate::new("repeated-evaluation-test").unwrap();
        let first = runtime
            .block_on(isolate.evaluate_async_body("return 'first';", "test:///first.js"))
            .unwrap();
        let second = runtime
            .block_on(isolate.evaluate_async_body("return 'second';", "test:///second.js"))
            .unwrap();
        assert_eq!(first, "first");
        assert_eq!(second, "second");
    }

    #[test]
    fn cancellation_removes_async_state() {
        let runtime = test_runtime();
        let mut isolate = TokioIsolate::new("cancelled-evaluation-test").unwrap();
        let result = runtime.block_on(async {
            tokio::time::timeout(
                Duration::from_millis(10),
                isolate.evaluate_async_body("await new Promise(() => {});", "test:///cancelled.js"),
            )
            .await
        });
        assert!(result.is_err(), "evaluation unexpectedly completed");
        let state_count = isolate
            .evaluate(
                "Object.keys(globalThis).filter(key => key.startsWith('__kunlunAsyncState')).length",
                "test:///cancelled-state.js",
            )
            .unwrap();
        assert_eq!(state_count, "0");
    }

    #[test]
    fn evaluates_and_drops_isolate_inside_tokio_runtime() {
        let runtime = test_runtime();
        runtime.block_on(async {
            let mut isolate = TokioIsolate::new("nested-runtime-test").unwrap();
            let value = isolate
                .evaluate_async_body("await sleep(1); return 'ok';", "test:///nested.js")
                .await
                .unwrap();
            assert_eq!(value, "ok");
        });
    }

    #[test]
    fn tokio_timer_resolves_jsc_promise_in_order() {
        let runtime = test_runtime();
        let mut isolate = TokioIsolate::new("timer-test").unwrap();
        let value = runtime
            .block_on(isolate.evaluate_async_body(
                "const order = ['before'];\n\
                 const timer = sleep(5).then(() => order.push('after'));\n\
                 order.push('middle');\n\
                 await timer;\n\
                 return order.join(',');",
                "test:///timer.js",
            ))
            .unwrap();
        assert_eq!(value, "before,middle,after");
    }

    #[test]
    fn reports_async_javascript_exceptions() {
        let runtime = test_runtime();
        let mut isolate = TokioIsolate::new("async-error-test").unwrap();
        let error = runtime
            .block_on(isolate.evaluate_async_body(
                "await sleep(1); throw new Error('async boom');",
                "test:///async-error.js",
            ))
            .unwrap_err();
        assert!(
            matches!(&error, RuntimeError::Jsc(JscError::Exception(message)) if message.contains("async boom")),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn reads_text_through_kunlun_fs_with_explicit_permission() {
        let runtime = test_runtime();
        let root = std::env::temp_dir().join(format!(
            "kunlun-runtime-fs-test-{}-{}",
            std::process::id(),
            NEXT_EVALUATION_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("message.txt");
        std::fs::write(&file, "hello from kunlun:fs").unwrap();
        let permissions = HostPermissions::none().allow_read_root(&root).unwrap();
        let mut isolate = TokioIsolate::new_with_permissions("fs-test", permissions).unwrap();
        let path = serde_json::to_string(file.to_str().unwrap()).unwrap();
        let value = runtime
            .block_on(isolate.evaluate_async_body(
                &format!(
                    "const fs = await kunlun.import('kunlun:fs'); return await fs.readTextFile({path});"
                ),
                "test:///fs.js",
            ))
            .unwrap();
        assert_eq!(value, "hello from kunlun:fs");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_text_files_over_the_bootstrap_limit() {
        let runtime = test_runtime();
        let root = std::env::temp_dir().join(format!(
            "kunlun-runtime-large-fs-test-{}-{}",
            std::process::id(),
            NEXT_EVALUATION_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("large.txt");
        std::fs::write(&file, vec![b'a'; host::MAX_HTTP_RESPONSE_BYTES + 1]).unwrap();
        let permissions = HostPermissions::none().allow_read_root(&root).unwrap();
        let mut isolate = TokioIsolate::new_with_permissions("large-fs-test", permissions).unwrap();
        let path = serde_json::to_string(file.to_str().unwrap()).unwrap();
        let error = runtime
            .block_on(isolate.evaluate_async_body(
                &format!(
                    "const fs = await kunlun.import('kunlun:fs'); return await fs.readTextFile({path});"
                ),
                "test:///large-fs.js",
            ))
            .unwrap_err();
        assert!(
            matches!(&error, RuntimeError::Jsc(JscError::Exception(message)) if message.contains("file exceeds")),
            "unexpected error: {error:?}"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_host_calls_above_the_in_flight_limit() {
        let runtime = test_runtime();
        let mut isolate = TokioIsolate::new("host-call-limit-test").unwrap();
        let value = runtime
            .block_on(isolate.evaluate_async_body(
                &format!(
                    "const fs = await kunlun.import('kunlun:fs');\
                     const calls = Array.from(\
                       {{ length: {} }},\
                       () => fs.readTextFile('/definitely-not-allowed')\
                         .then(() => 'resolved', error => String(error)));\
                     const results = await Promise.all(calls);\
                     return results.filter(result => result.includes('too many in-flight')).length;",
                    host::MAX_IN_FLIGHT_HOST_CALLS + 1
                ),
                "test:///host-call-limit.js",
            ))
            .unwrap();
        assert_eq!(value, "1");
    }

    #[test]
    fn sends_http_request_through_completion_channel() {
        let runtime = test_runtime();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request).unwrap();
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 17\r\nConnection: close\r\n\r\nhello from server",
                )
                .unwrap();
        });

        let permissions = HostPermissions::none().allow_net_host("127.0.0.1");
        let mut isolate = TokioIsolate::new_with_permissions("http-test", permissions).unwrap();
        let url = serde_json::to_string(&format!("http://{address}/hello")).unwrap();
        let value = runtime
            .block_on(isolate.evaluate_async_body(
                &format!(
                    "const http = await kunlun.import('kunlun:http');\n\
                     const response = await http.request({url});\n\
                     return response.status + ':' + response.body;"
                ),
                "test:///http.js",
            ))
            .unwrap();
        server.join().unwrap();
        assert_eq!(value, "200:hello from server");
    }

    #[test]
    fn denies_builtin_io_without_capability_grants() {
        let runtime = test_runtime();
        let mut isolate = TokioIsolate::new("denied-fs-test").unwrap();
        let error = runtime
            .block_on(isolate.evaluate_async_body(
                "const fs = await kunlun.import('kunlun:fs'); return await fs.readTextFile('/etc/hosts');",
                "test:///denied-fs.js",
            ))
            .unwrap_err();
        assert!(
            matches!(&error, RuntimeError::Jsc(JscError::Exception(message)) if message.contains("read access denied")),
            "unexpected error: {error:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn denies_symlink_escape_from_read_root() {
        use std::os::unix::fs::symlink;

        let runtime = test_runtime();
        let id = NEXT_EVALUATION_ID.fetch_add(1, Ordering::Relaxed);
        let base = std::env::temp_dir().join(format!(
            "kunlun-runtime-symlink-test-{}-{id}",
            std::process::id()
        ));
        let root = base.join("allowed");
        let outside = base.join("outside.txt");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&outside, "must stay private").unwrap();
        let link = root.join("escape.txt");
        symlink(&outside, &link).unwrap();

        let permissions = HostPermissions::none().allow_read_root(&root).unwrap();
        let mut isolate =
            TokioIsolate::new_with_permissions("symlink-escape-test", permissions).unwrap();
        let path = serde_json::to_string(link.to_str().unwrap()).unwrap();
        let error = runtime
            .block_on(isolate.evaluate_async_body(
                &format!(
                    "const fs = await kunlun.import('kunlun:fs'); return await fs.readTextFile({path});"
                ),
                "test:///symlink-escape.js",
            ))
            .unwrap_err();
        assert!(
            matches!(&error, RuntimeError::Jsc(JscError::Exception(message)) if message.contains("cannot read")),
            "unexpected error: {error:?}"
        );
        std::fs::remove_dir_all(base).unwrap();
    }
}
