//! Shared HTTP observations. Development output is never pinned-platform evidence.
use kunlun_runtime::{
    AdmissionPolicy, HostPermissions, RequestContext, RuntimeResourceCounts, ShutdownOutcome,
    TokioIsolate, admit_artifact,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;

const DEADLINE: Duration = Duration::from_secs(10);
const PROBE: &str = include_str!("fixtures/request-authority-http.js");
const CONTRACT: &str = include_str!("fixtures/request-authority-http.contract.json");
const REVOCATION: &str = include_str!("fixtures/request-authority-http-revocation.js");

const MAX_HEADERS: usize = 16 * 1024;
const MAX_BODY: usize = 1024 * 1024;
const MAX_CHUNKS: usize = 4096;

struct HttpRequest {
    method: String,
    route: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl HttpRequest {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn request_line(reader: &mut impl Read, budget: &mut usize) -> io::Result<String> {
    let mut bytes = Vec::new();
    loop {
        if *budget == 0 {
            return Err(invalid("fixture request line/header limit exceeded"));
        }
        *budget -= 1;
        let mut byte = [0];
        reader.read_exact(&mut byte)?;
        bytes.push(byte[0]);
        if bytes.ends_with(b"\r\n") {
            bytes.truncate(bytes.len() - 2);
            return String::from_utf8(bytes).map_err(|_| invalid("non-UTF8 fixture request line"));
        }
    }
}

fn read_request(reader: &mut impl Read) -> io::Result<HttpRequest> {
    let mut budget = MAX_HEADERS;
    let first = request_line(reader, &mut budget)?;
    let parts: Vec<_> = first.split_whitespace().collect();
    if parts.len() != 3 || parts[2] != "HTTP/1.1" {
        return Err(invalid("invalid fixture request line or HTTP version"));
    }
    let mut request = HttpRequest {
        method: parts[0].to_owned(),
        route: parts[1].to_owned(),
        headers: Vec::new(),
        body: Vec::new(),
    };
    loop {
        let line = request_line(reader, &mut budget)?;
        if line.is_empty() {
            break;
        }
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| invalid("invalid fixture header"))?;
        let name = name.to_ascii_lowercase();
        if name.is_empty() || request.headers.iter().any(|(key, _)| key == &name) {
            return Err(invalid("empty or duplicate fixture header"));
        }
        request.headers.push((name, value.trim().to_owned()));
    }
    if let Some(encoding) = request.header("transfer-encoding") {
        if encoding != "chunked" || request.header("content-length").is_some() {
            return Err(invalid("unsupported or ambiguous fixture framing"));
        }
        for chunk in 0..=MAX_CHUNKS {
            let mut line_budget = MAX_HEADERS;
            let line = request_line(reader, &mut line_budget)?;
            let size = usize::from_str_radix(line.split(';').next().unwrap(), 16)
                .map_err(|_| invalid("invalid fixture chunk size"))?;
            if size == 0 {
                // Consume bounded trailers, including the final empty line.
                let mut trailer_budget = MAX_HEADERS;
                while !request_line(reader, &mut trailer_budget)?.is_empty() {}
                return Ok(request);
            }
            if chunk == MAX_CHUNKS || size > MAX_BODY - request.body.len() {
                return Err(invalid("fixture chunk count or body limit exceeded"));
            }
            let start = request.body.len();
            request.body.resize(start + size, 0);
            reader.read_exact(&mut request.body[start..])?;
            let mut end = [0; 2];
            reader.read_exact(&mut end)?;
            if end != *b"\r\n" {
                return Err(invalid("invalid fixture chunk ending"));
            }
        }
        unreachable!();
    }
    let length: usize = request
        .header("content-length")
        .unwrap_or("0")
        .parse()
        .map_err(|_| invalid("invalid fixture content-length"))?;
    if length > MAX_BODY {
        return Err(invalid("fixture body limit exceeded"));
    }
    request.body.resize(length, 0);
    reader.read_exact(&mut request.body)?;
    Ok(request)
}

#[test]
fn fixture_parser_reads_content_length_and_chunked_bodies() {
    let mut fixed =
        &b"POST /echo HTTP/1.1\r\nContent-Length: 6\r\nContent-Type: text/plain\r\n\r\nhello!"[..];
    let request = read_request(&mut fixed).unwrap();
    assert_eq!(request.method, "POST");
    assert_eq!(request.route, "/echo");
    assert_eq!(request.header("content-type"), Some("text/plain"));
    assert_eq!(request.header("cookie"), None);
    assert_eq!(request.body, b"hello!");
    let mut chunked = &b"PUT /replay/307 HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n2;extension=yes\r\nhe\r\n3\r\nllo\r\n0\r\nX-Trailer: ignored\r\n\r\nremaining"[..];
    let request = read_request(&mut chunked).unwrap();
    assert_eq!(request.body, b"hello");
    assert_eq!(chunked, b"remaining");
}

#[test]
fn fixture_parser_rejects_oversized_body_before_reading() {
    let source = format!(
        "POST /echo HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
        MAX_BODY + 1
    );
    assert_eq!(
        read_request(&mut source.as_bytes()).err().unwrap().kind(),
        io::ErrorKind::InvalidData
    );
}

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
    thread: Option<std::thread::JoinHandle<io::Result<()>>>,
    active: Arc<Mutex<Option<TcpStream>>>,
    receipt: Option<oneshot::Receiver<()>>,
    release: std::sync::mpsc::Sender<()>,
}

impl Server {
    fn new() -> Self {
        Self::with_accepted_signal(None)
    }

    fn with_accepted_signal(accepted: Option<std::sync::mpsc::Sender<()>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stopped = Arc::clone(&stop);
        let received = Arc::clone(&requests);
        let active = Arc::new(Mutex::new(None));
        let current = Arc::clone(&active);
        let (receipt_sender, receipt) = oneshot::channel();
        let (release, released) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let mut receipt_sender = Some(receipt_sender);
            loop {
                let (mut stream, _) = listener.accept()?;
                {
                    let mut active = current.lock().unwrap();
                    if stopped.load(Ordering::Acquire) {
                        return Ok(());
                    }
                    *active = Some(stream.try_clone()?);
                }
                stream.set_read_timeout(Some(DEADLINE))?;
                stream.set_write_timeout(Some(DEADLINE))?;
                if let Some(signal) = &accepted {
                    let _ = signal.send(());
                }
                let request = match read_request(&mut stream) {
                    Ok(request) => request,
                    Err(_) if stopped.load(Ordering::Acquire) => return Ok(()),
                    Err(error) => return Err(error),
                };
                let route = request.route.as_str();
                received.lock().unwrap().push(route.to_owned());
                if route == "/pending" {
                    receipt_sender
                        .take()
                        .ok_or_else(|| invalid("duplicate pending request"))?
                        .send(())
                        .map_err(|_| io::Error::other("pending receipt receiver closed"))?;
                    released
                        .recv_timeout(DEADLINE)
                        .map_err(|error| io::Error::other(format!("pending release: {error}")))?;
                }
                let response = if route == "/redirect" {
                    format!(
                        "HTTP/1.1 302 Found\r\nLocation: http://localhost:{}/forbidden?auth=auth-private&provider=provider-private&billing=billing-private\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        address.port()
                    )
                } else if let Some((status, location)) = match route {
                    "/same-origin" => Some(("302 Found", "/ok")),
                    "/loop" => Some(("302 Found", "/loop")),
                    "/rewrite/301" => Some(("301 Moved Permanently", "/echo")),
                    "/rewrite/302" => Some(("302 Found", "/echo")),
                    "/rewrite/303" => Some(("303 See Other", "/echo")),
                    "/replay/307" => Some(("307 Temporary Redirect", "/echo")),
                    "/replay/308" => Some(("308 Permanent Redirect", "/echo")),
                    _ => None,
                } {
                    format!(
                        "HTTP/1.1 {status}\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                } else {
                    let (body, extra) = match route {
                        "/ok" => ("你好, scoped HTTP".to_owned(), String::new()),
                        "/pending" => ("unexpected destination reached".to_owned(), String::new()),
                        "/echo" => {
                            let body = json!({
                                "method": request.method,
                                "body": std::str::from_utf8(&request.body).map_err(|_| invalid("non-UTF8 echo body"))?,
                                "content_type": request.header("content-type"),
                                "content_encoding": request.header("content-encoding"),
                                "content_language": request.header("content-language"),
                                "content_location": request.header("content-location"),
                                "authorization": request.header("authorization"),
                                "cookie": request.header("cookie"),
                                "custom": request.header("x-authority-custom"),
                            }).to_string();
                            let mut extra = format!(
                                "Content-Type: application/json\r\nX-Observed-Method: {}\r\n",
                                request.method
                            );
                            for (incoming, observed) in [
                                ("content-type", "Content-Type"),
                                ("content-encoding", "Content-Encoding"),
                                ("content-language", "Content-Language"),
                                ("content-location", "Content-Location"),
                                ("authorization", "Authorization"),
                                ("cookie", "Cookie"),
                                ("x-authority-custom", "Custom"),
                            ] {
                                if let Some(value) = request.header(incoming) {
                                    extra.push_str(&format!("X-Observed-{observed}: {value}\r\n"));
                                }
                            }
                            (body, extra)
                        }
                        _ => return Err(invalid(&format!("unexpected fixture route: {route}"))),
                    };
                    let length = body.len();
                    let payload = if request.method == "HEAD" { "" } else { &body };
                    format!(
                        "HTTP/1.1 200 OK\r\nX-Authority-Probe: http\r\n{extra}Content-Length: {length}\r\nConnection: close\r\n\r\n{payload}"
                    )
                };
                let _ = stream.write_all(response.as_bytes());
                *current.lock().unwrap() = None;
            }
        });
        Self {
            address,
            stop,
            requests,
            thread: Some(thread),
            active,
            receipt: Some(receipt),
            release,
        }
    }

    fn finish(&mut self) -> Vec<String> {
        self.shutdown().expect("fixture server failed");
        self.requests.lock().unwrap().clone()
    }

    fn shutdown(&mut self) -> io::Result<()> {
        {
            let active = self
                .active
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            self.stop.store(true, Ordering::Release);
            if let Some(stream) = active.as_ref() {
                let _ = stream.shutdown(Shutdown::Both);
            }
        }
        if let Some(thread) = self.thread.take() {
            let _ = self.release.send(());
            // Wake blocking accept, then join; no sleep determines fixture ordering.
            let _ = TcpStream::connect_timeout(&self.address, DEADLINE);
            return thread
                .join()
                .map_err(|_| io::Error::other("fixture server panicked"))?;
        }
        Ok(())
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

#[test]
fn fixture_head_echo_mirrors_received_headers_without_body() {
    let mut server = Server::new();
    for include_headers in [true, false] {
        let mut client = TcpStream::connect(server.address).unwrap();
        client.set_read_timeout(Some(DEADLINE)).unwrap();
        client.set_write_timeout(Some(DEADLINE)).unwrap();
        let mut upload = String::from("HEAD /echo HTTP/1.1\r\n");
        let headers = [
            ("Content-Type", "text/plain", "Content-Type"),
            ("Content-Encoding", "identity", "Content-Encoding"),
            ("Content-Language", "en", "Content-Language"),
            ("Content-Location", "/source", "Content-Location"),
            ("Authorization", "Bearer fixture", "Authorization"),
            ("Cookie", "fixture=yes", "Cookie"),
            ("X-Authority-Custom", "fixture", "Custom"),
        ];
        if include_headers {
            for (incoming, value, _) in headers {
                upload.push_str(&format!("{incoming}: {value}\r\n"));
            }
        }
        upload.push_str("\r\n");
        client.write_all(upload.as_bytes()).unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        let (head, body) = response.split_once("\r\n\r\n").unwrap();
        assert!(body.is_empty());
        assert!(head.contains("X-Observed-Method: HEAD\r\n"));
        for (_, value, observed) in headers {
            assert_eq!(
                head.contains(&format!("X-Observed-{observed}: {value}\r\n")),
                include_headers
            );
        }
    }
    assert_eq!(server.finish(), ["/echo", "/echo"]);
}

#[test]
fn fixture_parser_rejects_truncated_and_malformed_uploads() {
    for (source, kind) in [
        (
            &b"POST /echo HTTP/1.1\r\nContent-Length: 3\r\n\r\nx"[..],
            io::ErrorKind::UnexpectedEof,
        ),
        (
            &b"POST /echo HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nx"[..],
            io::ErrorKind::UnexpectedEof,
        ),
        (
            &b"POST /echo HTTP/1.1\r\nContent-Length: nope\r\n\r\n"[..],
            io::ErrorKind::InvalidData,
        ),
        (
            &b"POST /echo HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n1\r\nx!!"[..],
            io::ErrorKind::InvalidData,
        ),
        (
            &b"POST /echo HTTP/1.1\r\nbroken\r\n\r\n"[..],
            io::ErrorKind::InvalidData,
        ),
    ] {
        assert_eq!(read_request(&mut &source[..]).err().unwrap().kind(), kind);
    }
}

#[test]
fn fixture_server_reports_bad_uploads_without_panicking_on_drop() {
    for source in [
        &b"POST /echo HTTP/1.1\r\nContent-Length: 3\r\n\r\nx"[..],
        &b"POST /echo HTTP/1.1\r\nContent-Length: nope\r\n\r\n"[..],
    ] {
        let server = Server::new();
        let mut client = TcpStream::connect(server.address).unwrap();
        client.set_write_timeout(Some(DEADLINE)).unwrap();
        client.write_all(source).unwrap();
        client.shutdown(Shutdown::Write).unwrap();
        // Joining before stopping ensures the parse error is not mistaken for cancellation.
        let mut server = server;
        assert!(server.thread.take().unwrap().join().unwrap().is_err());
        drop(server);
    }
}

#[test]
fn fixture_server_drop_interrupts_stalled_upload_during_unwind() {
    let (accepted, receipt) = std::sync::mpsc::channel();
    let server = Server::with_accepted_signal(Some(accepted));
    let mut client = TcpStream::connect(server.address).unwrap();
    client.set_write_timeout(Some(DEADLINE)).unwrap();
    client
        .write_all(b"POST /echo HTTP/1.1\r\nContent-Length: 3\r\n\r\nx")
        .unwrap();
    receipt.recv_timeout(DEADLINE).unwrap();
    let start = std::time::Instant::now();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _server = server;
        panic!("intentional fixture unwind");
    }));
    assert!(result.is_err());
    assert!(
        start.elapsed() < DEADLINE / 2,
        "drop waited for socket timeout"
    );
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
