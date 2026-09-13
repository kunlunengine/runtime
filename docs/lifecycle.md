# Cancellation, streaming, and shutdown

## AbortSignal contract

Every isolate installs `AbortController` and `AbortSignal`. The supported surface includes
`aborted`, `reason`, `onabort`, `addEventListener`, `removeEventListener`, `throwIfAborted`, and
`AbortSignal.abort`. Calling `abort()` without a reason creates an `Error` named `AbortError`.

Filesystem and HTTP operations accept a standard `signal` option. A pre-aborted signal rejects with
its existing reason before a host request is admitted. An in-flight abort rejects the JavaScript
operation with that same reason object and sends the request's monotonic token to the host. The host
removes the pending Promise and aborts its task once; repeated cancellation is a no-op. A completion
that raced with cancellation carries only owned data and is discarded when its token no longer has
a pending Promise. Operation listeners are removed on resolve, reject, EOF, cancellation, and
producer failure.

This cancellation boundary can interrupt timers and asynchronous host work. It cannot preempt
synchronous JavaScript; the engine watchdog and execution deadlines are owned by #32.

## Bounded streams

`kunlun:fs.openReadStream(path, { signal })` and
`kunlun:http.requestStream(url, { signal })` return pull-driven byte streams. Each stream implements
`read()`, `cancel()`, and `AsyncIterable<Uint8Array>`. HTTP returns status and headers after the
response arrives, with the byte stream in `body`.

Each producer has a two-slot channel of 64 KiB chunks. A producer waits when both slots are full, so
a slow JavaScript consumer bounds buffered body data to 128 KiB per stream. The completion channel
is separately bounded by the 256 in-flight host-call limit. Messages crossing Tokio threads contain
only IDs, byte vectors, strings, and response metadata; Deferred Promises and all other JSC handles
remain in the isolate-local dispatcher.

`read()` resolves to `{ done: false, value: Uint8Array }` for a chunk and `{ done: true }` for EOF.
Only one read may be in flight for a stream. File reads reject non-regular files. Network read
failures, premature producer exit, duplicate reads, and unknown or closed stream IDs reject with
distinct messages. Consumer cancellation drops the receiver and aborts the producer; a sender also
stops cleanly if its receiver was dropped. The original non-streaming text APIs retain their 1 MiB
limit.

## Process and isolate shutdown

The CLI listens for SIGINT and SIGTERM while running async bodies and native modules. The first
signal performs this sequence:

1. stop admitting new host operations;
2. drop the active evaluation, which removes its async state and cancels timers, host calls, stream
   pulls, and stream producers;
3. run the final microtask checkpoint on the isolate thread;
4. wait for tracked Tokio and blocking host workers;
5. release the isolate before the caller-provided Tokio runtime is dropped.

The default grace period is 5 seconds and can be set with `--shutdown-grace-ms <milliseconds>`.
`TokioIsolate::shutdown` exposes the same protocol to embedders and returns `Graceful` or `Forced`.
If the grace period expires, or if SIGINT/SIGTERM arrives again during draining, the CLI takes the
forced path immediately and drops the isolate and runtime. No JavaScript cleanup is run from a
destructor, and shutdown never moves a JSC value to a worker thread.
