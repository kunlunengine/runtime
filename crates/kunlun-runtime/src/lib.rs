//! Tokio-backed JavaScriptCore host primitives for Kunlun Runtime.

mod builtins;
mod host;

pub use builtins::{
    BUILTIN_MODULES, BuiltinModuleDescriptor, TYPESCRIPT_DECLARATIONS, is_builtin_specifier,
};
pub use host::HostPermissions;

use kunlun_jsc::{JscError, JscVm};
use std::cell::RefCell;
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::runtime::{Builder, Runtime};
use tokio::task::LocalSet;

static NEXT_EVALUATION_ID: AtomicU64 = AtomicU64::new(1);

pub const EVENT_LOOP_BACKEND: &str = "Tokio current-thread runtime + LocalSet";

#[derive(Debug)]
pub enum RuntimeError {
    Jsc(JscError),
    EventLoop(std::io::Error),
    HostInitialization(String),
    Timeout(Duration),
}

impl Display for RuntimeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Jsc(error) => Display::fmt(error, formatter),
            Self::EventLoop(error) => {
                write!(formatter, "could not create Tokio event loop: {error}")
            }
            Self::HostInitialization(error) => {
                write!(formatter, "could not initialize host services: {error}")
            }
            Self::Timeout(duration) => {
                write!(
                    formatter,
                    "JavaScript async evaluation timed out after {duration:?}"
                )
            }
        }
    }
}

impl Error for RuntimeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Jsc(error) => Some(error),
            Self::EventLoop(error) => Some(error),
            Self::HostInitialization(_) => None,
            Self::Timeout(_) => None,
        }
    }
}

impl From<JscError> for RuntimeError {
    fn from(error: JscError) -> Self {
        Self::Jsc(error)
    }
}

/// A JSC isolate pinned to one Tokio current-thread event loop.
///
/// `LocalSet` is declared before the VM so outstanding local tasks (and their
/// protected Promise resolvers) are dropped before the JSC context.
pub struct TokioIsolate {
    local: LocalSet,
    host: host::HostDispatcher,
    vm: JscVm,
    runtime: Runtime,
    host_error: Rc<RefCell<Option<JscError>>>,
}

impl TokioIsolate {
    pub fn new(name: &str) -> Result<Self, RuntimeError> {
        Self::new_with_permissions(name, HostPermissions::none())
    }

    pub fn new_with_permissions(
        name: &str,
        permissions: HostPermissions,
    ) -> Result<Self, RuntimeError> {
        let runtime = Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(RuntimeError::EventLoop)?;
        let local = LocalSet::new();
        let mut vm = JscVm::new(name)?;
        let host = host::HostDispatcher::new(permissions)
            .map_err(|error| RuntimeError::HostInitialization(error.to_string()))?;
        let host_error = Rc::new(RefCell::new(None));
        let timer_error = Rc::clone(&host_error);

        vm.install_sleep_scheduler(move |duration, promise| {
            let timer_error = Rc::clone(&timer_error);
            tokio::task::spawn_local(async move {
                tokio::time::sleep(duration).await;
                if let Err(error) = promise.resolve_undefined() {
                    *timer_error.borrow_mut() = Some(error);
                }
            });
        })?;
        host.install(&vm)?;
        builtins::install_builtin_modules(&mut vm)?;

        Ok(Self {
            local,
            host,
            vm,
            runtime,
            host_error,
        })
    }

    pub fn evaluate(&mut self, source: &str, source_url: &str) -> Result<String, RuntimeError> {
        self.vm.evaluate(source, source_url).map_err(Into::into)
    }

    /// Evaluates JavaScript as the body of an async function.
    ///
    /// The body may use `await`, call the Promise-returning `sleep(ms)` host
    /// function, and return a value. ESM/top-level-await support is a separate
    /// module-loader milestone.
    pub fn evaluate_async_body(
        &mut self,
        source: &str,
        source_url: &str,
        timeout: Duration,
    ) -> Result<String, RuntimeError> {
        *self.host_error.borrow_mut() = None;
        let Self {
            local,
            host,
            vm,
            runtime,
            host_error,
        } = self;
        runtime.block_on(local.run_until(async {
            host.start_completion_pump(Rc::clone(host_error));
            run_async_body(vm, source, source_url, timeout, Rc::clone(host_error)).await
        }))
    }
}

async fn run_async_body(
    vm: &mut JscVm,
    source: &str,
    source_url: &str,
    timeout: Duration,
    host_error: Rc<RefCell<Option<JscError>>>,
) -> Result<String, RuntimeError> {
    let id = NEXT_EVALUATION_ID.fetch_add(1, Ordering::Relaxed);
    let state = format!("__kunlunAsyncState{id}");
    let wrapper = format!(
        "const __state = globalThis.{state} = {{ done: false, value: undefined, error: undefined }};\n\
         (async () => {{\n\
           try {{\n\
             const __value = await (async () => {{\n{source}\n}})();\n\
             __state.value = String(__value);\n\
           }} catch (__error) {{\n\
             __state.error = String(__error) + '\\n' + String(__error?.stack ?? '');\n\
           }} finally {{\n\
             __state.done = true;\n\
           }}\n\
         }})();"
    );

    if let Err(error) = vm.evaluate(&wrapper, source_url) {
        cleanup_state(vm, &state);
        return Err(error.into());
    }

    let wait = async {
        loop {
            if let Some(error) = host_error.borrow_mut().take() {
                return Err(RuntimeError::Jsc(error));
            }

            let done = vm.evaluate(&format!("globalThis.{state}.done"), "kunlun:async-poll")?;
            if done == "true" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        Ok(())
    };

    if tokio::time::timeout(timeout, wait).await.is_err() {
        cleanup_state(vm, &state);
        return Err(RuntimeError::Timeout(timeout));
    }

    let has_error = vm.evaluate(
        &format!("globalThis.{state}.error !== undefined"),
        "kunlun:async-result",
    )? == "true";
    let result = if has_error {
        vm.evaluate(&format!("globalThis.{state}.error"), "kunlun:async-result")
            .map_err(RuntimeError::Jsc)
            .and_then(|message| Err(RuntimeError::Jsc(JscError::Exception(message))))
    } else {
        vm.evaluate(&format!("globalThis.{state}.value"), "kunlun:async-result")
            .map_err(RuntimeError::Jsc)
    };
    cleanup_state(vm, &state);
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

    #[test]
    fn awaits_native_promise_jobs() {
        let mut isolate = TokioIsolate::new("promise-test").unwrap();
        let value = isolate
            .evaluate_async_body(
                "const value = await Promise.resolve(21); return value * 2;",
                "test:///promise.js",
                Duration::from_secs(1),
            )
            .unwrap();
        assert_eq!(value, "42");
    }

    #[test]
    fn tokio_timer_resolves_jsc_promise_in_order() {
        let mut isolate = TokioIsolate::new("timer-test").unwrap();
        let value = isolate
            .evaluate_async_body(
                "const order = ['before'];\n\
                 const timer = sleep(5).then(() => order.push('after'));\n\
                 order.push('middle');\n\
                 await timer;\n\
                 return order.join(',');",
                "test:///timer.js",
                Duration::from_secs(1),
            )
            .unwrap();
        assert_eq!(value, "before,middle,after");
    }

    #[test]
    fn reports_async_javascript_exceptions() {
        let mut isolate = TokioIsolate::new("async-error-test").unwrap();
        let error = isolate
            .evaluate_async_body(
                "await sleep(1); throw new Error('async boom');",
                "test:///async-error.js",
                Duration::from_secs(1),
            )
            .unwrap_err();
        assert!(
            matches!(&error, RuntimeError::Jsc(JscError::Exception(message)) if message.contains("async boom")),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn reads_text_through_kunlun_fs_with_explicit_permission() {
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
        let value = isolate
            .evaluate_async_body(
                &format!(
                    "const fs = await kunlun.import('kunlun:fs'); return await fs.readTextFile({path});"
                ),
                "test:///fs.js",
                Duration::from_secs(2),
            )
            .unwrap();
        assert_eq!(value, "hello from kunlun:fs");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sends_http_request_through_completion_channel() {
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
        let value = isolate
            .evaluate_async_body(
                &format!(
                    "const http = await kunlun.import('kunlun:http');\n\
                     const response = await http.request({url});\n\
                     return response.status + ':' + response.body;"
                ),
                "test:///http.js",
                Duration::from_secs(5),
            )
            .unwrap();
        server.join().unwrap();
        assert_eq!(value, "200:hello from server");
    }

    #[test]
    fn denies_builtin_io_without_capability_grants() {
        let mut isolate = TokioIsolate::new("denied-fs-test").unwrap();
        let error = isolate
            .evaluate_async_body(
                "const fs = await kunlun.import('kunlun:fs'); return await fs.readTextFile('/etc/hosts');",
                "test:///denied-fs.js",
                Duration::from_secs(2),
            )
            .unwrap_err();
        assert!(
            matches!(&error, RuntimeError::Jsc(JscError::Exception(message)) if message.contains("read access denied")),
            "unexpected error: {error:?}"
        );
    }
}
