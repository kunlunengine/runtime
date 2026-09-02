# JavaScriptCore Distribution Manifest

[`distribution/jsc/manifest.json`](../distribution/jsc/manifest.json) is the reviewable source of
truth for the WebKit revision and every input that can change a Kunlun JavaScriptCore artifact. Its
shape is documented for editors and review tooling by
[`distribution/jsc/manifest.schema.json`](../distribution/jsc/manifest.schema.json). The repository
does not execute that schema: the Rust validator is the sole enforced source of manifest acceptance
and additionally checks cross-field and local-file integrity rules.

The manifest controls the default `bundled-jsc` Cargo backend, but does not authorize network
access. Cargo consumes only an explicitly installed local artifact plus a trusted verification
receipt digest. It never resolves a URL, downloads, extracts, or falls back to the system engine.

## Selecting a Cargo backend

All three runtime crates expose the same mutually exclusive features:

| Selection | Supported targets | Behavior |
| --- | --- | --- |
| Default / `bundled-jsc` | macOS arm64/x64, Linux glibc arm64/x64 | Require an offline-verified pinned artifact |
| `--no-default-features --features system-jsc` | macOS arm64/x64 | Link the host framework for development only |
| Neither, both, or an unsupported target | None | Actionable compile-time failure |

Feature forwarding disables dependency defaults so opting into `system-jsc` at the runtime crate
selects the same backend throughout the dependency graph. `--all-features` is intentionally invalid.
`system-jsc` does not establish product compatibility, even if compiled with optimizations. Unset
`KUNLUN_JSC_DIST_DIR` and `KUNLUN_JSC_RECEIPT_SHA256` before switching to system development; setting
distribution variables with `system-jsc` is an error rather than an ignored configuration.

### Install and consume a local artifact

Obtain the archive, SPDX SBOM, and provenance from the reviewed release process **outside Cargo**.
For a target marked `published`, the verifier checks all three files against the checked-in manifest
digests before extracting. Provenance authenticity must be reviewed before those digests are
published; matching bytes is not a substitute for authenticating the publisher.

```bash
python3 distribution/jsc/scripts/jsc_artifact.py verify \
  --manifest distribution/jsc/manifest.json \
  --target aarch64-apple-darwin \
  --archive /trusted/artifacts/kunlun-jsc-4b62d53e-aarch64-apple-darwin.tar.zst \
  --sbom /trusted/artifacts/kunlun-jsc-4b62d53e-aarch64-apple-darwin.spdx.json \
  --provenance /trusted/artifacts/kunlun-jsc-4b62d53e-aarch64-apple-darwin.intoto.jsonl \
  --install-dir /absolute/path/to/new-verified-jsc > /trusted/verification-result.json

export KUNLUN_JSC_DIST_DIR=/absolute/path/to/new-verified-jsc
export KUNLUN_JSC_RECEIPT_SHA256=$(jq -er .receipt_sha256 /trusted/verification-result.json)
export DYLD_LIBRARY_PATH="$KUNLUN_JSC_DIST_DIR/lib"
cargo test --workspace
cargo run -p kunlun-runtime -- doctor
```

Use the selected Linux triple and its artifact names on Linux, and set `LD_LIBRARY_PATH` instead
of `DYLD_LIBRARY_PATH`. Native verification runs on the matching platform with `zstd` and its native
inspection tools installed. `--install-dir` must not already exist. The verifier installs only the
validated extracted bytes; `--skip-native` can never produce an installable receipt.

All current targets are still `planned`, so no published release is implied. Controlled local/CI
source builds explicitly pass `--source-build` instead of `--provenance`; this trusts the caller's
audited source-build process and records `source-build`, not `published`. The platform build scripts
do this after native verification and expose the new `verified/<target>` directory and receipt hash
to their corpus jobs. Never use `--source-build` to bless an arbitrary downloaded archive.

The directory contains `.kunlun-jsc-verification.json` recording the manifest digest, target,
verification mode, archive/SBOM digests, and SHA-256 of every extracted file. Cargo requires its
**separately supplied** digest from the trusted verification result, then rehashes the entire tree.
It rejects missing/extra/corrupt files, symlinks, metadata mismatches, a different manifest, and a
header different from the checked-in ABI header. Cargo watches the receipt, entire tree, and all
consumed files so edits invalidate its cached build-script result. A writable receipt plus a digest
computed from that same untrusted receipt establishes no trust. Keep the installed tree, digest,
and dynamic-loader environment under the same trusted control through linking and execution;
verification is not a defense against a process that can rewrite them concurrently or inject libraries.

`doctor` reports `backend`, full pinned `engine revision` (or explicitly unknown host-managed
revision), `target`, `distribution mode`, distribution description, and `hermetic`. Here `hermetic`
means Kunlun controls the selected JSC build, not that libc/system dependencies disappear.

The offline backend policy and corruption tests run with `cargo test -p xtask`; actual Cargo feature
rejection tests run with `python3 distribution/jsc/scripts/test_cargo_backends.py` after
`cargo fetch --locked` has cached Rust dependencies. Every rejection invocation uses `--offline`.
Miri tests the shared ownership guards through `cargo +nightly miri test -p xtask jsc_ownership`,
without creating an unsupported engine backend.

## Validate the manifest

Run the lightweight repository task from the repository root:

```bash
cargo xtask jsc-manifest validate
```

The command reads only checked-in files. It rejects unknown or missing fields, abbreviated WebKit
revisions, malformed SHA-256 values, duplicate patches, inconsistent target metadata, unpinned
toolchains, and local patch or license inputs whose contents do not match their recorded digest.
`cargo test --workspace` also runs negative tests for these rules.

## Format and artifact states

The v1 manifest contains these review boundaries:

- `source` pins the canonical WebKit repository, full 40-character commit, and commit URL.
- `build` records the configuration, upstream build driver, ordered arguments for each host,
  deterministic environment, and feature flags.
- `toolchains` assigns exact tool versions to reusable macOS and Linux profiles. Linux additionally
  pins a multi-architecture OCI image index by digest and an Ubuntu archive snapshot timestamp;
  macOS pins the Xcode build and SDK directly.
- `targets` carries all four supported target triples, deployment baseline, archive layout, runtime
  libraries, SBOM, and provenance records without separate platform manifests.
- `patches` is ordered. Every entry must explain its purpose and name a checked-in file whose
  SHA-256 is verified. An empty list means the pinned source is unpatched.
- `licenses` inventories local and pinned-upstream license inputs. Local inputs are hashed on every
  validation; upstream hashes are reviewed against the pinned WebKit tree without adding network
  access to local builds.
- `abi` versions the Kunlun shim and lists the headers consumers may use from an archive.

Each target artifact is either `planned` or `published`. A planned artifact has stable output paths
but all three integrity values (archive, SBOM, and provenance) must be `null`; this avoids inventing
hashes before the platform build exists. A published artifact must populate all three values with
lowercase SHA-256 digests. Platform build work changes one target atomically from planned to
published after producing and checking all three files.

Artifact paths are repository-relative logical release paths. They are not checked into this
repository and a Cargo build must never download them implicitly. The planned archive layout is:

```text
include/kunlun_jsc.h
lib/libJavaScriptCore.{dylib,so}
lib/libkunlun_jsc.{dylib,so}
licenses/<reviewed license inputs>
metadata/build.json
metadata/runtime-dependencies.json  # Linux only
```

External SBOM/provenance entries inside build metadata contain only their logical path and format,
not their digests: embedding the SBOM digest would create a circular hash dependency when a target
is published. Reviewed release digests live in the distribution manifest.

The metadata records the source revision, effective build arguments and feature flags, deployment
target, observed tool versions, runner image identity, ABI, license inventory, and logical SBOM and
provenance paths. The separate SPDX document inventories every regular file in the archive by
SHA-1 and SHA-256. Archives contain no symlinks, build directories, private WebKit headers, or
unrelated tools.

## When to run the artifact workflows

`Build pinned JSC for macOS` (`jsc-macos.yml`) and `Build pinned JSC for Linux`
(`jsc-linux.yml`) are path-filtered artifact gates, not required checks for every pull request. A
pull request that changes the JSC crates, runtime corpus, distribution inputs/tooling, workspace
lockfiles, or the platform workflow runs one cached build for both architectures on that platform.
The workflows also run weekly on separate days and can be dispatched manually. Weekly and default
manual runs require an independent cold rebuild; pull-request runs do not generate signed release
evidence because untrusted pull-request jobs have read-only credentials.

The normal `Check Rust` workflow remains the bounded merge-queue gate. It validates the manifest,
artifact tooling, fail-closed offline Cargo behavior, Rust code, macOS system-framework corpus,
ASan/UBSan ownership harness, and Miri ownership invariants automatically. The expensive artifact
workflows are intentionally not required for unrelated paths; GitHub path-filtered workflows must
not be configured as a repository-wide required check because a skipped workflow would remain
pending.

| Change or purpose | macOS builder | Linux builder | When / mode |
| --- | --- | --- | --- |
| Host/CLI code or documentation unrelated to the JSC boundary | No | No | Normal PR checks are sufficient. |
| Rust JSC bindings, pinned-backend selection, or the shared binding corpus | Yes | Yes | Path-filtered PR runs build once; use release validation on the final candidate. |
| WebKit revision, patches, shared engine flags, C ABI/shim, licenses, or shared packaging logic | Yes | Yes | Path-filtered PR runs build once; release validation is mandatory before accepting new artifacts. |
| macOS build/cache workflow, Xcode/SDK, or macOS deployment settings only | Yes | No | Validate the affected platform; use release mode for final artifact/build-input changes. |
| Linux build/cache workflow, OCI/APT toolchain, ELF policy, or glibc baseline only | No | Yes | Validate the affected platform; use release mode for final artifact/build-input changes. |
| Publish a four-target release or refresh release evidence | Yes | Yes | Maintainer runs release validation from the same reviewed ref/commit for both workflows. |

The relevant platform means **both architectures**: macOS builds arm64 and Intel on the pinned
Apple Silicon Xcode image; Linux builds arm64 and x64 on matching native runners. If a shared
manifest edit changes only one platform's toolchain, run that platform; a shared JSC flag/revision
change affects both. A documentation-only workflow/runbook edit does not require an engine rebuild.

Both workflows offer the same manual input and apply equivalent modes to automatic events:

- **Pull-request validation:** restore the compiler cache, build and package once, verify the
  artifact, run the binding corpus and `doctor`, and upload a short-lived unsigned candidate. A
  cache miss still performs a full build. This mode is never sufficient for publication.
- **Fast manual validation — `compare_rebuild=false`:** perform the same single build, then sign and
  verify candidate provenance and the exact SPDX predicate before uploading evidence. Signed output
  alone is not release approval because this mode has no independent-rebuild report.
- **Release validation — scheduled or `compare_rebuild=true` (manual default):** do all of the above,
  then build again from a new source checkout with a fresh, non-shared compiler cache and require
  byte-identical archives. This deliberately takes a full cold build even when the first build hits
  its cache. Do not dispatch it for every routine Rust-only change.

In GitHub, open **Actions → Build pinned JSC for macOS / Linux → Run workflow**, select the
candidate branch/tag, and uncheck `compare_rebuild` for fast validation. The CLI equivalent is:

```bash
# Candidate branch: run only the platform(s) selected by the table above.
gh workflow run jsc-macos.yml --ref YOUR_BRANCH -f compare_rebuild=false
gh workflow run jsc-linux.yml --ref YOUR_BRANCH -f compare_rebuild=false

# Reviewed release candidate: use an immutable tag pinned to the reviewed head SHA.
reviewed_tag=YOUR_IMMUTABLE_REVIEWED_TAG
gh workflow run jsc-macos.yml --ref "$reviewed_tag" -f compare_rebuild=true
gh workflow run jsc-linux.yml --ref "$reviewed_tag" -f compare_rebuild=true
```

Both build scripts run the [shim ownership ASan/UBSan corpus](./jsc-binding.md#ownership-verification)
against the freshly built engine before packaging, without instrumenting release libraries. Changes
to the callback/buffer header require fresh artifacts on both platforms.

Link the relevant run(s), their exact head SHA, and the chosen mode in the PR. If artifact inputs
change afterward, rerun the affected platform(s). Before publication, verify both workflows used
the intended reviewed commit and that all four target jobs, independent comparisons, binding
tests, attestation jobs, and signed-evidence verification passed. `verify-evidence` rejects an
unlisted or corrupt file, re-runs archive/manifest/license/SPDX/native validation, verifies the local
SLSA bundle against the exact workflow and source commit, and requires the signed SPDX predicate to
equal the supplied SBOM before upload. Download the archive, SBOM, provenance, checksums, and rebuild
report from **Artifacts**. Uploading temporary workflow artifacts does not publish a release or
change a manifest target to `published`; durable publication and digest review remain separate.

## M1 exit-gate procedure

Run the final gate from one immutable reviewed tag whose commit contains every intended M1 change.
Start from a clean clone, confirm `git status --porcelain` is empty, and record the tag and full SHA.
On macOS, run the bounded repository gate locally:

```bash
cargo xtask jsc-manifest validate
python3 -m unittest discover -s distribution/jsc/scripts/tests -v
cargo fetch --locked
python3 distribution/jsc/scripts/test_cargo_backends.py
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --no-default-features --features system-jsc -- -D warnings
cargo test --workspace --no-default-features --features system-jsc
distribution/jsc/scripts/test-native-ownership.sh
cargo run -p kunlun-runtime --no-default-features --features system-jsc -- doctor
cargo +nightly miri test -p xtask jsc_ownership
```

`test_cargo_backends.py` requires dependencies to have been cached with `cargo fetch --locked`; every
Cargo rejection probe itself runs `--offline`. The default bundled backend must fail before native
compilation when a separately verified local artifact and receipt digest are absent. No build script
may add a downloader or treat Cargo's dependency cache as native-engine authority.

Dispatch both platform workflows with `compare_rebuild=true` from the same tag as shown above. A
weekly run is reusable evidence only when its full source SHA is the reviewed SHA and no relevant
input changed afterward. The exit gate passes only when `Check Rust` and both artifact workflows are
green, all four targets ran the same workspace corpus and `doctor` against pinned artifacts, both
sanitizers and Miri passed, every independent rebuild was byte-identical, and every signed evidence
set passed `verify-evidence`. A hosted runner/toolchain mismatch is a blocking failure requiring a
separately reviewed manifest refresh; never relax exact verification to make the gate green.

Link the three runs and their full SHA from the M1 tracking issue and release record. Preserve the
four evidence sets outside short-lived workflow storage before updating any manifest target to
`published`; then rerun `cargo xtask jsc-manifest validate` with the archive, SBOM, and provenance
digests populated atomically.

## Controlled macOS builds

[`distribution/jsc/scripts/build-macos.sh`](../distribution/jsc/scripts/build-macos.sh) is the only
supported macOS artifact entry point. It fails before compilation unless the checked-out WebKit
commit, patch digests, Xcode build, Apple Clang, macOS SDK, CMake, Python, Perl, Ruby, and Git match
the manifest exactly. Both Apple Silicon and Intel artifacts are built independently by passing an
explicit architecture to the pinned upstream `Tools/Scripts/build-jsc` driver. The Intel artifact
is cross-built on the same Apple Silicon toolchain, avoiding an unrecorded second Xcode image.

The script then:

1. builds the WebKit `JavaScriptCore.framework` with the recorded feature flags and macOS deployment
   target;
2. extracts its engine dylib, gives it the stable `@rpath/libJavaScriptCore.dylib` install name, and
   builds the Kunlun C ABI shim against that exact framework;
3. copies only the public Kunlun header, two dylibs, reviewed licenses, and generated build metadata
   into a clean staging tree;
4. generates an SPDX 2.3 JSON inventory and a deterministic ustar+zstd archive with normalized
   order, ownership, modes, timestamps, and single-threaded compression; and
5. verifies the archive layout, SPDX checksums, target architecture, Mach-O install names,
   dependencies, and the shim export allowlist.

Run it from an exact toolchain host with a detached checkout of the manifest revision:

```bash
distribution/jsc/scripts/build-macos.sh \
  --target aarch64-apple-darwin \
  --webkit-root /absolute/path/to/WebKit \
  --output /absolute/path/to/output \
  --compilation-cache-dir /absolute/path/to/xcode-cache
```

Use `x86_64-apple-darwin` for the Intel build. The output directory is build-specific and contains
large intermediate WebKit products in addition to `artifacts/` and `staging/`.

### Persistent Xcode compilation cache

The pinned WebKit uses Xcode's native LLVM content-addressable compilation cache, not Linux's
ccache wrapper. The workflow now saves/restores that CAS between successful runs, separated by
target architecture and exact manifest toolchain profile. The key also covers the manifest,
patches, macOS scripts, and workflow. Fallback restores stay within the same toolchain and target;
Xcode validates compilation inputs before reusing entries. Only the CAS is cached, not source
checkouts, complete DerivedData trees, test results, or trusted release archives.

The first source/output paths are stable across fresh hosted runners to preserve cache reuse.
[`macos-cache-settings.sh`](../distribution/jsc/scripts/macos-cache-settings.sh) explicitly selects
the cache directory, sets Xcode's `2G` cache size limit, enables cache diagnostics, and disables the
legacy ccache wrapper and remote cache plugins. Cache hit information appears in the Xcode build
log. `actions/cache` saves a new entry only after a successful job; the first run, evicted entries,
or changed compiler inputs can still require full compilation.

Omitting `--compilation-cache-dir` creates a new empty CAS under the build output **on every
invocation**, rather than inheriting Xcode's global cache. The independent rebuild deliberately
omits it, so it cannot reuse the first build's restored cache. Cache paths must contain no spaces
or shell metacharacters because the pinned upstream `build-jsc` passes settings through make.

This follows the pinned [WebKit caching guidance](https://github.com/WebKit/WebKit/blob/4b62d53ec6c16753020dbe69e59bf761ed0948e3/Tools/ccache/README.md)
and Apple's [compilation-cache build settings](https://developer.apple.com/documentation/xcode/build-settings-reference).

### CI evidence and independent rebuilds

The `Build pinned JSC for macOS` workflow uses GitHub's real, Apple Silicon
`xcode-27` hosted-runner label. This public-preview label is deliberately not treated as a toolchain
pin: GitHub updates the image in place. The workflow therefore verifies every recorded tool version
before compilation and fails closed when the hosted image no longer matches the manifest. A toolchain
refresh requires a separate reviewed manifest update; the build must never accept a newer image
implicitly. The image includes Rosetta 2 for the Intel corpus.

For both target triples the workflow checks out the pinned source inputs, builds and verifies the
artifact, and runs `cargo test --workspace` plus `kunlun-runtime doctor` with
`KUNLUN_JSC_DIST_DIR`, `KUNLUN_JSC_RECEIPT_SHA256`, and `DYLD_LIBRARY_PATH` selecting the
verified installation. `doctor` must report `pinned Kunlun JSC artifact` and `hermetic: true`; the system framework is not a release
fallback.

Release-candidate runs build each target twice from independent source checkouts and a fresh second
compiler cache by default. The
comparison report lists every differing archive member and fails publication unless both archives
are byte-identical. If a future toolchain introduces unavoidable nondeterminism, its member-level
cause must be documented and reviewed before changing this gate.

GitHub's `actions/attest` action binds the archive digest to signed SLSA v1 build provenance and
also attests the SPDX document. The workflow verifies the local bundle's signature, signer workflow,
source commit, archive subject, and exact signed SPDX predicate before upload. Each uploaded artifact
set contains the archive, SPDX SBOM, Sigstore attestation bundle at the manifest's `.intoto.jsonl`
path, and `SHA256SUMS`; release-validation runs also contain the rebuild report. After
the files are copied to durable release storage, update the target atomically to `published` with
the three reviewed digests; temporary workflow-artifact URLs alone are not a publication record.

## Controlled Linux glibc builds

[`distribution/jsc/scripts/run-linux-container.sh`](../distribution/jsc/scripts/run-linux-container.sh)
is the host entry point for Linux artifacts. It accepts only the two manifest Linux triples and
requires a native runner of the matching architecture; emulated or cross-architecture publication
builds fail before compilation. The script validates the manifest, pulls the Ubuntu image and a
CA trust-store donor by their multi-architecture OCI digests, and builds a local toolchain image from
[`distribution/jsc/linux/Dockerfile`](../distribution/jsc/linux/Dockerfile). APT resolves through
the manifest's `package_snapshot` URL without a live-archive fallback, so the base filesystem and
every package index are immutable review inputs. The donor contributes only its CA bundle, allowing
the minimal Ubuntu base to verify the signed snapshot service before the snapshot-pinned
`ca-certificates` package replaces it. APT still verifies Ubuntu's archive signatures and package
hashes; only snapshot `Valid-Until` expiry is disabled because the timestamped archive is immutable.

Both OCI digests and the package snapshot are recorded in the archive metadata. The actual WebKit
build then runs from the derived image in a new container with `--network none`, a read-only runtime
checkout, and fixed `/workspace` mount paths. This separates the audited toolchain installation
phase from the no-network compilation phase and keeps source/output paths identical for independent
rebuilds without embedding Docker's build timestamp in the artifact identity.

Inside the container,
[`distribution/jsc/scripts/build-linux.sh`](../distribution/jsc/scripts/build-linux.sh):

1. verifies the native architecture, Ubuntu point release, Clang, LLD, CMake, ccache, Ninja,
   Python, ICU, Perl, Ruby, Git, binutils, patchelf, and zstd versions against the manifest;
2. checks the exact clean WebKit revision and reviewed patches, then invokes the upstream
   `Tools/Scripts/build-jsc --jsc-only` path with the manifest feature flags and LLD;
3. normalizes the engine and shim to the stable `libJavaScriptCore.so` and `libkunlun_jsc.so`
   SONAMEs with an `$ORIGIN` runpath;
4. records each ELF machine, `DT_NEEDED` entry, required GLIBC/GLIBCXX/CXXABI version, and shim
   export in `metadata/runtime-dependencies.json`; and
5. rejects an unexpected dependency, architecture, SONAME, runpath, exported shim symbol, or symbol
   version newer than the recorded glibc/libstdc++ baseline before packaging.

Run the wrapper on a matching Linux host with Docker and a detached checkout of the pinned WebKit
revision:

```bash
distribution/jsc/scripts/run-linux-container.sh \
  --target aarch64-unknown-linux-gnu \
  --webkit-root /absolute/path/to/WebKit \
  --output /absolute/path/to/output \
  --ccache-dir /absolute/path/to/ccache
```

Use `x86_64-unknown-linux-gnu` on an x86_64 host. The
`Build pinned JSC for Linux` workflow runs both native architectures, executes the same workspace
test corpus and `kunlun-runtime doctor` against the staged libraries with `KUNLUN_JSC_DIST_DIR`
and the trusted `KUNLUN_JSC_RECEIPT_SHA256`, requires a byte-identical second build by default, and uploads the archive, SPDX inventory, signed
SLSA provenance, checksums, and member-level rebuild report. It uses the path-filtered, scheduled,
and manual trigger policy described above. Each architecture restores a bounded
ccache whose key covers the target, manifest, patches, Linux toolchain definition, and build entry
points. A changed input may reuse only compiler-validated entries from an older key. The Linux
configuration disables precompiled headers so the expensive JSC translation units are
cacheable without relaxing ccache's macro or timestamp checks. Verbose cache statistics are emitted
after every build. The independent
second build does not mount the persisted cache, so rebuild evidence is produced from freshly
compiled objects. OIDC and attestation write permissions remain isolated to the release-evidence
jobs after verified outputs cross the job boundary as workflow artifacts.

## Updating WebKit or another build input

Use one focused pull request for a revision or build-input update:

1. Start from the canonical WebKit repository and select a reviewed commit. Record the full commit
   SHA, canonical commit URL, and its commit timestamp as `SOURCE_DATE_EPOCH`; never use a branch,
   tag, or abbreviated revision as the pin.
2. Review the WebKit range from the old revision to the new one for JSC/WTF/bmalloc behavior,
   security changes, build-system changes, license changes, and shim compatibility.
3. Rebase every local patch, preserve its application order, update its purpose when necessary, and
   recompute its SHA-256. Remove a patch when its behavior is upstream rather than retaining an
   empty or duplicate entry.
4. Update exact tool versions, deployment baselines, build arguments, and feature flags whenever the
   build environment changes. A change to any of these is an artifact-input change even if the
   WebKit commit stays fixed.
5. Re-inventory licenses from the pinned source and all packaged dependencies. Recompute every
   changed input digest. A platform artifact may not become `published` until its complete generated
   license bundle is represented by its SBOM.
6. Build every affected target in the recorded toolchain, generate SPDX 2.3 JSON and SLSA v1
   provenance, compute the three SHA-256 values, and change only successfully produced targets to
   `published`.
7. Run `cargo xtask jsc-manifest validate` and the normal workspace checks. Confirm with a clean or
   monitored build environment that no Cargo build script initiates network access.

## Reviewer checklist

- [ ] The WebKit revision is a full SHA in the canonical repository and the reviewed commit range is
      described in the pull request.
- [ ] Build arguments, feature flags, exact tool versions, deployment targets, and
      `SOURCE_DATE_EPOCH` match the build environment.
- [ ] The four supported triples remain in one manifest and each toolchain matches its target OS.
- [ ] Every patch is necessary, ordered, purpose-documented, unique, and digest-verified.
- [ ] Local and upstream license inputs match the pinned source; the SBOM covers packaged
      dependencies and generated license material.
- [ ] Every target changed to `published` has matching archive, SPDX SBOM, and SLSA provenance
      SHA-256 values.
- [ ] The macOS artifact was produced on the controlled runner, passed the workspace corpus and
      `doctor`, and has a byte-identical independent-rebuild report.
- [ ] The Linux artifact was produced on its matching native runner from the pinned OCI/APT
      snapshots with build-time networking disabled; ELF dependencies and symbol baselines were
      recorded, and the workspace corpus, `doctor`, and independent rebuild passed.
- [ ] The public-header set and `abi.shim_version` match the shim compatibility decision.
- [ ] The manifest validator, workspace tests, formatting, and lints pass.
- [ ] No ordinary Cargo build gained a network fetch or an implicit native artifact download.
