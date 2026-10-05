# M3 scoped authority conformance

Status: **shared implementation checks available; physical qualification deferred**.
These are shared observation and evidence slices for
[#50](https://github.com/kunlunengine/runtime/issues/50) and
[#53](https://github.com/kunlunengine/runtime/issues/53), not the full M3 exit gate.
Neither a native-only pass nor successful evidence-tool unit tests qualify the
independent Node adapter. No mock Node permission implementation is provided.
Physical execution is separately scheduled in
[#63](https://github.com/kunlunengine/runtime/issues/63); implementation work need
not wait for those runners, but no report is promoted to formal qualification.

## Corpus and comparison policy

The reviewed inputs are:

- [`request-authority.js`](../crates/kunlun-runtime/tests/fixtures/request-authority.js):
  identical probe bytes executed in two consecutive request environments in one
  application, so the second invocation can attempt to reuse the first handle.
- [`request-authority.contract.json`](../crates/kunlun-runtime/tests/fixtures/request-authority.contract.json):
  versioned setup requirements and independently specified expected observations.
- [`request_environment.rs`](../crates/kunlun-runtime/tests/request_environment.rs):
  the native adapter, using real artifact admission, host bindings, and bounded
  isolate shutdown rather than a JavaScript-only permission simulation.

This hand-authored probe is **not** a Core/BuildEngine-produced server artifact.
Producer portability remains [#52](https://github.com/kunlunengine/runtime/issues/52).
Its initial coverage is declaration/grant projection, optional absence, frozen
and non-serializable bindings, filesystem allow/deny outcomes, and stale-handle
denial across requests. The contract is the source of truth for exact cases.
Existing native and Node tests cover more than this shared slice; do not infer
cross-adapter coverage for admission, redirects, concurrent contexts,
cancellation/revocation, diagnostic redaction, or teardown merely from this
probe passing.

Both adapters must run the source unchanged against real owned host resources,
not precompute the expected output. The harness supplies the absolute path of
the existing private escape file through the contract's explicit test input;
neither traversal nor absolute-path denial relies on a nonexistent target.
Compare the complete two-request observation array against the checked-in
contract. Object key ordering is immaterial; types,
array order, missing/extra observations, successful contents, and allow/deny
outcomes are not normalized. Backend-specific error prose is not part of this
first observation contract; typed denial diagnostics and redaction require their
own shared cases before full qualification.

Both harnesses take their capability declarations from the contract. This
filesystem-only slice has no required HTTP declarations or network deployment
grants, so both adapters project an empty HTTP map. The native harness must not
inherit the broader HTTP setup used by its other, non-shared integration tests.

Each report binds the probe and contract by SHA-256 and the Runtime checkout by
full commit ID. Changing either input invalidates older reports.

## Native collection

The pinned macOS/Linux artifact workflows collect this slice **after** the
existing M2 tests; no M0–M2 requirements are removed or relaxed. They upload
`m3-authority-native-<target>` evidence separately from M2. A collection failure
is not converted into a successful observation.

From a clean reviewed checkout, using an already verified pinned artifact and
the same loader environment as the M2 procedure:

```sh
python3 distribution/jsc/scripts/m3_authority.py collect-native \
  --target aarch64-apple-darwin \
  --output "$RUNNER_TEMP/m3-authority-native-aarch64-apple-darwin"
```

`KUNLUN_JSC_DIST_DIR` and `KUNLUN_JSC_RECEIPT_SHA256` are required; Cargo does not
download an engine or fall back to the system framework. Use a new output
directory outside the checkout. Collection records the verified artifact,
manifest, receipt, backend, source identity, physical runner information, logs,
and actual observations. Failed collection retains a failed report. Reports are
CI evidence, not cryptographic attestations of adapter behavior: use artifacts
from trusted, reviewed runs, not caller-authored JSON.

M2 and M3 use one child-environment helper: after verifying the artifact identity,
they reconstruct the loader path from its `lib` directory and launch Cargo
directly. This also works when macOS SIP has removed `DYLD_LIBRARY_PATH` before
the system Python interpreter starts. Other inherited variables are preserved;
the M3 observations-path override is confined to the probe child. No intermediary
`/usr/bin/env` or shell is used to launch these Cargo commands.

For local developer feedback only, the integration test can export its raw
observations with an explicit macOS system backend:

```sh
KUNLUN_AUTHORITY_OBSERVATIONS="$TMPDIR/authority-observations.json" \
  cargo test --locked -p kunlun-runtime --test request_environment \
  --no-default-features --features system-jsc \
  adapter_neutral_authority_probe_runs_unchanged_across_requests -- --exact
```

The output file must not already exist. Raw observations carry the actual
backend identity; they are not a qualified report and cannot satisfy the
pinned comparison gate.

## Independent Node integration

The Core follow-up implements real declaration/grant admission, filesystem/HTTP
request bindings, portable manifest loading, and request lifecycle enforcement
in `@kunlun-js/runtime-node`. Its `scripts/collect-node-authority.mjs` consumes
the Runtime probe and contract directly, force-rebuilds the actual adapter, and
records/rechecks the emitted implementation hashes. It does not simulate
permissions or manufacture expected observations.

The baseline implementation was merged in
[Core PR #7](https://github.com/kunlunengine/core/pull/7), with merge commit
`a612ed336d3b51bc338b498653a35d8512be5390`. It is not a published adapter release
or a completed cross-adapter qualification. The source-preview package version
remains `0.1.0`; that version alone cannot identify these changes or subsequent
redirect fixes. Local development verification of the filesystem slice does not
qualify the four-platform gate.

From the Core checkout, with its dependencies installed:

```sh
pnpm authority:node \
  --runtime-root /path/to/runtime \
  --output /path/to/new/node-development \
  --development
```

Development reports explicitly use `status: "development"` and
`qualification: false`; the strict comparison rejects them. For reviewed
evidence, commit/review both repositories first, omit `--development`, and run
on each physical supported platform. Use new output directories outside the
checkouts so collection does not dirty the reviewed source.

Source-built adapter evidence is acceptable when tied to an exact reviewed Core
commit and package version; it must not be presented as a published package.
Producer output and its build recipe/hashes are a separate #52 deliverable and
are required for the later application corpus.

The Node runner emits `report.json` using the same common fields as native
collection (`schema_version`, `suite`, `status`, `commit`, `target`, `runner`,
`fixture_sha256`, `contract_sha256`, `observations`), sets `adapter` to `node`,
and records its exact package name, package version, Core source commit, and
Node version in `node`. The comparison tool validates those identities against
explicit operator-supplied expectations; it never manufactures a Node report.
It also requires `node.build.command` to be `pnpm run build --force` and
`node.build.modules` to contain exactly `index.js`, `application.js`, and
`authority.js`, each with a full lowercase SHA-256. The emitted module hashes
must match across all four Node reports for the same reviewed source. Missing
build records, incremental-only build commands, incomplete/extra inventories,
invalid hashes, and cross-platform differences fail closed. These records still
come from trusted reviewed collector runs, not cryptographic build attestations.
See the validator and its synthetic protocol tests for the precise field types.
Those tests validate the evidence format, not Node compatibility.

## Shared HTTP development check

[`request-authority-http.js`](../crates/kunlun-runtime/tests/fixtures/request-authority-http.js)
and its [contract](../crates/kunlun-runtime/tests/fixtures/request-authority-http.contract.json)
add a separate unchanged, two-request HTTP slice. It covers required-grant
admission, optional/undeclared omission, opaque handles, allowed Unicode response
content and headers, exact-host denial even when the other host is admitted,
undeclared destinations, credentials, unsupported schemes, pre-aborted calls,
manual redirects, redirect escalation, previous-request reuse, and redaction
of private diagnostic inputs. A second unchanged probe verifies host-triggered
application revocation: the server acknowledges receipt but withholds headers,
the host revokes/closes authority, invocation must reject before response release,
and later request admission must fail. Deadlines bound failures; explicit receipt
and release barriers determine ordering, not sleeps.

The expanded contract checks 28 observations in each request, including successful
default/explicit same-origin following, explicit escaped-host denial, error mode,
and the 20-hop limit. The servers consume real Content-Length/chunked uploads and
echo the received method, bytes and headers. POST 301/302 and PUT 303 must become
GET without body metadata headers, while GET/HEAD 303 keep their method and headers.
String/binary buffered 307/308 bodies must be replayed intact. Streamed 301/302 and
307/308 must reject without a destination request; streamed 303 may switch to GET.
These cases follow the [Fetch redirect rules](https://fetch.spec.whatwg.org/#http-redirect-fetch),
not a normalization of differing adapter behavior. Unrelated same-origin
Authorization, Cookie, and custom headers must remain intact.

The server records actual traffic: reaching a
forbidden or aborted destination fails even if the returned observations look
correct. Both complete ordered traffic lists must match the contract, including
exactly 21 `/loop` requests per invocation (initial request plus 20 allowed hops).
Ephemeral ports are host inputs, not normalized response observations.

Run the native side with a new output file:

```sh
KUNLUN_M3_HTTP_OBSERVATIONS="$TMPDIR/native-http.json" \
  cargo test --locked -p kunlun-runtime --test request_authority_http \
  --no-default-features --features system-jsc
```

Then run the same probe against the **real existing Core build**, without
modifying that checkout:

```sh
node distribution/jsc/scripts/check-authority-http.mjs \
  --core-root /path/to/core \
  --native "$TMPDIR/native-http.json" \
  --output "$TMPDIR/node-http.json"
```

The check imports Core's actual `createRequestAuthority`, rechecks the emitted
module hashes and source commit, compares all observations and traffic exactly,
and exits nonzero for any mismatch. It does not force-rebuild the adapter and
always records `qualification: false`; these reports cannot satisfy the pinned
eight-report gate. Native output is written only after shutdown, empty resource
counts, server join, and fixture cleanup. Existing outputs are never overwritten.
CI tests the report validator and runs the native fixture, not a fake Node provider.

Native fixture parsing returns bounded errors for malformed or truncated uploads.
Teardown interrupts an active socket before joining the server, and `Drop` does
not panic when the test is already unwinding. Dedicated receipt handshakes test
stalled-upload interruption; the normal success path still rejects server errors
before exporting evidence. HEAD responses reflect every tested received header
without emitting a response body, so header loss cannot hide behind HEAD semantics.

**Local redirect parity (development only):**
[Core PR #9](https://github.com/kunlunengine/core/pull/9), commit
`52e992b4ca59d7ddb73442f59d2b953abcf00fbd`, fixes the merged PR #7 baseline that
returned `redirect_escape: "returned:302"`. The actual rebuilt adapter on Node
20.20.2, 22.22.1, and 24.15.0 has been compared independently with native
system-JSC on local macOS arm64. All three runs match both 28-observation requests,
all 101 ordered server paths, and the revocation/later-admission observations.
The redirect escape is denied, not normalized from a returned 302.

Core PR #9 merged on 2026-10-05 at
`96b5c565385bf5160a81892d07e22b997bc31413`, with final PR head
`c2394d6944a4eb9ac89be007940f16fdaa2a8937`. Landing the Core implementation is
no longer a blocker. The development reports above remain tied to `52e992b4`;
they must not be relabeled as evidence for the later merged source. New reviewed
collection must use the exact updated Core commit and emitted module hashes.

The verified shared inputs are:

- Fixture SHA-256: `0071829a29bc3acca1a1db1e4c52625b044380e977c393d074302fd968846cd7`
- Contract SHA-256: `59c38f183d75fc63c282ee3c6023e26c9cc89d0484c31eccc66264eb5d9fed31`

These reports remain `status: "development"` and `qualification: false`. The
comparison recomputes current corpus hashes, compares complete observations and
traffic with native, and checks the exact Core source commit and Node version.
Matching observations and emitted module hashes across three local Node versions
do not replace reviewed pinned physical-platform collection.

This corpus does not qualify redirect replay of an already-created byte-backed
Node Request on Node 20/22 or multipart FormData. Core documents those upstream
limits; the shared binary case uses explicit RequestInit bytes, and the native
profile does not support FormData. Do not generalize this pass to full Fetch
compatibility or hide one-shot body differences by buffering streams.

This is not full HTTP/lifecycle qualification. The next slice covers bounded
invocation cancellation after headers and concurrent isolate independence; it
does not qualify inbound response delivery or background work. In particular,
`waitUntil()` remains unsupported in the Core slice.

## Shared post-headers and concurrent-isolate check

[`request-authority-lifecycle.js`](../crates/kunlun-runtime/tests/fixtures/request-authority-lifecycle.js)
and its [contract](../crates/kunlun-runtime/tests/fixtures/request-authority-lifecycle.contract.json)
run unchanged on both real adapters. The fixture sends response headers and the
first chunk, but withholds the rest. JavaScript starts a second body read before
sending `/ready?pending=true`; receipt of that request is the host's explicit
progress barrier. A microtask checkpoint rejects an already-settled read, and
settlement while the acknowledgement is pending rejects the probe as well.
The host reads an immutable observation captured at that barrier, not a fabricated
pending value after cancellation. No delay is used to infer that headers/body
were received.

The host then either revokes the application or cancels just the invocation.
Native cancellation drops the consumed evaluation future; Node cancellation
aborts the real invocation's host-owned signal. Both must stop before the fixture
releases either pending response. Application revocation denies later admission.
Request cancellation leaves the application usable: a subsequent request denies
the retained handle and body reader, then successfully uses its own fresh handle.
Repeated bounded close/shutdown must complete.

The concurrent slice runs two real isolates/VM realms and independent authority
owners. While A is still blocked, B must fetch successfully without seeing A's
retained handle or progress globals. Only after that result does the host revoke
A, observe rejection, and invoke B successfully again. Both ordered traffic
lists are compared exactly. This is overlapping invocation execution in separate
isolates, not concurrent JavaScript invocations in one native isolate, a process
isolation claim, or a test of an inbound server's scheduling policy.

The shared probe checks the exposed `fs`/`http` shape, non-serialization and
absence of context/identity/secret fields. Native additionally supplies distinct
auth/provider/billing sentinels to real `RequestContext` objects, verifies their
owners and request-specific revocation, and asserts empty resource counts before
and after graceful shutdown. These remain separately labeled `native_assertions`.
Node has no public host-context or resource-counter API in this bounded primitive:
the checker does **not** synthesize those assertions or claim credential-service
parity. Neither adapter exposes a provider, billing or secret service in v1.
Any future such service must bring its own shared owner/redaction cases.

```sh
KUNLUN_M3_LIFECYCLE_OBSERVATIONS="$TMPDIR/native-lifecycle.json" \
  cargo test --locked -p kunlun-runtime --test request_authority_lifecycle \
  --no-default-features --features system-jsc \
  shared_post_headers_lifecycle_matches_contract -- --exact

node distribution/jsc/scripts/check-authority-lifecycle.mjs \
  --core-root /path/to/existing/core-build \
  --native "$TMPDIR/native-lifecycle.json" \
  --output "$TMPDIR/node-lifecycle.json"
```

Both developer reports are `status: "development"`, `qualification: false`.
The checker is read-only, binds/rechecks actual Core module hashes and commit,
and checks observations, lifecycle, concurrency and traffic against the contract
and native report. Native context/counters are checked only for native reports.
Successful Node cleanup requires authority-owned connections to close before
the server joins; forced fixture interruption is reserved for failed cleanup.
Native requires resource drain **before** releasing held fixture sockets, so peer
EOF cannot be credited as cancellation cleanup. It exports only after bounded
shutdown, resource accounting and fixture joins.
Existing output files are never overwritten.

Latest local verification of this follow-up used the real existing Core PR #9
final-head build at `c2394d6944a4eb9ac89be007940f16fdaa2a8937` on Node 20.20.2,
22.22.1 and 24.15.0, against native system-JSC on macOS arm64. Both HTTP and
lifecycle checks matched their complete contracts and native observations,
including concurrent traffic. The probe regression tests also reject immediate
EOF/rejection and settlement during acknowledgement, and detect nested,
symbol-keyed/symbol-valued and prototype secret data without calling getters.
Workspace tests, Clippy, formatting, 104 Python tests, 64 Node tests and workflow
lint passed. These are working-tree development checks, not clean reviewed
pinned-platform collection or evidence for Core's later merge commit.

## Reviewed HTTP and lifecycle evidence collection

The same strict reviewed-commit collector now also supports
`request-authority-http/v1` and `request-authority-lifecycle/v1`. Native
collection requires a verified pinned JSC distribution and its receipt, runs
exactly the selected probe, validates its actual backend, and rechecks
source/corpus/artifact identities. The pinned workflows upload each suite
separately from filesystem and M2 evidence.

```sh
for slice in http lifecycle; do
  python3 distribution/jsc/scripts/m3_authority.py collect-native \
    --suite "request-authority-$slice/v1" \
    --target "$TARGET" --output "$EVIDENCE/$slice/native-$TARGET"

  python3 distribution/jsc/scripts/m3_authority.py collect-node \
    --suite "request-authority-$slice/v1" \
    --core-root "$CORE_ROOT" --target "$TARGET" \
    --output "$EVIDENCE/$slice/node-$TARGET" \
    --commit "$REVIEWED_RUNTIME_COMMIT" --node-commit "$REVIEWED_CORE_COMMIT" \
    --node-package-version "$REVIEWED_NODE_ADAPTER_VERSION"
done
```

Native uses the same `KUNLUN_JSC_DIST_DIR` / `KUNLUN_JSC_RECEIPT_SHA256` loader
environment as filesystem collection. Node collection requires clean exact
reviewed Runtime and Core sources, a matching physical runner and Node process
architecture, and a new output directory outside both checkouts. It deliberately
reinstalls dependencies from the reviewed frozen lockfile into a fresh
evidence-local pnpm store with lifecycle scripts disabled, then force-builds
Core with `pnpm run build --force`. It executes the real selected probe and
rechecks package/source/lockfile/corpus identities, dependency closure and emitted
module hashes. Actual observation-process executable/architecture/platform and
Linux glibc identity are recorded; ambient Node/preload/library injection is
rejected. A preliminary `node -p` result alone is not process provenance.
Unlike the read-only developer command, run this only in a disposable Core
checkout where you intend to reinstall dependencies and build. Errors retain
failed reports, not successful empty results.

`collect-node-http` remains an alias with HTTP as the default suite. The earlier
filesystem collector/report shape remains compatible; new HTTP/lifecycle formal
reports must include the strengthened installation/runtime provenance.
Developer reports cannot substitute for any formal suite.

The strengthened report additionally binds:

- `node.runtime`: the observation process's exact version, architecture,
  platform, executable SHA-256 and libc identity.
- `node.dependencies`: content hashes of the actual runtime dependency closure,
  including the HTTP transport dependency, rechecked after execution.
- `node.installation`: the frozen/no-scripts installation recipe, fresh store
  and CI settings, and the reviewed lockfile SHA-256.

Installation/dependency records must agree across the four Node reports for the
same reviewed Core source. Executable hashes are process identities, not a
requirement that different platforms share the same native Node binary. A report
with an `error` field cannot be accepted as a successful execution.

## Four-platform comparison

Place exactly one native and one real Node report for each supported target in
separate child directories beneath the evidence directory (eight reports):

- `aarch64-apple-darwin`
- `x86_64-apple-darwin`
- `aarch64-unknown-linux-gnu`
- `x86_64-unknown-linux-gnu`

Then, from the same reviewed Runtime checkout:

```sh
python3 distribution/jsc/scripts/m3_authority.py compare \
  --evidence "$RUNNER_TEMP/m3-authority-evidence" \
  --commit "$REVIEWED_RUNTIME_COMMIT" \
  --node-commit "$REVIEWED_CORE_COMMIT" \
  --node-package-version "$REVIEWED_NODE_ADAPTER_VERSION"
```

Missing, duplicate, skipped, failed, stale, mismatched, system-JSC, and
wrong-architecture evidence fails closed. The pinned macOS workflow currently
builds/runs its x64 target on an arm64 host using Rosetta. Those observations
remain useful diagnostics, but **do not qualify physical Intel coverage**.
The comparison rejects them; a physical Intel pinned run is still required.
This does not redefine or weaken the existing M2 gate.

The command defaults to `request-authority/v1`. For HTTP or lifecycle, add
`--suite request-authority-http/v1` or `--suite request-authority-lifecycle/v1`
and use separate evidence directories with eight reports **per suite**. HTTP
comparison additionally requires the revocation probe hash, complete lifecycle
and exact ordered traffic. Lifecycle additionally compares the concurrency
observations/traffic and verifies native-only accounting without inventing it
for Node. Both require matching reviewed installation/dependency provenance.

A successful comparison qualifies only its selected slice, not the whole
of #50 or #53. Do not wire an absent Node job as a skipped-but-green M3 gate.
Full required-check integration must wait for reviewed adapter commits and a
complete physical-platform evidence workflow; this change adds no Actions
aggregation job.

## Remaining closeout work

| Work | Owner / tracker | Completion evidence |
| --- | --- | --- |
| Shared bounded #50 implementation checks | Runtime #50 and real Core adapter | HTTP denials plus post-headers revocation/cancellation, stale stream/handle denial, concurrent isolate independence, opaque env and actual traffic; native-only context/counter proof remains labeled separately |
| Reviewed HTTP/lifecycle collection tooling | Runtime #50 / #53 | Exact probe execution, pinned backend and clean source checks, frozen dependencies/actual Node process, real forced build, strict eight-report comparison |
| Four physical platform runs | Deferred Runtime #63, formal gate #53 | Updated reviewed Runtime/Core/corpus identities, pinned receipts and complete per-slice reports; no Rosetta or developer-report substitution |
| Same generated portable application artifact | Core producer / Runtime #52 | Build recipe, producer/adapter versions, artifact hashes, both adapters |
| Inbound lifecycle and full application corpus | Runtime #51 / #53 | Routing/HTTP/streaming, bounded queues, errors, drain/restart and resource-accounting cases |
| Native-boundary regression and consumer qualification | M0–M2, Runtime #42 / #53 | Pinned sanitizers/Miri plus actual public Wuling SSR/docs results and measurements |

Physical execution is intentionally split from the #50 implementation schedule
into #63; #53 retains the formal qualification requirement. M4 business-logic
development may proceed in parallel: these deferred evidence tasks are not
prerequisites for starting that work. This scheduling decision does not qualify
an adapter, platform or release. Keep #50 open until its
implementation/shared-acceptance closeout is reviewed, and keep #53/#63 open
until their respective evidence gates pass. Only a reviewed closeout PR satisfying
the relevant issue should use a closing reference; prose negating a closing keyword
next to an issue number can still create an accidental GitHub closing link.
