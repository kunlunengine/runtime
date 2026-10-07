//! Serial, thread-affine Fetch entry dispatch. This is not an HTTP transport.
use crate::authority_scope::AuthorityScope;
use crate::{
    AdmittedArtifact, ApplicationAuthority, DEFAULT_WATCHDOG_INTERVAL, RequestContext,
    RuntimeError, RuntimeLimits, RuntimeResourceCounts, TerminationReason, TokioIsolate,
    run_private_callable,
};
use kunlun_jsc::PrivateCallable;
use serde::{Deserialize, Serialize};
use std::cell::Cell;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

const MAX_BODY: usize = 1024 * 1024;
// Decimal byte arrays and JSON escaping have bounded transient overhead.
const MAX_ENVELOPE_BYTES: usize = 8 * MAX_BODY;
const MAX_HEADERS: usize = 256;
const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_URL: usize = 16 * 1024;

/// Plain request data; construction of the profile Request happens on the isolate.
/// `body: None` distinguishes a missing body from an empty body.
#[derive(Debug, Serialize)]
pub struct DispatchRequest {
    pub url: String,
    pub method: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

/// Fully buffered, bounded response. No engine reference or live authority escapes.
/// Header pairs preserve separate Set-Cookie fields. HEAD responses have no body.
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DispatchResponse {
    pub status: u16,
    pub status_text: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchErrorKind {
    Startup,
    InvalidEntry,
    InvalidRequest,
    Handler,
    InvalidResponse,
    ResponseBody,
    Background,
    Deadline,
    Cancelled,
    Retired,
    Engine,
}

/// Deliberately excludes application exception text, caller context and secrets.
#[derive(Debug)]
pub struct DispatchError {
    pub kind: DispatchErrorKind,
}

impl fmt::Display for DispatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fetch dispatch failed: {:?}", self.kind)
    }
}
impl std::error::Error for DispatchError {}

fn error(kind: DispatchErrorKind) -> DispatchError {
    DispatchError { kind }
}

/// Send + Sync plain cancellation state; never carries a JS value.
#[derive(Clone, Debug)]
pub struct DispatchCancellation(Arc<AuthorityScope>);

impl Default for DispatchCancellation {
    fn default() -> Self {
        Self(Arc::new(AuthorityScope::new()))
    }
}
impl DispatchCancellation {
    pub fn cancel(&self) {
        self.0.revoke();
    }
}

/// Owns one admitted application, VM, module cache and persistent module state.
/// A mutable borrow permits exactly one request at a time; there is no queue.
/// Dropping an in-flight invocation retires this dispatcher before another request.
pub struct FetchDispatcher {
    isolate: TokioIsolate,
    control: PrivateCallable,
    authority: ApplicationAuthority,
    budget: Duration,
    retired: Cell<bool>,
}

struct RetireOnDrop<'a> {
    retired: &'a Cell<bool>,
    authority: &'a ApplicationAuthority,
    armed: bool,
}
impl Drop for RetireOnDrop<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.retired.set(true);
            self.authority.revoke();
        }
    }
}

impl FetchDispatcher {
    /// Admission is a type-level prerequisite: no root path or unchecked source
    /// is accepted here. Startup must finish before the caller accepts requests.
    pub async fn load(
        artifact: AdmittedArtifact,
        limits: RuntimeLimits,
    ) -> Result<Self, DispatchError> {
        if limits.execution_timeout.is_zero() || limits.execution_timeout > Duration::from_secs(30)
        {
            return Err(error(DispatchErrorKind::Startup));
        }
        let limits = RuntimeLimits {
            watchdog_interval: limits.watchdog_interval.min(DEFAULT_WATCHDOG_INTERVAL),
            ..limits
        };
        let (entry, sources, _assets, authority) = artifact.into_parts();
        let mut isolate = TokioIsolate::new_with_permissions_and_limits(
            "fetch-entry",
            authority.host_permissions(),
            limits,
        )
        .map_err(|_| error(DispatchErrorKind::Startup))?;
        isolate.authority_scope = Some(
            authority
                .claim_isolate()
                .map_err(|_| error(DispatchErrorKind::Startup))?,
        );
        isolate
            .install_module_sources(sources)
            .map_err(|_| error(DispatchErrorKind::Startup))?;
        let source = format!(
            "(() => {{ 'use strict'; const loadEntry = () => import({});\n{}\n}})()",
            serde_json::to_string(&entry).map_err(|_| error(DispatchErrorKind::Startup))?,
            include_str!("fetch_dispatch.js"),
        );
        // Capture runtime intrinsics before import() can execute entry/TLA.
        // The callable is VM-owned and never published on the application global.
        // The loader validates referrers even for absolute imports: use the
        // admitted canonical entry, not an unregistered synthetic kunlun: URL.
        let control = isolate
            .vm
            .evaluate_private_callable(&source, &entry)
            .map_err(|_| error(DispatchErrorKind::Startup))?;
        let result = tokio::time::timeout(
            limits.execution_timeout,
            run_private_callable(
                &mut isolate.vm,
                &isolate.timers,
                &mut isolate.host,
                control,
                "load",
            ),
        )
        .await
        .map_err(|_| error(DispatchErrorKind::Deadline))?
        .map_err(|failure| match failure {
            RuntimeError::Jsc(value)
                if value.termination_reason() == Some(TerminationReason::DeadlineExceeded) =>
            {
                error(DispatchErrorKind::Deadline)
            }
            _ => error(DispatchErrorKind::Startup),
        })?;
        validate_startup_result(&result)?;
        Ok(Self {
            isolate,
            control,
            authority,
            budget: limits.execution_timeout,
            retired: Cell::new(false),
        })
    }

    pub fn resource_counts(&self) -> RuntimeResourceCounts {
        self.isolate.resource_counts()
    }

    /// Stop admission and revoke all application/request handles.
    pub fn stop(&self) {
        self.retired.set(true);
        self.authority.revoke();
    }

    /// Handler, body consumption and registered background tasks share one
    /// wall-clock budget and the native execution watchdog budget. Cancellation
    /// drops host work, aborts the JS signal when executable, and retires the VM.
    pub async fn dispatch(
        &mut self,
        request: DispatchRequest,
        context: RequestContext,
        cancellation: &DispatchCancellation,
    ) -> Result<DispatchResponse, DispatchError> {
        if self.retired.get() {
            return Err(error(DispatchErrorKind::Retired));
        }
        if request.url.len() > MAX_URL
            || request.method.len() > 32
            || request.headers.len() > MAX_HEADERS
            || request
                .headers
                .iter()
                .map(|(name, value)| name.len() + value.len())
                .sum::<usize>()
                > MAX_HEADER_BYTES
            || request
                .body
                .as_ref()
                .is_some_and(|body| body.len() > MAX_BODY)
        {
            return Err(error(DispatchErrorKind::InvalidRequest));
        }
        let environment = self
            .authority
            .begin_request(context)
            .map_err(|_| error(DispatchErrorKind::Retired))?;
        let application = Arc::clone(
            self.isolate
                .authority_scope
                .as_ref()
                .expect("dispatcher owns an admitted authority"),
        );
        let command = format!(
            "{{\"request\":{},\"environment\":{}}}",
            serde_json::to_string(&request)
                .map_err(|_| error(DispatchErrorKind::InvalidRequest))?,
            environment.projection(),
        );
        let mut retirement = RetireOnDrop {
            retired: &self.retired,
            authority: &self.authority,
            armed: true,
        };
        let _host_scope = self.isolate.host.enter_request(environment.host_scope());
        let invocation = run_private_callable(
            &mut self.isolate.vm,
            &self.isolate.timers,
            &mut self.isolate.host,
            self.control,
            &command,
        );
        let result = tokio::select! {
            biased;
            _ = cancellation.0.cancelled() => Err(error(DispatchErrorKind::Cancelled)),
            _ = application.cancelled() => Err(error(DispatchErrorKind::Cancelled)),
            result = tokio::time::timeout(self.budget, invocation) => match result {
                Err(_) => Err(error(DispatchErrorKind::Deadline)),
                Ok(Err(RuntimeError::Jsc(error_value)))
                    if error_value.termination_reason() == Some(TerminationReason::DeadlineExceeded) =>
                    Err(error(DispatchErrorKind::Deadline)),
                Ok(Err(_)) => Err(error(DispatchErrorKind::Engine)),
                Ok(Ok(value)) => Ok(value),
            },
        };
        // Drop runs the same revocation on caller cancellation.
        environment.revoke();
        let value = result?;
        if value.len() > MAX_ENVELOPE_BYTES {
            return Err(error(DispatchErrorKind::Engine));
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Envelope {
            response: Option<DispatchResponse>,
            error: Option<String>,
        }
        let envelope: Envelope =
            serde_json::from_str(&value).map_err(|_| error(DispatchErrorKind::Engine))?;
        let decoded = match (envelope.response, envelope.error.as_deref()) {
            (Some(response), None) if valid_response(&response) => Ok(response),
            (None, Some(code)) => Err(error(match code {
                "request" => DispatchErrorKind::InvalidRequest,
                "handler" => DispatchErrorKind::Handler,
                "response" => DispatchErrorKind::InvalidResponse,
                "body" => DispatchErrorKind::ResponseBody,
                "background" => DispatchErrorKind::Background,
                _ => return Err(error(DispatchErrorKind::Engine)),
            })),
            _ => return Err(error(DispatchErrorKind::Engine)),
        };
        // Only a completed trusted envelope permits reuse. A suspended JS
        // continuation must never resume inside another request's host scope.
        retirement.armed = false;
        decoded
    }
}

fn validate_startup_result(result: &str) -> Result<(), DispatchError> {
    match result {
        "ready" => Ok(()),
        "invalid-entry" => Err(error(DispatchErrorKind::InvalidEntry)),
        "startup" => Err(error(DispatchErrorKind::Startup)),
        _ => Err(error(DispatchErrorKind::Engine)),
    }
}

#[cfg(test)]
mod startup_tests {
    use super::*;

    #[test]
    fn private_load_results_preserve_startup_and_export_failure_phases() {
        assert!(validate_startup_result("ready").is_ok());
        for (result, kind) in [
            ("invalid-entry", DispatchErrorKind::InvalidEntry),
            ("startup", DispatchErrorKind::Startup),
            ("pending", DispatchErrorKind::Engine),
            ("unexpected", DispatchErrorKind::Engine),
        ] {
            assert_eq!(validate_startup_result(result).unwrap_err().kind, kind);
        }
    }
}

/// JS Response properties are application-mutable. Validate the actual copied
/// output, not the constructor's earlier metadata or a mutable JS getter.
fn valid_response(response: &DispatchResponse) -> bool {
    response.body.len() <= MAX_BODY
        && (200..=599).contains(&response.status)
        && (!matches!(response.status, 204 | 205 | 304) || response.body.is_empty())
        && response.status_text.encode_utf16().count() <= 1024
        && response.status_text.chars().all(http_field_character)
        && response.headers.len() <= MAX_HEADERS
        && response
            .headers
            .iter()
            .map(|(name, value)| name.len() + value.len())
            .sum::<usize>()
            <= MAX_HEADER_BYTES
        && response.headers.iter().all(|(name, value)| {
            !name.is_empty()
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
                && value.chars().all(http_field_character)
        })
}

// Match the Fetch profile's Latin-1 field-value/reason-phrase syntax: HTAB,
// visible ASCII, or obs-text; never CR/LF, NUL, DEL, or Unicode above U+00FF.
fn http_field_character(character: char) -> bool {
    matches!(character, '\t' | '\u{20}'..='\u{7e}' | '\u{80}'..='\u{ff}')
}

#[cfg(test)]
mod metadata_tests {
    use super::*;

    fn response() -> DispatchResponse {
        DispatchResponse {
            status: 200,
            status_text: "OK".into(),
            headers: vec![("content-type".into(), "text/plain".into())],
            body: vec![0, 255],
        }
    }

    #[test]
    fn response_metadata_rechecks_profile_syntax_and_advertised_bounds() {
        assert!(valid_response(&response()));
        for text in ["\r\nX-Injected: yes", "bad\0text", "bad\u{7f}", "非Latin1"] {
            let mut value = response();
            value.status_text = text.into();
            assert!(!valid_response(&value));
        }
        let mut value = response();
        value.status_text = "a".repeat(1024);
        assert!(valid_response(&value));
        value.status_text.push('a');
        assert!(!valid_response(&value));
        value.status_text = "a".repeat(7_000_000);
        assert!(!valid_response(&value));
        value.status_text = "\t\u{ff}".into();
        assert!(valid_response(&value));
    }

    #[test]
    fn header_names_and_values_cannot_inject_controls() {
        for name in ["bad\r\nname", "", "bad name", "unicode-é", "bad:name"] {
            let mut value = response();
            value.headers = vec![(name.into(), "value".into())];
            assert!(!valid_response(&value));
        }
        for field in ["value\nsmuggled", "\r", "bad\0", "bad\u{7f}", "非Latin1"] {
            let mut value = response();
            value.headers = vec![("name".into(), field.into())];
            assert!(!valid_response(&value));
        }
        let mut value = response();
        value.headers = vec![("!#$%&'*+-.^_`|~".into(), "\t\u{ff}".into())];
        assert!(valid_response(&value));
    }

    #[test]
    fn null_body_status_and_byte_limits_are_revalidated() {
        for status in [204, 205, 304] {
            let mut value = response();
            value.status = status;
            assert!(!valid_response(&value));
            value.body.clear();
            assert!(valid_response(&value));
        }
        let mut value = response();
        value.body = vec![0; MAX_BODY + 1];
        assert!(!valid_response(&value));
        let mut value = response();
        value.headers = vec![("name".into(), "value".into()); MAX_HEADERS + 1];
        assert!(!valid_response(&value));
        value.headers = vec![("name".into(), "x".repeat(MAX_HEADER_BYTES))];
        assert!(!valid_response(&value));
    }
}

#[cfg(test)]
mod adapter_tests {
    // These deliberately test the JS adapter without module loading. They are
    // development evidence on system-jsc, NOT admitted-artifact qualification.
    use super::*;
    use crate::{CapabilityRequirements, HostPermissions};
    use std::future::Future;

    async fn adapter(entry: &str) -> FetchDispatcher {
        adapter_with_setup(entry, "").await
    }

    async fn adapter_with_setup(entry: &str, setup: &str) -> FetchDispatcher {
        let authority = ApplicationAuthority::new(
            &HostPermissions::none(),
            &CapabilityRequirements {
                required: vec![],
                optional: vec![],
            },
        );
        let limits = RuntimeLimits {
            execution_timeout: Duration::from_millis(500),
            ..RuntimeLimits::default()
        };
        let mut isolate = TokioIsolate::new_with_permissions_and_limits(
            "dispatch-adapter-test",
            authority.host_permissions(),
            limits,
        )
        .unwrap();
        isolate.authority_scope = Some(authority.claim_isolate().unwrap());
        let source = format!(
            "(() => {{ 'use strict'; {setup}\nconst loadEntry = async () => ({{default: {entry}}});\n{}\n}})()",
            include_str!("fetch_dispatch.js")
        );
        let control = isolate
            .vm
            .evaluate_private_callable(&source, "test:///dispatch-adapter.js")
            .unwrap();
        assert_eq!(
            run_private_callable(
                &mut isolate.vm,
                &isolate.timers,
                &mut isolate.host,
                control,
                "load"
            )
            .await
            .unwrap(),
            "ready"
        );
        FetchDispatcher {
            isolate,
            control,
            authority,
            budget: limits.execution_timeout,
            retired: Cell::new(false),
        }
    }

    fn request(method: &str) -> DispatchRequest {
        DispatchRequest {
            url: "https://service.test/route?q=1".into(),
            method: method.into(),
            headers: vec![],
            body: None,
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn adapter_sync_thenable_and_binary_request() {
        for body in [
            "return new Response('sync');",
            "return {then(resolve) { resolve(new Response('thenable')); }};",
            "return request.bytes().then(bytes => new Response(bytes));",
        ] {
            let mut dispatcher =
                adapter(&format!("{{fetch(request, env, ctx) {{ {body} }} }}")).await;
            let mut input = request("POST");
            input.body = Some(vec![0, 255, 17]);
            let response = dispatcher
                .dispatch(
                    input,
                    RequestContext::new(),
                    &DispatchCancellation::default(),
                )
                .await
                .unwrap();
            assert_eq!(
                response.body,
                if body.contains("sync") {
                    b"sync".to_vec()
                } else if body.contains("thenable") {
                    b"thenable".to_vec()
                } else {
                    vec![0, 255, 17]
                }
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn adapter_response_tasks_and_closed_context() {
        let mut dispatcher = adapter(
            r#"{ async fetch(request, env, ctx) {
              if (request.signal !== ctx.signal) throw Error('signal');
              if ('auth' in env) throw Error('caller leaked');
              if (globalThis.oldContext) {
                let closed = false;
                try { oldContext.waitUntil(Promise.resolve()); } catch (_) { closed = true; }
                if (!closed || globalThis.completed !== 1) throw Error('scope');
              }
              globalThis.oldContext = ctx;
              ctx.waitUntil((async () => { await sleep(0); globalThis.completed = 1; })());
              return new Response(new Uint8Array([0, 255]), {
                status: 202, headers: [['set-cookie', 'a=1'], ['set-cookie', 'b=2']]
              });
            }}"#,
        )
        .await;
        let cancellation = DispatchCancellation::default();
        for _ in 0..2 {
            let mut context = RequestContext::new();
            context.insert("auth", "private");
            let response = dispatcher
                .dispatch(request("GET"), context, &cancellation)
                .await
                .unwrap();
            assert_eq!(response.status, 202);
            assert_eq!(response.body, [0, 255]);
            assert_eq!(response.headers.len(), 2);
            assert_eq!(
                dispatcher.resource_counts(),
                RuntimeResourceCounts::default()
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn adapter_failures_are_typed_and_bounded() {
        for (body, kind) in [
            ("throw Error('private');", DispatchErrorKind::Handler),
            ("return 42;", DispatchErrorKind::InvalidResponse),
            (
                "ctx.waitUntil(Promise.reject(Error('private'))); return new Response('ok');",
                DispatchErrorKind::Background,
            ),
            (
                "for (let i=0; i<33; i++) ctx.waitUntil(Promise.resolve()); return new Response('ok');",
                DispatchErrorKind::Handler,
            ),
            (
                "return new Response(new Uint8Array(1048577));",
                DispatchErrorKind::ResponseBody,
            ),
            (
                "return new Response(new ReadableStream({pull(c) { c.error(Error('private')); }}));",
                DispatchErrorKind::ResponseBody,
            ),
        ] {
            let mut dispatcher =
                adapter(&format!("{{fetch(request, env, ctx) {{ {body} }} }}")).await;
            let error = dispatcher
                .dispatch(
                    request("GET"),
                    RequestContext::new(),
                    &DispatchCancellation::default(),
                )
                .await
                .unwrap_err();
            assert_eq!(error.kind, kind, "{body}");
            assert!(!error.to_string().contains("private"));
            assert_eq!(
                dispatcher.resource_counts(),
                RuntimeResourceCounts::default()
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn adapter_head_cancels_and_bad_request_does_not_invoke_handler() {
        let mut dispatcher = adapter(
            r#"{fetch() {
              globalThis.calls = (globalThis.calls || 0) + 1;
              return new Response(new ReadableStream({
                cancel() { globalThis.cancelledBody = true; }
              }), {status: 201});
            }}"#,
        )
        .await;
        let cancellation = DispatchCancellation::default();
        let mut invalid = request("GET");
        invalid.body = Some(vec![]);
        assert_eq!(
            dispatcher
                .dispatch(invalid, RequestContext::new(), &cancellation)
                .await
                .unwrap_err()
                .kind,
            DispatchErrorKind::InvalidRequest
        );
        let response = dispatcher
            .dispatch(request("HEAD"), RequestContext::new(), &cancellation)
            .await
            .unwrap();
        assert_eq!(response.status, 201);
        assert!(response.body.is_empty());
        assert_eq!(
            dispatcher
                .isolate
                .evaluate("calls === 1 && cancelledBody", "test:///check.js")
                .unwrap(),
            "true"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn adapter_cancellation_and_drop_abort_signal_and_retire() {
        for explicit in [true, false] {
            let mut dispatcher = adapter(
                r#"{fetch(request, env, ctx) {
                  ctx.signal.addEventListener('abort', () => { globalThis.aborted = true; });
                  ctx.waitUntil(new Promise(() => {}));
                  return new Response('pending');
                }}"#,
            )
            .await;
            let cancellation = DispatchCancellation::default();
            {
                let invocation =
                    dispatcher.dispatch(request("GET"), RequestContext::new(), &cancellation);
                tokio::pin!(invocation);
                std::future::poll_fn(|cx| {
                    assert!(invocation.as_mut().poll(cx).is_pending());
                    std::task::Poll::Ready(())
                })
                .await;
                if explicit {
                    cancellation.cancel();
                    assert_eq!(
                        invocation.await.unwrap_err().kind,
                        DispatchErrorKind::Cancelled
                    );
                }
            }
            assert!(dispatcher.retired.get());
            assert_eq!(
                dispatcher.resource_counts(),
                RuntimeResourceCounts::default()
            );
            assert_eq!(
                dispatcher
                    .isolate
                    .evaluate("aborted", "test:///check.js")
                    .unwrap(),
                "true"
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn adapter_application_cannot_reenter_or_close_trusted_dispatch() {
        let mut dispatcher = adapter(
            r#"{fetch(request, env, ctx) {
              globalThis.calls = (globalThis.calls || 0) + 1;
              const nested = () => {
                ctx.waitUntil(new Promise(() => {}));
                throw Error('extra handler invocation');
              };
              if (calls === 1 && typeof globalThis.__kunlunFetchDispatch === 'function')
                globalThis.__kunlunFetchDispatch({}, env).catch(() => {});
              if (calls === 1 && typeof globalThis.__kunlunFetchClose === 'function')
                globalThis.__kunlunFetchClose();
              // Public lookalikes can neither control invocation nor cleanup.
              globalThis.__kunlunFetchDispatch = nested;
              globalThis.__kunlunFetchClose = () => { throw Error('forged cleanup'); };
              globalThis.currentSignal = ctx.signal;
              if (calls === 1) return new Response('first');
              ctx.waitUntil(new Promise(() => {}));
              return new Response('second');
            }}"#,
        )
        .await;
        let cancellation = DispatchCancellation::default();
        assert_eq!(
            dispatcher
                .dispatch(request("GET"), RequestContext::new(), &cancellation)
                .await
                .unwrap()
                .body,
            b"first"
        );
        {
            let invocation =
                dispatcher.dispatch(request("GET"), RequestContext::new(), &cancellation);
            tokio::pin!(invocation);
            std::future::poll_fn(|cx| {
                assert!(invocation.as_mut().poll(cx).is_pending());
                std::task::Poll::Ready(())
            })
            .await;
            cancellation.cancel();
            assert_eq!(
                invocation.await.unwrap_err().kind,
                DispatchErrorKind::Cancelled
            );
        }
        assert_eq!(
            dispatcher
                .isolate
                .evaluate(
                    "calls === 2 && currentSignal.aborted",
                    "test:///private-control.js"
                )
                .unwrap(),
            "true"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn adapter_old_completion_cannot_clear_later_request() {
        let dispatcher = adapter(
            r#"{fetch(request, env, ctx) {
              globalThis.calls = (globalThis.calls || 0) + 1;
              globalThis.currentSignal = ctx.signal;
              ctx.waitUntil(new Promise(resolve => {
                if (calls === 1) globalThis.finishOld = resolve;
              }));
              return new Response(null);
            }}"#,
        )
        .await;
        // Exercise the same private protocol directly to force the otherwise
        // forbidden close/reuse sequence. Production cancellation retires the VM.
        let command = format!(
            "{{\"request\":{},\"environment\":{{\"scope\":0,\"capabilities\":[]}}}}",
            serde_json::to_string(&request("GET")).unwrap()
        );
        let vm = &dispatcher.isolate.vm;
        vm.call_private_callable(dispatcher.control, &command)
            .unwrap();
        crate::checkpoint(vm).unwrap();
        // Defense in depth even for trusted-owner misuse: no second handler.
        assert!(
            vm.call_private_callable(dispatcher.control, &command)
                .is_err()
        );
        assert_eq!(vm.evaluate("calls", "test:///reentry.js").unwrap(), "1");
        vm.call_private_callable(dispatcher.control, "close")
            .unwrap();
        vm.call_private_callable(dispatcher.control, &command)
            .unwrap();
        crate::checkpoint(vm).unwrap();
        vm.evaluate("finishOld()", "test:///old-completion.js")
            .unwrap();
        crate::checkpoint(vm).unwrap();
        assert_eq!(
            vm.call_private_callable(dispatcher.control, "poll")
                .unwrap(),
            "pending"
        );
        vm.call_private_callable(dispatcher.control, "close")
            .unwrap();
        assert_eq!(
            vm.evaluate("currentSignal.aborted && calls === 2", "test:///check.js")
                .unwrap(),
            "true"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn adapter_entry_tampering_cannot_complete_pending_wait_until() {
        let mut dispatcher = adapter(
            r#"(() => {
              Promise.resolve = () => new Promise(resolve => resolve([]));
              Promise.all = () => Promise.resolve([]);
              Promise.prototype.then = function(fulfilled) {
                return Promise.resolve(fulfilled ? fulfilled([]) : []);
              };
              Promise.prototype.constructor = function FakePromise() {};
              Object.freeze = value => value;
              Object.defineProperty = () => {};
              Array.prototype.some = () => false;
              Array.prototype.toJSON = () => [];
              String.prototype.charCodeAt = () => 0;
              ReadableStreamDefaultReader.prototype.read = () => Promise.resolve({done: true});
              return {fetch(request, env, ctx) {
                globalThis.currentSignal = ctx.signal;
                globalThis.frozenContext = Object.isFrozen(ctx);
                ctx.waitUntil(new Promise(resolve => { globalThis.finishTask = resolve; }));
                JSON.stringify = () => '{"response":null,"error":"background"}';
                JSON.parse = () => ({});
                Object.prototype.toJSON = () => ({response: null, error: 'background'});
                return new Response(null);
              }};
            })()"#,
        )
        .await;
        let cancellation = DispatchCancellation::default();
        // Unresolved task must prevent any response even though the application's
        // aggregate promises, prototype then and JSON claim instant completion.
        {
            let invocation =
                dispatcher.dispatch(request("GET"), RequestContext::new(), &cancellation);
            tokio::pin!(invocation);
            std::future::poll_fn(|cx| {
                assert!(invocation.as_mut().poll(cx).is_pending());
                std::task::Poll::Ready(())
            })
            .await;
            cancellation.cancel();
            assert_eq!(
                invocation.await.unwrap_err().kind,
                DispatchErrorKind::Cancelled
            );
        }
        assert_eq!(
            dispatcher
                .isolate
                .evaluate(
                    "frozenContext && currentSignal.aborted",
                    "test:///tampered-check.js"
                )
                .unwrap(),
            "true"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn adapter_entry_tampered_aggregate_still_waits_then_reuses() {
        let mut dispatcher = adapter(
            r#"(() => {
              Promise.resolve = () => new Promise(resolve => resolve([]));
              Promise.all = () => Promise.resolve([]);
              Array.prototype.some = () => false;
              return {fetch(request, env, ctx) {
                globalThis.calls = (globalThis.calls || 0) + 1;
                if (calls === 2 && !globalThis.taskDone) throw Error('released early');
                ctx.waitUntil((async () => {
                  await sleep(0);
                  globalThis.taskDone = true;
                })());
                return new Response(null);
              }};
            })()"#,
        )
        .await;
        for _ in 0..2 {
            dispatcher
                .dispatch(
                    request("GET"),
                    RequestContext::new(),
                    &DispatchCancellation::default(),
                )
                .await
                .unwrap();
        }
        assert_eq!(
            dispatcher
                .isolate
                .evaluate("calls === 2 && taskDone", "test:///waited.js")
                .unwrap(),
            "true"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn adapter_entry_json_tampering_cannot_forge_a_response() {
        let mut dispatcher = adapter(
            r#"(() => {
              JSON.parse = () => ({});
              JSON.stringify = () =>
                '{"response":{"status":200,"status_text":"","headers":[],"body":[]},"error":null}';
              Object.prototype.toJSON = () => ({response: {}, error: null});
              return {fetch() { globalThis.called = true; return new Response(null); }};
            })()"#,
        )
        .await;
        // The mutable profile URL constructor can now fail. Its failure must
        // still be reported through trusted JSON, never the forged response.
        assert_eq!(
            dispatcher
                .dispatch(
                    request("GET"),
                    RequestContext::new(),
                    &DispatchCancellation::default()
                )
                .await
                .unwrap_err()
                .kind,
            DispatchErrorKind::InvalidRequest
        );
        assert_eq!(
            dispatcher
                .isolate
                .evaluate("typeof called", "test:///json-check.js")
                .unwrap(),
            "undefined"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn adapter_snapshots_mutable_response_metadata_once() {
        let mut dispatcher = adapter(
            r#"{fetch() {
              const response = new Response('body', {status: 202, headers: {'x-test': 'snapshot'}});
              const headers = response.headers, body = response.body;
              globalThis.reads = {status: 0, statusText: 0, headers: 0, body: 0};
              for (const [name, value] of Object.entries({status: 202, statusText: 'OK', headers, body}))
                Object.defineProperty(response, name, {get() {
                  if (++reads[name] !== 1) throw Error('read twice');
                  return value;
                }});
              return response;
            }}"#,
        ).await;
        let response = dispatcher
            .dispatch(
                request("GET"),
                RequestContext::new(),
                &DispatchCancellation::default(),
            )
            .await
            .unwrap();
        assert_eq!(response.status, 202);
        assert_eq!(response.status_text, "OK");
        assert!(
            response
                .headers
                .contains(&("x-test".into(), "snapshot".into()))
        );
        assert_eq!(response.body, b"body");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn adapter_abort_survives_mutable_primordials_and_public_close() {
        let mut dispatcher = adapter(
            r#"{fetch(request, env, ctx) {
              globalThis.signal = ctx.signal;
              globalThis.notifications = 0;
              const notify = event => {
                if (!Object.isFrozen(event) || event.target !== signal)
                  throw Error('bad abort event');
                globalThis.notifications++;
              };
              signal.addEventListener('abort', notify, {once: true});
              signal.addEventListener('abort', {handleEvent: notify});
              signal.onabort = notify;
              ctx.waitUntil(new Promise(() => {}));
              Object.freeze = () => { throw Error('modified freeze'); };
              Object.defineProperty = () => { throw Error('modified define'); };
              WeakMap.prototype.get = () => { throw Error('modified get'); };
              WeakMap.prototype.set = () => { throw Error('modified set'); };
              Array.prototype[Symbol.iterator] = () => { throw Error('modified iterator'); };
              Array.prototype.slice = () => { throw Error('modified slice'); };
              Array.prototype.filter = () => [];
              Reflect.apply = () => { throw Error('modified apply'); };
              AbortController.prototype.abort = () => { throw Error('modified abort'); };
              AbortSignal.prototype.removeEventListener = () => { throw Error('modified remove'); };
              globalThis.__kunlunFetchClose = () => { throw Error('forged close'); };
              return null;
            }}"#,
        )
        .await;
        let cancellation = DispatchCancellation::default();
        {
            let invocation =
                dispatcher.dispatch(request("GET"), RequestContext::new(), &cancellation);
            tokio::pin!(invocation);
            std::future::poll_fn(|cx| {
                assert!(invocation.as_mut().poll(cx).is_pending());
                std::task::Poll::Ready(())
            })
            .await;
            cancellation.cancel();
            assert_eq!(
                invocation.await.unwrap_err().kind,
                DispatchErrorKind::Cancelled
            );
        }
        assert_eq!(
            dispatcher
                .isolate
                .evaluate(
                    "signal.aborted && signal.reason.name === 'AbortError' && notifications === 3",
                    "test:///abort-primordials.js"
                )
                .unwrap(),
            "true"
        );
        assert!(dispatcher.retired.get());
        assert_eq!(
            dispatcher.resource_counts(),
            RuntimeResourceCounts::default()
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn adapter_body_reader_lookup_does_not_use_application_instance_methods() {
        let mut dispatcher = adapter(
            r#"{fetch() {
              const response = new Response('trusted-reader');
              response.body.getReader = () => { throw Error('instance reader lookup'); };
              Uint8Array.prototype.slice = () => { throw Error('modified byte slice'); };
              Uint8Array.prototype.set = () => { throw Error('modified byte set'); };
              Uint8Array.prototype[Symbol.iterator] = () => { throw Error('modified byte iterator'); };
              TextEncoder.prototype.encode = () => { throw Error('modified encoder'); };
              return response;
            }}"#,
        ).await;
        let response = dispatcher
            .dispatch(
                request("GET"),
                RequestContext::new(),
                &DispatchCancellation::default(),
            )
            .await
            .unwrap();
        assert_eq!(response.body, b"trusted-reader");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn adapter_sloppy_handler_cannot_reflect_the_private_caller() {
        let mut dispatcher = adapter(
            r#"{fetch: Function('request', 'env', 'ctx',
              "globalThis.reflectedCaller = arguments.callee.caller; return new Response('private');"
            )}"#,
        ).await;
        assert_eq!(
            dispatcher
                .dispatch(
                    request("GET"),
                    RequestContext::new(),
                    &DispatchCancellation::default()
                )
                .await
                .unwrap()
                .body,
            b"private"
        );
        assert_eq!(
            dispatcher
                .isolate
                .evaluate("reflectedCaller === null", "test:///private-caller.js")
                .unwrap(),
            "true"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn adapter_unobservable_handler_retires_before_a_later_request() {
        let mut dispatcher = adapter(
            r#"{fetch(request, env, ctx) {
              globalThis.handlerCalls = (globalThis.handlerCalls ?? 0) + 1;
              globalThis.requestSignal = ctx.signal;
              if (handlerCalls > 1) {
                releaseOld(new Response('old'));
                return new Response('next');
              }
              const pending = new Promise(resolve => { globalThis.releaseOld = resolve; });
              Promise.prototype.then.call(pending, () => { globalThis.oldContinuationRan = true; });
              Object.defineProperty(pending, 'constructor', {
                get() { throw Error('handler observer installation'); }
              });
              return pending;
            }}"#,
        )
        .await;
        assert_eq!(
            dispatcher
                .dispatch(
                    request("GET"),
                    RequestContext::new(),
                    &DispatchCancellation::default()
                )
                .await
                .unwrap_err()
                .kind,
            DispatchErrorKind::Engine
        );
        assert!(dispatcher.retired.get());
        assert_eq!(
            dispatcher
                .dispatch(
                    request("GET"),
                    RequestContext::new(),
                    &DispatchCancellation::default()
                )
                .await
                .unwrap_err()
                .kind,
            DispatchErrorKind::Retired
        );
        assert_eq!(
            dispatcher
                .isolate
                .evaluate(
                    "handlerCalls === 1 && requestSignal.aborted && !globalThis.oldContinuationRan",
                    "test:///handler-observer.js"
                )
                .unwrap(),
            "true"
        );
        assert_eq!(
            dispatcher.resource_counts(),
            RuntimeResourceCounts::default()
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn adapter_unobservable_wait_until_does_not_wait_for_other_pending_tasks() {
        let mut dispatcher = adapter(
            r#"{fetch(request, env, ctx) {
              globalThis.requestSignal = ctx.signal;
              ctx.waitUntil(new Promise(() => {}));
              const pending = new Promise(() => {});
              Object.defineProperty(pending, 'constructor', {
                get() { throw Error('background observer installation'); }
              });
              ctx.waitUntil(pending);
              return new Response(null);
            }}"#,
        )
        .await;
        assert_eq!(
            dispatcher
                .dispatch(
                    request("GET"),
                    RequestContext::new(),
                    &DispatchCancellation::default()
                )
                .await
                .unwrap_err()
                .kind,
            DispatchErrorKind::Engine
        );
        assert!(dispatcher.retired.get());
        assert_eq!(
            dispatcher
                .isolate
                .evaluate("requestSignal.aborted", "test:///background-observer.js")
                .unwrap(),
            "true"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn adapter_unobservable_body_read_and_cancel_retire() {
        // Test-only instrumentation models a captured reader operation returning
        // a genuine Promise whose observer cannot be installed. It does not
        // replace the module loader or claim native artifact evidence.
        for (method, setup) in [
            (
                "GET",
                r#"
              ReadableStreamDefaultReader.prototype.read = function() {
                globalThis.readerOperation = 'read';
                const pending = new Promise(() => {});
                Object.defineProperty(pending, 'constructor', {
                  get() { throw Error('read observer installation'); }
                });
                return pending;
              };
            "#,
            ),
            (
                "HEAD",
                r#"
              ReadableStreamDefaultReader.prototype.cancel = function() {
                globalThis.readerOperation = 'head-cancel';
                const pending = new Promise(() => {});
                Object.defineProperty(pending, 'constructor', {
                  get() { throw Error('cancel observer installation'); }
                });
                return pending;
              };
            "#,
            ),
            (
                "GET",
                r#"
              ReadableStreamDefaultReader.prototype.read = function() {
                return Promise.reject(Error('ordinary body failure'));
              };
              ReadableStreamDefaultReader.prototype.cancel = function() {
                globalThis.readerOperation = 'error-cancel';
                const pending = new Promise(() => {});
                Object.defineProperty(pending, 'constructor', {
                  get() { throw Error('cleanup observer installation'); }
                });
                return pending;
              };
            "#,
            ),
        ] {
            let mut dispatcher = adapter_with_setup(
                r#"{fetch(request, env, ctx) {
                  globalThis.requestSignal = ctx.signal;
                  return new Response('body');
                }}"#,
                setup,
            )
            .await;
            assert_eq!(
                dispatcher
                    .dispatch(
                        request(method),
                        RequestContext::new(),
                        &DispatchCancellation::default()
                    )
                    .await
                    .unwrap_err()
                    .kind,
                DispatchErrorKind::Engine,
                "{method}: {setup}"
            );
            assert!(dispatcher.retired.get());
            assert_eq!(
                dispatcher
                    .isolate
                    .evaluate(
                        "typeof readerOperation === 'string' && requestSignal.aborted",
                        "test:///body-observer.js"
                    )
                    .unwrap(),
                "true"
            );
            assert_eq!(
                dispatcher
                    .dispatch(
                        request("GET"),
                        RequestContext::new(),
                        &DispatchCancellation::default()
                    )
                    .await
                    .unwrap_err()
                    .kind,
                DispatchErrorKind::Retired
            );
            assert_eq!(
                dispatcher.resource_counts(),
                RuntimeResourceCounts::default()
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn adapter_failed_native_promise_observation_retires() {
        let mut dispatcher = adapter(
            r#"{fetch(request, env, ctx) {
              const pending = new Promise(() => {});
              Object.defineProperty(pending, 'constructor', {get() { throw Error('species lookup'); }});
              ctx.waitUntil(pending);
              return new Response(null);
            }}"#,
        ).await;
        assert_eq!(
            dispatcher
                .dispatch(
                    request("GET"),
                    RequestContext::new(),
                    &DispatchCancellation::default()
                )
                .await
                .unwrap_err()
                .kind,
            DispatchErrorKind::Engine
        );
        assert!(dispatcher.retired.get());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn adapter_async_and_cpu_deadlines_retire() {
        for body in [
            "ctx.waitUntil(new Promise(() => {})); return new Response('pending');",
            "while (true) {}",
        ] {
            let mut dispatcher =
                adapter(&format!("{{fetch(request, env, ctx) {{ {body} }} }}")).await;
            dispatcher.budget = Duration::from_millis(20);
            let error = dispatcher
                .dispatch(
                    request("GET"),
                    RequestContext::new(),
                    &DispatchCancellation::default(),
                )
                .await
                .unwrap_err();
            assert_eq!(error.kind, DispatchErrorKind::Deadline, "{body}");
            assert!(dispatcher.retired.get());
            assert_eq!(
                dispatcher.resource_counts(),
                RuntimeResourceCounts::default()
            );
        }
    }
}
