use kunlun_runtime::{HostPermissions, ShutdownOutcome, TokioIsolate};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

struct TestServer {
    base: String,
    stop: Arc<AtomicBool>,
    completed_failed_upload: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl TestServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let completed_failed_upload = Arc::new(AtomicBool::new(false));
        let worker_completed_failed_upload = Arc::clone(&completed_failed_upload);
        let worker = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !worker_stop.load(Ordering::Acquire) && Instant::now() < deadline {
                match listener.accept() {
                    Ok((stream, _)) => serve(stream, port, &worker_completed_failed_upload),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(error) => panic!("test server accept: {error}"),
                }
            }
        });
        Self {
            base,
            stop,
            completed_failed_upload,
            worker: Some(worker),
        }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
    }
}

fn serve(mut stream: TcpStream, port: u16, completed_failed_upload: &AtomicBool) {
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut first = String::new();
    if reader.read_line(&mut first).unwrap_or(0) == 0 {
        return;
    }
    let path = first.split_whitespace().nth(1).unwrap_or("/");
    let mut length = 0;
    let mut chunked = false;
    let mut multi_values = Vec::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        if line == "\r\n" {
            break;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(value) = lower.strip_prefix("x-multi:") {
            multi_values.push(value.trim().to_owned());
        }
        if let Some(value) = lower.strip_prefix("content-length:") {
            length = value.trim().parse().unwrap();
        }
        if lower.starts_with("transfer-encoding:") && lower.contains("chunked") {
            chunked = true;
        }
    }
    let mut body = Vec::new();
    if chunked {
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                return;
            }
            let size = usize::from_str_radix(line.trim().split(';').next().unwrap(), 16).unwrap();
            if size == 0 {
                break;
            }
            let old = body.len();
            body.resize(old + size, 0);
            if reader.read_exact(&mut body[old..]).is_err() {
                return;
            }
            let mut end = [0; 2];
            if reader.read_exact(&mut end).is_err() {
                return;
            }
            assert_eq!(end, *b"\r\n");
        }
    } else if length > 0 {
        body.resize(length, 0);
        if reader.read_exact(&mut body).is_err() {
            return;
        }
    }
    match path {
        "/redirect" => {
            let _ = write!(
                stream,
                "HTTP/1.1 302 Found\r\nLocation: /binary\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
            );
        }
        "/redirect-307" => {
            let _ = write!(
                stream,
                "HTTP/1.1 307 Temporary Redirect\r\nLocation: /echo\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
            );
        }
        "/deny-redirect" => {
            let _ = write!(
                stream,
                "HTTP/1.1 302 Found\r\nLocation: http://localhost:{port}/binary\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
            );
        }
        "/binary" => {
            let payload: Vec<u8> = (0..100_000).map(|n| (n % 256) as u8).collect();
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: {}\r\nX-Test: one\r\nX-Test: two\r\n\r\n",
                payload.len()
            );
            let _ = stream.write_all(&payload);
        }
        "/echo" => {
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(&body);
        }
        "/failed-upload" => {
            completed_failed_upload.store(true, Ordering::Release);
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
            );
        }
        "/verify-stream" => {
            let valid = body.len() == 12 * 131_072
                && body
                    .iter()
                    .enumerate()
                    .all(|(index, byte)| *byte == (index % 251) as u8);
            let answer = if valid { "stream-ok" } else { "stream-corrupt" };
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{answer}",
                answer.len()
            );
        }
        "/headers" => {
            let value = multi_values.join("|");
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{value}",
                value.len()
            );
        }
        "/empty" => {
            let _ = write!(
                stream,
                "HTTP/1.1 204 No Content\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
            );
        }
        "/head" => {
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
            );
        }
        "/broken" => {
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 10\r\n\r\nbad"
            );
        }
        "/slow" => {
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 4\r\n\r\n"
            );
            let _ = stream.flush();
            thread::sleep(Duration::from_millis(200));
            let _ = stream.write_all(b"slow");
        }
        _ => {
            let _ = write!(
                stream,
                "HTTP/1.1 404 Not Found\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
            );
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn fetch_objects_validate_and_consume_bodies() {
    let mut isolate = TokioIsolate::new("fetch-objects").unwrap();
    let result = isolate
        .evaluate_async_body(
            include_str!("fixtures/fetch-profile.js"),
            "test:///fetch-profile.js",
        )
        .await
        .unwrap();
    assert_eq!(result, "fetch-objects-ok");
    let shared = isolate
        .evaluate_async_body(
            include_str!("fixtures/fetch-shared.js"),
            "test:///fetch-shared.js",
        )
        .await
        .unwrap();
    assert_eq!(shared, "fetch-shared-ok");
}

#[tokio::test(flavor = "current_thread")]
async fn fetch_streams_uploads_redirects_and_denies_undeclared_hosts() {
    let server = TestServer::start();
    let mut denied = TokioIsolate::new_with_permissions(
        "fetch-denied",
        HostPermissions::none().allow_net_host("127.0.0.1"),
    )
    .unwrap();
    let source = format!(
        "try {{ await fetch({:?}); return 'unexpected'; }} catch (error) {{ return String(error).includes('Fetch capability denied') ? 'denied' : String(error); }}",
        server.base
    );
    assert_eq!(
        denied
            .evaluate_async_body(&source, "test:///deny.js")
            .await
            .unwrap(),
        "denied"
    );

    let permissions = HostPermissions::none().allow_fetch_host("127.0.0.1");
    let mut isolate = TokioIsolate::new_with_permissions("fetch-network", permissions).unwrap();
    let source = r#"
        const base = __BASE__;
        const check = (ok, message) => { if (!ok) throw Error(message); };
        const legacy = await kunlun.import('kunlun:http');
        try { await legacy.request(base + '/binary'); throw Error('legacy bypass'); }
        catch (error) { check(String(error).includes('network access denied'), 'legacy network isolation'); }
        const binary = await fetch(base + '/redirect');
        check(binary.redirected && binary.url.endsWith('/binary') && binary.status === 200, 'redirect metadata');
        check(binary.headers.get('x-test') === 'one, two', 'duplicate response headers');
        const reader = binary.body.getReader();
        let count = 0;
        for (;;) {
          const { done, value } = await reader.read();
          if (done) break;
          for (const byte of value) check(byte === count++ % 256, 'binary order');
        }
        reader.releaseLock();
        check(count === 100000, 'binary length');
        const text = await fetch(base + '/echo', { method: 'POST', body: '中文' });
        check(await text.text() === '中文', 'Unicode upload');
        const binaryUpload = await fetch(base + '/echo', { method: 'POST', body: new Uint8Array([0, 1, 255]) });
        check((await binaryUpload.bytes()).join(',') === '0,1,255', 'binary buffered upload');
        const replay = await fetch(base + '/redirect-307', { method: 'POST', body: 'replay' });
        check(replay.redirected && await replay.text() === 'replay', '307 body replay');
        const repeated = await fetch(base + '/headers', { headers: new Headers([['x-multi', 'one'], ['x-multi', 'two']]) });
        check(await repeated.text() === 'one|two', 'multiple request header values');
        const upload = new ReadableStream({
          pull(controller) {
            if (this.next === undefined) this.next = 0;
            if (this.next++ < 4) controller.enqueue(new Uint8Array([1, 2, 3, 255]));
            else controller.close();
          }
        });
        const echoed = await fetch(base + '/echo', { method: 'POST', body: upload, duplex: 'half' });
        check((await echoed.bytes()).join(',') === '1,2,3,255,'.repeat(4).slice(0, -1), 'stream upload order');
        const requestUpload = new Request(base + '/echo', { method: 'POST',
          body: new ReadableStream({ start(controller) {
            controller.enqueue(new Uint8Array([4, 5, 6])); controller.close();
          } }), duplex: 'half' });
        const requestEcho = await fetch(requestUpload);
        check(requestUpload.bodyUsed && (await requestEcho.bytes()).join(',') === '4,5,6', 'fetch(Request) stream upload');
        let uploadOffset = 0;
        const largeUpload = new ReadableStream({ pull(controller) {
          if (uploadOffset === 12 * 131072) { controller.close(); return; }
          const chunk = new Uint8Array(131072);
          for (let i = 0; i < chunk.length; i++) chunk[i] = (uploadOffset + i) % 251;
          uploadOffset += chunk.length;
          controller.enqueue(chunk);
        } }, { highWaterMark: 0 });
        const checked = await fetch(base + '/verify-stream', { method: 'POST', body: largeUpload, duplex: 'half' });
        check(await checked.text() === 'stream-ok', 'large upload order and bounded streaming');
        const empty = await fetch(base + '/empty');
        check(empty.status === 204 && empty.body === null && await empty.text() === '', 'null body');
        const head = await fetch(base + '/head', { method: 'HEAD' });
        check(head.status === 200 && head.body === null, 'HEAD null body');
        const missing = await fetch(base + '/missing');
        check(missing.status === 404 && !missing.ok, 'HTTP error status');
        await missing.body.cancel();
        const manual = await fetch(base + '/redirect', { redirect: 'manual' });
        check(manual.status === 302 && manual.headers.get('location') === '/binary', 'manual redirect');
        await manual.body.cancel();
        try { await fetch(base + '/redirect', { redirect: 'error' }); throw Error('redirect followed'); }
        catch (error) { check(String(error).includes('redirect disallowed'), 'redirect error mode'); }
        try { await fetch(base + '/deny-redirect'); throw Error('redirect escaped'); }
        catch (error) { check(String(error).includes('Fetch capability denied'), 'redirect denial'); }
        try { await (await fetch(base + '/broken')).text(); throw Error('broken body resolved'); }
        catch (error) { check(String(error).includes('body') || String(error).includes('request'), 'producer error'); }
        const failedUpload = new ReadableStream({ pull(controller) {
          if (!this.sent) { this.sent = true; controller.enqueue(new Uint8Array([1, 2, 3])); }
          else throw Error('upload producer failed');
        } }, { highWaterMark: 0 });
        try { await fetch(base + '/failed-upload', { method: 'POST', body: failedUpload }); throw Error('failed upload resolved'); }
        catch (error) { check(String(error).includes('upload producer failed'), 'upload producer error'); }
        const abort = new AbortController();
        const reason = Error('test abort');
        abort.abort(reason);
        try { await fetch(base + '/binary', { signal: abort.signal }); throw Error('pre-abort missed'); }
        catch (error) { check(error === reason, 'pre-abort identity'); }
        const pending = new AbortController();
        const slow = await fetch(base + '/slow', { signal: pending.signal });
        const read = slow.body.getReader().read();
        await sleep(10);
        const pendingReason = Error('pending abort');
        pending.abort(pendingReason);
        try { await read; throw Error('pending read resolved'); }
        catch (error) { check(error === pendingReason, 'pending abort identity'); }
        const idle = new AbortController();
        const idleResponse = await fetch(base + '/slow', { signal: idle.signal });
        const idleReason = Error('idle abort');
        idle.abort(idleReason);
        try { await idleResponse.body.getReader().read(); throw Error('idle read resolved'); }
        catch (error) { check(error === idleReason, 'post-header idle abort identity'); }
        return 'fetch-network-ok';
    "#.replace("__BASE__", &serde_json::to_string(&server.base).unwrap());
    assert_eq!(
        isolate
            .evaluate_async_body(&source, "test:///fetch-network.js")
            .await
            .unwrap(),
        "fetch-network-ok"
    );
    let counts = isolate.resource_counts();
    assert_eq!(counts.pending_host_calls, 0, "{counts:?}");
    assert_eq!(counts.request_ids, 0, "{counts:?}");
    assert_eq!(counts.streams, 0, "{counts:?}");
    assert_eq!(counts.uploads, 0, "{counts:?}");
    assert!(
        !server.completed_failed_upload.load(Ordering::Acquire),
        "truncated upload was accepted as complete"
    );
    assert_eq!(
        isolate.shutdown(Duration::from_secs(1)).await.unwrap(),
        ShutdownOutcome::Graceful
    );
    assert_eq!(isolate.resource_counts(), Default::default());
}
