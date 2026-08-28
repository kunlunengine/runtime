# Kunlun Runtime

The native JavaScriptCore host for [Kunlun Engine](https://github.com/kunlunengine/core).

This repository is now in the embedding and async-host stage. The checked-in bootstrap owns a
JavaScriptCore context on macOS, evaluates scripts with source URLs, converts exceptions, can mark a
context as inspectable, and runs JSC Promises on a caller-provided Tokio event loop. The native
`sleep(ms)` host function returns a real JSC Deferred Promise; the isolate settles due timers while
its async evaluation future is polled, so JavaScript continuations execute on the owning thread.

The bootstrap also exposes capability-gated `kunlun:fs` and `kunlun:http` modules through
`kunlun.import()`. Their Tokio operations return plain completion data to the isolate; JSC Promise
handles never cross the completion channel. Native ESM syntax/TLA still requires the pinned WebKit
module-loader shim. Full Fetch objects, the remote inspector, and sandboxing remain roadmap work.

## Workspace

```text
kunlun-runtime -> kunlun-jsc -> kunlun-jsc-sys -> JavaScriptCore
```

- `kunlun-jsc-sys` owns raw C declarations and native linking.
- `kunlun-jsc` owns safe, `!Send + !Sync` contexts and protected Promise resolvers.
- `kunlun-runtime` owns Tokio, isolate lifecycle, async evaluation, and the native process entry.

## Bootstrap

Requires Rust 1.85 or newer. Ordinary development builds use the macOS system JavaScriptCore
framework. The controlled M1 pipeline can now build and verify pinned macOS arm64/x64 artifacts;
Linux artifacts and product-backend feature selection remain milestone work.

```bash
cargo test
cargo run -p kunlun-runtime -- doctor
cargo run -p kunlun-runtime -- eval '21 * 2'
cargo run -p kunlun-runtime -- eval-async 'await sleep(10); return 21 * 2;'
cargo run -p kunlun-runtime -- eval-async --allow-read . \
  "const fs = await kunlun.import('kunlun:fs'); return await fs.readTextFile('README.md');"
cargo run -p kunlun-runtime -- types
```

The project deliberately reports the limitations of this bootstrap instead of pretending that an
async classic-script host is already an ESM/Fetch-compatible application runtime.

## Design documents

- [ROADMAP.md](./ROADMAP.md) — ordered milestones, gates, and cross-repository work.
- [docs/architecture.md](./docs/architecture.md) — runtime boundaries and artifact protocol.
- [docs/jsc-binding.md](./docs/jsc-binding.md) — how WebKit/JSC is built and bound to Rust.
- [docs/jsc-distribution.md](./docs/jsc-distribution.md) — pinned engine inputs, artifact metadata,
  validation, and revision review procedure.
- [docs/devtools.md](./docs/devtools.md) — Web Inspector backend, web frontend, and IDE strategy.
- [docs/builtins.md](./docs/builtins.md) — built-in module ABI, permissions, and TypeScript types.
- [docs/kunlun-cli.md](./docs/kunlun-cli.md) — the Vite+-class `kunlun` command surface and generator model.

## Non-goals

- Claiming that a JavaScript realm is a security sandbox.
- Reimplementing a package manager inside `kunlun`.
- Maintaining separate debugger engines for a TUI, native GUI, and every IDE.
- Depending on the host's arbitrary JSC version in production releases.

Kunlun Runtime is released under the [MIT License](./LICENSE).
