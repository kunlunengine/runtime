use kunlun_jsc::{ContextGroup, JscStatus, JscVm, PromiseRejectionTransition as Transition};

fn checkpoint(vm: &JscVm) {
    assert!(!vm.microtask_checkpoint().unwrap());
}

#[test]
fn system_backend_reports_unsupported_checkpoint() {
    if JscVm::backend_info().supports_explicit_microtask_checkpoint {
        return;
    }
    let vm = JscVm::new("system-checkpoint").unwrap();
    assert_eq!(
        vm.microtask_checkpoint().unwrap_err().status(),
        Some(JscStatus::Unsupported)
    );
}

#[test]
fn nested_jobs_wait_for_explicit_checkpoint_and_preserve_fifo() {
    if !JscVm::backend_info().supports_explicit_microtask_checkpoint {
        return;
    }
    let vm = JscVm::new("fifo").unwrap();
    vm.evaluate("globalThis.order = []; Promise.resolve().then(() => { order.push('a'); Promise.resolve().then(() => order.push('c')); }); Promise.resolve().then(() => order.push('b'));", "test:///fifo.js").unwrap();
    assert_eq!(
        vm.evaluate("order.join(',')", "test:///read.js").unwrap(),
        ""
    );
    vm.set_name("unrelated-call").unwrap();
    checkpoint(&vm);
    assert_eq!(
        vm.evaluate("order.join(',')", "test:///read.js").unwrap(),
        "a,b,c"
    );
    checkpoint(&vm);
    assert!(vm.take_promise_rejections().is_empty());
}

#[test]
fn rejection_snapshot_suppresses_same_turn_handlers_and_reports_late_transition() {
    if !JscVm::backend_info().supports_explicit_microtask_checkpoint {
        return;
    }
    let vm = JscVm::new("rejections").unwrap();
    vm.evaluate("Promise.reject('sync').catch(() => {}); const p = Promise.reject('microtask'); Promise.resolve().then(() => p.catch(() => {})); globalThis.late = Promise.reject(new Error('late boom'));", "test:///rejections.js").unwrap();
    assert!(vm.take_promise_rejections().is_empty());
    checkpoint(&vm);
    let events = vm.take_promise_rejections();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].transition, Transition::Unhandled);
    assert_eq!(events[0].isolate_id, vm.isolate_id());
    assert_eq!(
        events[0].exception.source_url(),
        Some("test:///rejections.js")
    );
    assert!(
        events[0]
            .exception
            .exception_text()
            .unwrap()
            .contains("late boom")
    );
    checkpoint(&vm);
    assert!(vm.take_promise_rejections().is_empty());
    vm.evaluate("late.catch(() => {});", "test:///handler.js")
        .unwrap();
    checkpoint(&vm);
    let handled = vm.take_promise_rejections();
    assert_eq!(handled.len(), 1);
    assert_eq!(handled[0].transition, Transition::Handled);
    assert_eq!(handled[0].rejection_id, events[0].rejection_id);
    assert_eq!(
        handled[0].exception.source_url(),
        events[0].exception.source_url()
    );
    drop(vm);
    assert!(events[0].exception.to_string().contains("late boom"));
}

#[test]
fn contexts_in_one_group_have_independent_queues_and_rejection_ids() {
    if !JscVm::backend_info().supports_explicit_microtask_checkpoint {
        return;
    }
    let group = ContextGroup::new().unwrap();
    let first = group.create_vm("same-name").unwrap();
    let second = group.create_vm("same-name").unwrap();
    for vm in [&first, &second] {
        vm.evaluate("globalThis.done = false; Promise.resolve().then(() => { done = true; throw 'boom'; });", "test:///group.js").unwrap();
    }
    checkpoint(&first);
    assert_eq!(second.evaluate("done", "test:///read.js").unwrap(), "false");
    let events = first.take_promise_rejections();
    assert_eq!(events.len(), 1);
    assert!(second.take_promise_rejections().is_empty());
    checkpoint(&second);
    assert_ne!(
        events[0].isolate_id,
        second.take_promise_rejections()[0].isolate_id
    );
}

#[test]
fn rejection_conversion_is_reentrant_safe_and_new_work_waits_for_next_checkpoint() {
    if !JscVm::backend_info().supports_explicit_microtask_checkpoint {
        return;
    }
    let vm = JscVm::new("conversion").unwrap();
    vm.evaluate("globalThis.converted = 0; globalThis.pending = Promise.reject({ toString() { converted++; pending.catch(() => {}); Promise.resolve().then(() => globalThis.followup = 42); return 'custom reason'; }, get stack() { throw new Error('hostile stack getter'); } });", "test:///conversion.js").unwrap();
    assert!(vm.microtask_checkpoint().unwrap());
    let events = vm.take_promise_rejections();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].transition, Transition::Unhandled);
    assert_eq!(
        vm.evaluate("typeof followup", "test:///read.js").unwrap(),
        "undefined"
    );
    checkpoint(&vm);
    let handled = vm.take_promise_rejections();
    assert_eq!(handled.len(), 1);
    assert_eq!(handled[0].transition, Transition::Handled);
    // Diagnostic conversion may itself enqueue another job.
    while vm.microtask_checkpoint().unwrap() {}
    assert!(vm.take_promise_rejections().is_empty());
    assert_eq!(vm.evaluate("followup", "test:///read.js").unwrap(), "42");
}
