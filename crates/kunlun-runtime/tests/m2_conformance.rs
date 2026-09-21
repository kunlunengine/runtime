//! The same checked-in corpus runs on every bundled release target.
//! Capability failures are failures, never skips. Timeouts bound hangs; no
//! assertion depends on sleeping long enough for another actor to make progress.

use kunlun_jsc::{JscVm, PromiseRejectionTransition, ResourcePolicy};
use kunlun_runtime::{
    HostPermissions, ModuleSources, RuntimeError, RuntimeLimits, RuntimeResourceCounts,
    ShutdownOutcome, TerminationReason, TokioIsolate,
};
use std::future::{Future, poll_fn};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::Poll;
use std::time::Duration;

const BOUND: Duration = Duration::from_secs(10);

fn run(future: impl Future<Output = ()>) {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future);
}

fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/m2")
}

fn capabilities() {
    let info = JscVm::backend_info();
    assert!(info.hermetic, "{info:?}");
    assert!(info.supports_native_modules, "{info:?}");
    assert!(info.supports_deferred_promises, "{info:?}");
    assert!(info.supports_explicit_microtask_checkpoint, "{info:?}");
    assert!(info.supports_execution_watchdog, "{info:?}");
    assert!(info.supports_heap_telemetry, "{info:?}");
}

fn isolate(root: &Path) -> TokioIsolate {
    capabilities();
    let permissions = HostPermissions::none().allow_read_root(root).unwrap();
    let mut isolate = TokioIsolate::new_with_permissions("m2-conformance", permissions).unwrap();
    isolate
        .install_module_sources(ModuleSources::new(corpus()).unwrap())
        .unwrap();
    isolate
}

fn set_path(isolate: &mut TokioIsolate, path: &Path) {
    let path = serde_json::to_string(path.to_str().unwrap()).unwrap();
    isolate
        .evaluate(
            &format!("globalThis.m2Path = {path}"),
            "test:///m2/setup.js",
        )
        .unwrap();
}

async fn body(isolate: &mut TokioIsolate, source: &str) -> Result<String, RuntimeError> {
    tokio::time::timeout(
        BOUND,
        isolate.evaluate_async_body(source, "test:///m2/fixture.js"),
    )
    .await
    .expect("M2 fixture stalled")
}

fn assert_no_handles(isolate: &TokioIsolate) {
    let counts = isolate.resource_counts();
    // A completed/aborted worker can still be exiting on another thread.
    // Graceful shutdown below is the synchronization boundary for active_tasks.
    assert_eq!(counts.pending_timers, 0, "{counts:?}");
    assert_eq!(counts.pending_host_calls, 0, "{counts:?}");
    assert_eq!(counts.request_ids, 0, "{counts:?}");
    assert_eq!(counts.streams, 0, "{counts:?}");
}

async fn shutdown(isolate: &mut TokioIsolate) {
    assert_eq!(
        isolate.shutdown(BOUND).await.unwrap(),
        ShutdownOutcome::Graceful
    );
    assert_eq!(isolate.resource_counts(), RuntimeResourceCounts::default());
}

#[test]
fn bundled_capabilities_are_required() {
    capabilities();
}

#[test]
fn cyclic_graph_and_repeated_dynamic_imports_preserve_identity() {
    run(async {
        let mut isolate = isolate(&corpus());
        for _ in 0..8 {
            tokio::time::timeout(BOUND, isolate.evaluate_module("graph.mjs"))
                .await
                .expect("module graph stalled")
                .unwrap();
            assert_eq!(
                isolate
                    .evaluate("m2GraphResult", "test:///m2/check.js")
                    .unwrap(),
                "graph-ok"
            );
            assert_no_handles(&isolate);
        }
        shutdown(&mut isolate).await;
    });
}

#[test]
fn nested_microtasks_timer_and_host_completion_have_explicit_order() {
    run(async {
        let mut isolate = isolate(&corpus());
        set_path(&mut isolate, &corpus().join("payload.txt"));
        for _ in 0..32 {
            assert_eq!(
                body(&mut isolate, include_str!("fixtures/m2/ordering.js"))
                    .await
                    .unwrap(),
                "microtask,nested,timer,timer-microtask,host,host-microtask"
            );
            assert_no_handles(&isolate);
        }
        shutdown(&mut isolate).await;
    });
}

#[test]
fn cancellation_before_and_after_poll_releases_timers_and_retires_modules() {
    run(async {
        for module in [false, true] {
            for polled in [false, true] {
                let mut isolate = isolate(&corpus());
                {
                    let mut evaluation: std::pin::Pin<
                        Box<dyn Future<Output = Result<(), RuntimeError>> + '_>,
                    > = if module {
                        Box::pin(isolate.evaluate_module("cancel.mjs"))
                    } else {
                        Box::pin(async {
                            isolate
                                .evaluate_async_body(
                                    include_str!("fixtures/m2/cancel.js"),
                                    "test:///m2/cancel.js",
                                )
                                .await
                                .map(|_| ())
                        })
                    };
                    if polled {
                        poll_fn(|cx| {
                            assert!(evaluation.as_mut().poll(cx).is_pending());
                            Poll::Ready(())
                        })
                        .await;
                    }
                    // Dropping before polling never starts evaluation; dropping
                    // after Pending deterministically exercises active cleanup.
                }
                assert_no_handles(&isolate);
                let result = isolate.evaluate("1 + 1", "test:///m2/reuse.js");
                if module && polled {
                    assert!(
                        result
                            .unwrap_err()
                            .to_string()
                            .contains("discard this isolate")
                    );
                } else {
                    assert_eq!(result.unwrap(), "2");
                }
                shutdown(&mut isolate).await;
            }
        }
    });
}

struct StreamFile(PathBuf);

impl StreamFile {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "kunlun-m2-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("bytes"), vec![42_u8; 1048576]).unwrap();
        Self(root)
    }
}

impl Drop for StreamFile {
    fn drop(&mut self) {
        let result = std::fs::remove_dir_all(&self.0);
        if !std::thread::panicking() {
            result.unwrap();
        }
    }
}

#[test]
fn slow_streams_and_abort_races_release_host_resources() {
    run(async {
        let file = StreamFile::new();
        let mut isolate = isolate(&file.0);
        set_path(&mut isolate, &file.0.join("bytes"));
        for _ in 0..8 {
            assert_eq!(
                body(&mut isolate, include_str!("fixtures/m2/streams.js"))
                    .await
                    .unwrap(),
                "streams-ok"
            );
            assert_no_handles(&isolate);
        }
        for should_throw in [false, true] {
            isolate
                .evaluate(
                    &format!("globalThis.m2Throw = {should_throw}"),
                    "test:///m2/stream-mode.js",
                )
                .unwrap();
            let result = body(&mut isolate, include_str!("fixtures/m2/stream-teardown.js")).await;
            if should_throw {
                assert!(
                    result
                        .unwrap_err()
                        .to_string()
                        .contains("m2 stream failure")
                );
            } else {
                assert_eq!(result.unwrap(), "abandoned");
            }
            assert_no_handles(&isolate);
        }
        shutdown(&mut isolate).await;
    });
}

#[test]
fn abort_listeners_are_removed_on_completion_failure_and_cancellation() {
    run(async {
        let mut isolate = isolate(&corpus());
        set_path(&mut isolate, &corpus().join("payload.txt"));
        assert_eq!(
            body(&mut isolate, include_str!("fixtures/m2/abort-listeners.js"))
                .await
                .unwrap(),
            "listeners-ok"
        );
        assert_no_handles(&isolate);
        shutdown(&mut isolate).await;
    });
}

#[test]
fn rejection_records_are_drained_handled_and_isolate_owned() {
    capabilities();
    for _ in 0..16 {
        let vm = JscVm::new("m2-rejections").unwrap();
        for _ in 0..32 {
            vm.evaluate(
                "globalThis.m2Rejected = Promise.reject(new Error('m2 rejection'));",
                "test:///m2/rejection.js",
            )
            .unwrap();
            while vm.microtask_checkpoint().unwrap() {}
            assert_eq!(vm.reported_rejection_count(), 1);
            let records = vm.take_promise_rejections();
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].transition, PromiseRejectionTransition::Unhandled);
            assert!(vm.take_promise_rejections().is_empty());
            vm.evaluate("m2Rejected.catch(() => {});", "test:///m2/handled.js")
                .unwrap();
            while vm.microtask_checkpoint().unwrap() {}
            assert_eq!(vm.reported_rejection_count(), 0);
            let handled = vm.take_promise_rejections();
            assert_eq!(handled.len(), 1);
            assert_eq!(handled[0].transition, PromiseRejectionTransition::Handled);
            assert_eq!(handled[0].rejection_id, records[0].rejection_id);
            vm.collect_garbage().unwrap();
        }
        // Leave both a diagnostic and queued work to VM destruction. The native
        // sanitizer and Miri ledger corpus separately check those owners' drops.
        vm.evaluate("Promise.reject('teardown');", "test:///m2/teardown.js")
            .unwrap();
        while vm.microtask_checkpoint().unwrap() {}
        assert_eq!(vm.reported_rejection_count(), 1);
    }
}

#[test]
fn repeated_startup_error_and_shutdown_return_resource_counts_to_zero() {
    run(async {
        for _ in 0..32 {
            let mut isolate = isolate(&corpus());
            set_path(&mut isolate, &corpus().join("payload.txt"));
            let error = body(&mut isolate, include_str!("fixtures/m2/error.js"))
                .await
                .unwrap_err();
            assert!(error.to_string().contains("m2 expected failure"), "{error}");
            assert_no_handles(&isolate);
            assert_eq!(
                body(&mut isolate, "return 'recovered';").await.unwrap(),
                "recovered"
            );
            // Work outside an async evaluation belongs to the isolate and must be
            // cleared by teardown, rather than an evaluation cleanup guard.
            isolate
                .evaluate("sleep(1000000)", "test:///m2/teardown.js")
                .unwrap();
            assert_eq!(isolate.resource_counts().pending_timers, 1);
            shutdown(&mut isolate).await;
        }
    });
}

#[test]
fn infinite_javascript_is_interrupted_and_terminal() {
    capabilities();
    let mut isolate = TokioIsolate::new_with_permissions_and_limits(
        "m2-infinite",
        HostPermissions::none(),
        RuntimeLimits {
            execution_timeout: Duration::from_millis(50),
            watchdog_interval: Duration::from_millis(2),
            ..RuntimeLimits::default()
        },
    )
    .unwrap();
    for source in [include_str!("fixtures/m2/infinite.js"), "1 + 1"] {
        let RuntimeError::Jsc(error) = isolate
            .evaluate(source, "test:///m2/infinite.js")
            .unwrap_err()
        else {
            panic!("expected engine termination");
        };
        assert_eq!(
            error.termination_reason(),
            Some(TerminationReason::DeadlineExceeded)
        );
        assert_no_handles(&isolate);
    }
}

#[test]
fn allocation_pressure_has_copied_telemetry_and_a_terminal_hard_limit() {
    capabilities();
    // Use the existing VM policy API so the limit is relative to the observed
    // heap, not a platform-specific guess about JSC's baseline allocation size.
    let vm = JscVm::new("m2-allocation").unwrap();
    vm.evaluate(
        include_str!("fixtures/m2/allocation.js"),
        "test:///m2/allocation.js",
    )
    .unwrap();
    vm.collect_garbage().unwrap();
    let statistics = vm.heap_statistics().unwrap();
    assert!(statistics.heap_capacity >= statistics.heap_size);
    assert!(statistics.accounted_bytes() > 1);
    let limit = statistics.accounted_bytes() / 2;
    vm.set_resource_policy(ResourcePolicy {
        execution_timeout: Some(BOUND),
        watchdog_interval: Duration::from_millis(2),
        soft_heap_limit: Some(limit),
        hard_heap_limit: Some(limit),
    })
    .unwrap();
    let _scope = vm.execution_scope().unwrap();
    let error = vm.enforce_resource_policy().unwrap_err();
    assert_eq!(
        error.termination_reason(),
        Some(TerminationReason::MemoryLimitExceeded)
    );
    assert_eq!(
        vm.evaluate("1 + 1", "test:///m2/terminal.js")
            .unwrap_err()
            .termination_reason(),
        Some(TerminationReason::MemoryLimitExceeded)
    );
}
