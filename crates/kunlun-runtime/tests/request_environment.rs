//! Native request authority checks; these tests make no Node compatibility claim.
use kunlun_runtime::{
    AdmissionPolicy, Capability, HostPermissions, RequestContext, RuntimeResourceCounts,
    ShutdownOutcome, TokioIsolate, admit_artifact,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::oneshot;

static NEXT: AtomicU64 = AtomicU64::new(0);
const DEADLINE: Duration = Duration::from_secs(10);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "kunlun-request-env-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/runtime-manifest/v1");
        for path in [
            "manifest.json",
            "server.mjs",
            "chunks/问候% space.mjs",
            "maps/server.mjs.map",
            "assets/help/欢迎 组件.svg",
        ] {
            let destination = root.join(path);
            fs::create_dir_all(destination.parent().unwrap()).unwrap();
            fs::copy(source.join(path), destination).unwrap();
        }
        fs::write(root.join("message.txt"), "public message").unwrap();
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("manifest.json")).unwrap()).unwrap();
        // Both hosts are admitted: the selected handle must still stay narrower.
        manifest["capabilities"]["required"] = json!([
            {"name": "http.host", "resource": "127.0.0.1"},
            {"name": "http.host", "resource": "localhost"}
        ]);
        manifest["capabilities"]["optional"] = json!([
            {"name": "fs.binding", "resource": "public-data"},
            {"name": "fs.binding", "resource": "missing-optional"}
        ]);
        fs::write(
            root.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        Self(root)
    }

    fn policy(&self, bind: bool) -> AdmissionPolicy {
        let mut permissions = HostPermissions::none()
            .allow_net_host("127.0.0.1")
            .allow_net_host("localhost")
            .allow_net_host("undeclared.example.test")
            .allow_read_root(&self.0)
            .unwrap()
            .bind_read_root("undeclared", &self.0)
            .unwrap();
        if bind {
            permissions = permissions.bind_read_root("public-data", &self.0).unwrap();
        }
        let digest = Sha256::digest(fs::read(self.0.join("manifest.json")).unwrap());
        AdmissionPolicy::new(
            digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
            permissions,
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[test]
fn adapter_neutral_authority_probe_runs_unchanged_across_requests() {
    const SOURCE: &str = include_str!("fixtures/request-authority.js");
    const CONTRACT: &[u8] = include_bytes!("fixtures/request-authority.contract.json");
    let contract: serde_json::Value = serde_json::from_slice(CONTRACT).unwrap();
    assert_eq!(contract["schema_version"], 1);
    assert_eq!(contract["suite"], "request-authority/v1");
    let fixture = Fixture::new();
    // Other native cases need HTTP authority; this shared slice must instead
    // use exactly the contract declarations, just like the Core collector.
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.0.join("manifest.json")).unwrap()).unwrap();
    let declarations = &contract["setup"]["declarations"];
    manifest["capabilities"] = json!({
        "required": declarations.get("required").cloned().unwrap_or_else(|| json!([])),
        "optional": declarations["optional"]
    });
    fs::write(
        fixture.0.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    let public_root = fixture.0.join("public");
    fs::create_dir(&public_root).unwrap();
    fs::write(public_root.join("message.txt"), "public message").unwrap();
    // The traversal target exists and is readable by the process, but is outside
    // the selected binding. A nonexistent target would not prove confinement.
    fs::write(fixture.0.join("escape.txt"), "private message").unwrap();
    let permissions = HostPermissions::none()
        .bind_read_root("public-data", &public_root)
        .unwrap()
        .bind_read_root("undeclared", &fixture.0)
        .unwrap();
    let policy = AdmissionPolicy::new(
        sha256_hex(&fs::read(fixture.0.join("manifest.json")).unwrap()),
        permissions,
    );
    let artifact = admit_artifact(&fixture.0, &policy).unwrap();
    let authority = artifact.authority();
    for resource in ["127.0.0.1", "localhost"] {
        assert!(!authority.contains(&Capability {
            name: "http.host".to_owned(),
            resource: resource.to_owned(),
        }));
    }
    let mut isolate =
        TokioIsolate::new_with_authority("shared-authority-probe", authority).unwrap();
    isolate
        .evaluate(
            &format!(
                "globalThis.requestAuthorityInputs = Object.freeze({{ absolutePath: {} }}); 'ready'",
                json!(fixture.0.join("escape.txt").to_str().unwrap())
            ),
            "test:///request-authority-inputs.js",
        )
        .unwrap();
    let rt = runtime();
    let mut observations = Vec::<serde_json::Value>::new();
    for _ in 0..2 {
        let output = rt
            .block_on(async {
                tokio::time::timeout(
                    DEADLINE,
                    isolate.evaluate_request_body(
                        authority.begin_request(RequestContext::new()).unwrap(),
                        SOURCE,
                        "test:///request-authority.js",
                    ),
                )
                .await
            })
            .expect("authority probe exceeded deadline")
            .unwrap();
        observations.push(serde_json::from_str(&output).unwrap());
    }
    assert_eq!(json!(observations), contract["expected_observations"]);
    assert_eq!(
        rt.block_on(isolate.shutdown(DEADLINE)).unwrap(),
        ShutdownOutcome::Graceful
    );
    assert_eq!(isolate.resource_counts(), RuntimeResourceCounts::default());
    if let Some(path) = std::env::var_os("KUNLUN_AUTHORITY_OBSERVATIONS") {
        write_authority_observations(Path::new(&path), SOURCE.as_bytes(), CONTRACT, &observations)
            .expect("could not create authority observations report");
    }
}

fn write_authority_observations(
    path: &Path,
    source: &[u8],
    contract: &[u8],
    observations: &[serde_json::Value],
) -> std::io::Result<()> {
    let backend = kunlun_jsc::JscVm::backend_info();
    let report = json!({
        "schema_version": 1,
        "suite": "request-authority/v1",
        "fixture_sha256": sha256_hex(source),
        "contract_sha256": sha256_hex(contract),
        "observations": observations,
        "backend": {
            "backend": backend.backend,
            "target": backend.target,
            "engine_revision": backend.engine_revision,
            "distribution_mode": backend.distribution_mode,
            "hermetic": backend.hermetic
        }
    });
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(&serde_json::to_vec_pretty(&report)?)?;
    file.write_all(b"\n")?;
    file.sync_all()
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
fn authority_observations_report_records_exact_bytes_and_never_overwrites() {
    let fixture = Fixture::new();
    let path = fixture.0.join("observations.json");
    let observations = vec![json!({"request": 1}), json!({"request": 2})];
    write_authority_observations(&path, b"abc", b"", &observations).unwrap();
    let original = fs::read(&path).unwrap();
    let report: serde_json::Value = serde_json::from_slice(&original).unwrap();
    assert_eq!(report.as_object().unwrap().len(), 6);
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["suite"], "request-authority/v1");
    assert_eq!(
        report["fixture_sha256"],
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        report["contract_sha256"],
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(report["observations"], json!(observations));
    let backend = kunlun_jsc::JscVm::backend_info();
    assert_eq!(
        report["backend"],
        json!({
            "backend": backend.backend,
            "target": backend.target,
            "engine_revision": backend.engine_revision,
            "distribution_mode": backend.distribution_mode,
            "hermetic": backend.hermetic
        })
    );
    assert_eq!(
        write_authority_observations(&path, b"other", b"other", &[])
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::AlreadyExists
    );
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
fn projection_identity_does_not_pass_through_javascript_serialization_hooks() {
    let fixture = Fixture::new();
    let policy = fixture.policy(true);
    let permissions = HostPermissions::none()
        .bind_read_root("public-data", &fixture.0)
        .unwrap();
    assert!(!format!("{permissions:?}").contains(fixture.0.to_str().unwrap()));
    let artifact = admit_artifact(&fixture.0, &policy).unwrap();
    let authority = artifact.authority();
    let mut isolate = TokioIsolate::new_with_authority("serialization-hooks", authority).unwrap();
    let rt = runtime();
    assert_eq!(
        rt.block_on(isolate.evaluate_request_body(
            authority.begin_request(RequestContext::new()).unwrap(),
            r#"
            const original = JSON.stringify;
            let exposed = false;
            JSON.stringify = function(value) {
              if (value && 'scope' in value) exposed = true;
              return original(value);
            };
            Object.prototype.toJSON = function() {
              if ('scope' in this) exposed = true;
              return this;
            };
            try {
              let sourceAccess = false;
              try { arguments.callee.toString(); sourceAccess = true; } catch (_) {}
              if (sourceAccess) throw Error('request wrapper source exposed');
              if (await env.fs['public-data'].readTextFile('message.txt') !== 'public message')
                throw Error('read failed');
              if (exposed) throw Error('request identity exposed');
              return 'ok';
            } finally {
              JSON.stringify = original;
              delete Object.prototype.toJSON;
            }
            "#,
            "test:///request-hooks.js",
        ))
        .unwrap(),
        "ok"
    );
}

#[test]
fn projection_omits_optional_and_undeclared_deployment_grants() {
    let fixture = Fixture::new();
    let artifact = admit_artifact(&fixture.0, &fixture.policy(false)).unwrap();
    let authority = artifact.authority();
    let mut isolate = TokioIsolate::new_with_authority("omission", authority).unwrap();
    let mut context = RequestContext::new();
    context.insert("auth", "host-only-secret");
    let request = authority.begin_request(context).unwrap();
    assert_eq!(request.context_value("auth"), Some("host-only-secret"));
    let result = runtime().block_on(isolate.evaluate_request_body(
        request,
        r#"
        if (Object.getPrototypeOf(env) !== null || !Object.isFrozen(env)) throw Error('shape');
        if (!Object.isFrozen(env.fs) || !Object.isFrozen(env.http)) throw Error('mutable maps');
        if (Object.keys(env.fs).length !== 0) throw Error('optional or undeclared fs exposed');
        if (Object.keys(env.http).sort().join(',') !== '127.0.0.1,localhost') throw Error('net grants');
        if (Object.keys(env).sort().join(',') !== 'fs,http') throw Error('context exposed');
        if ('auth' in env || 'context' in env || 'auth' in globalThis) throw Error('secret exposed');
        for (const key of ['__kunlunRequestBridge', '__kunlunCreateScopedFetch', '__kunlunFetchBridge']) {
          if (key in globalThis) throw Error('internal bridge exposed: ' + key);
        }
        for (const value of [env, env.http['127.0.0.1']]) {
          let denied = false;
          try { JSON.stringify(value); } catch (_) { denied = true; }
          if (!denied) throw Error('serialized authority');
        }
        return 'ok';
        "#,
        "test:///request-projection.js",
    )).unwrap();
    assert_eq!(result, "ok");
}

#[test]
fn filesystem_handles_are_scoped_opaque_and_expire_between_requests() {
    let fixture = Fixture::new();
    let outside = Fixture::new();
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.0.join("message.txt"), fixture.0.join("escape")).unwrap();
    let artifact = admit_artifact(&fixture.0, &fixture.policy(true)).unwrap();
    let authority = artifact.authority();
    let mut isolate = TokioIsolate::new_with_authority("filesystem", authority).unwrap();
    let mut paths = vec![
        "../message.txt".to_owned(),
        outside.0.join("message.txt").to_str().unwrap().to_owned(),
    ];
    #[cfg(unix)]
    paths.push("escape".to_owned());
    let source = format!(
        r#"
        const handle = env.fs['public-data'];
        if (!Object.isFrozen(handle) || Object.getPrototypeOf(handle) !== null) throw Error('shape');
        if (await handle.readTextFile('message.txt') !== 'public message') throw Error('read');
        let serialized = false;
        try {{ JSON.stringify(handle); serialized = true; }} catch (_) {{}}
        if (serialized) throw Error('serialized handle');
        async function denied(operation) {{
          try {{ await operation(); }} catch (error) {{
            const text = String(error);
            if (text.includes({root}) || text.includes({outside})) throw Error('host path leaked');
            return;
          }}
          throw Error('authority widened');
        }}
        for (const path of {paths}) await denied(() => handle.readTextFile(path));
        const counterfeit = Object.assign({{}}, handle, {{
          scope: 'invented', capabilityName: 'fs.binding', capabilityResource: 'undeclared'
        }});
        await denied(() => counterfeit.readTextFile({absolute}));
        const forged = __kunlunCreateEnvironment({{
          scope: 'invented', capabilities: [{{name: 'fs.binding', resource: 'public-data'}}]
        }});
        await denied(() => forged.fs['public-data'].readTextFile('message.txt'));
        globalThis.retainedHandle = handle;
        return 'ok';
        "#,
        root = json!(fixture.0.to_str().unwrap()),
        outside = json!(outside.0.to_str().unwrap()),
        paths = json!(paths),
        absolute = json!(outside.0.join("message.txt").to_str().unwrap()),
    );
    let rt = runtime();
    assert_eq!(
        rt.block_on(isolate.evaluate_request_body(
            authority.begin_request(RequestContext::new()).unwrap(),
            &source,
            "test:///request-filesystem.js",
        ))
        .unwrap(),
        "ok"
    );
    assert_eq!(
        rt.block_on(isolate.evaluate_request_body(
            authority.begin_request(RequestContext::new()).unwrap(),
            r#"
            let denied = false;
            try { await retainedHandle.readTextFile('message.txt'); } catch (_) { denied = true; }
            if (!denied) throw Error('retained handle still live');
            return await env.fs['public-data'].readTextFile('message.txt');
            "#,
            "test:///request-filesystem-next.js",
        ))
        .unwrap(),
        "public message"
    );
}

#[test]
fn wrong_isolate_environment_is_rejected_before_javascript_runs() {
    let fixture = Fixture::new();
    let first = admit_artifact(&fixture.0, &fixture.policy(true)).unwrap();
    let second = admit_artifact(&fixture.0, &fixture.policy(true)).unwrap();
    let _owner = TokioIsolate::new_with_authority("owner", first.authority()).unwrap();
    let mut other = TokioIsolate::new_with_authority("other", second.authority()).unwrap();
    let request = first
        .authority()
        .begin_request(RequestContext::new())
        .unwrap();
    let rt = runtime();
    assert!(
        rt.block_on(other.evaluate_request_body(
            request,
            "globalThis.wrongIsolateExecuted = true; return 'bad';",
            "test:///wrong-isolate.js",
        ))
        .is_err()
    );
    assert_eq!(
        other
            .evaluate("typeof wrongIsolateExecuted", "test:///check.js")
            .unwrap(),
        "undefined"
    );
    assert_eq!(
        rt.block_on(
            other.evaluate_request_body(
                second
                    .authority()
                    .begin_request(RequestContext::new())
                    .unwrap(),
                "return await env.fs['public-data'].readTextFile('message.txt');",
                "test:///correct-isolate.js",
            )
        )
        .unwrap(),
        "public message"
    );
}

#[test]
fn failed_and_dropped_invocations_revoke_retained_handles_and_release_timers() {
    let fixture = Fixture::new();
    let artifact = admit_artifact(&fixture.0, &fixture.policy(true)).unwrap();
    let authority = artifact.authority();
    let mut isolate = TokioIsolate::new_with_authority("cancel-request", authority).unwrap();
    let rt = runtime();
    assert!(
        rt.block_on(isolate.evaluate_request_body(
            authority.begin_request(RequestContext::new()).unwrap(),
            "globalThis.failedHandle = env.fs['public-data']; throw Error('failure');",
            "test:///failed-request.js",
        ))
        .is_err()
    );
    rt.block_on(async {
        use std::future::Future;
        let invocation = isolate.evaluate_request_body(
            authority.begin_request(RequestContext::new()).unwrap(),
            "globalThis.cancelledHandle = env.fs['public-data']; await sleep(60000);",
            "test:///cancelled-request.js",
        );
        tokio::pin!(invocation);
        std::future::poll_fn(|cx| {
            assert!(invocation.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        // Drop at a confirmed suspension point rather than racing a timer.
    });
    assert_eq!(isolate.resource_counts(), RuntimeResourceCounts::default());
    assert_eq!(
        rt.block_on(isolate.evaluate_request_body(
            authority.begin_request(RequestContext::new()).unwrap(),
            r#"
            for (const handle of [failedHandle, cancelledHandle]) {
              let denied = false;
              try { await handle.readTextFile('message.txt'); } catch (_) { denied = true; }
              if (!denied) throw Error('ended request retained authority');
            }
            return await env.fs['public-data'].readTextFile('message.txt');
            "#,
            "test:///next-request.js",
        ))
        .unwrap(),
        "public message"
    );
}

#[test]
fn revoking_one_live_request_preserves_the_other_context_and_native_handle() {
    let fixture = Fixture::new();
    let artifact = admit_artifact(&fixture.0, &fixture.policy(true)).unwrap();
    let authority = artifact.authority();
    let mut isolate = TokioIsolate::new_with_authority("live-requests", authority).unwrap();
    let mut a_context = RequestContext::new();
    a_context.insert("auth", "private-A");
    let mut b_context = RequestContext::new();
    b_context.insert("auth", "private-B");
    let a = authority.begin_request(a_context).unwrap();
    let b = authority.begin_request(b_context).unwrap();
    assert_eq!(a.context_value("auth"), Some("private-A"));
    assert_eq!(b.context_value("auth"), Some("private-B"));
    a.revoke();
    assert_eq!(a.context_value("auth"), None);
    assert_eq!(b.context_value("auth"), Some("private-B"));
    let rt = runtime();
    assert!(
        rt.block_on(isolate.evaluate_request_body(
            a,
            "globalThis.revokedExecuted = true; return 'bad';",
            "test:///revoked-request.js",
        ))
        .is_err()
    );
    assert_eq!(
        rt.block_on(isolate.evaluate_request_body(
            b,
            r#"
            if (typeof revokedExecuted !== 'undefined') throw Error('revoked source ran');
            if ('auth' in env || 'context' in env) throw Error('context exposed');
            return await env.fs['public-data'].readTextFile('message.txt');
            "#,
            "test:///surviving-request.js",
        ))
        .unwrap(),
        "public message"
    );
}

// Blocking I/O lives on a server thread. Request receipt and response release
// are explicit handshakes; deadlines bound failures, never sequence assertions.
fn server(
    response: String,
) -> (
    String,
    oneshot::Receiver<()>,
    std::sync::mpsc::Sender<()>,
    std::thread::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (received, receipt) = oneshot::channel();
    let (release, released) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(DEADLINE)).unwrap();
        stream.set_write_timeout(Some(DEADLINE)).unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            stream.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
        }
        received.send(()).unwrap();
        released.recv_timeout(DEADLINE).unwrap();
        // Revocation may have already closed the client.
        let _ = stream.write_all(response.as_bytes());
    });
    (format!("http://{address}/"), receipt, release, thread)
}

#[test]
fn selected_http_handle_allows_its_host_but_not_another_admitted_redirect_host() {
    for redirect in [false, true] {
        let response = if redirect {
            "HTTP/1.1 302 Found\r\nLocation: http://localhost:9/forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        } else {
            "HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello"
        };
        let (url, receipt, release, thread) = server(response.to_owned());
        let fixture = Fixture::new();
        let artifact = admit_artifact(&fixture.0, &fixture.policy(false)).unwrap();
        let mut isolate =
            TokioIsolate::new_with_authority("http-scope", artifact.authority()).unwrap();
        let request = artifact
            .authority()
            .begin_request(RequestContext::new())
            .unwrap();
        let source = format!(
            "return await (await env.http['127.0.0.1'].fetch({})).text();",
            json!(url)
        );
        let result = runtime().block_on(async {
            let response = isolate.evaluate_request_body(request, &source, "test:///http-scope.js");
            let control = async {
                tokio::time::timeout(DEADLINE, receipt)
                    .await
                    .unwrap()
                    .unwrap();
                release.send(()).unwrap();
            };
            let (result, ()) = tokio::join!(response, control);
            result
        });
        thread.join().unwrap();
        if redirect {
            let error = result.unwrap_err().to_string();
            assert!(error.to_lowercase().contains("denied"), "{error}");
        } else {
            assert_eq!(result.unwrap(), "hello");
        }
    }
}

#[test]
fn authority_revocation_wakes_pending_response_and_shutdown_drains_resources() {
    let (url, receipt, release, thread) =
        server("HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello".to_owned());
    let fixture = Fixture::new();
    let artifact = admit_artifact(&fixture.0, &fixture.policy(false)).unwrap();
    let authority = artifact.authority();
    let mut isolate = TokioIsolate::new_with_authority("revoke-inflight", authority).unwrap();
    let source = format!(
        "return await (await env.http['127.0.0.1'].fetch({})).text();",
        json!(url)
    );
    let rt = runtime();
    let result = rt.block_on(async {
        let evaluation = isolate.evaluate_request_body(
            authority.begin_request(RequestContext::new()).unwrap(),
            &source,
            "test:///revoke-inflight.js",
        );
        let revoke = async {
            tokio::time::timeout(DEADLINE, receipt)
                .await
                .unwrap()
                .unwrap();
            authority.revoke();
        };
        let (result, ()) = tokio::join!(tokio::time::timeout(DEADLINE, evaluation), revoke);
        result
    });
    // Only release the server after evaluation has woken (or its deadline failed).
    release.send(()).unwrap();
    thread.join().unwrap();
    assert!(result.expect("revocation must wake pending fetch").is_err());
    assert_eq!(
        rt.block_on(isolate.shutdown(DEADLINE)).unwrap(),
        ShutdownOutcome::Graceful
    );
    assert_eq!(isolate.resource_counts(), RuntimeResourceCounts::default());
}

#[test]
fn authority_revocation_cancels_a_blocked_streaming_upload() {
    let (url, receipt, release, thread) =
        server("HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_owned());
    let fixture = Fixture::new();
    let artifact = admit_artifact(&fixture.0, &fixture.policy(false)).unwrap();
    let authority = artifact.authority();
    let mut isolate = TokioIsolate::new_with_authority("revoke-upload", authority).unwrap();
    let source = format!(
        r#"
        const body = new ReadableStream({{
          start(controller) {{ controller.enqueue(new Uint8Array([1])); }},
          cancel() {{ globalThis.uploadCancelled = true; }},
        }});
        await env.http['127.0.0.1'].fetch({}, {{method: 'POST', body}});
        return 'unexpected success';
        "#,
        json!(url)
    );
    let rt = runtime();
    let result = rt.block_on(async {
        let evaluation = isolate.evaluate_request_body(
            authority.begin_request(RequestContext::new()).unwrap(),
            &source,
            "test:///revoke-upload.js",
        );
        let revoke = async {
            tokio::time::timeout(DEADLINE, receipt)
                .await
                .unwrap()
                .unwrap();
            authority.revoke();
        };
        let (result, ()) = tokio::join!(tokio::time::timeout(DEADLINE, evaluation), revoke);
        result
    });
    release.send(()).unwrap();
    thread.join().unwrap();
    assert!(
        result
            .expect("revocation must wake blocked upload")
            .is_err()
    );
    assert_eq!(
        isolate
            .evaluate("uploadCancelled", "test:///upload-cancelled.js")
            .unwrap(),
        "true"
    );
    assert_eq!(
        rt.block_on(isolate.shutdown(DEADLINE)).unwrap(),
        ShutdownOutcome::Graceful
    );
    assert_eq!(isolate.resource_counts(), RuntimeResourceCounts::default());
}

#[test]
fn ambient_proxy_variables_do_not_change_http_destination() {
    let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
    proxy.set_nonblocking(true).unwrap();
    let proxy_url = format!("http://localhost:{}", proxy.local_addr().unwrap().port());
    let destination = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", destination.local_addr().unwrap());
    drop(destination);
    // The exact granted destination refuses a connection. An ambient proxy must
    // not be contacted instead. Child-only env changes avoid process-global races.
    let source = format!(
        "try {{ await (await kunlun.import('kunlun:http')).request({}); \
         throw Error('unexpected response'); }} catch (error) {{ return String(error); }}",
        json!(url),
    );
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_kunlun-runtime"))
        .args([
            "eval-async",
            "--allow-net",
            "127.0.0.1",
            "--execution-timeout-ms",
            "2000",
            &source,
        ])
        .env("HTTP_PROXY", &proxy_url)
        .env("http_proxy", &proxy_url)
        .env("HTTPS_PROXY", &proxy_url)
        .env("https_proxy", &proxy_url)
        .env("ALL_PROXY", &proxy_url)
        .env("all_proxy", &proxy_url)
        .env("NO_PROXY", "")
        .env("no_proxy", "")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        proxy.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}
