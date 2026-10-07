# M3 Fetch dispatch foundation (#51)

This is an initial library-only slice, independent of M4. **It does not serve
HTTP and does not complete #51.** There is no listener, HTTP parser, CLI serving
command, implicit Node fallback, permissive CORS, or claim of Core qualification.

## Entry loading

`FetchDispatcher::load(AdmittedArtifact, RuntimeLimits)` accepts only the result
of #48 admission. The caller must obtain the expected manifest digest from
trusted deployment metadata, call `admit_artifact`, and finish loading before
accepting requests. Failed admission cannot evaluate application code.

Loading installs the existing closed-graph source snapshot policy and imports
the canonical admitted entry using the pinned JSC native module loader. Static
imports, dynamic imports, module identity and top-level await use that same
loader; mutable files are not reopened for source. No generated application
module registration or alternate resolution policy is added.
The private adapter uses the admitted entry's canonical URL as its script origin:
the existing loader validates referrers even for an absolute dynamic import, so
an unregistered synthetic `kunlun:` adapter URL is not a valid origin.

Preflight requires a non-null object default export with a callable `fetch`
property. The handler is captured once and called with the default object as
`this`, using `fetch(request, env, executionContext)`. A synchronous Response,
promise or thenable fulfilling with a Response is supported. Missing/malformed
exports are `InvalidEntry`; linking, parsing, evaluation, rejected TLA and engine
startup failures are `Startup`. A startup timeout is `Deadline`.

The VM, namespace, handler closure and module state remain on the isolate thread
and live for the application. The dispatcher owns the authority as well as the
VM. Failed/cancelled startup drops both. There is no raw JSC value API, new ABI,
or engine reference crossing a Tokio task boundary.

## Trusted lifecycle boundary

Startup captures required intrinsics **before** the native entry import or TLA runs.
The adapter is rooted in a VM-private callable slot, not an application-callable dispatch/close
global. Its factory is strict code, so a sloppy application handler cannot reflect the private
caller through legacy `Function.caller`. Invocation, observation and cleanup use that same slot
and a dedicated driver, avoiding
the generic developer evaluator's application-visible async state. Reentry fails closed;
completion verifies the request-state identity before clearing it.

Accepted `waitUntil` work is observed through captured native Promise operations, not application
replacements of `Promise.resolve`, `Promise.all`, or a returned species promise. Observation
installation failure is an engine error that retires the dispatcher, never evidence that background
work completed. Private records avoid application-mutable prototype serialization hooks.
AbortSignal's internal state/notification operations are likewise captured during bootstrap.
The same fail-closed observation rule applies to handler completion, body reads, HEAD/body-error
cancellation, and `waitUntil`. An observer-installation failure immediately publishes an engine
failure, closes/aborts the current JS request, and causes Rust to revoke authority and retire the
dispatcher. It is not converted into reusable `Handler`/`ResponseBody` failure or delayed behind
other unresolved background tasks. A later request cannot resolve an old unobserved continuation.

This is not hostile-code isolation or a promise that every application-facing Fetch/stream API is
immutable. Deliberate changes to those APIs can alter application-owned body behavior or make
profile operations fail. They cannot substitute the trusted lifecycle controls or turn pending
accepted background work into reusable completion. Budgets, authority revocation and Rust output
validation still apply.

## Scheduling and request lifetime

The API is serial: one mutable dispatcher borrow, one request, no internal
queue. Callers own admission/concurrency limits outside this slice. It requires
a caller-provided Tokio runtime with time enabled on the isolate thread.

`DispatchRequest` and `DispatchResponse` contain only strings, header pairs and
byte vectors. Request construction uses #49's actual profile Request/Headers;
GET/HEAD bodies, credential-bearing or non-HTTP(S) URLs, prohibited methods and
transport-controlled headers are rejected before invoking the handler. URL,
method and profile-normalized headers are delivered without a routing layer.
Caller context belongs to the Rust request only and is never projected into JS.

Each invocation creates #50's request-local environment and host scope. Its
authority remains live through the handler, response-body consumption and every
accepted `waitUntil` task. Return, typed failure, deadline, explicit cancellation,
and dropping the invocation all revoke request handles and cancel host work.
Retaining a request handle or execution context does not confer authority on a
later request. Application grants and module state remain application-scoped.

`executionContext` is frozen and supports exactly:

- `signal`: the same profile AbortSignal supplied to Request. Cancellation
  aborts it using the profile's AbortError reason while JS remains executable.
- `waitUntil(promise)`: up to 32 registrations during handler execution.
  Registration closes when the handler settles, including rejection. Later
  calls throw. Rejection handlers are attached immediately; tasks are awaited
  together, including when the handler or body fails.

The response is not released to the caller until body consumption and background
work finish. Background rejection is `Background`, not an untracked failure.
This deliberately has **no detached post-response work**. `passThroughOnException`
and other platform extensions are absent.

## Bounds and cancellation

| Resource | Bound |
| --- | --- |
| Outstanding request | One per dispatcher; no queue |
| Request URL / method | 16 KiB / 32 bytes |
| Request and returned response headers | 256 pairs, 64 KiB of names/values |
| Response status text | 1024 JS code units |
| Request / returned response body | 1 MiB each, buffered |
| Serialized response envelope before Rust decoding | 8 MiB |
| Response producer chunks | 4096 read results, including empty chunks |
| `waitUntil` registrations | 32 |
| Startup / whole request budget | `execution_timeout`, positive and at most 30 seconds |
| Native watchdog polling | Caller interval capped at the existing 10 ms default |

Handler, body and background work share one request budget; tasks cannot extend
it. Tokio's timeout bounds suspended work; the existing native execution scope
and watchdog bound non-yielding JS. Normal watchdog polling has scheduling
granularity, not hard real-time guarantees. Heap limits remain RuntimeLimits
limits. JS/Rust serialization uses bounded byte arrays (with transient copies
and decimal JSON overhead), not a zero-copy transport.

`DispatchCancellation` is cloneable Send + Sync cancellation state, independent
of JSC. A transport can cancel it on disconnect or shutdown. Pre-cancelled
requests do not invoke the handler. Cancellation is observed at the next host
yield; non-yielding JS is bounded by the native execution deadline, not immediate
remote interruption.

The async-loop request finalizer aborts the signal and cancels an acquired body
reader before removing timers/host calls. A terminated native VM cannot execute
JS listeners; native cleanup still runs. Cancellation, dropped invocation,
deadline, engine failure or malformed bridge data **permanently retire** the
dispatcher: the next request fails `Retired`, and the embedder must drop/reload it.
Retirement also revokes application authority.
This prevents suspended continuations from running inside a later request scope.
Normal completion and fully settled typed JS failures permit serial reuse.
`stop()` stops admission and revokes application authority; it is not graceful
HTTP drain. Dropping the owner performs existing isolate/host teardown.
Host table entries are removed synchronously, but worker cancellation is scheduled through Tokio
abort handles. `active_tasks` can remain nonzero until the executor next polls and drops the aborted
workers; cancellation/retirement is **not an awaited worker drain**. The embedder must keep its
executor alive for teardown and separately qualify bounded drain before exposing an HTTP lifecycle.
The immediate-zero cancellation assertions in the initial adapter tests cover pure-JS suspension,
not an in-flight HTTP worker. `tests/worker_cancellation.rs` separately holds a real loopback HTTP
connection open, drops a confirmed suspended host invocation, checks synchronous table cleanup,
and verifies the aborted worker reaches zero after executor progress. That shared-cleanup test
does not exercise admitted dispatcher loading.

## Response and diagnostics

Responses are fully consumed into bounded plain bytes. HEAD cancels the producer
and returns no body, retaining status and headers. Redirect status and Location
are preserved without following an inbound response redirect. Separate
Set-Cookie values are retained; other repeated values use profile Headers'
comma-combined iteration. No CORS headers are synthesized.
The Rust response boundary independently revalidates status, null-body statuses, body/header byte
bounds, ASCII HTTP token header names, Latin-1 field-value/reason-phrase syntax, and the 1024 UTF-16
code-unit status-text limit. Mutable JS Response properties cannot bypass these copied-output
checks. HTTP transport-controlled headers and serialization are still the future adapter's concern.

Handler throw/rejection is `Handler`; non-Response return is `InvalidResponse`;
locked, oversized, failed, or over-chunked bodies are `ResponseBody`. No invalid
value is coerced into a response. All failures occur before this API releases a
response, so post-headers network failure semantics cannot be tested here.

Errors intentionally report a typed phase rather than raw exception text,
source snippets or caller data. Admission/native module identity and indexed
source-map loading are unchanged. Secret-safe, source-mapped dispatch diagnostic
locations are still a follow-up; this slice does not expose application stacks.

## Evidence and prerequisites

`tests/fetch_dispatch.rs` compiles tests for actual admitted, checked-in ESM
artifacts, mutable-file replacement, denied imports, malformed defaults,
parse/TLA failures, binary/status/header fidelity, module state, scoped
background filesystem access, revoked retained handles, HEAD, redirects,
failure cleanup, confirmed suspended-future drop, cancellation and deadline.
System JSC explicitly ignores the six loader-dependent tests because it lacks
the pinned module extension. Integrity/admission and canonical-referrer contract tests still run.
An additional system-only negative test verifies that real entry loading fails
without a fallback.

Internal `fetch_dispatch::adapter_tests` exercise the real isolate-local adapter
and lifecycle with inline test handlers but **do not exercise artifact module
loading**. They may be run using the explicit developer-only system backend.
They are not a replacement for the admitted native corpus.

Qualification requires an offline-verified pinned JSC artifact and trusted
receipt, as described in [jsc-distribution.md](./jsc-distribution.md):

```sh
cargo test -p kunlun-runtime --locked --test fetch_dispatch
cargo test -p kunlun-runtime --locked --lib fetch_dispatch
cargo clippy -p kunlun-runtime --locked --all-targets -- -D warnings
```

Development-only checks on a supported macOS host, with distribution variables
unset:

```sh
cargo fmt --all -- --check
cargo test -p kunlun-runtime --locked --no-default-features --features system-jsc
cargo clippy -p kunlun-runtime --locked --all-targets --no-default-features --features system-jsc -- -D warnings
```

Initial development validation on macOS with explicit `system-jsc`: the full
workspace test suite, formatting check, and all-targets Clippy with warnings denied
passed. The adapter suite includes adversarial reentry, pre-entry primordial/Promise tampering,
private lifecycle observation, once-snapshotted metadata, and abort notification tests. The
integration file passed
integrity admission, canonical-referrer validation and unsupported-backend rejection, with six real-entry tests
explicitly ignored. Default `cargo check --locked --offline` rejected the absent
verified bundled artifact as designed. No bundled native dispatch execution has
been established by that evidence.

Still pending: production TCP/HTTP adaptation and lifecycle; inbound streaming
and backpressure; slow-client/output queues; transport header policy; CORS;
disconnect wiring; pre/post-headers failure behavior; admission-stop and graceful
drain/restart qualification; source-mapped safe diagnostics; unchanged actual
Core-produced artifact qualification and cross-runtime/platform corpus (#52/#53).
Do not advertise a Fetch HTTP server or full #51 support from this foundation.
