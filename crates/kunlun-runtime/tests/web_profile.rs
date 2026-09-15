use kunlun_runtime::TokioIsolate;
use std::{cell::RefCell, rc::Rc};

#[tokio::test(flavor = "current_thread")]
async fn web_profile_conformance() {
    let mut isolate = TokioIsolate::new("web-profile").unwrap();
    assert_eq!(
        isolate
            .evaluate_async_body(
                include_str!("fixtures/web-profile.js"),
                "test:///web-profile.js"
            )
            .await
            .unwrap(),
        "web-profile-ok"
    );
}

#[test]
fn console_has_bounded_structured_records_and_source_context() {
    let mut isolate = TokioIsolate::new("console").unwrap();
    let records = Rc::new(RefCell::new(Vec::new()));
    let captured = Rc::clone(&records);
    isolate.set_console_sink(move |record| captured.borrow_mut().push(record.clone()));
    isolate.evaluate("const cycle = {}; cycle.self = cycle; console.warn({z:1,a:2}, cycle); console.error('😀'.repeat(10000));", "test:///console.js").unwrap();
    let records = records.borrow();
    assert_eq!(records[0].level, "warn");
    assert_eq!(records[0].message, "{ a: 2, z: 1 } { self: [Circular] }");
    assert!(
        records[0].source.contains("test:///console.js"),
        "{}",
        records[0].source
    );
    assert_eq!(records[1].level, "error");
    assert!(records[1].message.len() <= 8192);
}

#[tokio::test(flavor = "current_thread")]
async fn host_readable_stream_supports_slow_consumers_and_abort() {
    use kunlun_runtime::HostPermissions;
    let root = std::env::temp_dir().join(format!("kunlun-web-stream-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("bytes");
    std::fs::write(&path, vec![42_u8; 200_000]).unwrap();
    let permissions = HostPermissions::none().allow_read_root(&root).unwrap();
    let mut isolate = TokioIsolate::new_with_permissions("web-stream", permissions).unwrap();
    let source = format!(
        r#"
        const fs = await kunlun.import('kunlun:fs');
        const source = await fs.openReadStream({path});
        const readable = source.toReadableStream();
        let length = 0;
        await readable.pipeTo(new WritableStream({{ async write(chunk) {{
            await sleep(1); length += chunk.length;
            if (chunk.some(byte => byte !== 42)) throw new Error('corrupt bytes');
        }} }}));
        if (length !== 200000) throw new Error('truncated stream');
        const abort = new AbortController();
        const second = (await fs.openReadStream({path}, {{ signal: abort.signal }})).toReadableStream();
        const reader = second.getReader();
        await reader.read();
        const reason = new Error('cancel-host');
        abort.abort(reason);
        try {{ await reader.read(); throw new Error('missing abort'); }}
        catch (error) {{ if (error !== reason) throw error; }}
        await reader.closed.catch(() => {{}});
        const third = (await fs.openReadStream({path})).toReadableStream();
        await third.cancel('unused');
        return 'host-stream-ok';
    "#,
        path = serde_json::to_string(path.to_str().unwrap()).unwrap()
    );
    let result = isolate
        .evaluate_async_body(&source, "test:///host-stream.js")
        .await;
    std::fs::remove_dir_all(root).unwrap();
    assert_eq!(result.unwrap(), "host-stream-ok");
}
