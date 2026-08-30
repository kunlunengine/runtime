# Contributing to Kunlun Runtime

Thank you for helping build Kunlun Runtime. This repository contains the native JavaScriptCore host
for Kunlun Engine. Contributions are welcome across Rust, C/C++, WebKit/JSC integration, build
reproducibility, security, testing, and documentation.

Please read the [Code of Conduct](./CODE_OF_CONDUCT.md) before participating. Report suspected
vulnerabilities according to the [security policy](./SECURITY.md), not in a public issue.

## Before You Start

1. Read the [roadmap](./ROADMAP.md) and the relevant design documents under [`docs/`](./docs/).
2. Search existing issues and pull requests to avoid duplicate work.
3. For a substantial change, comment on the corresponding issue before implementation. Open a
   proposal issue first if the change alters the public API, C ABI, engine distribution, safety
   invariants, runtime semantics, or supported platforms.
4. Keep pull requests focused. A small change with explicit tests and reviewable invariants is
   preferred to a broad rewrite.

Milestone 1 is the current focus. Good starting points are issues labeled `m1` and `help wanted`.

## Development Setup

The workspace requires Rust 1.85 or newer. The default `bundled-jsc` backend requires local pinned
libraries to be verified; it never downloads or falls back. Follow the [offline artifact setup](./docs/jsc-distribution.md#selecting-a-cargo-backend)
for macOS arm64/x64 and Linux glibc arm64/x64. Linux without a verified artifact is no longer a
runtime backend. Repository tooling remains available with `cargo test -p xtask`.

The commands below use the explicit macOS `system-jsc` developer backend. For a verified product
artifact, omit `--no-default-features --features system-jsc` and set the distribution environment
as documented. Do not use `--all-features`: the two engine backends are mutually exclusive.

Run the baseline checks before opening a pull request:

```bash
cargo build --workspace --no-default-features --features system-jsc
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --no-default-features --features system-jsc -- -D warnings
cargo test --workspace --no-default-features --features system-jsc
```

On a supported macOS host, also run:

```bash
cargo run -p kunlun-runtime --no-default-features --features system-jsc -- doctor
```

If a platform-specific or sanitizer check cannot run locally, say so in the pull request and link
the corresponding CI result.

The pinned macOS/Linux JSC artifact workflows are manual, not required on every PR. Run the
affected platform(s) when changing JSC bindings, native build inputs, or packaging; use
`compare_rebuild=false` while iterating and the default `true` for release-candidate evidence.
See [when to run the artifact workflows](./docs/jsc-distribution.md#when-to-run-the-artifact-workflows)
for the change-to-platform matrix, cache behavior, and dispatch commands. Link the candidate SHA,
mode, and relevant run results in the PR.

## Engineering Expectations

Changes at the JSC boundary must preserve the invariants documented in
[`docs/jsc-binding.md`](./docs/jsc-binding.md):

- context-bound values never outlive their context group;
- owned JSC values are protected and unprotected exactly once;
- JSC callbacks and handles remain on the isolate thread;
- foreign-thread messages contain no JSC pointer;
- Rust panics and C++ exceptions never cross the C ABI;
- teardown cancels host work and releases engine resources in a defined order.

For native, FFI, or `unsafe` changes:

- keep the Kunlun-owned C ABI minimal and do not expose WebKit C++ types;
- document every `unsafe` block with the invariant that makes it sound;
- add a targeted test for each new lifetime, ownership, callback, or layout assumption;
- record changes to WebKit revisions, patches, flags, hashes, licenses, or SBOM inputs;
- never add an implicit network download to a Cargo build script.

Generated bindings must be reproducible from the checked-in header and allowlist. Do not hand-edit
generated output.

## Commits and Pull Requests

- Create a branch from `main` and use a short descriptive name.
- Write imperative commit subjects with a useful prefix such as `feat:`, `fix:`, `docs:`, `ci:`, or
  `build:`.
- Link the issue the pull request addresses, preferably with `Closes #123` when appropriate.
- Explain the behavior and safety impact, the validation performed, and any known limitations.
- Update documentation when behavior, APIs, build inputs, or platform support changes.
- Avoid unrelated formatting or refactoring in the same pull request.

All changes go through review and the merge queue. When a pull request is approved and all required
checks pass, select **Merge when ready**. The aggregate required check is produced by the repository's
GitHub App; do not add an Actions aggregation job. Do not bypass the queue except during a documented
repository emergency.

By submitting a contribution, you agree that it may be distributed under this repository's
[MIT License](./LICENSE).

## Reviews and Maintainer Path

Reviewers should focus on correctness, safety, compatibility, test evidence, and maintainability.
Disagreement is resolved through technical evidence and the documented product decisions, not
authority or volume.

Consistent contributors may be invited to help with triage, then receive write access, and
eventually become maintainers. Maintainers are expected to review other contributors' work, uphold
the security and conduct policies, and steward at least one technical area—not merely merge their
own changes.
