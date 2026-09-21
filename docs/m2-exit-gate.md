# M2 conformance and release exit gate

The gate is `.github/workflows/m2.yml` (**M2 exit gate**). It always runs on pull requests,
main pushes, and merge-queue commits, including documentation-only changes. Configure
**M2 required gate** as a required repository/merge-queue check; changing branch protection is
a maintainer action, not something the workflow can enforce itself.

The final job requires successful macOS, Linux, and Miri jobs **and exactly four successful
platform reports** at the checkout's full commit SHA. Missing, skipped, cancelled, failed,
duplicated, or mismatched evidence cannot produce a green gate. There is no system-JSC fallback.

## Validation ladder

1. **Portable policy tests:** `Check Rust` validates the manifest, offline backend rejection,
   artifact tooling, gate evidence rejection, formatting/linting, and Web declarations.
2. **Developer baseline:** `Check Rust` runs macOS system-framework tests and explicit
   `test-native-ownership.sh --system` ASan/UBSan. This is useful fast feedback, **not M2 evidence**.
3. **Pinned native boundary:** both platform builders compile the shim and ownership harness
   with ASan/UBSan, assertions enabled, and `KUNLUN_JSC_BUNDLED`, against the just-built engine.
   Absence of the pinned M2 success marker is fatal to the corpus runner.
4. **One runtime corpus:** `m2_gate.py run` runs `doctor`, the entire locked bundled workspace,
   and the dedicated `m2_conformance` integration suite identically for:
   - `aarch64-apple-darwin`
   - `x86_64-apple-darwin`
   - `aarch64-unknown-linux-gnu`
   - `x86_64-unknown-linux-gnu`
5. **Rust-owned invariants:** pinned `nightly-2026-09-01` Miri runs ownership guards and the
   production rejection ledger through `xtask`, without linking or executing native JSC.
6. **Release validation:** the weekly run and manual default additionally require independent
   cold rebuilds with byte-identical archives, signed provenance/SPDX, and evidence verification.
   A normal PR gate does not perform the cold rebuild or authorize publication.

Linux tests run on matching native glibc runners. Both macOS artifacts are built on the pinned
Apple Silicon Xcode image; the x86_64 binaries execute via Rosetta after an explicit availability
check. This is x86_64 artifact coverage, not a claim of physical Intel hardware coverage.
Unavailable runners, Rosetta, toolchains, or capabilities block the gate rather than reducing
the matrix. `doctor` must report native ESM, explicit microtasks, deferred Promises, watchdog,
heap telemetry, host streams/abort, Temporal, the correct backend/target/revision, and hermetic mode.

## Corpus and leak coverage

`crates/kunlun-runtime/tests/m2_conformance.rs` and `tests/fixtures/m2/` are the common corpus.
The runner hashes their names and contents and requires every checked-in `#[test]` to pass:
zero tests, ignored tests, and filtered tests are failures.

| Area | Assertion / synchronization |
| --- | --- |
| Cyclic ESM, live bindings, TLA, dynamic import | Shared cyclic graph; repeated concurrent imports retain namespace identity and execute once |
| Promise, timer, host-completion ordering | Nested FIFO jobs, `sleep(0)` dispatch, then explicitly sequenced filesystem completion; no disk-versus-timer timing assumption |
| Cancellation | Drop before first poll and after an observed `Pending`; cover reusable script and retired module evaluations |
| Streams and abort listeners | Slow consumer yields through dispatcher; cancel before/during open and after a read; retained-signal listener accounting across success, error, EOF, and abort |
| Host resource ownership | Copied pending timer/call/request/stream counts reach zero after completion/error/cancel; active task count reaches zero at graceful shutdown |
| Rejection ownership | Repeated unhandled/handled transitions retain IDs and copied diagnostics; draining reports is independent of the ledger; Miri checks exact drops and isolate separation |
| Native module/root ownership | Test-only protection-obligation, module-handle, registration and backing-store counters are zero after each context teardown, including abandoned loading graphs |
| Deadline and memory policy | Infinite JS must terminate with a terminal deadline error; allocation pressure uses copied telemetry and a relative hard limit, not a platform-specific heap guess |
| Repeated lifecycle | Repeated startup/error/recovery/shutdown; abandon streams and pending jobs; zero host resources at the join boundary |

The full workspace also retains loader failure, malformed graphs, reentrancy, callback panic,
rejection conversion/GC, allocation-pressure watchdog, bounded stream, and signal-shutdown tests.
Correctness uses checkpoints, polling, channels, or completion/join boundaries. Timeouts are only
failure bounds (or the resource policy under test), never evidence that another actor has progressed.

Leak checks are ownership checks, **not a claim that RSS returns to baseline**. Release JSC itself
is not sanitizer-instrumented; ASan/UBSan instrument the shim/harness. LeakSanitizer remains disabled
because uninstrumented JSC process-global caches are not attributable to an isolate. Native counters
cover shim roots/handles and backing allocations, Rust counts cover host resources and diagnostics,
and JS tests cover listener cleanup. JSC-internal graph/job/GC storage is released by VM teardown;
these checks do not provide allocation-by-allocation accounting of WebKit internals.

## Rerun from one immutable reviewed commit

Start with a clean checkout. Pin an immutable reviewed tag to the full candidate SHA (do not move
the tag afterward), then dispatch the **combined** workflow, not two unrelated platform runs:

```bash
reviewed_tag=YOUR_IMMUTABLE_REVIEWED_TAG
git rev-parse "$reviewed_tag^{commit}"
git status --porcelain
gh workflow run m2.yml --ref "$reviewed_tag" -f compare_rebuild=true
gh run list --workflow m2.yml --event workflow_dispatch
gh run view RUN_ID --json headSha,conclusion,jobs,url
gh run download RUN_ID --pattern 'm2-*' --dir evidence
python3 distribution/jsc/scripts/m2_gate.py summarize \
  --evidence evidence --commit "$(git rev-parse "$reviewed_tag^{commit}")" \
  --output m2-summary.md
```

Run the summarizer from that same reviewed checkout. Confirm the run's `headSha`, all four targets,
both reusable build/release workflows, Miri, and **M2 required gate** succeeded. Preserve `m2-*`
reports/logs and all four `kunlun-jsc-*` signed artifact sets before their 30-day retention expires.
Record the run URL, commit, manifest/corpus hashes, artifact/receipt hashes, and independent-rebuild
reports in the M2 tracking issue/release record. Reports are evidence from a trusted workflow, not
signed release authorizations on their own.

For an already verified local artifact from the same audited build, reproduce one target with:

```bash
export KUNLUN_JSC_DIST_DIR=/absolute/path/to/verified/artifact
export KUNLUN_JSC_RECEIPT_SHA256=TRUSTED_VERIFIER_RESULT_DIGEST
export DYLD_LIBRARY_PATH="$KUNLUN_JSC_DIST_DIR/lib" # LD_LIBRARY_PATH on Linux
python3 distribution/jsc/scripts/m2_gate.py run \
  --target aarch64-apple-darwin --output /new/evidence/m2-aarch64-apple-darwin \
  --native-log /trusted/build/pinned-asan-ubsan.log
cargo +nightly-2026-09-01 miri test --locked -p xtask -- jsc_ownership jsc_rejection_ledger
```

The runner rejects a dirty checkout (including untracked source files), a different manifest/receipt/target, missing native
verification, missing capabilities, or incomplete test inventory. Cargo rehashes the installed
artifact tree before linking. Logs and a failed report are retained on failure; the outer process-group
watchdog kills a stalled native test as well as Cargo. Keep artifact files, receipt trust, loader
environment, and the source-build log under trusted control throughout execution.

All manifest targets currently remain `planned`. Green validation does not publish them, update
their digests, waive review, bypass the merge queue, or close the milestone by itself.
