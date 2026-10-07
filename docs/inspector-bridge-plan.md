# Pinned JSC Inspector bridge: initial #67 investigation

This records the runtime-owned bridge entry point for
[#67](https://github.com/kunlunengine/runtime/issues/67), in parallel with M3 and the
[draft #66 boundary](./devtools-protocol-v1.md). It is **not an implemented ABI, transport,
pause pump, or native qualification result**.

## Existing and missing surfaces

The pinned WebKit revision is `4b62d53ec6c16753020dbe69e59bf761ed0948e3`, controlled by
`distribution/jsc/manifest.json`. The public Kunlun shim currently exposes context naming and
inspectability, but no frontend connection, WIP message dispatch, or pause-loop callback.
`ENABLE_REMOTE_INSPECTOR` and the Linux socket-inspector build patch do not establish those APIs.

The exact pinned source provides the relevant **internal**, not stable public-C, surfaces:

- [JSGlobalObjectInspectorController](https://github.com/WebKit/WebKit/blob/4b62d53ec6c16753020dbe69e59bf761ed0948e3/Source/JavaScriptCore/inspector/JSGlobalObjectInspectorController.h):
  frontend connect/disconnect and message dispatch.
- [JSGlobalObjectDebugger](https://github.com/WebKit/WebKit/blob/4b62d53ec6c16753020dbe69e59bf761ed0948e3/Source/JavaScriptCore/inspector/JSGlobalObjectDebugger.cpp):
  the synchronous nested debugger event loop while paused.
- [JSContextRef.h](https://github.com/WebKit/WebKit/blob/4b62d53ec6c16753020dbe69e59bf761ed0948e3/Source/JavaScriptCore/API/JSContextRef.h):
  public context naming/inspectability, not a portable message bridge.

The macOS system framework is useful only for explicit inspectability/bootstrap development.
Do not use its private C++ symbols, represent it as pinned Inspector evidence, or promise Linux
parity from a macOS framework pass.

## Selected implementation direction

Use a narrowly scoped pinned-WebKit source patch to expose an opaque Kunlun-owned C extension.
Wrap it in `kunlun-jsc-sys`/`kunlun-jsc` only after the patched artifacts compile and export the
reviewed symbols. No WebKit object layouts, `String`, global-object pointers, rooted values,
or frontend-channel pointers cross the public ABI.

The minimal extension needs lifecycle (create/attach/detach/close), bounded UTF-8 WIP ingress and
egress, pause pumping, and a forced-resume/teardown path. Reuse existing status, thread-affinity,
exception-containment and callback ownership rules. Version the extension and extend native symbol,
header and artifact verification; a build flag alone must not turn its capability on.

Keep the shim transport-neutral. The runtime endpoint owns explicit inspection opt-in,
authenticated restricted-local transport and bounded plain-data queues; the standalone DevTools
product owns its general service, session orchestration, semantic clients and authorization
presentation. Do not expose WebKit's private socket protocol as the stable product boundary.
Non-loopback transport remains unavailable until token plus TLS/authenticated-proxy policy is
implemented and reviewed.

## Pause-loop constraints to prove

JSC pauses *inside* the synchronous engine entrypoint, before Rust returns to Tokio. The current
caller-owned current-thread Tokio runtime cannot be recursively driven from that callback.

- Never call nested `block_on` or enter ordinary application execution from the pause pump.
- Pump only permitted debugger work and bounded plain-data notifications on the isolate thread.
  If host I/O is scheduled solely on the blocked current-thread executor, it will not progress;
  qualification must prove worker scheduling, not merely drain an empty queue.
- JSC references/callbacks stay isolate-local. Foreign-thread messages contain no JSC values.
- Disconnect, deadline/cancellation, closed queues and shutdown must wake the paused engine and
  force resume/teardown without relying on the ordinary suspended application loop.
- Detach/rebuild advances generation; queued stale messages and late completions cannot touch
  the replacement isolate. Preserve #66 epochs/handle invalidation and no-replay rules.
- Audit/redaction and bounds remain mandatory while paused; no client bypass or fake success.

## Native gate still pending

Build and verify the updated artifacts on pinned macOS/Linux, then exercise real classic/ESM
breakpoints, step, scopes, evaluation, source naming, an awaited timer, host work during pause,
disconnect while paused, repeated attach/detach, oversized frames, and cancellation/teardown.
Run existing M1/M2 regressions plus header/ABI and leak/sanitizer checks.

No verified pinned artifact is configured in the initial local development environment. The
engine-independent #66 fixtures and system-JSC smoke tests are separate evidence; neither proves
this bridge. Real Core-produced request/HMR qualification is #69's item-level M3 integration lane,
not a prerequisite for developing or qualifying the generic native bridge.
