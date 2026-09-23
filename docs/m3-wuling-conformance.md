# M3 Wuling SSR/docs consumer qualification

Status date: 2026-09-22. **Requirements draft for producer/consumer review, not an implemented
artifact contract, executable corpus, or compatibility result.** This is the first slice of
[Runtime #42](https://github.com/kunlunengine/runtime/issues/42), under
[M3 #47](https://github.com/kunlunengine/runtime/issues/47).

## Current baseline

| Component | Inspected state | Consequence |
| --- | --- | --- |
| Runtime `be0347b9d891d30f2d12503ec9d19086cfaa1ee8` | [M2 main gate restored](./m2-exit-gate.md#m2-closeout-status); native ESM, foundational Web APIs, bounded host I/O and cancellation exist | Reuse M2; do not treat `run-module` as an HTTP server |
| Core `1327eefc010354a89e876c7423cae408eb9776ae` | The [runtime API](https://github.com/kunlunengine/core/blob/1327eefc010354a89e876c7423cae408eb9776ae/packages/runtime-api/src/index.ts#L57-L60) accepts an in-memory application; the [Node adapter](https://github.com/kunlunengine/core/blob/1327eefc010354a89e876c7423cae408eb9776ae/packages/runtime-node/src/index.ts#L19-L27) starts that object | This is a reference transport, not a loader for an integrity-addressed Fetch artifact |
| Core producer | [Core #6](https://github.com/kunlunengine/core/issues/6) and the v0.4 artifact work remain open/planned | No reviewed Core-generated portable artifact or exact producer/adapter version pair is available for this qualification |
| Native application profile | [M2 profile](./runtime-profile-v1.md) does not introduce Fetch `Request`, `Response`, `Headers` or inbound serving | #48–#53 are prerequisites; no React/HeroUI SSR compatibility result exists yet |

The restored M2 baseline permits this contract work. It does not satisfy #42 acceptance.
The existing Wuling Node production path remains independently deliverable; the current Core
Node adapter is not automatically a qualified artifact reference merely because it uses Node.

## Ownership and delivery order

- [Core #6](https://github.com/kunlunengine/core/issues/6): renderer, metadata, packed-package
  reference app, build emission, hydration and browser/server boundary.
- [Wuling #65](https://github.com/zixiao-labs/Wuling-DevOps/issues/65) and
  [#66](https://github.com/zixiao-labs/Wuling-DevOps/issues/66): public repository/docs content,
  visibility, authorization, canonical origin, cache policy and invalidation.
- Runtime: artifact admission, host authority, Fetch execution, HTTP lifecycle and native parity.
  [#52](https://github.com/kunlunengine/runtime/issues/52) coordinates the Core producer and
  runtime-node versions; it does not transfer renderer ownership here.

Delivery order:

1. Review these case requirements with Core/Wuling; agree the versioned wire/entry contract in
   [#48](https://github.com/kunlunengine/runtime/issues/48).
2. Implement data-only admission and negative fixtures in #48. A hand-authored module is useful
   for admission tests but must be labelled synthetic, not Core-produced or SSR-qualified.
3. Implement [Fetch/body semantics (#49)](https://github.com/kunlunengine/runtime/issues/49),
   [scoped grants (#50)](https://github.com/kunlunengine/runtime/issues/50) and
   [HTTP lifecycle (#51)](https://github.com/kunlunengine/runtime/issues/51); coordinate #52.
4. Grow the [shared harness (#53)](https://github.com/kunlunengine/runtime/issues/53) with those
   slices. Run one reviewed Core artifact on the pinned Node reference and all four JSC targets.
5. Review #42's real renderer and enterprise cases, limitations and measurements. A generic
   hello-service pass is not Wuling SSR acceptance.

Do not replace Core with a Runtime-owned HTML renderer, create a second Wuling-specific manifest,
or route native tests silently through a Node subprocess. Desktop/mobile #43, Qingting's Rust
CLI/SDK and its still-undecided FFI/IPC/protocol are independent workstreams. Product identity,
budget, billing, fencing, durable usage records and external deployment updates stay with their
owners. This work does not qualify hostile multi-tenant code, Windows, full Node/npm, or Actions.

## Minimal shared fixture inputs

The following values and paths are **proposed synthetic test data**, not Wuling production routes
or an approved product policy. Core/Wuling review must pin the final inputs and golden outputs
before adapter qualification. Use no real accounts, provider credentials, repository data or
billing secrets.

- Public origin `https://public.example`, tested with base paths `/` and `/wuling/`.
  Include an untrusted incoming Host/forwarded-origin value that must not replace the configured
  canonical origin. Proxy trust configuration is a deployment decision.
- Public repository page `repos/demo`, README page `repos/demo/readme`, docs page `help/intro`,
  one release page, and a shared shell with a small hydrated control in the real renderer tier.
- Text `武陵 / Wuling — café 😀` plus a title containing `&`, `<`, `>` and quotes. Fix locale,
  content revision and ID inputs in the producer; do not mask nondeterminism by deleting metadata.
- A renamed path, a missing path and a deleted path, each with its own expected status/location.
  Select deletion and protected-route statuses with Wuling; do not infer its policy in Runtime.
- Indexed subpath assets, including a Unicode/space-containing filename, a split server module
  and a source map. Distinguish public URL paths from artifact-internal module URLs.
- Two request contexts A/B and an anonymous context, with distinct **synthetic** private markers.
  Include public/private instance modes and server-only provider/billing modules. Secret-bearing
  runtime values must never be baked into the artifact, browser output or public goldens.
- A deterministic producer that can pause before headers, pause after headers, emit byte chunks,
  fail, and observe cancellation. Harness synchronization must be outside public responses.

Use two labelled tiers: a minimal transport/admission smoke fixture while M3 grows, and the
Core-generated React/HeroUI renderer fixture required for the real consumer qualification.
Omitting hydration or substituting static HTML in the smoke tier must not check off the renderer
tier. Neither tier has been delivered by this document.

## Public content and HTTP cases

Case IDs are review anchors for the future shared corpus, not names of tests already running.
Core/Wuling supply deterministic goldens; Node and JSC must each satisfy them as well as agree
with each other. Equal wrong outputs do not pass.

| ID | Case | Required oracle |
| --- | --- | --- |
| P01 | Public repository, README and release GET | Readable no-JS body, expected status/content type, shared shell, correct content revision; no private markers |
| P02 | Docs and derived public content | Help page shares the shell; HTML, Markdown, navigation/search manifest, sitemap and `llms.txt` are served as emitted by Core/Wuling, not regenerated in Runtime |
| P03 | HEAD paired with every public GET | Same application-selected status and applicable representation headers; no response-body bytes; no abandoned body producer |
| P04 | Rename/legacy redirect | Exact reviewed redirect status and Location, including origin/base path; inspect the first response without automatically following it |
| P05 | Missing/deleted route | Reviewed 404/410 policy; no soft-404 homepage, stale title or deleted content; HEAD retains the status |
| P06 | Unicode and metadata escaping | Decoded text, title, description, canonical and OG URLs match the goldens; escaping is safe and stable, including UTF-8 split across stream chunks |
| P07 | Origin and subpath assets | Both base paths work, canonical/OG URLs use the configured origin, and emitted asset bytes/URLs resolve without root-path assumptions or double decoding |
| P08 | Hydrated control | SSR is readable without JS; Core's browser test verifies hydration with deterministic locale/IDs; browser output excludes server-only imports/markers |
| P09 | Public → private/deleted/content-changed | Wuling-supplied replacement snapshot invalidates public content and derived metadata/assets; disabled public discovery yields no reusable anonymous private content |
| P10 | Protected route and context separation | Sequential and deliberately interleaved A/B/anonymous requests obey the fixture's policy; public body/headers and diagnostics contain no private auth/provider/billing markers |
| P11 | Headers and CORS | Preserve application status and header values, including separate Set-Cookie values where applicable; explicit allowed/denied-origin and preflight goldens; no inferred authority from CORS |

Comparison rules for #53:

- Compare status exactly. Compare header names case-insensitively, with explicit handling for
  repeated values (never comma-fold Set-Cookie). Enumerate transport-only exclusions such as
  connection framing; do not ignore application headers, cache policy or unexpected cookies.
- Fix renderer inputs and compare decoded content/metadata against reviewed goldens. Only
  normalize explicitly documented transport differences; do not broadly strip whitespace,
  attributes, scripts, IDs or tags to obtain parity.
- Compare concatenated stream bytes, terminal outcome and backpressure observations, not TCP
  chunk boundaries. Test HEAD on the real HTTP transport, not only a direct function call.
- Keep public cache/authorization expectations in consumer fixtures. Runtime checks isolation
  and faithful execution; it does not decide whether a repository should be public.

## Lifecycle and resource cases

Use observable barriers (request admitted, headers committed, producer pull/cancel observed,
task joined) rather than sleeps to prove ordering. Timeouts are failure bounds only. Record the
configured queue/chunk, in-flight, body, execution, cancellation and drain limits before a run;
unknown/unbounded limits cannot produce a qualification pass.

| ID | Trigger | Required outcome |
| --- | --- | --- |
| L01 | Slow client stops reading after one chunk | Producer demand stalls at the configured bound; report peak queued bytes/chunks, not just elapsed time |
| L02 | Abort before admission, then after admission but before headers | First variant never invokes the handler; second propagates cancellation and joins request work without committing a successful response |
| L03 | Abort after headers / disconnect during body | Preserve already committed status; terminate the body, cancel the producer and release pending work/listeners within the bound |
| L04 | Producer fails before or after headers | Before: agreed error response and sanitized diagnostic. After: terminal transport/body failure, never rewrite status or append a fake successful document |
| L05 | Stop admission with active and keep-alive clients | Observe the admission-stop boundary; no new handler starts, including requests on existing connections; admitted work drains only within the declared grace period |
| L06 | Grace period expires / repeated close | Cancel remaining work, join resources and make close idempotent; retained request handles cannot retain or acquire authority after closure |
| L07 | Repeated startup → stream/error → drain/cancel → restart | A fresh instance serves the fixture; old context/tasks/handles cannot reappear; record iteration count, resource counts and memory trend |
| L08 | Concurrent A/B with one cancelled | Cancelling A neither cancels B nor transfers A's request context/grants into B or a later anonymous request |

Bounded body/request/response queues and execution/memory-limit failures must be exercised on
the real adapters, reusing [M2 resource policy](./resource-policy.md) and
[lifecycle primitives](./lifecycle.md). Host-owned task/request/stream counts must return to the
declared idle baseline, and to zero at final join. Do not equate ownership counts with RSS
returning to baseline or claim allocation-by-allocation accounting of JSC internals.

## Admission and diagnostic cases

These requirements feed #48/#50; they do not define a second manifest schema or error enum.
For A01–A04, the oracle must prove that no application module was evaluated and no traffic was
admitted, not merely observe an eventual HTTP 500. A preflight validates every indexed source,
asset and map, not just the entry. Checksums detect alteration; publisher trust is a separate
deployment input.

| ID | Input | Required outcome |
| --- | --- | --- |
| A01 | Unsupported schema, engine ABI, runtime profile or required compatibility feature | Deterministic category, offending field and supported expectation; reject before evaluation |
| A02 | Missing, tampered or conflicting entry/chunk/asset/map | Identify the logical artifact member and integrity failure; never fetch a replacement implicitly |
| A03 | Traversal, escaped separator, symlink escape or duplicate canonical identity | Reject using the reviewed artifact-root and M2 canonical URL rules, consistently on all targets |
| A04 | Missing required grant / grant outside allowed scope | Declaration is not authority; intersect with deployment grants and reject required denials before evaluation; no secret values in diagnostics |
| A05 | Files replaced between validation and use | Executed/served bytes remain the validated bytes or admission fails; no validate-then-reopen race |
| A06 | Undeclared dynamic source request | Declared graph is checked at admission; a later undeclared import fails at resolution without loading/evaluating the target or escalating authority |
| A07 | Runtime error in mapped renderer source | Sanitized diagnostic identifies artifact/module and original source location with the reviewed map; public response omits stack, host paths and caller context |
| A08 | Missing/invalid Fetch export, startup throw, rejected or non-settling TLA | No traffic admitted; actionable entry/startup diagnostic with artifact/source identity; bounded termination and cleanup, including failed startup followed by restart |
| A09 | Handler returns a non-Response value, directly or through a Promise | Deterministic invalid-return diagnostic and reviewed error response; no implicit coercion into successful HTML, cross-request context leakage or abandoned work |
| A10 | Integrity-valid declared map with malformed JSON, invalid mappings/source indexes or unsupported format | Fail map validation before entry evaluation or traffic admission; identify the artifact/map and validation category without raw source, host paths or caller context; release preflight resources and permit a clean restart with a valid artifact |

A05 is different from an invalid input at initial preflight: an immutable validated snapshot
may safely execute despite a subsequent path replacement. Prove either that the snapshot's
original bytes are used, or that replacement is rejected before evaluation and traffic admission.
Unlike data-only preflight rejection, A08 may evaluate the entry before detecting an invalid
export or startup failure; it must still fail before serving. A09 exercises request dispatch.

A10 must recompute the manifest's integrity entry for the malformed map so it reaches map
validation rather than merely repeating A02. A valid sparse map with no mapping for a particular
frame is not malformed: include that variant in A07 and retain the generated module/line/column
with an explicit unmapped result, never fabricate an original location or suppress the error.
The supported map formats and diagnostic categories remain subject to #48 contract review.

## Contract review handoff

The next implementation item is #48, not a renderer in Runtime. The roadmap's
`kunlun.runtime-manifest/v1` and `export default { fetch(request, env, executionContext) }`
are proposals. Record agreement with Core/runtime-api owners on #48 before freezing:

| Decision | Required agreement |
| --- | --- |
| Manifest versioning | Required fields, unknown field/feature handling, ABI versus runtime-profile negotiation, deterministic error categories |
| Artifact identity | Root/URL rules, exact bytes and paths covered by hashes, complete source/asset/map index, publisher trust input, handling of dynamic imports |
| Validated-byte lifetime | How the loader and asset server use the same bytes admitted at preflight, including replacement and symlink cases |
| Entry/lifetime | Sync/async Response returns, invalid-return behavior, request URL construction, scoped env projection, executionContext operations and expiry |
| HTTP transport | HEAD body disposal, headers/cookies, CORS configuration, streaming errors, disconnect propagation, admission-stop and bounded drain semantics |
| Producer/reference pair | First supported BuildEngine, packed-package build command, exact Core/runtime-api/runtime-node versions, and explicit unsupported-builder diagnostics |
| Consumer goldens | Wuling-owned protected/deleted-route policy, canonical origin, visibility transitions and public-cache expectations; Core-owned renderer output |

Prefer an immutable snapshot of validated artifact bytes over hashing paths and reopening them
later: the latter admits replacement races. The snapshot's size limits, storage and loader
integration still need review in #48. Do not add an unbounded in-memory copy as an incidental fix.

Reading the upstream RFCs is not owner approval. No producer/consumer sign-off, manifest schema,
golden artifact, or new public Runtime API is approved by this document.

## Qualification evidence and fallback

The eventual #53/#42 report must bind results to:

- Runtime commit, Core/BuildEngine/runtime-api/runtime-node package versions and commits,
  lockfile, packed-package build invocation, manifest and complete artifact digests.
- Corpus/golden revision and digest; exact Node executable version, JSC revision, distribution
  manifest/archive/receipt digests, runtime profile/ABI and host OS/architecture.
- All four pinned JSC targets from [the M2 matrix](./m2-exit-gate.md#validation-ladder), identifying
  macOS x64/Rosetta honestly; Node results for the same qualified hosts and same artifact bytes.
- Case ID, adapter, expected/actual result and evidence location. Distinguish `passed`, `failed`,
  `blocked` and `not-run`; missing artifact, skipped cases or unavailable targets do not pass.
- Configured limits, workload/concurrency, warmup and sample/iteration counts; startup to
  admission-ready, peak/steady RSS, latency distribution, cancellation/drain latency, peak queues
  and post-join ownership counters. Record units and measurement method. Agree budgets before
  qualification; do not invent performance numbers or retroactively fit limits to observations.
- Raw sanitized diagnostics, resource observations and CI run URLs retained with the exact
  artifacts. M2 success cannot be substituted for M3 measurements.

For each unsupported renderer API/module, record the exact bundle/member digest and import or
call site, API/symbol, observed failure phase, mapped source location if available, the declared
profile's support status, and the blocking issue. Distinguish an unsupported API from a renderer
bug, denied grant, integrity failure or version mismatch. Static API scans are leads, not proof
that a bundle executes; no React/HeroUI unsupported-API findings have been measured in this slice.

Fallback must be explicit: the deployment owner selects the existing Node production path when
native qualification is unavailable. Failed native admission must not automatically retry on
Node, bypass grants, or run altered bytes. Qualify the selected Node path separately; do not
equate Wuling's existing production service with the future same-artifact reference adapter.
Container installation, rollback and restart orchestration belong to the external deployment
owner, not to a self-modifying Runtime.

Documentation/Context7 follow-up belongs to the qualification slice: publish only actually
executed public examples and their support matrix in the curated documentation. Keep this
requirements draft labelled planned and out of implemented-example indexing. The existing
`context7.json` configuration is not evidence of a refresh or successful retrieval; no refresh,
submission or executable-example publication was performed here.

## First-slice completion boundary

This slice records upstream requirements, case IDs and blockers so #48 can be reviewed without
inventing application policy. It does **not** check off any #42 native acceptance checkbox.
Remaining prerequisites are explicit:

- [ ] Core/Wuling review and approve the minimum inputs/goldens.
- [ ] Core/runtime-api and Runtime agree #48's versioned artifact/entry contract.
- [ ] #48–#51 implement admission, Fetch, scoped grants and serving/lifecycle.
- [ ] #52 provides the pinned Core-generated renderer artifact and Node reference pair.
- [ ] #53 executes the shared corpus, then #42 records real parity/resources/limitations.
- [ ] Publish implemented examples and verify actual documentation/Context7 retrieval.
