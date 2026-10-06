# Runtime → DevTools contract v1 (draft, initial #66 slice)

This is **business-contract evidence only**, not native Inspector qualification or completion
of [#66](https://github.com/kunlunengine/runtime/issues/66) or the
[#65](https://github.com/kunlunengine/runtime/issues/65) M4 exit gate.
The engine-independent `kunlun-devtools-protocol` crate owns only plain-data boundary types,
serialization, bounded decoding, permission classification, and immutable source slicing.
It contains no transport, production session broker, evaluator, authority store, or engine.
General DevTools service/core, CLI, GUI, MCP, source-map implementation, and cross-runtime
adapters remain outside this Cargo workspace as specified in [devtools.md](devtools.md).

## Authoritative schema and portable fixtures

Rust serde types in `crates/kunlun-devtools-protocol/src/lib.rs` are the single schema source.
The example emits a language-neutral JSON Schema (draft 2020-12), derived by Schemars from
the same types. There is deliberately no separately hand-maintained JSON Schema.
JSON Schema specifies structural validity; the rules below and `decode` add byte limits,
safe integer ranges, and lifecycle semantics. A schema-valid frame is not necessarily an
authorized or state-valid command.

```sh
cargo run -p kunlun-devtools-protocol --example schema > /tmp/devtools-v1.schema.json
cargo test -p kunlun-devtools-protocol
cargo clippy -p kunlun-devtools-protocol --all-targets -- -D warnings
cargo fmt -p kunlun-devtools-protocol -- --check
```

`fixtures/devtools-v1/wire.json` is a portable array of frames, including denied evaluation,
allowlisted audit metadata, a virtual source, replacement, and overflow. Conformance tests
validate every frame against generated JSON Schema, Rust decoding, and JSON round-trip
equality; malformed version/operation/extra-field frames must fail both applicable checks.
The test-only mock and receiver model cover lifecycle and stream failures. They neither
execute JavaScript nor prove a WIP bridge works.

## Normative draft boundary rules

- Transport supplies authenticated peer identity and framing. Each UTF-8 JSON frame is
  at most **65,536 bytes**, including all JSON escaping. Reject before parsing/dispatch.
  No binary attachment, unbounded recording, snapshot, or log stream is defined in this slice.
  Inspection defaults off; loopback/restricted local IPC is the default. Remote transport
  authentication and TLS policy are pending the native endpoint, not implemented here.
- Exchange `hello.versions` and `welcome.version` before session traffic. Versions are exact
  strings (`"1"`); `"1.0"` is not equivalent. Select `"1"` if offered, otherwise
  `unknown_version`. Unknown operation names yield `unknown_operation`, never success.
  Additional object fields are rejected; nullable fields must be explicitly present (`null`
  for a failure without request correlation). Capability support is explicit and typed;
  backend-specific unsupported reasons preserve WIP/CDP/native differences.
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
  A transport failure after dispatch means **outcome unknown**, not permission to retry.
  Implementations bound deduplication storage; when exhausted terminate the session rather than
  evicting tombstones and admitting replay. This slice does not implement that production store.
- Supported commands must have real responses, not no-op success. The mock supports only
  attach/detach, pause/continue transitions, handle validation, and fixture source lookup.
  It explicitly rejects evaluation/step/captures/mutation/logs/breakpoints. No native capability
  is advertised. Command payloads for these operations are draft review vocabulary; response
  result types and backend-specific extensions are pending adapter review.
- Authorization is semantic, shared by every frontend: `inspect`, `control`, `evaluate`,
  `mutate`, `capture_sensitive`. Read-only inspection grants none of the others.
  Breakpoint/pause/continue/step require control; expressions require evaluate; object writes
  require mutate; CPU/heap artifacts require capture_sensitive. Unsupported does not bypass
  permission checking. Remote attach and every control/evaluation/mutation/capture attempt
  must emit audit metadata, including denied and failed attempts, before returning a result.
  Audit delivery failure must fail closed before a sensitive action. Mock audit is test-only.
- Audit is allowlisted identity, request ID, operation, permission and outcome only. Never emit
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
  Source-map discovery/lookup, artifact provenance, and fixture maps are pending.
- Events have contiguous sequence numbers starting at 1 per session/epoch. Duplicate,
  out-of-order, gap or unexpected identity terminates the stream (`event_order`), invalidates
  handles and demands resynchronization. No reordering buffer or history replay is defined.
  At most **64 frames** may be queued, additionally subject to per-frame bounds. Overflow
  terminates (`event_overflow`) rather than silently dropping debugger/audit events.
  Terminal diagnostics use an out-of-band failure/connection close if the queue is full.
  Disconnect cancels outstanding commands and drains bounded queues; engine owners must
  qualify actual cancellation and isolate teardown. There is no cancellation command yet.

## Implemented versus pending

Implemented: exact version selection; typed IDs/epochs/handles, commands, capabilities and errors;
semantic permission map; bounded decoder and UTF-8 source chunks; allowlisted audit schema;
portable wire fixtures and generated-schema cross-checks; test-only pause/reconnect/no-replay,
denied/unsupported/source/overflow/reorder checks.

Pending within #66: adapter/client owner review before freezing v1; target discovery metadata;
complete response/correlation model and source-map fixtures/lookup; breakpoint/step/scopes/value
result vocabulary; bounded logs, diagnostic captures/snapshots/recordings; explicit cancellation;
production authority/audit integration; real transport and adapter conformance. The separate
DevTools owner/repository and implementation issue have not been selected here.

Pending native/application evidence under #65/#67/#69: authenticated JSC/WIP edge, isolate-local
pause loop, macOS/Linux qualification, actual CLI/MCP workflows, source maps, native reconnect,
and Core-produced M3 request/HMR artifacts. Mock passes cannot close any of those rows.
