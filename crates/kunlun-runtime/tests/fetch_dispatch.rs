//! Real admitted module tests require the pinned loader extension. On system
//! JSC they compile but are explicitly ignored, never replaced by a fake module.
use kunlun_jsc::ModuleLoader;
use kunlun_runtime::{
    AdmissionErrorKind, AdmissionPolicy, AdmittedArtifact, DispatchCancellation, DispatchErrorKind,
    DispatchRequest, FetchDispatcher, HostPermissions, RequestContext, RuntimeLimits,
    RuntimeResourceCounts, admit_artifact,
};
use sha2::{Digest, Sha256};
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "kunlun-fetch-dispatch-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fetch-entry");
        for entry in fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            fs::copy(entry.path(), root.join(entry.file_name())).unwrap();
        }
        fs::create_dir(root.join("public")).unwrap();
        fs::write(root.join("public/message.txt"), "scoped fixture").unwrap();
        Self(root)
    }
    fn entry(&self, entry: &str) {
        let path = self.0.join("manifest.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        manifest["entry"] = serde_json::json!(format!("./{entry}"));
        fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    }
    fn policy(&self) -> AdmissionPolicy {
        AdmissionPolicy::new(
            Sha256::digest(fs::read(self.0.join("manifest.json")).unwrap())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
            HostPermissions::none()
                .bind_read_root("public-data", self.0.join("public"))
                .unwrap(),
        )
    }
    fn admit(&self) -> AdmittedArtifact {
        admit_artifact(&self.0, &self.policy()).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn request(path: &str, method: &str) -> DispatchRequest {
    DispatchRequest {
        url: format!("https://service.test{path}"),
        method: method.into(),
        headers: vec![("x-caller".into(), "test".into())],
        body: None,
    }
}
fn limits() -> RuntimeLimits {
    RuntimeLimits {
        execution_timeout: Duration::from_secs(2),
        ..RuntimeLimits::default()
    }
}

#[test]
fn checked_in_artifact_admits_and_changed_bytes_are_denied_before_loading() {
    let fixture = Fixture::new();
    let _artifact = fixture.admit();
    fs::write(
        fixture.0.join("server.mjs"),
        "globalThis.mustNotRun = true;",
    )
    .unwrap();
    let error = match admit_artifact(&fixture.0, &fixture.policy()) {
        Ok(_) => panic!("unchecked source admitted"),
        Err(error) => error,
    };
    assert_eq!(error.kind, AdmissionErrorKind::Integrity);
}

#[test]
fn private_adapter_import_requires_an_admitted_canonical_referrer() {
    let fixture = Fixture::new();
    let (entry, sources, _, _) = fixture.admit().into_parts();
    assert_eq!(sources.resolve(&entry, Some(&entry)).unwrap(), entry);
    // Absolute imports do not bypass referrer validation. The adapter must not
    // originate from an unregistered synthetic builtin URL.
    assert!(
        sources
            .resolve(&entry, Some("kunlun:fetch-adapter"))
            .is_err()
    );
}

#[cfg(feature = "system-jsc")]
#[tokio::test(flavor = "current_thread")]
async fn system_backend_rejects_real_entry_loading_without_fallback() {
    let fixture = Fixture::new();
    let failure = match FetchDispatcher::load(fixture.admit(), limits()).await {
        Ok(_) => panic!("system JSC unexpectedly loaded a native artifact"),
        Err(failure) => failure,
    };
    assert_eq!(failure.kind, DispatchErrorKind::Startup);
}

#[tokio::test(flavor = "current_thread")]
#[cfg_attr(
    feature = "system-jsc",
    ignore = "requires pinned native module loader"
)]
async fn admitted_entry_preserves_response_metadata_module_state_and_background_scope() {
    let fixture = Fixture::new();
    let mut dispatcher = FetchDispatcher::load(fixture.admit(), limits())
        .await
        .unwrap();
    // Replacing admitted files cannot replace the loaded application snapshot.
    fs::write(fixture.0.join("server.mjs"), "throw Error('mutable file');").unwrap();
    let cancellation = DispatchCancellation::default();
    let mut context = RequestContext::new();
    context.insert("auth", "must remain host-only");
    let response = dispatcher
        .dispatch(request("/binary?q=1", "GET"), context, &cancellation)
        .await
        .unwrap();
    assert_eq!(response.status, 202);
    assert_eq!(response.status_text, "Accepted");
    assert_eq!(response.body, [0, 255, 1]);
    assert!(
        response
            .headers
            .contains(&("x-route".into(), "https://service.test/binary?q=1".into()))
    );
    assert_eq!(
        response
            .headers
            .iter()
            .filter(|(name, _)| name == "set-cookie")
            .count(),
        2
    );
    for _ in 0..2 {
        let response = dispatcher
            .dispatch(
                request("/scoped", "GET"),
                RequestContext::new(),
                &cancellation,
            )
            .await
            .unwrap();
        assert_eq!(response.body, b"scoped fixture");
        assert_eq!(
            dispatcher.resource_counts(),
            RuntimeResourceCounts::default()
        );
    }
    let response = dispatcher
        .dispatch(
            request("/state", "GET"),
            RequestContext::new(),
            &cancellation,
        )
        .await
        .unwrap();
    assert_eq!(response.body, b"4:3");
}

#[tokio::test(flavor = "current_thread")]
#[cfg_attr(
    feature = "system-jsc",
    ignore = "requires pinned native module loader"
)]
async fn startup_diagnostics_deny_malformed_and_unindexed_entries() {
    for (entry, kind) in [
        ("no-default.mjs", DispatchErrorKind::InvalidEntry),
        ("invalid-default.mjs", DispatchErrorKind::InvalidEntry),
        ("tla-error.mjs", DispatchErrorKind::Startup),
        ("parse-error.mjs", DispatchErrorKind::Startup),
        ("denied-import.mjs", DispatchErrorKind::Startup),
    ] {
        let fixture = Fixture::new();
        fixture.entry(entry);
        let error = match FetchDispatcher::load(fixture.admit(), limits()).await {
            Ok(_) => panic!("invalid entry loaded: {entry}"),
            Err(error) => error,
        };
        assert_eq!(error.kind, kind, "{entry}");
        assert!(!error.to_string().contains("private"));
    }
}

#[tokio::test(flavor = "current_thread")]
#[cfg_attr(
    feature = "system-jsc",
    ignore = "requires pinned native module loader"
)]
async fn handler_body_and_background_failures_release_the_request() {
    let fixture = Fixture::new();
    let mut dispatcher = FetchDispatcher::load(fixture.admit(), limits())
        .await
        .unwrap();
    let cancellation = DispatchCancellation::default();
    for (path, kind) in [
        ("/throw", DispatchErrorKind::Handler),
        ("/invalid", DispatchErrorKind::InvalidResponse),
        ("/background-error", DispatchErrorKind::Background),
        ("/body-error", DispatchErrorKind::ResponseBody),
        ("/oversized", DispatchErrorKind::ResponseBody),
    ] {
        let error = dispatcher
            .dispatch(request(path, "GET"), RequestContext::new(), &cancellation)
            .await
            .unwrap_err();
        assert_eq!(error.kind, kind, "{path}");
        assert!(!error.to_string().contains("private"));
        assert_eq!(
            dispatcher.resource_counts(),
            RuntimeResourceCounts::default()
        );
    }
}

#[tokio::test(flavor = "current_thread")]
#[cfg_attr(
    feature = "system-jsc",
    ignore = "requires pinned native module loader"
)]
async fn head_redirect_and_invalid_routing_inputs_are_explicit() {
    let fixture = Fixture::new();
    let mut dispatcher = FetchDispatcher::load(fixture.admit(), limits())
        .await
        .unwrap();
    let cancellation = DispatchCancellation::default();
    let response = dispatcher
        .dispatch(
            request("/head", "HEAD"),
            RequestContext::new(),
            &cancellation,
        )
        .await
        .unwrap();
    assert_eq!(response.status, 201);
    assert!(response.body.is_empty());
    let response = dispatcher
        .dispatch(
            request("/redirect", "GET"),
            RequestContext::new(),
            &cancellation,
        )
        .await
        .unwrap();
    assert_eq!(response.status, 307);
    assert!(
        response
            .headers
            .contains(&("location".into(), "https://service.test/target".into()))
    );
    let mut input = request("/", "GET");
    input.body = Some(vec![]);
    assert_eq!(
        dispatcher
            .dispatch(input, RequestContext::new(), &cancellation)
            .await
            .unwrap_err()
            .kind,
        DispatchErrorKind::InvalidRequest
    );
    assert_eq!(
        dispatcher.resource_counts(),
        RuntimeResourceCounts::default()
    );
}

#[tokio::test(flavor = "current_thread")]
#[cfg_attr(
    feature = "system-jsc",
    ignore = "requires pinned native module loader"
)]
async fn dropping_a_confirmed_suspended_request_retires_the_dispatcher() {
    let fixture = Fixture::new();
    let mut dispatcher = FetchDispatcher::load(fixture.admit(), limits())
        .await
        .unwrap();
    let cancellation = DispatchCancellation::default();
    {
        let invocation = dispatcher.dispatch(
            request("/wait", "GET"),
            RequestContext::new(),
            &cancellation,
        );
        tokio::pin!(invocation);
        std::future::poll_fn(|cx| {
            assert!(invocation.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
    }
    assert_eq!(
        dispatcher.resource_counts(),
        RuntimeResourceCounts::default()
    );
    assert_eq!(
        dispatcher
            .dispatch(request("/", "GET"), RequestContext::new(), &cancellation)
            .await
            .unwrap_err()
            .kind,
        DispatchErrorKind::Retired
    );
}

#[tokio::test(flavor = "current_thread")]
#[cfg_attr(
    feature = "system-jsc",
    ignore = "requires pinned native module loader"
)]
async fn explicit_cancellation_deadline_and_stop_have_no_reusable_scope() {
    for deadline in [false, true] {
        let fixture = Fixture::new();
        let mut dispatcher = FetchDispatcher::load(fixture.admit(), limits())
            .await
            .unwrap();
        if deadline {
            dispatcher.stop();
        }
        let cancellation = DispatchCancellation::default();
        if !deadline {
            cancellation.cancel();
        }
        let error = dispatcher
            .dispatch(request("/", "GET"), RequestContext::new(), &cancellation)
            .await
            .unwrap_err();
        assert_eq!(
            error.kind,
            if deadline {
                DispatchErrorKind::Retired
            } else {
                DispatchErrorKind::Cancelled
            }
        );
        assert_eq!(
            dispatcher.resource_counts(),
            RuntimeResourceCounts::default()
        );
    }
    let fixture = Fixture::new();
    let mut short = limits();
    short.execution_timeout = Duration::from_millis(100);
    let mut dispatcher = FetchDispatcher::load(fixture.admit(), short).await.unwrap();
    let cancellation = DispatchCancellation::default();
    assert_eq!(
        dispatcher
            .dispatch(
                request("/wait", "GET"),
                RequestContext::new(),
                &cancellation
            )
            .await
            .unwrap_err()
            .kind,
        DispatchErrorKind::Deadline
    );
    assert_eq!(
        dispatcher.resource_counts(),
        RuntimeResourceCounts::default()
    );
    assert_eq!(
        dispatcher
            .dispatch(request("/", "GET"), RequestContext::new(), &cancellation)
            .await
            .unwrap_err()
            .kind,
        DispatchErrorKind::Retired
    );
}
