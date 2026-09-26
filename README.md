# Kunlun Runtime

The native JavaScriptCore host for [Kunlun Engine](https://github.com/kunlunengine/core).

This repository is now in the embedding and async-host stage. The checked-in bootstrap owns a
JavaScriptCore context on macOS, evaluates scripts with source URLs, converts exceptions, can mark a
context as inspectable, and runs JSC Promises on a caller-provided Tokio event loop. The native
`sleep(ms)` host function returns a real JSC Deferred Promise; the isolate settles due timers while
its async evaluation future is polled, so JavaScript continuations execute on the owning thread.

The bootstrap also exposes capability-gated `kunlun:fs` and `kunlun:http` modules through
`kunlun.import()`. Their Tokio operations return plain completion data to the isolate; JSC Promise
handles never cross the completion channel. The pinned WebKit backend additionally runs native
ESM graphs, live bindings, cycles, top-level await, and dynamic imports through the canonical URL
resolver. The M3 application Fetch profile is available; inbound Fetch entry dispatch,
the remote inspector, and sandboxing remain roadmap work.

## Workspace

```text
kunlun-runtime -> kunlun-jsc -> kunlun-jsc-sys -> JavaScriptCore
```

- `kunlun-jsc-sys` owns raw C declarations and native linking.
- `kunlun-jsc` owns safe, `!Send + !Sync` contexts, protected Promise resolvers, revocable host
  callbacks, and checked ArrayBuffer/TypedArray handles. See the [ownership API](./docs/jsc-binding.md#safe-host-callbacks-and-buffers).
- `kunlun-runtime` owns Tokio, isolate lifecycle, async evaluation, and the native process entry.

## Bootstrap

Requires Rust 1.85 or newer. The default `bundled-jsc` backend requires an explicitly verified local
artifact for macOS arm64/x64 or Linux glibc arm64/x64. Missing artifacts, conflicting features, and
unsupported targets fail at build time; Cargo never downloads an engine or falls back to the OS.
See [offline artifact setup](./docs/jsc-distribution.md#selecting-a-cargo-backend).

For fast **macOS development only**, opt into the host system framework:

```bash
cargo test --workspace --no-default-features --features system-jsc
cargo run -p kunlun-runtime --no-default-features --features system-jsc -- doctor
cargo run -p kunlun-runtime --no-default-features --features system-jsc -- eval '21 * 2'
cargo run -p kunlun-runtime --no-default-features --features system-jsc -- eval-async 'await sleep(10); return 21 * 2;'
cargo run -p kunlun-runtime --no-default-features --features system-jsc -- eval-async --allow-read . \
  "const fs = await kunlun.import('kunlun:fs'); return await fs.readTextFile('README.md');"
cargo run -p kunlun-runtime --no-default-features --features system-jsc -- types
```

With a verified pinned artifact, run a bundled ESM entrypoint using
`cargo run -p kunlun-runtime -- run-module dist/server.mjs`. The entry's directory is the module
root; `--allow-read` and `--allow-net` grant additional built-in I/O permissions. The system backend
does not implement native modules. See [module loading](./docs/module-loading.md).

The pinned engine exposes native Temporal by default. Cargo also sets `JSC_useTemporal=true` before
starting development commands, preserving explicit environment overrides. `doctor` reports actual
availability and checks leap-day arithmetic. See [Temporal](./docs/module-loading.md#temporal).

The M2 [Web runtime profile v1](./docs/runtime-profile-v1.md) provides console, UTF-8 encoding,
URL, Web Streams and crypto primitives, available globally and from `kunlun:web`.
The M3 [application Fetch profile](./docs/fetch-profile-v1.md) adds Request,
Response, Headers, and outbound fetch with independent capability grants.
The M3 [runtime manifest v1](./docs/runtime-manifest-v1.md) defines data-only server artifacts
and fail-closed admission; Fetch entry invocation remains tracked by #51.

## Design documents

- [ROADMAP.md](./ROADMAP.md) — ordered milestones, gates, and cross-repository work.
- [docs/architecture.md](./docs/architecture.md) — runtime boundaries and artifact protocol.
- [docs/jsc-binding.md](./docs/jsc-binding.md) — how WebKit/JSC is built and bound to Rust.
- [docs/jsc-distribution.md](./docs/jsc-distribution.md) — pinned engine inputs, artifact metadata,
  validation, and revision review procedure.
- [docs/devtools.md](./docs/devtools.md) — Inspector backend and the standalone DevTools platform.
- [docs/kunlun-desktop.md](./docs/kunlun-desktop.md) — accepted pinned-CEF presentation architecture
  and native sandbox/signing/update qualification gates.
- [docs/wuling-host.md](./docs/wuling-host.md) — Wuling ADE preview host contract, consumer fixture,
  scoped capabilities, independent Qingting sessions and update adapter boundary.
- [docs/wuling-mobile.md](./docs/wuling-mobile.md) — separate remote-first mobile preview RFC and
  viewport, lifecycle, accessibility and device qualification requirements.
- [docs/builtins.md](./docs/builtins.md) — built-in module ABI, permissions, and TypeScript types.
- [docs/microtasks.md](./docs/microtasks.md) — explicit Promise checkpoints and rejection transitions.
- [docs/lifecycle.md](./docs/lifecycle.md) — AbortSignal, bounded host streams, and graceful shutdown.
- [docs/resource-policy.md](./docs/resource-policy.md) — execution deadlines, isolate cancellation,
  heap telemetry, and terminal memory policy.
- [docs/m2-exit-gate.md](./docs/m2-exit-gate.md) — four-platform conformance, leak/sanitizer evidence,
  and the immutable-commit M2 release-validation procedure.
- [docs/module-loading.md](./docs/module-loading.md) — M2 URL resolver contract and native ESM boundary.
- [docs/runtime-manifest-v1.md](./docs/runtime-manifest-v1.md) — M3 artifact schema, integrity,
  admission, and proposed Fetch entry contract.
- [docs/kunlun-cli.md](./docs/kunlun-cli.md) — the Vite+-class `kunlun` command surface and generator model.
- [docs/cli-toolchain-plan.md](./docs/cli-toolchain-plan.md) — package-manager, toolchain, native-build,
  and Lightning-provider decisions.

## Non-goals

- Claiming that a JavaScript realm is a security sandbox.
- Building a different debugger product for every terminal, desktop shell, agent, and IDE.
- Depending on the host's arbitrary JSC version in production releases.

Kunlun Runtime is released under the [MIT License](./LICENSE).
