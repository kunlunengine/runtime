# JavaScriptCore Distribution Manifest

[`distribution/jsc/manifest.json`](../distribution/jsc/manifest.json) is the reviewable source of
truth for the WebKit revision and every input that can change a Kunlun JavaScriptCore artifact. Its
shape is documented for editors and review tooling by
[`distribution/jsc/manifest.schema.json`](../distribution/jsc/manifest.schema.json). The repository
does not execute that schema: the Rust validator is the sole enforced source of manifest acceptance
and additionally checks cross-field and local-file integrity rules.

The manifest is metadata and policy. It does not make a target available to Cargo. In particular,
ordinary Cargo builds continue to use the existing explicit backend and perform no manifest-driven
download.

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
  pins a multi-architecture OCI image index by digest; macOS pins the Xcode build and SDK directly.
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
```

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
- [ ] The public-header set and `abi.shim_version` match the shim compatibility decision.
- [ ] The manifest validator, workspace tests, formatting, and lints pass.
- [ ] No ordinary Cargo build gained a network fetch or an implicit native artifact download.
