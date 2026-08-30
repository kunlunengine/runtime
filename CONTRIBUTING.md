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

The workspace requires Rust 1.85 or newer. Ordinary development builds link Apple's system
`JavaScriptCore.framework` on macOS. Controlled pipelines build pinned macOS and Linux glibc
arm64/x64 artifacts, but product-backend feature selection is not yet implemented. Ordinary
non-macOS builds continue to use an unsupported stub; the Linux backend is enabled only while the
controlled artifact corpus points `KUNLUN_JSC_DIST_DIR` at a verified local staging tree.

Run the baseline checks before opening a pull request:

```bash
cargo build --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

On a supported macOS host, also run:

```bash
cargo run -p kunlun-runtime -- doctor
```

If a platform-specific or sanitizer check cannot run locally, say so in the pull request and link
the corresponding CI result.

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
