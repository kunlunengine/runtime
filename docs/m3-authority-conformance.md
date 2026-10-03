# M3 authority conformance: first evidence slice

Status: **in progress, not qualified**. This is the first shared observation and
evidence slice for [#50](https://github.com/kunlunengine/runtime/issues/50) and
[#53](https://github.com/kunlunengine/runtime/issues/53), not the full M3 exit gate.
Neither a native-only pass nor successful evidence-tool unit tests qualify the
independent Node adapter. No mock Node permission implementation is provided.

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

The implementation is committed in [Core PR #7](https://github.com/kunlunengine/core/pull/7)
at `43de55918eda6415d1e20f29452a782383fb7098`. That PR is still open; it is not
a published adapter release or a completed cross-adapter qualification. The
source-preview package version remains `0.1.0`; that version alone cannot
identify these changes. Local development verification of the filesystem slice
does not qualify the four-platform gate.

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

The server records actual traffic: reaching a
forbidden or aborted destination fails even if the returned observations look
correct. Ephemeral ports are host inputs, not normalized response observations.

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

**Known Core dependency:** with PR #7's commit above, both Node invocations
record `redirect_escape: "returned:302"` instead of the contract's `"denied"`.
Node's scoped transport does not follow redirects even with `redirect: "follow"`;
native Fetch follows only after rechecking destination authority and rejects the
escape. All other HTTP observations and server traffic match in the local run.
The pre-headers revocation/admission observations also match. Returning a redirect
is not normalized into denial. Core must implement the
agreed Fetch redirect behavior before this slice can claim parity.

This is not full HTTP/lifecycle qualification. Request-owned confidential context
has no equivalent Node host-context API yet. Concurrent requests, cancellation
and revocation after headers, background work, and complete diagnostic
coverage still need shared host orchestration and formal collectors; native-only
tests are not substitutes. In particular, `waitUntil()` remains unsupported in
the Core slice.

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

A successful comparison qualifies only `request-authority/v1`, not the whole
of #50 or #53. Do not wire an absent Node job as a skipped-but-green M3 gate.
Full required-check integration must wait for reviewed adapter commits and a
complete physical-platform evidence workflow; this change adds no Actions
aggregation job.

## Remaining closeout work

| Work | Owner / tracker | Completion evidence |
| --- | --- | --- |
| Review and freeze the committed Node adapter | Core PR #7, Runtime #52 | Reviewed clean Core commit, exact package version, unchanged probe executions |
| Fix scoped Node redirect behavior | Core adapter, Runtime #50 / #53 | HTTP shared probe returns denial for redirect escalation, not a raw 302 |
| Complete shared #50 adversarial coverage | Runtime #50 / #53 and Core adapter | Cross-isolate/concurrent contexts, revocation/cancellation after headers, full teardown/diagnostics, and formal HTTP collection on both adapters |
| Four physical platform runs | Runtime #53 | Exact reviewed source/fixture identities and pinned receipts; no Rosetta substitution |
| Same generated portable application artifact | Core producer / Runtime #52 | Build recipe, producer/adapter versions, artifact hashes, both adapters |
| Inbound lifecycle and full application corpus | Runtime #51 / #53 | Routing/HTTP/streaming, bounded queues, errors, drain/restart and resource-accounting cases |
| Native-boundary regression and consumer qualification | M0–M2, Runtime #42 / #53 | Pinned sanitizers/Miri plus actual public Wuling SSR/docs results and measurements |

Keep #50 open until its complete acceptance criteria, including independent
adapter denial parity, have evidence. Keep #53 open until its broader application
and consumer exit criteria are met. Only a reviewed closeout PR satisfying the
relevant issue should use a closing reference; prose negating a closing keyword
next to an issue number can still create an accidental GitHub closing link.
