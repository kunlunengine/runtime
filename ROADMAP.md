# Kunlun Runtime Roadmap

Status date: 2026-08-29

This roadmap starts from the actual repository state, not from the aspirations in the core README.
Before this revision the repository contained only a `Hello, world!` binary and no runtime
protocol, tests, JSC ownership model, or development-tool decision.

## Product decisions

1. **JavaScriptCore is the execution engine; Kunlun owns the host.** The final runtime ships a
   pinned, reproducible WebKit/JSC build. The OS framework is only a bootstrap/developer option.
2. **A JSC realm is not a security boundary.** Capabilities restrict host authority. Hostile code
   additionally requires a process, container, or microVM boundary.
3. **There is one debugger platform, not one debugger per shell.** This repository supplies the JSC
   Inspector endpoint. A separately named DevTools product will reuse it from a standalone desktop
   app, CLI/TUI, MCP and Skill integrations, a deeper Claude Code plugin, browser fallback, and DAP
   clients. The platform may have multiple Web/native protocol adapters, but one session and tooling
   model.
4. **`kunlun` matches Vite+'s coherent workflow, not its internal implementation.** It presents
   create/install/dev/check/test/build/run commands. pnpm is the initial package-management provider,
   not a permanent architectural boundary; an integrated provider remains in scope behind the same
   contract. Building delegates to Nasti or another `BuildEngine`, tests to Lightning, and native
   execution to this repository.
5. **The native artifact is a built server module plus a versioned manifest.** The current core
   application manifest contains route metadata but no executable handlers, so it is insufficient
   for a native runtime on its own.

## Workstreams and milestones

### M0 — Embedding proof (complete)

Goal: replace `Hello, world!` with an honest, testable engine slice.

- [x] Rust library and `kunlun-runtime` binary split.
- [x] macOS system-framework link for a zero-download bootstrap.
- [x] owned context with `!Send + !Sync` thread affinity.
- [x] classic-script evaluation, source URLs, UTF-8 conversion, and exception propagation.
- [x] inspectable-context primitive and `doctor` smoke test.
- [x] Virtual Cargo Workspace split into `kunlun-jsc-sys`, `kunlun-jsc`, and `kunlun-runtime`.
- [x] Tokio current-thread isolate loop, JSC Deferred Promise bridge, and Promise-returning
  `sleep(ms)` host function.
- [x] `async`/`await`, timer ordering, and asynchronous exception tests.
- [x] Plain-data Tokio completion channel; Deferred Promises remain isolate-local.
- [x] Capability-gated `kunlun:fs` read and `kunlun:http` request bootstrap modules.
- [x] Ambient `@kunlun-js/runtime-types` declarations checked against the built-in registry.
- [x] architecture, JSC binding, DevTools, and CLI decisions recorded.
- [x] CI on macOS arm64/x64 and formatting/lint policy.

Exit gate: `cargo test --workspace`, `kunlun-runtime doctor`, synchronous/asynchronous exception
paths, and a Tokio-to-JSC Promise resolution test pass on supported macOS builders. No ESM, Fetch,
or runtime-compatibility claim is allowed at this milestone.

### M1 — Reproducible JSC distribution and safe binding (current)

Goal: make the engine version controlled by Kunlun rather than by the host OS.

- Pin an exact WebKit commit and record build flags, patches, archive hashes, licenses, and SBOM.
- Add `kunlun-jsc-sys`: bindgen allowlist for a small C header plus a Kunlun-owned C ABI shim for
  capabilities absent from the public JavaScriptCore C API.
- Add `kunlun-jsc`: RAII contexts, rooted/protected values, typed errors, callbacks, typed arrays,
  and explicit thread affinity.
- Support `bundled-jsc` for release/CI and `system-jsc` only as an opt-in developer feature.
- Build macOS arm64/x64 and Linux glibc arm64/x64 artifacts from source in controlled CI.
- Add ASan/UBSan jobs for the shim and Rust Miri tests for wrapper-owned invariants where possible.

Exit gate: the same test corpus passes against the pinned engine on macOS and Linux; no build script
downloads an unaudited native archive implicitly.

### M2 — ESM, promises, and host event loop

Goal: run real bundled server entrypoints rather than classic scripts.

- [ ] URL-based native ESM resolver with file, `kunlun:`, and generated-module schemes.
- [x] Built-in module registry and bootstrap loader for `kunlun:` specifiers.
- [ ] Native module linking, cyclic graph handling, dynamic import, `import.meta.url`, and source maps.
- [x] Initial Deferred Promise bridge and native Promise/`async`/`await` continuation execution.
- [x] Caller-driven Tokio host loop and Promise-returning timer primitive.
- [ ] Promise rejection tracking and an explicit deterministic microtask checkpoint API.
- [x] Extend the host loop from timers to plain-data filesystem/HTTP completion messages.
- [ ] Add signals, cancellation/AbortSignal, streaming I/O, and graceful shutdown.
- Execution deadlines, cooperative cancellation, heap telemetry, and out-of-memory policy.
- Console, text encoding, URL, streams, and crypto primitives required by the runtime profile.

Exit gate: ESM/TLA/dynamic-import/Promise tests are deterministic, leak checks are clean, and a
cancelled request cannot keep an isolate alive indefinitely.

### M3 — Kunlun application runtime compatibility

Goal: execute an artifact produced by `@kunlun-js/core` and a `BuildEngine`.

- Define `kunlun.runtime-manifest/v1` with engine ABI, entry URL, assets, capability declarations,
  source maps, compatibility flags, and integrity hashes.
- Add the server-entry contract: `export default { fetch(request, env, executionContext) }`.
- Extend core/build adapters to emit a `consumer: 'server'` bundle and runtime manifest.
- Implement Fetch `Request`/`Response`, streaming bodies, aborts, headers, HTTP server lifecycle,
  CORS, and graceful drain behavior compatible with `@kunlun-js/runtime-api`.
- Convert declared capabilities into opaque, scoped host handles; deny undeclared operations.
- Add shared conformance fixtures that run on both `runtime-node` and native JSC.

Exit gate: the same hello-service, routing, streaming, error, CORS, and shutdown conformance suite
passes on Node and JSC without application-source changes.

### M4 — Inspector and developer tools

Goal: source-level debugging for GUI, terminal, and agent-only environments without requiring an IDE.

- Bridge JSC Inspector messages through an authenticated, loopback-only-by-default transport.
- Serve target discovery, session multiplexing, sourcemap lookup, virtual sources, and structured
  debugger events through a versioned DevTools service contract.
- Integrate pause-loop pumping so breakpoints do not deadlock the host event loop.
- Make `kunlun inspect` and `kunlun repl` discover or launch the standalone DevTools CLI/desktop
  client; keep structured logs, request traces, capability audit events, and heap/CPU capture usable
  headlessly.
- Expose agent-safe semantic operations through MCP plus a companion Skill so Codex, Claude Code,
  and other agents can debug without an installed IDE. Package the same service in a Claude Code
  plugin with Skills, agents, hooks, and MCP integration where deeper lifecycle integration helps.
- Build the desktop client as the first showcase for the prospective Kunlun Desktop framework
  (Kunlun Engine plus CEF or a platform WebView), with an optional embedded agent using the same
  public tool contract.
- Add a DAP bridge for VS Code, JetBrains, Zed, and other compatible clients without making any IDE
  the product boundary. Keep the browser-hosted WebKit Web Inspector as a bootstrap/fallback client.
- Evolve the standalone DevTools platform beyond JSC into shared Web/native sessions, including
  React, Vue, Nuxt, Kunlun state tooling, and Xcode/native-debug adapters. This is the replacement
  path for Logos' embedded `vscode-js-debug`, not a fork of it in this runtime.
- Require an explicit token and TLS/proxy policy for non-loopback inspection; default off in
  production.

Exit gate: set breakpoint, step, inspect scopes, evaluate, map bundled sources, debug an awaited
operation, and reconnect after HMR on macOS and Linux from both the standalone client and an
MCP-capable coding agent. The broader Web/native unification can continue after the JSC slice ships.

### M5 — Isolation and multi-tenant hardening

Goal: make authority and failure containment explicit.

- Isolate pools with per-tenant/request budgets and no cross-isolate JS values.
- Capability grants scoped by extension, tenant, request, resource, and operation.
- Brokered filesystem/network/secrets APIs with audit trails and revocation.
- Worker-process mode as the minimum boundary for untrusted extensions; optional container/microVM
  driver for hostile multi-tenant execution.
- Fuzz module resolution, structured clone, HTTP parsing boundaries, and capability decoding.

Exit gate: threat-model review is complete, denial tests pass, and crash/OOM/timeout containment is
verified across process boundaries.

### M6 — Stable distribution

Goal: make the native runtime installable and supportable.

- Signed artifacts, checksums, provenance attestations, SBOM, license bundle, and reproducible-build
  checks.
- `kunlun runtime install/list/use/doctor` version management.
- Stable engine ABI compatibility policy and runtime manifest negotiation.
- Performance gates for startup, request latency, memory, module load, and snapshot feasibility.
- Windows is gated on a supportable WebKit/JSC build and debugger story; it is not silently promised.

Exit gate: `kunlun` can select and verify a native runtime in local development and CI, and rollback
to a previous compatible runtime without changing the application.

## CLI workstream (owned primarily by `kunlunengine-core`)

This runs in parallel after the artifact contract in M3 starts to stabilize.

| Phase | User-visible result | Runtime dependency |
| --- | --- | --- |
| C0 | Replace fixed `new` writer with generator protocol and first-party templates | None |
| C1 | `create`, `install`, `dev`, `check`, `test`, `build`, `run`, `doctor` coherent surface | Node fallback |
| C2 | Native runtime selection, download verification, server artifacts, `inspect` | M3/M4 |
| C3 | Workspace task graph, filters, parallelism, local/remote cache | Stable command contracts |

The detailed command and template design is in [docs/kunlun-cli.md](./docs/kunlun-cli.md).

## Release labels

- **0.0.x / embedding preview:** M0–M1; engine developers only.
- **0.1 / runtime preview:** M2–M3; application conformance, no hostile-code claim.
- **0.2 / developer preview:** M4; debugging and CLI integration.
- **0.3 / isolation preview:** M5; untrusted extension experiments.
- **1.0:** M6 plus a published compatibility and security-support policy.

Version numbers are capability labels, not calendar promises. A milestone does not advance when its
exit gate is incomplete.
