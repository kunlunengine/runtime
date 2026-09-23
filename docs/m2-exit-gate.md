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

## M2 closeout status

Verified on 2026-09-22: implementation and the ordinary four-platform main gate are green.
[#27](https://github.com/kunlunengine/runtime/issues/27) is closed;
[#46](https://github.com/kunlunengine/runtime/issues/46) remains open with unchecked closeout
criteria. This record supplies the restored baseline evidence, not an automatic issue closure
or signed release approval.
The same evidence is recorded in [#46's post-merge report](https://github.com/kunlunengine/runtime/issues/46#issuecomment-5775217809).

The [failed main run at `c277c6995d3518eb13af55843918df8caf62dcf9`](https://github.com/kunlunengine/runtime/actions/runs/35565536861)
terminated the macOS x64 module-deadline fixture during trusted Streams bootstrap.
[PR #54](https://github.com/kunlunengine/runtime/pull/54) separated the bounded bootstrap and
application scopes; it did not remove x64, disable enforcement, or waive a test.

| Evidence | Commit | Result |
| --- | --- | --- |
| [PR combined gate](https://github.com/kunlunengine/runtime/actions/runs/35690502557) | `02b6df613b15fed9bb0a75a13cf71067fd470851` | Passed |
| [Main combined gate](https://github.com/kunlunengine/runtime/actions/runs/35693402408) | `be0347b9d891d30f2d12503ec9d19086cfaa1ee8` | Four platform jobs, Miri and **M2 required gate** passed |
| [Main Check Rust](https://github.com/kunlunengine/runtime/actions/runs/35693402156) | `be0347b9d891d30f2d12503ec9d19086cfaa1ee8` | Passed |

All four downloaded main reports were accepted by `m2_gate.py summarize` from that checkout.
Each reports `status: passed`, `native_sanitizers: passed`, all required capabilities and all
10 dedicated conformance tests. The runner also executes the locked bundled workspace.
Shared identities:

- WebKit revision: `4b62d53ec6c16753020dbe69e59bf761ed0948e3`.
- Distribution manifest SHA-256: `0f8ed89b32ba24b63933476e52a815d4a7976e3d3d01842dec5ccef2f7c3957e`.
- M2 corpus SHA-256: `4d1ff0dc38a494f5b0959535084c8331eede62eb13003f48f2bb9310ee2b1085`.

| Target | Archive SHA-256 | Verification receipt SHA-256 |
| --- | --- | --- |
| `aarch64-apple-darwin` | `4051e20e690a97e07a22b889f0774043086b241ea19f476b6fa335e34382889a` | `639efdd1575f461de972527b36eef96fc0e88073a02878735046ba07ba8656b2` |
| `x86_64-apple-darwin` | `f634a008323b45fef222ae9eefb4cb794ccee05c2b08938be0cf1967cf923f09` | `310d648db66dd47e9f0153aec3e7aba3f611346c878605cddff8fe46195ac65d` |
| `aarch64-unknown-linux-gnu` | `256c490a14268e358d856c589078a35a080ef2dfced7ae71dd041774cfa1591f` | `3b03bf72e1439bf4fc72fd950332e1aa564297d018fa098e90f4390a59a10df1` |
| `x86_64-unknown-linux-gnu` | `d13d9b40adf8b1d07d8037fb268f2c0a3348107ecc752db244bb2dd3f4240fef` | `438dbc0360292dd5ac8cd04583d2c37ba5cde7e02bdb7e89d09d043c60ce9e2b` |

The main run was ordinary CI: independent cold rebuilds and attestation jobs were skipped by
design. It does **not** supply signed release or physical Intel qualification. Reports/logs and
release inputs remain subject to Actions retention; these hashes do not archive their contents.
Preserve those inputs in the release record using the procedure below before claiming release
validation. Distribution target states remain `planned`.

The repeated local constructor/module deadline evidence is recorded in PR #54 and
[#46's implementation report](https://github.com/kunlunengine/runtime/issues/46#issuecomment-5771616306).
That report also discloses an intermediate stall in an unchanged arm64 callback-reentry test;
subsequent passes do not establish its cause or prove that the bootstrap fix resolved it.
Retain that observation during final closeout review. M3 work in
[#47](https://github.com/kunlunengine/runtime/issues/47) is not additional M2 exit evidence.

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
