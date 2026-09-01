use super::*;
use crate::{JscErrorKind, JscStatus};
use std::cell::Cell;

const URL: &str = "test:///boundary.js";

struct DropProbe {
    drops: Rc<Cell<usize>>,
    thread: std::thread::ThreadId,
}
impl Drop for DropProbe {
    fn drop(&mut self) {
        assert_eq!(self.thread, std::thread::current().id());
        self.drops.set(self.drops.get() + 1);
    }
}
fn probe(drops: &Rc<Cell<usize>>) -> DropProbe {
    DropProbe {
        drops: Rc::clone(drops),
        thread: std::thread::current().id(),
    }
}

#[test]
fn callbacks_return_values_and_convert_arguments() {
    let vm = JscVm::new("callbacks").unwrap();
    let number = vm
        .host_function("sum", |args| {
            Ok(CallbackReturn::Number(
                args.iter().map(|arg| arg.to_number().unwrap()).sum(),
            ))
        })
        .unwrap();
    number.set_global("sum").unwrap();
    assert_eq!(vm.evaluate("sum(20, '22')", URL).unwrap(), "42");
    let string = vm
        .host_function("echo", |args| {
            Ok(CallbackReturn::String(args[0].to_string().unwrap()))
        })
        .unwrap();
    string.set_global("echo").unwrap();
    assert_eq!(vm.evaluate("echo('昆仑')", URL).unwrap(), "昆仑");
    let yes = vm
        .host_function("yes", |_| Ok(CallbackReturn::Boolean(true)))
        .unwrap();
    yes.set_global("yes").unwrap();
    assert_eq!(vm.evaluate("yes() === true", URL).unwrap(), "true");
    let empty = vm
        .host_function("empty", |_| Ok(CallbackReturn::Undefined))
        .unwrap();
    empty.set_global("empty").unwrap();
    assert_eq!(vm.evaluate("empty() === undefined", URL).unwrap(), "true");
}

#[test]
fn callback_drop_revokes_js_aliases_and_releases_on_isolate_thread() {
    let drops = Rc::new(Cell::new(0));
    let vm = JscVm::new("revoke").unwrap();
    for _ in 0..128 {
        let owned = probe(&drops);
        let function = vm
            .host_function("host", move |_| {
                let _ = &owned;
                Ok(CallbackReturn::Number(42.0))
            })
            .unwrap();
        function.set_global("host").unwrap();
        assert_eq!(
            vm.evaluate("globalThis.alias = host; alias()", URL)
                .unwrap(),
            "42"
        );
        vm.collect_garbage().unwrap();
        drop(function);
        assert!(
            vm.evaluate("alias()", URL)
                .unwrap_err()
                .exception_text()
                .unwrap()
                .contains("unavailable")
        );
        vm.evaluate("delete globalThis.alias; delete globalThis.host", URL)
            .unwrap();
        vm.collect_garbage().unwrap();
    }
    assert_eq!(drops.get(), 128);
    drop(vm);
    assert_eq!(drops.get(), 128);
}

#[test]
fn callback_failure_and_context_teardown_release_state_once() {
    let drops = Rc::new(Cell::new(0));
    let vm = JscVm::new("failures").unwrap();
    let owned = probe(&drops);
    assert!(
        vm.host_function("bad\0name", move |_| {
            let _ = &owned;
            Ok(CallbackReturn::Undefined)
        })
        .is_err()
    );
    assert_eq!(drops.get(), 1);
    vm.evaluate(
        "Object.defineProperty(globalThis, 'bad', {set(value) { globalThis.failedAlias = value; throw Error('denied') }})",
        URL,
    )
    .unwrap();
    let owned = probe(&drops);
    {
        let function = vm
            .host_function("throws", move |_| {
                let _ = &owned;
                Err("host failure".into())
            })
            .unwrap();
        assert!(
            function
                .set_global("bad")
                .unwrap_err()
                .exception_text()
                .unwrap()
                .contains("denied")
        );
        function.set_global("throws").unwrap();
        assert_eq!(
            vm.evaluate("try { throws() } catch (e) { e.message }", URL)
                .unwrap(),
            "host failure"
        );
    }
    assert_eq!(drops.get(), 2);
    assert!(
        vm.evaluate("failedAlias()", URL)
            .unwrap_err()
            .exception_text()
            .unwrap()
            .contains("unavailable")
    );
    drop(vm);
    assert_eq!(drops.get(), 2);
}

#[test]
fn caught_panics_and_panicking_payload_destructors_cannot_unwind_into_jsc() {
    struct PanicOnDrop;
    impl Drop for PanicOnDrop {
        fn drop(&mut self) {
            panic!("payload drop");
        }
    }
    let vm = JscVm::new("panic").unwrap();
    let function = vm
        .host_function("panicHost", |_| std::panic::panic_any(PanicOnDrop))
        .unwrap();
    function.set_global("panicHost").unwrap();
    assert_eq!(vm.evaluate("try { panicHost() } catch (e) { e instanceof Error && e.message.includes('panicked') }", URL).unwrap(), "true");
    assert_eq!(vm.evaluate("21 * 2", URL).unwrap(), "42");
}

#[test]
fn reentrant_callback_conversion_does_not_borrow_registry_mutably() {
    let vm = JscVm::new("reentry").unwrap();
    let function = vm
        .host_function("echo", |args| {
            Ok(CallbackReturn::String(
                args[0].to_string().map_err(|e| e.to_string())?,
            ))
        })
        .unwrap();
    function.set_global("echo").unwrap();
    assert_eq!(
        vm.evaluate("echo({toString() { return echo('nested') }})", URL)
            .unwrap(),
        "nested"
    );
    assert!(
        vm.evaluate("echo({toString() { throw Error('conversion') }})", URL)
            .unwrap_err()
            .exception_text()
            .unwrap()
            .contains("conversion")
    );
}

#[test]
fn dropping_own_handle_during_callback_keeps_active_state_alive() {
    // A leaked VM permits a 'static closure to hold its own callback handle.
    // The API must remain sound even in this unusual but safe Rust program.
    let vm = Box::leak(Box::new(JscVm::new("self-drop").unwrap()));
    let handle: Rc<RefCell<Option<HostFunction<'static>>>> = Rc::new(RefCell::new(None));
    let drops = Rc::new(Cell::new(0));
    let owned = probe(&drops);
    let captured = Rc::clone(&handle);
    let function = vm
        .host_function("selfDrop", move |_| {
            captured.borrow_mut().take();
            assert_eq!(owned.drops.get(), 0);
            Ok(CallbackReturn::Number(42.0))
        })
        .unwrap();
    function.set_global("selfDrop").unwrap();
    *handle.borrow_mut() = Some(function);
    assert_eq!(vm.evaluate("selfDrop()", URL).unwrap(), "42");
    assert_eq!(drops.get(), 1);
    assert!(vm.evaluate("selfDrop()", URL).is_err());
}

#[test]
fn array_buffer_copies_rust_input_and_shares_bytes_with_js_views() {
    let vm = JscVm::new("buffers").unwrap();
    let mut input = vec![1, 2, 3, 4, 5, 6, 7, 8];
    let buffer = vm.array_buffer(&input).unwrap();
    input.fill(99);
    drop(input);
    let view = buffer.typed_array(TypedArrayKind::Uint8, 2, 4).unwrap();
    view.set_global("view").unwrap();
    assert_eq!(view.kind(), TypedArrayKind::Uint8);
    assert_eq!(view.byte_offset(), 2);
    assert_eq!(view.len().unwrap(), 4);
    assert_eq!(view.to_vec().unwrap(), vec![3, 4, 5, 6]);
    view.write(1, &[42, 43]).unwrap();
    assert_eq!(vm.evaluate("view.join(',')", URL).unwrap(), "3,42,43,6");
    vm.evaluate("view[0] = 9", URL).unwrap();
    assert_eq!(buffer.to_vec().unwrap(), vec![1, 2, 9, 42, 43, 6, 7, 8]);
    drop(buffer);
    vm.collect_garbage().unwrap();
    assert_eq!(view.to_vec().unwrap(), vec![9, 42, 43, 6]);
    let clone = view.clone();
    drop(view);
    assert_eq!(clone.to_vec().unwrap(), vec![9, 42, 43, 6]);
}

#[test]
fn every_typed_array_kind_matches_js_layout() {
    use TypedArrayKind::*;
    let kinds = [
        (Int8, "Int8Array"),
        (Uint8, "Uint8Array"),
        (Uint8Clamped, "Uint8ClampedArray"),
        (Int16, "Int16Array"),
        (Uint16, "Uint16Array"),
        (Int32, "Int32Array"),
        (Uint32, "Uint32Array"),
        (Float32, "Float32Array"),
        (Float64, "Float64Array"),
        (BigInt64, "BigInt64Array"),
        (BigUint64, "BigUint64Array"),
    ];
    let vm = JscVm::new("kinds").unwrap();
    let buffer = vm.array_buffer(&[0; 32]).unwrap();
    for (kind, name) in kinds {
        let size = kind.element_size();
        let view = buffer.typed_array(kind, size, 2).unwrap();
        view.set_global("view").unwrap();
        assert_eq!(vm.evaluate("[view.constructor.name, view.BYTES_PER_ELEMENT, view.byteOffset, view.length, view.byteLength].join('|')", URL).unwrap(), format!("{name}|{size}|{size}|2|{}", size * 2));
    }
}

#[test]
fn typed_buffers_reject_alignment_overflow_and_out_of_bounds() {
    let vm = JscVm::new("ranges").unwrap();
    let buffer = vm.array_buffer(&[0; 16]).unwrap();
    assert_eq!(
        buffer
            .typed_array(TypedArrayKind::Uint32, 1, 1)
            .err()
            .unwrap()
            .status(),
        Some(JscStatus::Misaligned)
    );
    assert_eq!(
        buffer
            .typed_array(TypedArrayKind::Float64, 0, usize::MAX)
            .err()
            .unwrap()
            .status(),
        Some(JscStatus::OutOfBounds)
    );
    assert_eq!(
        buffer
            .typed_array(TypedArrayKind::Uint8, usize::MAX, 0)
            .err()
            .unwrap()
            .status(),
        Some(JscStatus::OutOfBounds)
    );
    assert_eq!(
        buffer.write(16, &[1]).unwrap_err().status(),
        Some(JscStatus::OutOfBounds)
    );
    let view = buffer.typed_array(TypedArrayKind::Uint16, 4, 2).unwrap();
    assert_eq!(
        view.write(4, &[1]).unwrap_err().status(),
        Some(JscStatus::OutOfBounds)
    );
    assert_eq!(
        view.read(usize::MAX, &mut []).unwrap_err().status(),
        Some(JscStatus::OutOfBounds)
    );
    assert_eq!(buffer.to_vec().unwrap(), vec![0; 16]);
}

#[test]
fn zero_length_buffers_and_end_views_are_valid() {
    let vm = JscVm::new("empty").unwrap();
    for size in [0, 8] {
        let buffer = vm.array_buffer(&vec![0; size]).unwrap();
        buffer.read(size, &mut []).unwrap();
        buffer.write(size, &[]).unwrap();
        let view = buffer
            .typed_array(TypedArrayKind::Float64, size, 0)
            .unwrap();
        assert!(view.is_empty().unwrap());
        assert_eq!(view.to_vec().unwrap(), vec![]);
        assert_eq!(buffer.to_vec().unwrap(), vec![0; size]);
    }
}

#[test]
fn detached_buffers_fail_without_confusing_them_with_empty_buffers() {
    let vm = JscVm::new("detach").unwrap();
    for size in [0, 8] {
        let buffer = vm.array_buffer(&vec![0; size]).unwrap();
        let view = buffer.typed_array(TypedArrayKind::Uint8, 0, size).unwrap();
        buffer.set_global("buffer").unwrap();
        let detached = vm
            .evaluate(
                r#"(function () {
                    if (typeof structuredClone !== "function") return "unsupported";
                    var probe = new ArrayBuffer(1);
                    try { structuredClone(probe, {transfer: [probe]}); }
                    catch (_) { return "unsupported"; }
                    try { new Uint8Array(probe); return "unsupported"; }
                    catch (error) { if (!(error instanceof TypeError)) throw error; }
                    globalThis.transferred = structuredClone(buffer, {transfer: [buffer]});
                    try { new Uint8Array(buffer); return "false"; }
                    catch (error) {
                        if (error instanceof TypeError) return "true";
                        throw error;
                    }
                })()"#,
                URL,
            )
            .unwrap();
        if detached == "unsupported" {
            return;
        }
        assert_eq!(detached, "true");
        assert_eq!(
            buffer.len().unwrap_err().kind(),
            JscErrorKind::JavaScriptException
        );
        assert!(buffer.read(0, &mut []).is_err());
        assert!(buffer.write(0, &[]).is_err());
        assert!(buffer.typed_array(TypedArrayKind::Uint8, 0, 0).is_err());
        assert!(view.len().is_err());
        assert!(view.read(0, &mut []).is_err());
    }
}

#[test]
fn gc_stress_retains_js_buffers_after_all_rust_roots_drop() {
    for _ in 0..16 {
        let vm = JscVm::new("stress").unwrap();
        for i in 0..64 {
            let buffer = vm.array_buffer(&[i; 128]).unwrap();
            let view = buffer.typed_array(TypedArrayKind::Uint8, 0, 128).unwrap();
            view.set_global("saved").unwrap();
            drop(view);
            drop(buffer);
            vm.collect_garbage().unwrap();
            assert_eq!(vm.evaluate("saved[127]", URL).unwrap(), i.to_string());
        }
    }
}
