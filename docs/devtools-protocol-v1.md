# Runtime → DevTools contract v1 (draft, #66 continuation)

This is **business-contract evidence only**, not native Inspector qualification or completion
of [#66](https://github.com/kunlunengine/runtime/issues/66) or the
[#65](https://github.com/kunlunengine/runtime/issues/65) M4 exit gate.
The engine-independent `kunlun-devtools-protocol` crate owns only plain-data boundary types,
serialization, bounded decoding, permission classification, and immutable source slicing.
It contains no transport, production session broker, evaluator, authority store, or engine.
General DevTools service/core, CLI, GUI, MCP, source-map implementation, and cross-runtime
adapters remain outside this Cargo workspace as specified in [devtools.md](devtools.md).

The first slice landed in [PR #71](https://github.com/kunlunengine/runtime/pull/71).
This continuation adds discoverable target metadata, correlated semantic responses, registered
source-map fixtures, cancellation, and bounded mock business workflows. **v1 is not frozen.**
Before freezing it, the Inspector and headless-tool owners must review the draft.
Product delivery starts with Kunlun Desktop in its separate repository/workstream, followed by
the DevTools product hosted on it. Record the DevTools owner/repository/implementation issue when
selected; selecting that future repository is not a prerequisite for the runtime-owned #66
contract and mock work.
That product's “service/core” is not the application/build-toolchain
[`kunlunengine/core`](https://github.com/kunlunengine/core). No Core-generated artifact or native
Fetch server is a prerequisite for these contract tests.

## Authoritative schema and portable fixtures

Rust serde types in `crates/kunlun-devtools-protocol/src/` are the single schema source.
The example emits a language-neutral JSON Schema (draft 2020-12), derived by Schemars from
the same types. There is deliberately no separately hand-maintained JSON Schema.
JSON Schema specifies structural validity; the rules below and `decode` add byte limits,
safe integer ranges, and lifecycle semantics. A schema-valid frame is not necessarily an
authorized or state-valid command.

`schema()` uses the **serialization** contract: every nullable field is required but accepts
`null`. `decode` enforces that same canonical shape, including extra/duplicate-field rejection.
Use `decode`/`encode` at the boundary, not direct serde deserialization. Clients additionally
check request/response correlation (`correlate`, or `correlate_discovery`) and current state.

```sh
cargo run -p kunlun-devtools-protocol --example schema > /tmp/devtools-v1.schema.json
cargo test -p kunlun-devtools-protocol
cargo clippy -p kunlun-devtools-protocol --all-targets -- -D warnings
cargo fmt -p kunlun-devtools-protocol -- --check
```

Portable fixtures under `fixtures/devtools-v1/`:

| File | Interpretation |
| --- | --- |
| `wire.json` | Frame vocabulary catalogue, **not** an ordered native or mock session transcript |
| `workflows.json` | Six deterministic session cases, explicit grants, commands and exact result/error expectations |
| `module.js`, `module.js.map`, `original.ts` | Immutable registered generated/map/original virtual bytes |
| `source-map-cases.json` | 25 real v3-map parser/lookup, descriptor association and UTF-8 range cases; adapter recipe in `source-map-cases.md` |
| `bounded-cases.json` | Finite recording byte/count success and overflow expectations |

Session workflow cases begin with an explicit `setup-attach` request, then use the common
target/session identity and each step's epoch. `command` constructs a normal versioned request
and compares a correlated response; `queue` holds work before adapter dispatch; `completions`
checks the original cancelled request's response. `disconnect`/`replace` are test-driver
notifications, not remotely executable commands. `events` compares complete event count, sequence,
reason, frames and handles. `attach` uses the common fixture target metadata with the step's epoch,
state and next-sequence cursor to verify same-epoch reconnect and new-epoch attach.

Tests validate wire frames and workflow responses against generated JSON Schema, decoding,
and JSON round-trip equality. Negative structural inputs fail both applicable checks;
semantic failures such as stale epochs, unauthorized operations, or excessive requested limits
are tested separately. The test-only adapters and receiver/recording models never execute
JavaScript, capture a heap, translate WIP, or establish application/HMR qualification.

## Normative draft boundary rules

- Transport supplies authenticated peer identity and framing. Each UTF-8 JSON frame is
  at most **65,536 bytes**, including all JSON escaping. Reject before parsing/dispatch.
  Content limits do not override this serialized-frame limit. No binary attachment or
  unbounded recording, snapshot, or log stream is defined.
  Inspection defaults off; loopback/restricted local IPC is the default. Remote transport
  authentication and TLS policy are pending the native endpoint, not implemented here.
- Exchange `hello.versions` and `welcome.version` before session traffic. Versions are exact
  strings (`"1"`); `"1.0"` is not equivalent. Select `"1"` if offered, otherwise
  `unknown_version`. Unknown operation names yield `unknown_operation`, never success.
  Additional object fields are rejected; nullable fields must be explicitly present (`null`
  for a failure without request correlation). Capability support is explicit and typed;
  backend-specific unsupported reasons preserve WIP/CDP/native differences.
- After negotiation, `discover`/`discovered` correlate by version and connection-scoped request ID;
  discovery requires `inspect`. Clients do not invent a target/epoch merely to discover it.
  Target metadata is allowlisted ID, epoch, display name, backend (`mock`, `wip`, `cdp`, `native`),
  state (`available`, `terminated`) and unique operation capabilities. Omitted capabilities mean
  unsupported (`not_implemented`), never support by inference. A terminated target cannot attach.
  Connection-level malformed/negotiation/discovery failures use `failure`; session outcomes use
  `response`, never an uncorrelated source frame or connection-level failure.
- Every valid admitted session request gets one terminal `response` with the same version,
  request ID and complete `(target, session, epoch)` identity. `result.status` distinguishes
  operation-specific success from typed failure. Clients reject a success for another operation,
  foreign identities/handles, changed source revision/offset, or output exceeding the requested
  count/byte budget. Failure `unsupported.operation` must match the requested operation.
  Correlation does not prove authorization, current pause state, or native execution.
- Target IDs name a logical runtime endpoint and remain stable across replacement. Session
  IDs name the external product's attachment intent; the runtime does not allocate or broker
  them here. IDs are opaque ASCII `[A-Za-z0-9_.-]`, 1–128 bytes. Epochs and counters are
  positive JSON-safe integers, at most `9007199254740991`, never wrapped or reused.
  A fresh endpoint instance/rebuild increments epoch; loss of epoch continuity requires a
  new target ID. Clients must not infer native implementation from a stable ID.
- Attach is explicit. Detach/disconnect invalidates paused handles and cancels pending work;
  there is no automatic command replay. Reconnect within the same epoch requires a new attach,
  new request IDs, fresh snapshot/discovery, and newly acquired handles. Replacement increments
  epoch, invalidates all sources/scopes/objects and queued work, resets event sequence, and
  requires explicit attach. A replaced target rejects old-epoch commands before authorization
  or execution. A handle also contains pause generation and is invalid after resume.
- Request IDs are unique per session/epoch, including denied, failed, and cancelled attempts.
  Duplicate IDs return `duplicate_request`; never re-execute evaluation or mutation.
  A transport failure after dispatch means `outcome_unknown`, not permission to retry.
  Bound deduplication storage to **256 IDs per session/epoch**, including denied, failed and
  cancelled admitted attempts.
  Exhaustion terminates rather than evicting tombstones and admitting replay. Production
  deduplication storage remains the standalone product's responsibility.
- Supported commands have operation-specific results, not generic no-op success:

  | Operations | Success semantics |
  | --- | --- |
  | attach/detach | Fresh target/state and `next_sequence` cursor; detach invalidates session-owned handles/work/breakpoints |
  | list/read sources, map lookup | Bounded descriptors/chunks or registered generated→original mapping |
  | set/remove breakpoint | Opaque breakpoint ID and requested/resolved locations; no resolved locations means pending |
  | pause/continue/step | Control acknowledgement; only subsequent events establish the new execution state |
  | read scopes | Bounded scope/variable snapshots with pause-scoped handles and redacted or typed values |
  | evaluate/mutate | Typed scalar or bounded object preview, never implicit object traversal/getter execution |
  | read logs | Ordered bounded entries; durable logs cannot retain pause-scoped object handles |
  | CPU/heap capture | Bounded already-redacted JSON/text snapshot, subject to explicit sensitive-capture permission |
  | cancel | Correlated original request ID and `cancelled`/`too_late` status, not a new evaluation attempt |

  `step` is a single step-into execution-point operation in this draft. Additional step modes,
  frame-local evaluation, backend-specific capture formats and protocol extensions need owner
  review, not silent translation. A JSON snapshot remains valid JSON even when summarized with
  `truncated: true`; it is not a sliced malformed document.
  The lifecycle mock returns fixture-backed sources/scopes, redacted logs, breakpoint state
  and pause/continue/step transitions. Evaluation, mutation and native captures explicitly return
  unsupported. The separate source-map mock supports registered map lookup. Neither advertises
  native capability. A schema-only capture/value example does not establish capture/evaluation.
- Authorization is semantic, shared by every frontend: `inspect`, `control`, `evaluate`,
  `mutate`, `capture_sensitive`. Read-only inspection grants none of the others.
  Breakpoint/pause/continue/step require control; expressions require evaluate; object writes
  require mutate; CPU/heap artifacts require capture_sensitive. Unsupported does not bypass
  permission checking. Remote attach and every control/evaluation/mutation/capture attempt
  must emit audit metadata, including denied and failed attempts, before returning a result.
  Audit delivery failure must fail closed before a sensitive action. Mock audit is test-only.
- `cancel` requires inspection plus the original operation's permission in the **same** session
  and epoch; it cannot become an inspect-only process-control/evaluation backdoor. Unknown IDs
  fail with `request_not_found`. `cancelled` is allowed only if work stopped before side effects;
  the original request then receives a correlated `cancelled` failure, with its tombstone retained.
  Once execution may have started, return `too_late`; do not suppress the original result, undo
  effects, or replay. Revalidate authority and audit availability when queued work is dispatched.
  Retain the original operation's permission metadata with its tombstone: late cancellation has
  the same authority checks as pending cancellation. Cancelling another cancellation fails with
  `invalid_state`, rather than recursively delegating permission checks.
  There are at most **16 pending requests** per session. Excess requests fail with typed
  `limit_exceeded(pending_requests)` and remain tombstoned.
- Audit is allowlisted identity, request ID, operation, permission check and outcome only.
  `check: operation` records the operation's base permission. Cancellation additionally emits
  `check: cancellation` with the original request ID/operation and that operation's actual checked
  permission. An allowed `cancel/inspect` check and denied delegated `control` check are separate
  records; do not falsely report the already-granted inspection permission as denied.
  Reserve bounded audit delivery before dispatch; if it cannot accept a required check, fail closed.
  Never emit
  expressions, evaluated values, environment values, host grants, secrets, or raw headers.
  Default source/diagnostic access also requires explicit authority and redaction before
  transport; raw source is not intrinsically safe. URLs are display identities and must omit
  credentials/query secrets. This crate is not a secret detector or an authorization engine.
- Sources are immutable `(target, epoch, id, revision)` identities plus a display URL.
  Adapters resolve only registered identities: do not fetch a caller URL or open a caller path.
  Virtual sources use the same identity/retrieval model. Chunks use UTF-8 byte offsets, at most
  **16,384 content bytes**, with total bytes and EOF; chunks never split a code point.
  Invalid offsets, zero limit, or a limit too small for one code point produce `source_range`.
  Changed revision, unknown identity or foreign target must not return unrelated source bytes.
  `list_sources` optionally advertises a registered map identity; map bytes use the same bounded
  retrieval model. `lookup_source_map` returns the generated location, immutable map identity,
  and original registered location or explicit `null` when no token maps it. Missing/unregistered
  sources/maps fail with `source_not_found`; unsupported mapping is a typed unsupported operation.
  Lines and columns are zero-based, columns measured in **UTF-16 code units**, not UTF-8 bytes or
  Unicode scalar counts. Lookup uses the same-line greatest-lower-bound token; it must not borrow
  a token from a preceding generated line. A map name or `sourcesContent` is not authority to fetch
  or disclose an original source. Mock registration bounds maps to the runtime's existing
  **1 MiB per-map** policy. Artifact-aware registration is optional adapter input; no producer,
  artifact emission, network fetch or filesystem lookup is required by this fixture.
- Events have contiguous sequence numbers starting at 1 per session/epoch. Duplicate,
  out-of-order, gap or unexpected identity terminates the stream (`event_order`), invalidates
  handles and demands resynchronization. No reordering buffer or history replay is defined.
  An attach response supplies `next_sequence`; same-epoch reconnect starts at that fresh cursor,
  not at a guessed 1. Pause generations also remain monotonic across detach/reconnect. An epoch
  replacement notice is out-of-band invalidation of the old stream, not permission to process
  new-epoch events before discovery/attach. Counter exhaustion terminates, never wraps.
  At most **64 frames** may be queued, additionally subject to per-frame bounds. Overflow
  terminates (`event_overflow`) rather than silently dropping debugger/audit events.
  Terminal diagnostics use an out-of-band failure/connection close if the queue is full.
  Disconnect cancels undispatched work, invalidates handles/breakpoints, and drains live queues.
  Already-dispatched work whose result was lost remains outcome-unknown. Native owners must
  qualify actual cancellation and isolate teardown.
- Lists, stack frames, scopes, variables and log snapshots contain at most **64 entries per
  collection**, in addition to the whole-frame limit. Value strings/object previews contain at
  most **1,024 UTF-8 bytes**. Diagnostic snapshots contain at most **16,384 UTF-8 bytes** and
  cannot exceed the caller's budget. Adapters may return explicitly truncated bounded summaries;
  they cannot silently discard required debugger/audit events. Local recordings contain at most
  **64 validated event frames and 4,194,304 serialized bytes**, or a smaller explicit byte budget.
  Recording overflow stops with `limit_exceeded(recording)` and retains a bounded incomplete
  prefix for explicit export; it is not rolling replay storage. A recording never dispatches
  stored requests. Persistence/export transports remain standalone product implementation.

## Implemented versus pending

| #66 acceptance area | Runtime-owned evidence | Remaining review / later integration |
| --- | --- | --- |
| Versions, IDs, attach/detach, replacement/reconnect, terminal/errors, no replay | Derived wire schema, correlation/cursor model, deterministic lifecycle/handle/tombstone checks | Owner review and production/native conformance |
| Sources, virtual bytes, maps and bounded retrieval | Registered module/map/original fixtures, real map decoder/lookup, UTF-16/UTF-8 tests | Optional artifact integration is not a prerequisite |
| Capabilities and semantic debugger operations | Typed command/result/event vocabulary, mock breakpoints/control/scopes/logs, explicit unsupported captures/evaluation | Inspector/headless-tool review before freezing draft semantics |
| One permission/audit/redaction model | Shared permission classification, deny/unsupported precedence, cancellation authority, fail-closed audit tests, allowlisted metadata/redacted values | Authenticated grants, durable audit and adapter redaction in the product |
| Message/queue/snapshot/recording/cancellation bounds | Inbound/outbound limits, pending/history exhaustion, finite recording, overflow/reorder/teardown mock tests | Real transport cancellation/backpressure/isolate teardown |
| Independent deterministic consumption | Wire catalogue, six session workflows, 25 map/range/association cases, three recording cases | External CLI/MCP and JSC adapter consumption/conformance |
| Evidence separation | Every fixture/test labelled mock or vocabulary-only | Mock passes cannot close native/application M4 rows |

**The runtime-owned #66 draft still needs Inspector/headless-tool owner review before freezing v1.**
Desktop-first product delivery, standalone repository selection, and production DevTools business
implementation belong to separate workstreams/threads; they must not block this repository's
contract/mock work or be filled by adding a production general DevTools service crate here.

Pending native/application evidence under #65/#67/#69: authenticated JSC/WIP edge, isolate-local
pause loop, macOS/Linux qualification, actual CLI/MCP workflows, source maps, native reconnect,
and Core-produced M3 request/HMR artifacts. Mock passes cannot close any of those rows.
