# JavaScriptCore Binding Strategy

## Decision

Use a two-stage binding strategy:

1. **Bootstrap:** link Apple's system `JavaScriptCore.framework` on macOS to prove basic ownership,
   evaluation, exception handling, source URLs, and `JSGlobalContextSetInspectable` without a native
   dependency download.
2. **Product:** build an exact pinned WebKit revision and bind it through a Kunlun-owned C ABI shim.
   Release and CI builds use that hermetic engine on macOS and Linux.

The bootstrap exists in `src/jsc/macos.rs`. It must not grow into the production binding by accretion.
The public JSC C API does not expose everything a server runtime needs, including a complete ESM
loader, deterministic job-loop integration, robust termination controls, and a portable Inspector
transport.

## Why not simply depend on a Rust JSC crate?

Current Rust crates are useful prior art. In particular, `rust_jsc` exposes module-loader and
Inspector functions and publishes prebuilt static archives. Directly adopting a crate-controlled
download in a build script would, however, move engine versioning, native archive provenance, build
flags, patch policy, and availability outside Kunlun's release process.

Kunlun can reuse or contribute upstream wrapper ideas, but production artifacts require:

- an exact WebKit commit and reviewed patch set;
- reproducible source builds for every supported target;
- recorded checksums, provenance, SBOM, and license materials;
- no implicit network access from ordinary Cargo builds;
- a stable Kunlun C ABI that hides WebKit C++ implementation details.

This is an engineering and supply-chain boundary, not a claim that existing crates are unusable.

## Crate and ABI layout

### `kunlun-jsc-sys`

- Generated only from `include/kunlun_jsc.h` with a strict bindgen allowlist.
- Contains raw pointers and functions; no public safe abstractions.
- Links either the pinned distribution or the explicit `system-jsc` development backend.
- Runs ABI/layout assertions against the exact headers used for the native build.

### `kunlun-jsc`

- Owns context groups, global contexts, protected values, strings, modules, exceptions, and callbacks.
- Uses RAII for `JSGlobalContextRelease`, `JSStringRelease`, and `JSValueProtect/Unprotect` pairs.
- Makes all context-bound values `!Send + !Sync`.
- Converts Rust panics inside callbacks to JS exceptions; no unwind crosses the C boundary.
- Does not expose raw handles except in narrowly scoped `unsafe` extension points.

### `kunlun_jsc` shim

The C/C++ shim is versioned with the pinned WebKit source and provides the smallest API needed for:

- module resolve/fetch/link/evaluate and `import.meta`;
- microtask checkpoints and unhandled-rejection notification;
- execution deadlines, termination, and memory telemetry;
- Inspector frontend/backend message callbacks and pause-loop events;
- host-function creation and external ArrayBuffer lifetime hooks.

It returns status codes and explicit exception/result handles. It does not leak WebKit C++ types into
the Rust ABI.

## Distribution modes

| Mode | Intended use | Policy |
| --- | --- | --- |
| `bundled-jsc` | CI and released runtime | Default for products; pinned and verified |
| `system-jsc` | local binding development | Explicit opt-in; exact version reported by `doctor` |
| macOS framework bootstrap | M0 smoke test | Temporary; not compatibility-certified |

Prefer a dynamically linked, co-distributed engine where licensing and platform packaging require
it. Static/dynamic decisions and required relinking materials must receive a license review before
binary distribution.

## Safety invariants

1. A value never outlives its context group.
2. A stored JS value is protected/rooted exactly once for each owned Rust handle.
3. JSC callbacks run only on the isolate thread.
4. Foreign-thread completions contain no JSC pointer.
5. Rust panics and C++ exceptions do not cross the ABI.
6. Context teardown disconnects Inspector sessions and cancels host operations first.
7. Execution termination is followed by a documented recovery or isolate disposal path; the host
   does not assume a terminated VM is reusable.

Each invariant needs a targeted test, not only a code comment.

## Validation ladder

- C ABI compile/link smoke tests against every supported artifact.
- Rust unit tests for evaluation, exceptions, rooting, callbacks, typed arrays, and teardown.
- Test262 subsets for language/module behavior relevant to the runtime profile.
- Differential host tests against `runtime-node` for Fetch/runtime contracts.
- ASan/UBSan and stress tests for callback, GC, interrupt, and teardown races.
- Inspector protocol fixtures and sourcemap debugging tests.

## Source references

- WebKit documents inspectable C API contexts and `JSGlobalContextSetInspectable`:
  <https://webkit.org/blog/13936/enabling-the-inspection-of-web-content-in-apps/>
- WebKit's JavaScriptCore GC overview describes its non-compacting, generational, mostly concurrent
  collector: <https://webkit.org/blog/12967/understanding-gc-in-jsc-from-scratch/>
- WebKitGTK publishes its supported GLib-facing JavaScriptCore API separately:
  <https://webkitgtk.org/reference/jsc-glib/unstable/>

These references inform the design; the checked-in pinned headers and ABI tests are authoritative for
a Kunlun release.
