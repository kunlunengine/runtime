//! Adapter-neutral post-header lifecycle corpus; real loopback HTTP, no sleeps.
//! This is a development backend observation, never physical qualification.
use kunlun_runtime::{
    AdmissionPolicy, Capability, HostPermissions, RequestContext, RuntimeResourceCounts,
    ShutdownOutcome, TokioIsolate, admit_artifact,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;
use tokio::sync::oneshot;

const DEADLINE: Duration = Duration::from_secs(10);
const PROBE: &str = include_str!("fixtures/request-authority-lifecycle.js");
const CONTRACT: &[u8] = include_bytes!("fixtures/request-authority-lifecycle.contract.json");
static NEXT: AtomicU64 = AtomicU64::new(0);

fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

struct Artifact(PathBuf);

impl Artifact {
    fn new(contract: &Value) -> Self {
        let root = std::env::temp_dir().join(format!(
            "kunlun-lifecycle-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let fixture = Self(root);
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/runtime-manifest/v1");
        for path in [
            "manifest.json",
            "server.mjs",
            "chunks/问候% space.mjs",
            "maps/server.mjs.map",
            "assets/help/欢迎 组件.svg",
        ] {
            let destination = fixture.0.join(path);
            fs::create_dir_all(destination.parent().unwrap()).unwrap();
            fs::copy(source.join(path), destination).unwrap();
        }
        let mut manifest: Value =
            serde_json::from_slice(&fs::read(fixture.0.join("manifest.json")).unwrap()).unwrap();
        manifest["capabilities"] = contract["setup"]["declarations"].clone();
        fs::write(
            fixture.0.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        fixture
    }

    fn policy(&self, contract: &Value) -> AdmissionPolicy {
        let mut permissions = HostPermissions::none();
        for host in contract["setup"]["deployment_hosts"].as_array().unwrap() {
            permissions = permissions.allow_net_host(host.as_str().unwrap());
        }
        AdmissionPolicy::new(
            hash(&fs::read(self.0.join("manifest.json")).unwrap()),
            permissions,
        )
    }
}

impl Drop for Artifact {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Header parser is deliberately bounded; this corpus sends GET without bodies.
fn request(stream: &mut TcpStream) -> std::io::Result<String> {
    stream.set_read_timeout(Some(DEADLINE))?;
    stream.set_write_timeout(Some(DEADLINE))?;
    let mut bytes = Vec::new();
    while bytes.len() < 8192 {
        let mut byte = [0];
        stream.read_exact(&mut byte)?;
        bytes.push(byte[0]);
        if bytes.ends_with(b"\r\n\r\n") {
            let text = String::from_utf8(bytes)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
            let line = text.split("\r\n").next().unwrap();
            let mut parts = line.split(' ');
            let method = parts.next().unwrap();
            let route = parts.next().unwrap_or("");
            if method != "GET" || parts.next() != Some("HTTP/1.1") {
                return Err(std::io::Error::other("unexpected request line"));
            }
            return Ok(format!("{method} {route}"));
        }
    }
    Err(std::io::Error::other("oversized request headers"))
}

struct Server {
    address: std::net::SocketAddr,
    receipt: Option<oneshot::Receiver<()>>,
    release: mpsc::Sender<()>,
    stop: Arc<AtomicBool>,
    traffic: Arc<Mutex<Vec<String>>>,
    done: mpsc::Receiver<std::io::Result<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Server {
    fn new() -> Self {
        Self::start(false)
    }

    fn fresh_only() -> Self {
        Self::start(true)
    }

    fn start(fresh_only: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (received, receipt) = oneshot::channel();
        let (release, released) = mpsc::channel();
        let (completed, done) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let traffic = Arc::new(Mutex::new(Vec::new()));
        let thread_stop = Arc::clone(&stop);
        let thread_traffic = Arc::clone(&traffic);
        let thread = std::thread::spawn(move || {
            let result = (|| -> std::io::Result<()> {
                if !fresh_only {
                    let (mut body, _) = listener.accept()?;
                    if thread_stop.load(Ordering::Acquire) {
                        return Ok(());
                    }
                    let body_request = request(&mut body)?;
                    thread_traffic.lock().unwrap().push(body_request.clone());
                    if body_request != "GET /body" {
                        return Err(std::io::Error::other("expected /body"));
                    }
                    body.write_all(
                    b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nfirst\r\n",
                )?;
                    body.flush()?;
                    let (mut ready, _) = listener.accept()?;
                    if thread_stop.load(Ordering::Acquire) {
                        return Ok(());
                    }
                    let ready_request = request(&mut ready)?;
                    thread_traffic.lock().unwrap().push(ready_request.clone());
                    if ready_request != "GET /ready?pending=true" {
                        return Err(std::io::Error::other("missing pending-read ACK"));
                    }
                    let _ = received.send(());
                    // Neither request is completed until the host observes cancellation.
                    released
                        .recv_timeout(DEADLINE)
                        .map_err(std::io::Error::other)?;
                    drop(ready);
                    drop(body);
                }
                // After release, allow a fresh request. Shutdown wakes accept,
                // so successful and failed tests cannot leave a fixture thread.
                loop {
                    let (mut socket, _) = listener.accept()?;
                    if thread_stop.load(Ordering::Acquire) {
                        return Ok(());
                    }
                    let actual = request(&mut socket)?;
                    thread_traffic.lock().unwrap().push(actual.clone());
                    if actual != "GET /fresh" {
                        return Err(std::io::Error::other("unexpected stale-handle traffic"));
                    }
                    socket.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nfresh",
                    )?;
                }
            })();
            let _ = completed.send(result);
        });
        Self {
            address,
            receipt: Some(receipt),
            release,
            stop,
            traffic,
            done,
            thread: Some(thread),
        }
    }

    fn base(&self) -> String {
        format!("http://{}", self.address)
    }

    fn finish(&mut self) -> Vec<String> {
        self.stop.store(true, Ordering::Release);
        let _ = self.release.send(());
        let _ = TcpStream::connect_timeout(&self.address, DEADLINE);
        self.done
            .recv_timeout(DEADLINE)
            .expect("fixture did not stop within deadline")
            .expect("fixture failed");
        self.thread.take().unwrap().join().unwrap();
        self.traffic.lock().unwrap().clone()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            self.stop.store(true, Ordering::Release);
            let _ = self.release.send(());
            let _ = TcpStream::connect_timeout(&self.address, DEADLINE);
            let _ = thread.join();
        }
    }
}

fn context(contract: &Value, owner: &str) -> RequestContext {
    let mut context = RequestContext::new();
    for (name, value) in contract["setup"]["contexts"][owner].as_object().unwrap() {
        context.insert(name, value.as_str().unwrap());
    }
    context
}

fn inputs(isolate: &mut TokioIsolate, server: &Server, phase: &str) {
    isolate
        .evaluate(
            &format!(
                "globalThis.requestAuthorityLifecycleInputs = Object.freeze({}); 'ready'",
                json!({"base": server.base(), "phase": phase})
            ),
            "test:///request-authority-lifecycle-inputs.js",
        )
        .unwrap();
}

fn drain_resources(rt: &tokio::runtime::Runtime, isolate: &TokioIsolate) {
    // Aborted workers publish their final counts when the executor polls them.
    // The fixture must still withhold its response: peer EOF is not cleanup.
    rt.block_on(async {
        tokio::time::timeout(DEADLINE, async {
            while isolate.resource_counts() != RuntimeResourceCounts::default() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("cancelled request resources did not drain before fixture release");
    });
    assert_eq!(isolate.resource_counts(), RuntimeResourceCounts::default());
}

#[test]
fn shared_post_headers_lifecycle_matches_contract() {
    let contract: Value = serde_json::from_slice(CONTRACT).unwrap();
    assert_eq!(contract["suite"], "request-authority-lifecycle/v1");
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut observations = serde_json::Map::new();
    let mut traffic = serde_json::Map::new();
    for phase in ["application", "request"] {
        let fixture = Artifact::new(&contract);
        let artifact = admit_artifact(&fixture.0, &fixture.policy(&contract)).unwrap();
        let authority = artifact.authority();
        let mut isolate = TokioIsolate::new_with_authority("post-headers", authority).unwrap();
        let mut server = Server::new();
        inputs(&mut isolate, &server, phase);

        // These host-owned environments coexist but JS invocation is serial.
        let held_a = authority.begin_request(context(&contract, "A")).unwrap();
        let held_b = authority.begin_request(context(&contract, "B")).unwrap();
        let capability = Capability {
            name: "http.host".to_owned(),
            resource: "127.0.0.1".to_owned(),
        };
        let a_handle = held_a.handle(&capability).unwrap();
        let b_handle = held_b.handle(&capability).unwrap();
        for owner in ["A", "B"] {
            let environment = if owner == "A" { &held_a } else { &held_b };
            for name in ["auth", "provider", "billing"] {
                assert_eq!(
                    environment.context_value(name),
                    contract["setup"]["contexts"][owner][name].as_str()
                );
            }
        }
        assert!(a_handle.authorize_http(&held_a, &server.base()).is_ok());
        assert!(a_handle.authorize_http(&held_b, &server.base()).is_err());
        assert!(b_handle.authorize_http(&held_a, &server.base()).is_err());
        held_a.revoke();
        for name in ["auth", "provider", "billing"] {
            assert_eq!(held_a.context_value(name), None);
            assert_eq!(
                held_b.context_value(name),
                contract["setup"]["contexts"]["B"][name].as_str()
            );
        }
        assert!(a_handle.authorize_http(&held_a, &server.base()).is_err());
        assert!(b_handle.authorize_http(&held_b, &server.base()).is_ok());

        let receipt = server.receipt.take().unwrap();
        rt.block_on(async {
            let evaluation = isolate.evaluate_request_body(
                authority.begin_request(context(&contract, "A")).unwrap(),
                PROBE,
                "test:///request-authority-lifecycle.js",
            );
            tokio::pin!(evaluation);
            tokio::time::timeout(DEADLINE, async {
                tokio::select! {
                    biased;
                    result = &mut evaluation => panic!("invocation settled before ACK: {result:?}"),
                    received = receipt => received.expect("fixture failed before ACK"),
                }
                if phase == "application" {
                    authority.revoke();
                    assert!(evaluation.await.is_err(), "revoked invocation succeeded");
                }
                // Request phase: dropping this future cancels its owned request.
                // No body bytes or ACK response have been released yet.
            })
            .await
            .expect("post-header cancellation exceeded deadline");
        });
        let progress: Value = serde_json::from_str(
            &isolate
                .evaluate(
                    "requestAuthorityLifecycleBarrier",
                    "test:///request-authority-lifecycle-progress.js",
                )
                .unwrap(),
        )
        .unwrap();
        assert_eq!(progress, contract["expected_progress"][phase]);
        observations.insert(phase.to_owned(), progress);
        drain_resources(&rt, &isolate);
        server.release.send(()).unwrap();
        if phase == "request" {
            // B's host context and native handle still work after cancellation
            // of the separately executing request.
            for name in ["auth", "provider", "billing"] {
                assert_eq!(
                    held_b.context_value(name),
                    contract["setup"]["contexts"]["B"][name].as_str()
                );
            }
            assert!(b_handle.authorize_http(&held_b, &server.base()).is_ok());
            inputs(&mut isolate, &server, "survivor");
            let output = rt
                .block_on(async {
                    tokio::time::timeout(
                        DEADLINE,
                        isolate.evaluate_request_body(
                            held_b,
                            PROBE,
                            "test:///request-authority-lifecycle.js",
                        ),
                    )
                    .await
                })
                .expect("surviving request exceeded deadline")
                .unwrap();
            let survivor: Value = serde_json::from_str(&output).unwrap();
            assert_eq!(survivor, contract["expected_progress"]["survivor"]);
            observations.insert("survivor".to_owned(), survivor);
        } else {
            assert!(authority.begin_request(RequestContext::new()).is_err());
            assert_eq!(held_b.context_value("auth"), None);
            drop(held_b);
        }
        drop(held_a);
        drain_resources(&rt, &isolate);
        assert_eq!(
            rt.block_on(isolate.shutdown(DEADLINE)).unwrap(),
            ShutdownOutcome::Graceful
        );
        assert_eq!(
            rt.block_on(isolate.shutdown(DEADLINE)).unwrap(),
            ShutdownOutcome::Graceful
        );
        assert_eq!(isolate.resource_counts(), RuntimeResourceCounts::default());
        let actual = json!(server.finish());
        assert_eq!(actual, contract["expected_traffic"][phase]);
        traffic.insert(phase.to_owned(), actual);
    }
    // Two admitted applications and VM realms, polled on their owning thread.
    // A remains suspended at a real network barrier while B completes.
    let fixture_a = Artifact::new(&contract);
    let fixture_b = Artifact::new(&contract);
    let artifact_a = admit_artifact(&fixture_a.0, &fixture_a.policy(&contract)).unwrap();
    let artifact_b = admit_artifact(&fixture_b.0, &fixture_b.policy(&contract)).unwrap();
    let authority_a = artifact_a.authority();
    let authority_b = artifact_b.authority();
    let mut isolate_a = TokioIsolate::new_with_authority("concurrent-A", authority_a).unwrap();
    let mut isolate_b = TokioIsolate::new_with_authority("concurrent-B", authority_b).unwrap();
    let mut blocked = Server::new();
    let mut independent = Server::fresh_only();
    inputs(&mut isolate_a, &blocked, "application");
    inputs(&mut isolate_b, &independent, "independent");
    let owner_b = authority_b.begin_request(context(&contract, "B")).unwrap();
    let receipt = blocked.receipt.take().unwrap();
    let concurrency = rt.block_on(async {
        tokio::time::timeout(DEADLINE, async {
            let evaluation_a = isolate_a.evaluate_request_body(
                authority_a.begin_request(context(&contract, "A")).unwrap(),
                PROBE,
                "test:///request-authority-lifecycle.js",
            );
            tokio::pin!(evaluation_a);
            tokio::select! {
                biased;
                result = &mut evaluation_a => panic!("A settled before ACK: {result:?}"),
                received = receipt => received.expect("blocked fixture failed before ACK"),
            }
            let while_pending: Value = {
                let evaluation_b = isolate_b.evaluate_request_body(
                    authority_b.begin_request(context(&contract, "B")).unwrap(),
                    PROBE,
                    "test:///request-authority-lifecycle.js",
                );
                tokio::pin!(evaluation_b);
                tokio::select! {
                    biased;
                    result = &mut evaluation_a => panic!("A settled while B executing: {result:?}"),
                    result = &mut evaluation_b => serde_json::from_str(&result.unwrap()).unwrap(),
                }
            };
            authority_a.revoke();
            let rejected = evaluation_a.await.is_err();
            assert!(rejected, "revoked A invocation succeeded");
            for name in ["auth", "provider", "billing"] {
                assert_eq!(
                    owner_b.context_value(name),
                    contract["setup"]["contexts"]["B"][name].as_str()
                );
            }
            let after_revoked: Value = serde_json::from_str(
                &isolate_b
                    .evaluate_request_body(
                        authority_b.begin_request(context(&contract, "B")).unwrap(),
                        PROBE,
                        "test:///request-authority-lifecycle.js",
                    )
                    .await
                    .unwrap(),
            )
            .unwrap();
            for name in ["auth", "provider", "billing"] {
                assert_eq!(
                    owner_b.context_value(name),
                    contract["setup"]["contexts"]["B"][name].as_str()
                );
            }
            assert!(authority_a.begin_request(RequestContext::new()).is_err());
            let admitted = authority_b.begin_request(context(&contract, "B")).is_ok();
            json!({
                "while_other_pending": while_pending,
                "after_other_revoked": after_revoked,
                "other_invocation_rejected": rejected,
                "independent_admission_preserved": admitted
            })
        })
        .await
        .expect("cross-isolate invocation exceeded deadline")
    });
    assert_eq!(concurrency, contract["expected_concurrency"]);
    drop(owner_b);
    for isolate in [&isolate_a, &isolate_b] {
        drain_resources(&rt, isolate);
    }
    blocked.release.send(()).unwrap();
    for isolate in [&mut isolate_a, &mut isolate_b] {
        assert_eq!(
            rt.block_on(isolate.shutdown(DEADLINE)).unwrap(),
            ShutdownOutcome::Graceful
        );
        assert_eq!(
            rt.block_on(isolate.shutdown(DEADLINE)).unwrap(),
            ShutdownOutcome::Graceful
        );
        assert_eq!(isolate.resource_counts(), RuntimeResourceCounts::default());
    }
    let concurrent_traffic = json!({
        "blocked": blocked.finish(),
        "independent": independent.finish()
    });
    assert_eq!(concurrent_traffic, contract["expected_concurrent_traffic"]);
    let lifecycle = json!({
        "application_invocation": "rejected",
        "request_invocation": "cancelled",
        "later_application_admission_denied": true,
        "surviving_request_admitted": true,
        "repeated_close_completed": true
    });
    assert_eq!(lifecycle, contract["expected_lifecycle"]);
    let native_assertions = json!({
        "held_A_revoked_B_preserved": true,
        "cross_isolate_B_context_preserved": true,
        "resources_empty_before_shutdown": true,
        "shutdown": "graceful",
        "resources_empty_after_shutdown": true
    });
    assert_eq!(native_assertions, contract["expected_native_assertions"]);
    if let Some(path) = std::env::var_os("KUNLUN_M3_LIFECYCLE_OBSERVATIONS") {
        let backend = kunlun_jsc::JscVm::backend_info();
        let report = json!({
            "schema_version": 1,
            "suite": "request-authority-lifecycle/v1",
            "adapter": "native",
            "status": "development",
            "fixture_sha256": hash(PROBE.as_bytes()),
            "contract_sha256": hash(CONTRACT),
            "qualification": false,
            "backend": {
                "backend": backend.backend,
                "target": backend.target,
                "engine_revision": backend.engine_revision,
                "distribution_mode": backend.distribution_mode,
                "hermetic": backend.hermetic
            },
            "observations": observations,
            "lifecycle": lifecycle,
            "native_assertions": native_assertions,
            "concurrency": concurrency,
            "concurrent_traffic": concurrent_traffic,
            "traffic": traffic
        });
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .expect("cannot create raw lifecycle export (never overwrites)");
        file.write_all(&serde_json::to_vec_pretty(&report).unwrap())
            .unwrap();
        file.write_all(b"\n").unwrap();
        file.sync_all().unwrap();
    }
}
