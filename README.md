# Kunlun Runtime

The native JavaScriptCore host for [Kunlun Engine](https://github.com/kunlunengine/core).

This repository is now in the embedding-spike stage. The checked-in bootstrap owns a
JavaScriptCore context on macOS, evaluates scripts with source URLs, converts exceptions, and can
mark a context as inspectable. It is not yet the application runtime described by the public
Kunlun APIs: ESM, the async job loop, Fetch, capabilities, the remote inspector, and sandboxing are
roadmap work.

## Bootstrap

Requires Rust 1.85 or newer. The current spike uses the macOS system JavaScriptCore framework; the
hermetic WebKit distribution in Milestone 1 will add Linux and make engine versions reproducible.

```bash
cargo test
cargo run -- doctor
cargo run -- eval '21 * 2'
```

The project deliberately reports the limitations of this bootstrap instead of pretending that a
classic-script evaluator is a complete runtime.

## Design documents

- [ROADMAP.md](./ROADMAP.md) — ordered milestones, gates, and cross-repository work.
- [docs/architecture.md](./docs/architecture.md) — runtime boundaries and artifact protocol.
- [docs/jsc-binding.md](./docs/jsc-binding.md) — how WebKit/JSC is built and bound to Rust.
- [docs/devtools.md](./docs/devtools.md) — Web Inspector backend, web frontend, and IDE strategy.
- [docs/kunlun-cli.md](./docs/kunlun-cli.md) — the Vite+-class `kunlun` command surface and generator model.

## Non-goals

- Claiming that a JavaScript realm is a security sandbox.
- Reimplementing a package manager inside `kunlun`.
- Maintaining separate debugger engines for a TUI, native GUI, and every IDE.
- Depending on the host's arbitrary JSC version in production releases.

Kunlun Runtime is released under the [MIT License](./LICENSE).
