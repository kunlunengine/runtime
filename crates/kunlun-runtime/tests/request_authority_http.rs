//! Shared HTTP observations. Development output is never pinned-platform evidence.
use kunlun_runtime::{
    AdmissionPolicy, HostPermissions, RequestContext, RuntimeResourceCounts, ShutdownOutcome,
    TokioIsolate, admit_artifact,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;

const DEADLINE: Duration = Duration::from_secs(10);
const PROBE: &str = include_str!("fixtures/request-authority-http.js");
const CONTRACT: &str = include_str!("fixtures/request-authority-http.contract.json");
const REVOCATION: &str = include_str!("fixtures/request-authority-http-revocation.js");

fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

struct Artifact(PathBuf);

impl Artifact {
    fn new(contract: &Value) -> Self {
        let root =
            std::env::temp_dir().join(format!("kunlun-http-authority-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let artifact = Self(root);
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/runtime-manifest/v1");
        for path in [
            "manifest.json",
            "server.mjs",
            "chunks/问候% space.mjs",
            "maps/server.mjs.map",
            "assets/help/欢迎 组件.svg",
        ] {
            let destination = artifact.0.join(path);
            fs::create_dir_all(destination.parent().unwrap()).unwrap();
            fs::copy(source.join(path), destination).unwrap();
        }
        let mut manifest: Value =
            serde_json::from_slice(&fs::read(artifact.0.join("manifest.json")).unwrap()).unwrap();
        manifest["capabilities"] = contract["setup"]["declarations"].clone();
        fs::write(
            artifact.0.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        artifact
    }

    fn policy(&self, permissions: HostPermissions) -> AdmissionPolicy {
        AdmissionPolicy::new(
            hash(&fs::read(self.0.join("manifest.json")).unwrap()),
            permissions,
        )
    }
}

impl Drop for Artifact {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

struct Server {
    address: std::net::SocketAddr,
    stop: Arc<AtomicBool>,
    requests: Arc<Mutex<Vec<String>>>,
    thread: Option<std::thread::JoinHandle<()>>,
    receipt: Option<oneshot::Receiver<()>>,
    release: std::sync::mpsc::Sender<()>,
}

impl Server {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stopped = Arc::clone(&stop);
        let received = Arc::clone(&requests);
        let (receipt_sender, receipt) = oneshot::channel();
        let (release, released) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let mut receipt_sender = Some(receipt_sender);
            loop {
                let (mut stream, _) = listener.accept().unwrap();
                if stopped.load(Ordering::Acquire) {
                    break;
                }
                stream.set_read_timeout(Some(DEADLINE)).unwrap();
                stream.set_write_timeout(Some(DEADLINE)).unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    if stream.read_exact(&mut byte).is_err() {
                        break;
                    }
                    request.push(byte[0]);
                    assert!(request.len() < 8192, "unbounded fixture request");
                }
                let request = String::from_utf8(request).unwrap();
                let route = request.split_whitespace().nth(1).unwrap_or("/incomplete");
                received.lock().unwrap().push(route.to_owned());
                if route == "/pending" {
                    receipt_sender.take().unwrap().send(()).unwrap();
                    released.recv_timeout(DEADLINE).unwrap();
                }
                let response = if route == "/redirect" {
                    format!(
                        "HTTP/1.1 302 Found\r\nLocation: http://localhost:{}/forbidden?auth=auth-private&provider=provider-private&billing=billing-private\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        address.port()
                    )
                } else {
                    let body = if route == "/ok" {
                        "你好, scoped HTTP"
                    } else {
                        "unexpected destination reached"
                    };
                    format!(
                        "HTTP/1.1 200 OK\r\nX-Authority-Probe: http\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                };
                let _ = stream.write_all(response.as_bytes());
            }
        });
        Self {
            address,
            stop,
            requests,
            thread: Some(thread),
            receipt: Some(receipt),
            release,
        }
    }

    fn finish(&mut self) -> Vec<String> {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = self.release.send(());
            // Wake blocking accept, then join; no sleep determines fixture ordering.
            TcpStream::connect(self.address).unwrap();
            thread.join().unwrap();
        }
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.finish();
    }
}

#[test]
fn shared_http_authority_observations_match_contract() {
    let contract: Value = serde_json::from_str(CONTRACT).unwrap();
    let fixture = Artifact::new(&contract);
    assert!(admit_artifact(&fixture.0, &fixture.policy(HostPermissions::none())).is_err());
    let mut permissions = HostPermissions::none();
    for host in contract["setup"]["deployment_hosts"].as_array().unwrap() {
        permissions = permissions.allow_net_host(host.as_str().unwrap());
    }
    let artifact = admit_artifact(&fixture.0, &fixture.policy(permissions)).unwrap();
    let authority = artifact.authority();
    let mut isolate = TokioIsolate::new_with_authority("shared-http-authority", authority).unwrap();
    let mut server = Server::new();
    isolate
        .evaluate(
            &format!(
                "globalThis.requestAuthorityHttpInputs = Object.freeze({});",
                json!({"base": format!("http://{}", server.address)})
            ),
            "test:///http-authority-inputs.js",
        )
        .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut observations = Vec::new();
    for request in 0..2 {
        let mut context = RequestContext::new();
        context.insert("auth", format!("auth-private-{request}"));
        context.insert("provider", format!("provider-private-{request}"));
        context.insert("billing", format!("billing-private-{request}"));
        let output = runtime
            .block_on(async {
                tokio::time::timeout(
                    DEADLINE,
                    isolate.evaluate_request_body(
                        authority.begin_request(context).unwrap(),
                        PROBE,
                        "test:///request-authority-http.js",
                    ),
                )
                .await
            })
            .expect("HTTP probe deadline")
            .unwrap();
        observations.push(serde_json::from_str::<Value>(&output).unwrap());
    }
    let receipt = server.receipt.take().unwrap();
    let request = authority.begin_request(RequestContext::new()).unwrap();
    let pending = runtime.block_on(async {
        let evaluation = isolate.evaluate_request_body(
            request,
            REVOCATION,
            "test:///request-authority-http-revocation.js",
        );
        let revoke = async {
            tokio::time::timeout(DEADLINE, receipt)
                .await
                .expect("pending request receipt deadline")
                .unwrap();
            authority.revoke();
        };
        let (result, ()) = tokio::join!(tokio::time::timeout(DEADLINE, evaluation), revoke);
        result.expect("revocation must wake invocation before server response")
    });
    let lifecycle = json!({
        "pending_rejected_before_response": pending.is_err(),
        "later_admission_denied": authority.begin_request(RequestContext::new()).is_err(),
    });
    assert_eq!(lifecycle, contract["expected_lifecycle"]);
    assert_eq!(
        runtime.block_on(isolate.shutdown(DEADLINE)).unwrap(),
        ShutdownOutcome::Graceful
    );
    assert_eq!(isolate.resource_counts(), RuntimeResourceCounts::default());
    let requests = server.finish();
    assert_eq!(json!(requests), contract["expected_requests"]);
    assert_eq!(json!(observations), contract["expected_observations"]);
    // Emit only after bounded shutdown, server join and fixture removal.
    drop(isolate);
    drop(artifact);
    drop(fixture);
    if let Some(path) = std::env::var_os("KUNLUN_M3_HTTP_OBSERVATIONS") {
        let report = json!({
            "schema_version": 1,
            "suite": contract["suite"],
            "adapter": "native",
            "status": "development",
            "qualification": false,
            "fixture_sha256": hash(PROBE.as_bytes()),
            "contract_sha256": hash(CONTRACT.as_bytes()),
            "revocation_sha256": hash(REVOCATION.as_bytes()),
            "observations": observations,
            "lifecycle": lifecycle,
            "requests": requests,
        });
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .unwrap();
        writeln!(file, "{}", serde_json::to_string_pretty(&report).unwrap()).unwrap();
    }
}
