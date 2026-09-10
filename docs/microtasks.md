# Microtask checkpoints and rejection transitions

The pinned JSC backend gives each Kunlun global context its own GC-visible FIFO microtask queue.
Script evaluation, value conversion, module polling and other C API calls do not drain that queue.
Even two contexts sharing a context group require separate checkpoints. Native Promise machinery,
including async functions, top-level await and dynamic import, uses this queue without replacing
`Promise` or its prototype.

## Binding contract

`JscVm::microtask_checkpoint()` drains the context's queued jobs, including jobs those jobs enqueue.
It then records rejection transitions. The result is `true` if notification callbacks created work
that needs another checkpoint. The caller must continue checkpointing before deciding to idle.
`take_promise_rejections()` takes accumulated owned diagnostics without executing JavaScript.

The additive C ABI is `kunlun_jsc_microtask_checkpoint(context, callback, data, out_pending)` and
`kunlun_jsc_microtasks_stop(context)`. No WebKit C++ type crosses that ABI. Notification callback
code and user data are borrowed only for the synchronous checkpoint call. Reasons and source URLs
are borrowed only during the callback; Rust immediately copies them. Native and Rust exceptions
are contained at their respective boundaries. A failed notification is not retried automatically.

A checkpoint from running JavaScript, a host callback, or a notification callback returns
`INVALID_STATE`; it cannot recursively drain a queue. Notification delivery freezes a batch before
calling foreign code, and marks each transition before delivery. New rejections and handlers created
by diagnostic conversion wait until the next checkpoint. Throwing `toString`/`stack` getters retain
a fallback diagnostic, and the handled transition reuses the original diagnostic without invoking
those getters again.

## Rejection policy

- A rejection handled synchronously or by any job in the same drained queue produces no record.
- A rejection still unhandled after draining produces one `Unhandled` record.
- Attaching a handler after that report produces one `Handled` record at a later checkpoint.
- Each record contains a stable isolate ID, a context-local rejection ID, the transition and an owned
  `JscError`. The error contains the rejection source URL when a JS frame is available, exception
  text and available stack/source-map context. A host-originated rejection with no JS frame has an
  empty source URL rather than an invented location. Deferred host Promises retain a weak
  association with their creation source, so asynchronous host failures preserve the calling URL.

The engine roots tracked Promises until handled or stopped. Rust retains the original diagnostic
until the handled transition or VM teardown. Hosts should regularly take records. Bounding rejection
retention, runaway Promise chains and synchronous execution requires the resource policy in #32;
this API does not claim to preempt JavaScript.

## Tokio host boundaries

`TokioIsolate` checkpoints after script/module evaluation, before each event-loop pass, after every
individual timer settlement and host completion, and before reading completion state or choosing
to wait/finish. Notification-created work is checkpointed again before idle. Rejections are exposed
with `TokioIsolate::take_promise_rejections()`; embedding callers choose their reporting policy.
The CLI writes collected transitions to stderr when evaluation returns, including on failure.

Within a pass, the driver snapshots expired timers in registration order, checkpoints after each,
then settles available host completions in channel arrival order with a checkpoint after each.
A Promise continuation therefore precedes the next ready timer/completion. Newly registered timers
wait for the next pass. Network/filesystem arrival is external input, not a promised wall-clock
ordering: tests inject completion messages and set common timer deadlines directly without sleeps.

Cancellation drops host work. VM teardown stops its native queue and releases rejection roots before
revoking host callbacks, even if protected values temporarily keep the underlying context alive.
Stop is idempotent and discards pending jobs without executing JavaScript. Destructors never drain
untrusted work. Normal completion performs its final checkpoint before deciding to finish.

## System backend

The macOS `system-jsc` development backend reports explicit checkpoints as unsupported. It retains
the host framework's eager API-return behavior and cannot provide the pinned backend's rejection
tracking or ESM semantics. The Tokio bootstrap remains usable, but `doctor` reports the capability
as false. There is no JavaScript emulation or implicit backend fallback.
