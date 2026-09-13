# Execution and memory policy

## One monotonic execution scope

Every `TokioIsolate` evaluation opens one execution scope. Its deadline is calculated once from
`std::time::Instant` and is never extended by nested calls. The same absolute deadline therefore
covers classic evaluation, native module load/link/evaluate, top-level await, explicit microtask
drains, JavaScript entered from host callbacks, and time spent waiting for a timer or host I/O.

JSC's context-group watchdog interrupts synchronous JavaScript. Its callback runs on the isolate
thread and consults synchronized Rust state; if a checkpoint does not terminate, the shim explicitly
re-arms the next watchdog interval. Tokio waits use the earlier of the next timer, remaining
execution deadline, and watchdog polling interval, so an idle async evaluation also observes
cancellation and deadlines without relying on an arbitrary test sleep.

Watchdog configuration is isolate-thread-affine and cannot be changed or cleared reentrantly from
inside its callback. Replacement prepares all fallible native state before it removes the active
configuration, so an allocation failure cannot silently turn a limited isolate into an unlimited one.

`ExecutionHandle` is `Send + Sync` because it contains only an `Arc<Mutex<...>>`, no context, value,
callback, or VM pointer. `cancel()` affects an active evaluation and is a no-op while the isolate is
idle. The first cancellation, deadline, memory-limit, or OOM terminal reason wins. Once engine
termination occurs, subsequent JavaScript operations return that same typed reason without entering
JSC; the caller must discard the isolate. Late timer and host completions lose their pending token
and cannot settle a Promise.

## Heap telemetry and limits

The pinned engine copies three unsigned fields through the Kunlun C ABI:

| Field | Meaning |
| --- | --- |
| `heap_size` | live GC-managed bytes reported by JSC |
| `heap_capacity` | committed GC heap capacity reported by JSC |
| `extra_memory_size` | non-GC memory retained by GC objects |

Policy accounting is `heap_size + extra_memory_size`, using saturating arithmetic. Capacity is
telemetry, not counted a second time. No raw `Heap`, `VM`, or allocation pointer crosses the ABI,
and the snapshot is taken under the isolate API lock. The macOS system framework is a
development-only backend: its private heap layout is deliberately not bound, so
`heap_statistics()` returns `Unsupported` and runtime memory limits are not claimed there.

At an event-loop boundary, usage above the soft limit triggers a synchronous full collection on the
isolate thread followed by a fresh snapshot. Usage still above the hard limit is a typed
`MemoryLimitExceeded` termination. The watchdog also samples the pinned heap while synchronous
JavaScript runs, so allocation loops cannot avoid the hard limit merely by skipping host
checkpoints. A native `OUT_OF_MEMORY` becomes typed `OutOfMemory`; both cases are fail-closed and
make the isolate non-reusable. Process/container limits remain necessary for hostile code and for
catastrophic allocator or engine failures that cannot be recovered inside a realm.

## Defaults and configuration

The runtime defaults are a 30-second execution timeout, 10-millisecond watchdog interval, 128 MiB
soft heap limit, and 256 MiB hard heap limit. Embedders can pass `RuntimeLimits` to
`TokioIsolate::new_with_permissions_and_limits`. Async and module CLI commands accept:

```text
--execution-timeout-ms <milliseconds>
--heap-soft-limit-mb <mebibytes>
--heap-hard-limit-mb <mebibytes>
```

All values must be positive and the soft limit cannot exceed the hard limit. `doctor` reports the
selected backend's watchdog and telemetry support plus the defaults. These are resource-control
defaults for the M2 runtime preview, not a hostile-code sandbox or a stable M3 manifest contract.

## Verification

Tests cover an infinite synchronous loop terminated at a monotonic deadline, cancellation requested
from another native thread after JavaScript has entered, first-terminal-reason races, async deadline
cleanup with no later Promise settlement, invalid policy ordering, copied heap fields, hard-limit
termination, C/C++ ABI layout and symbol linkage, and sanitizer execution of the watchdog boundary.
Pinned macOS and Linux artifact workflows rebuild WebKit with the reviewed telemetry extension and
run the same Rust and native corpus on all four supported targets.
