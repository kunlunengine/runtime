# JavaScriptCore Binding Strategy

## Decision

Use a two-stage binding strategy:

1. **Bootstrap:** link Apple's system `JavaScriptCore.framework` on macOS to prove basic ownership,
   evaluation, exception handling, source URLs, `JSGlobalContextSetInspectable`, and Deferred Promise
   resolution from the host event loop without a native dependency download.
2. **Product:** build an exact pinned WebKit revision and bind it through a Kunlun-owned C ABI shim.
   Release and CI builds use that hermetic engine on macOS and Linux.

The bootstrap is split across `crates/kunlun-jsc-sys` and `crates/kunlun-jsc`. It must not grow into
the production binding by accretion.
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

- Owns explicit context groups, global contexts, protected values, strings, modules, exceptions, and callbacks.
- Uses RAII for `JSGlobalContextRelease`, `JSStringRelease`, and `JSValueProtect/Unprotect` pairs.
- Protects Deferred Promise resolver/rejector functions until the owning Tokio local task settles or
  drops them.
- Exposes a generic `(operation, JSON payload) -> DeferredPromise` host-call bridge without depending
  on Tokio, HTTP, filesystem, or application policy.
- Makes all context-bound values `!Send + !Sync`.
- Converts Rust panics inside callbacks to JS exceptions; no unwind crosses the C boundary.
- Does not expose raw handles except in narrowly scoped `unsafe` extension points.

The safe layer represents context groups and contexts as nested RAII owners. Every context retains
its group until after `JSGlobalContextRelease`, and public rooted values carry a lifetime tied to the
context that created them. Internally retained Promise resolvers use the same protection guard but
retain shared context ownership because the host scheduler must store them beyond the callback.
Every successful protection creates exactly one guard; cloning adds one protection, and each guard
removes exactly one protection on drop. A failed protection creates no release obligation.

`JscError` records a stable operation name, typed error kind, typed native status when applicable,
source URL for evaluation failures, exception text, and optional detail text without exposing a JSC
pointer. The `detail()` accessor provides the readable message for `InvalidInput`, `MissingValue`,
`HostFunction`, and `UnsupportedPlatform` errors when `exception_text()` is absent. The opaque raw
handles remain private to the platform module and the deliberately unsafe `kunlun-jsc-sys` crate.

### `kunlun_jsc` shim

[`crates/kunlun-jsc-sys/include/kunlun_jsc.h`](../crates/kunlun-jsc-sys/include/kunlun_jsc.h) is the
authoritative public ABI. ABI v1 wraps the context, string, evaluation, property, callback, Deferred
Promise, conversion, rooting, revocable callback state, and checked ArrayBuffer/TypedArray primitives.
The callback/buffer additions preserve all existing ABI v1 symbols. Keeping the
public JSC calls behind the same boundary means Rust declarations cannot drift from either the shim
or the eventual pinned engine build.

The header exposes only C-compatible opaque handles. Counts and sizes use fixed-width integers,
booleans use `uint8_t`, and every operation returns a fixed-width `kunlun_jsc_status`. Successful
context and string outputs are owned and released exactly once. Value and object outputs are
borrowed from their context unless paired with `kunlun_jsc_value_protect` and
`kunlun_jsc_value_unprotect`; no handle may be used after its context is released.

Every exported C++ entry point translates `std::bad_alloc` to `KUNLUN_JSC_STATUS_OUT_OF_MEMORY` and
all other C++ exceptions to `KUNLUN_JSC_STATUS_CPP_EXCEPTION`. The C++ callback bridge is also a
catch-all boundary. Rust callbacks catch panics before returning and report a JavaScript exception
plus `KUNLUN_JSC_STATUS_CALLBACK_ERROR`; unwinding across either ABI direction is forbidden.

Later versions of the shim will provide the smallest additional API needed for:

- module resolve/fetch/link/evaluate and `import.meta`;
- microtask checkpoints and unhandled-rejection notification;
- execution deadlines, termination, and memory telemetry;
- Inspector frontend/backend message callbacks and pause-loop events;
- additional zero-copy buffer adoption APIs, only after a separate ownership review.

It returns status codes and explicit exception/result handles. It does not leak WebKit C++ types into
the Rust ABI.

`kunlun-jsc-sys/build.rs` generates bindings only from the authoritative header with allowlists for
the `kunlun_jsc_` functions/types and `KUNLUN_JSC_` constants. The same build compiles the header as
both C and C++. The default `bundled-jsc` backend verifies a locally installed distribution before
linking it; explicit `system-jsc` compiles the bootstrap shim and links the macOS framework. It
invokes no downloader. Missing, conflicting, and unsupported backends fail before native compilation.
See [backend selection and trust](./jsc-distribution.md#selecting-a-cargo-backend).

## Safe host callbacks and buffers

`JscVm::host_function` creates a `HostFunction<'vm>` that owns a rooted JS function and an
isolate-local Rust closure. The closure accepts invocation-scoped `CallbackValue` arguments and
returns `CallbackReturn::{Undefined, Boolean, Number, String}` or an error message. Argument
conversion preserves structured JSC errors. Captures can contain `Rc` and other non-`Send` state.
Keep the handle alive for as long as JavaScript should be able to call it:

```rust
use kunlun_jsc::{CallbackReturn, JscVm, TypedArrayKind};

let vm = JscVm::new("host-example")?;
let echo = vm.host_function("echo", |args| {
    let text = args.first().ok_or("expected an argument")?
        .to_string().map_err(|error| error.to_string())?;
    Ok(CallbackReturn::String(text))
})?;
echo.set_global("echo")?;
assert_eq!(vm.evaluate("echo('hello')", "example:///host.js")?, "hello");

let buffer = vm.array_buffer(&[0; 16])?;
let view = buffer.typed_array(TypedArrayKind::Uint32, 4, 2)?;
view.set_global("words")?;
view.write(0, &42_u32.to_ne_bytes())?;
assert_eq!(vm.evaluate("words[0]", "example:///buffer.js")?, "42");
```

Callback ownership and failure rules:

- Rust owns the closure. Native GC finalizes only a C++ callback record and never reads or drops
  Rust `user_data`. `HostFunction::drop` revokes the native function before releasing its closure
  on the isolate thread. JS aliases then throw, even if they remain reachable.
- Each invocation retains an `Rc` to the callback state before entering user code. Reentrant
  conversion and dropping a callback's own handle cannot free active state. No registry borrow
  remains held while user code executes. A weak context reference avoids an automatic owner cycle;
  as with ordinary `Rc`, user-created strong cycles must be broken by the application.
- Registration failures drop untransferred Rust state. A failure to root a created function revokes
  it before dropping state. Failure to publish a global leaves the returned handle owned by Rust;
  the caller can retry or drop it. Explicit drop runs before the VM can be destroyed in safe Rust.
- Native dispatch checks the creation thread before touching Rust state. Rust also checks the
  callback's context. Public callback arguments and handles cannot escape their owner lifetime or
  become `Send`/`Sync`. Sharing a context group grants no cross-context callback authority.
- Callback and scheduler panics become JavaScript errors. The common panic boundary deliberately
  forgets the exceptional panic payload: a user-defined payload destructor may itself panic. Normal
  local state still unwinds and releases; builds using `panic=abort` remain process-aborting.

`ArrayBuffer<'vm>` copies the input bytes into an independent, aligned native allocation and roots
its JSC buffer. The external backing allocation belongs to JSC after the no-copy C API call; JSC's
contract also invokes the deallocator on JS creation failure. Validation and shim allocation failures
are handled before transfer. The finalizer contains no Rust callback or engine call, so it is safe
on collector threads and cannot race deallocation of the original Rust input. Its storage release
uses an atomic exchange and is idempotent while the state is live; JSC invokes the finalizer exactly
once. The native sanitizer harness counts every backing allocation through context-group teardown.

`TypedArray<'vm>` retains both its own root and an independent root for the backing buffer. All 11
C API element kinds are explicitly mapped, including clamped bytes and BigInt arrays. Creation
checks alignment and the element count without multiplication overflow. Reads/writes are bounded
by the view's byte range and use copies in native byte order, never typed pointer casts or Rust
slices into GC-managed storage. Clones add independent protections. Zero-length buffers and views
at the aligned end of a buffer are valid.

The API currently creates fixed, unshared buffers; it does not adopt arbitrary JS buffers,
SharedArrayBuffers, resizable buffers, or caller-owned raw allocations. A native zero-length view
checks detachment without consulting replaceable JavaScript properties. Detached buffers produce a
structured JS exception on length, view creation, and even empty reads/writes, instead of being
mistaken for empty attached buffers. JSC's public C byte-pointer API pins storage during nonempty
copy operations, so a subsequent JS `transfer()` may throw. No temporary byte pointer survives
another JSC API call. This limitation is intentional and must be revisited before promising
transferable zero-copy I/O.

The timer and built-in Promise schedulers retain their existing VM-owned registration lifecycle;
the explicit per-function handle above is the general callback API.

### Ownership verification

Run the normal workspace corpus with either selected backend. The added tests cover repeated
registration/drop, JS aliases after revocation, throwing publication setters, callback reentrancy,
self-drop, panic payloads, GC pressure, view kinds/offsets, alignment/range failures, zero length,
detachment, root clones, and teardown. Compile-fail examples enforce callback argument lifetimes
and handle thread affinity. The engine-free Miri harness includes panic cleanup invariants.

```bash
cargo test --workspace --no-default-features --features system-jsc
cargo +nightly miri test -p xtask jsc_ownership
distribution/jsc/scripts/test-native-ownership.sh
```

The last command explicitly uses macOS's system framework and selected Xcode compiler for a
bounded developer ASan+UBSan smoke test. The macOS PR matrix runs it on arm64/x64. Both controlled
platform build scripts also run an instrumented copy of the shim against the just-built pinned
engine, on all four artifact targets, before packaging. Linux's snapshot-pinned builder includes
`libclang-rt-18-dev`. Sanitizers cover the shim and harness, not the full JSC library. LeakSanitizer
is disabled because JSC has uninstrumented process-global caches; backing-storage leak counts are
asserted separately. The executable has a 120-second timeout. No sanitizer failure is skipped and
no instrumented binary is included in a release archive. Full-engine sanitizer builds remain #4.

The pinned [JSC typed-array API](https://github.com/WebKit/WebKit/blob/4b62d53ec6c16753020dbe69e59bf761ed0948e3/Source/JavaScriptCore/API/JSTypedArray.h)
and [object-finalizer contract](https://github.com/WebKit/WebKit/blob/4b62d53ec6c16753020dbe69e59bf761ed0948e3/Source/JavaScriptCore/API/JSObjectRef.h)
are the authoritative upstream inputs for these ownership decisions. Additive header changes
still require fresh artifacts: Cargo compares the verified public header with this checkout and
rejects stale distributions rather than linking an older shim silently.

## Distribution modes

| Mode | Intended use | Policy |
| --- | --- | --- |
| `bundled-jsc` | CI and released runtime | Default for products; pinned and verified |
| `system-jsc` | local binding development on macOS arm64/x64 | Explicit opt-in; host-managed revision reported as unknown, not compatibility-certified |

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

The platform-independent ownership guards are tested under Miri with fake opaque resources. These
tests cover exact-once release, protect/clone/unprotect balance, failed-protection cleanup, and the
required child-context-before-group teardown order without invoking the native engine. Run them
through the engine-free harness: `cargo +nightly miri test -p xtask jsc_ownership`.

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
