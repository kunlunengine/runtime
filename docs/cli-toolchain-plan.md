# `kunlun` CLI Package, Toolchain, Build, and Test Plan

Status: decision draft, 2026-09-03.

This document coordinates four repositories without moving their ownership boundaries:

| Plane | Owner | Current implementation |
| --- | --- | --- |
| workflow and provider selection | `kunlunengine/core` | `@kunlun-js/cli` and `BuildEngine` |
| build and transform | `zixiao-labs/Nasti` | Nasti 2.5.2, Rolldown 1.2.6, OXC 0.147 |
| test semantics and orchestration | `zixiao-labs/Lightning` | Lightning 2.1.0 |
| native execution and verified runtime artifacts | `kunlunengine/runtime` | pinned JSC plus Tokio host |

The source audit used Nasti commit `41e9618`, Lightning commit `d031aff`, and Core commit `2d1a67e`.
Versions are observations, not new compatibility ranges.

## Decisions

1. **Use a pnpm-compatible filesystem project format, not Yarn PnP.** The target engine is the native
   Rust `kunlun-pm`, not pnpm running under Node. During bootstrap, C1 can invoke the exact pnpm
   version pinned by the project as a compatibility oracle/fallback. The toolchain manager acquires
   and verifies it; Corepack is optional, not a prerequisite.
2. **Keep package resolution out of the native runtime.** Nasti resolves dependencies in development
   and emits a bundled server artifact for production. The native ESM resolver accepts artifact
   files, `kunlun:` built-ins, and registered generated modules; it rejects bare packages.
3. **Implement rustup-style selection and verification around a native launcher.** Toolchain
   discovery, download, verification, rollback, and package installation must work when `node`,
   `pnpm`, and `corepack` are absent from `PATH`. JavaScript package versions remain in
   `package.json` and the project lockfile; JS providers declare their Node requirement explicitly.
4. **Keep Nasti on Rolldown/OXC.** A native Nasti provider should reuse that compiler lineage where
   practical. An SWC implementation is a separate `BuildEngine`, because calling it Nasti while
   changing transforms, plugin hooks, and output semantics would make conformance ambiguous.
5. **Extend Lightning with a Kunlun/JSC executor.** Do not fork its collector, assertions, mocks,
   snapshots, reporters, or browser mode into a second framework.

## Package-management provider

### Target architecture

```text
native kunlun launcher
  -> PackageManagerProvider/v1
       -> kunlun-pm (Rust; default and release target)
       -> pnpm process bridge (bootstrap, migration, conformance oracle)
```

`kunlun-pm` owns dependency graph resolution, registry metadata and tarball retrieval, integrity and
signature policy, a content-addressable store, workspace linking, pruning, and lockfile updates. None
of those operations may invoke Node. The provider boundary exists to keep pnpm interoperability and
future formats possible, not to hide a permanent Node dependency.

### Why pnpm format wins the first round

Yarn Plug'n'Play makes `.pnp.cjs` and the PnP API part of runtime resolution, and cached packages may
be addressed inside zip archives. Supporting it natively would require executing Yarn's generated
loader contract or cloning its PnP API and zip filesystem behavior. That is a large Node/Yarn
compatibility surface with no benefit to a pre-bundled production artifact.

pnpm's isolated linker presents ordinary filesystem paths under `node_modules`, with dependency
links backed by `node_modules/.pnpm` and a content-addressable store. That matches Nasti's current
workspace and linked-package discovery, and it allows build tools to use ordinary filesystem APIs.
Hoisted mode can remain an explicit compatibility escape hatch; it is not the default.

### Provider contract

The top-level CLI owns stable verbs while a provider owns resolution and installation:

```text
kunlun install [--frozen]
kunlun add <spec...> [--dev]
kunlun remove <name...>
kunlun update [name...]
kunlun why <name>
kunlun exec <command...>
```

The internal `PackageManagerProvider/v1` operations are `detect`, `resolve`, `fetch`, `install`,
`mutate`, `prune`, `why`, and `exec`. Results carry a provider ID, `requiresNode` capability,
structured diagnostics, lifecycle-script decisions, changed manifest paths, and an exit status.
The native provider is selected whenever its declared lockfile/features cover the project. The
project-pinned pnpm bridge fills explicit compatibility gaps and provides differential fixtures; it
does not parse human-readable output to infer success.

Corepack remains an optional adapter for compatible hosts. It is not the default bootstrap contract:
Node stopped distributing it in v25, and Corepack 0.35 supports Node 22.22.2, 24.15.0, and 26+, not
the full Node range currently accepted by Kunlun/Nasti. `doctor` may recognize a compatible Corepack
installation, but project reproducibility cannot depend on its ambient presence.

Compatibility rules:

- `package.json#packageManager` is mandatory and exact for reproducible CLI operations.
- The resolved pnpm executable is keyed by name, exact version, integrity, and target. Installation
  is staged and verified before it enters the immutable toolchain store.
- `pnpm-lock.yaml` and `pnpm-workspace.yaml` remain pnpm-owned in the process-provider phase.
- Frozen CI rejects lockfile or manifest drift.
- Lifecycle scripts follow an explicit project allowlist; non-interactive runs never invent consent.
- Registry credentials stay in provider-native configuration and must not enter plans, logs, or the
  runtime manifest.
- The native provider must declare supported pnpm lockfile versions. It must round-trip unknown
  fields losslessly or refuse to write, and pass install-tree fixtures against real pnpm before being
  advertised as compatible.

### Node independence and lifecycle scripts

Native installation has a hard acceptance gate: with `node`, `pnpm`, and `corepack` absent from
`PATH`, `kunlun install --ignore-scripts --frozen` must resolve or validate the graph, populate the
store, and link a clean workspace on every supported host target.

Dependency lifecycle scripts are a separate capability from installation:

- `kunlun-pm` plans scripts and applies the project's explicit allowlist; it never silently falls
  back to Node.
- shell/native scripts can run through the native process broker with the declared permissions.
- JavaScript scripts may select `runtime = "kunlun"` once the required compatibility APIs exist.
- packages that explicitly require Node may select the `compat-node` script runner; `doctor` and
  `--json` output expose that dependency before installation mutates the tree.
- `--ignore-scripts` remains a complete, Node-free installation path, not a partially linked tree.

This separates “the package manager requires Node” from “one dependency's build script requires
Node.” Only the latter is an allowed compatibility condition.

### Native delivery plan

- **P0 — native shell:** add the Rust `kunlun-pm` binary, provider protocol, manifest/workspace
  discovery, read-only pnpm lockfile parser, deterministic install plan, and `why` graph.
- **P1 — fetch/store:** add registry configuration, authenticated metadata requests, integrity-
  checked tar extraction, offline cache, store GC roots, and concurrent fetches.
- **P2 — resolver/linker:** implement peer/optional/platform resolution, workspace protocols, the
  pnpm-compatible virtual store, bins, and atomic linking without Node.
- **P3 — mutation:** write supported lockfile versions, implement add/remove/update/prune, and run
  differential tree/lockfile tests against pinned pnpm releases.
- **P4 — scripts:** add permissioned native/Kunlun lifecycle runners and retain `compat-node` only as
  a feature-reported escape hatch.

The pnpm process bridge may be the default only through P1. P2 is the default-provider switch gate;
P3 is the “pnpm-compatible” write claim; P4 does not block Node-free `--ignore-scripts` installs.

## Rustup-style toolchain management

Toolchain behavior belongs behind these commands:

```text
kunlun toolchain install <channel|version> [--target <triple>]
kunlun toolchain list
kunlun toolchain use <channel|version>
kunlun toolchain update [channel]
kunlun toolchain remove <channel|version>
kunlun toolchain doctor
```

Selection order is: explicit `+toolchain` or flag, nearest project toolchain file, a user override for
the project path, then the global default. The selected release resolves to an immutable toolchain ID;
channels are movable aliases, not installation directories.

The project file should be `.kunlun/toolchain.json` with a versioned schema:

```json
{
  "$schema": "https://schemas.kunlun.dev/toolchain/v1.json",
  "schemaVersion": 1,
  "channel": "stable-2026-09",
  "profile": "default",
  "components": ["runtime", "kunlun-pm"],
  "targets": ["aarch64-apple-darwin"]
}
```

The launcher, selector, verifier, and `kunlun-pm` are native baseline components. Nasti and Lightning
stay project dependencies while they are JavaScript packages, and selecting them can require a Node
tools component. Their native binaries become optional toolchain components after their process
protocols are stable. Package installation itself must not inherit that Node requirement.

Installation uses a staging directory, signed release manifest, target-specific SHA-256, complete
file inventory, and atomic rename. The verification receipt and target/toolchain checks should reuse
the trust model already implemented for JSC distributions in `distribution/jsc/backend.rs`. Shims
must verify the selected component before execution, support rollback, and never download from an
ordinary Cargo build script.

## Build and transpile

Core already has the correct public boundary: a `BuildEngine` creates sessions and reports explicit
capabilities. Preserve that TypeScript API and add a versioned process transport:

```text
kunlun.build-provider/v1
  initialize -> capabilities
  build       -> diagnostics + artifacts + runtime manifest
  serve       -> session + URLs
  invalidate  -> changed module URLs
  cancel      -> terminal cancellation acknowledgement
  shutdown    -> clean provider exit
```

Messages are framed JSON over stdio initially. Diagnostics and progress are events; stdout contains
protocol frames only and human logs go to stderr. Every request has an ID, protocol version,
workspace root, target consumer, cancellation token, and trace ID.

Delivery path:

- **B0:** keep the in-process Nasti adapter and add golden `BuildEngine` fixtures.
- **B1:** put the existing Node Nasti implementation behind the process protocol. This validates
  crash handling, cancellation, and diagnostics without changing compiler output.
- **B2:** add `nasti-native` using Rolldown/OXC crates and only compiled-in first-party plugins.
- **B3:** add a hybrid plugin host if JavaScript plugins are required. Native-only mode must reject
  unsupported plugins clearly rather than silently skipping hooks.
- **B4:** require byte/integrity-stable server output, source maps, and
  `kunlun.runtime-manifest/v1` conformance on both Node and native providers.

SWC remains useful as an independently named provider or focused transform component. It is not the
default Native Nasti plan: current Nasti already depends on OXC transforms and Rolldown bundling, so
SWC would create a second behavior matrix instead of being a mechanical port.

## Lightning and native testing

A small JSC test runner is straightforward; Lightning compatibility is the real scope. Lightning
already owns collection, nested hooks, concurrency, retries, `.only`, mocks, snapshots, dependency-
aware watch mode, coverage reporting, sharding, and Playwright browser execution. Reimplementing
those features would create two subtly different products.

Add an executor/pool extension instead:

```text
test.pool = "kunlun"

Lightning orchestrator
  -> Nasti transforms a spec and its runner dependencies
  -> kunlun-runtime test-worker starts a fresh JSC isolate/process
  -> JSON event stream returns collection errors and TestResult records
  -> existing Lightning reporters/snapshot coordinator produce output
```

Phases:

- **T0:** `kunlun test` invokes the project-pinned Lightning Node provider.
- **T1:** define `lightning.executor/v1` and add a `kunlun` pool implemented as one runtime worker
  process per isolated test file. Keep browser tests on Playwright.
- **T2:** bundle a JSC-safe Lightning runtime containing collector, runner, assertions, and mock
  primitives. Node filesystem/process helpers remain in the orchestrator.
- **T3:** add source-mapped failures, deterministic unhandled-rejection reporting, timeout/cancel,
  snapshot requests, and watch invalidation.
- **T4:** compare the same fixture corpus across Lightning's `inline`, `threads`, `forks`, and
  `kunlun` pools. Unsupported APIs must be feature-reported, never silently changed.

T1 depends on M2 native ESM, top-level await, deterministic microtask checkpoints, dynamic import,
and rejection tracking. Full runtime conformance and streamed I/O depend on M3. V8 coverage cannot
simply be relabeled for JSC; JSC coverage needs an Inspector-backed provider with its own capability
name.

## Cross-repository first work packets

| Packet | Repository | Acceptance gate |
| --- | --- | --- |
| M2-R1 URL resolver | runtime | canonical `file:`/built-in/generated URLs; escape and denial tests |
| M2-R2 JSC module ABI | runtime | static ESM graph, cycles, TLA, `import.meta.url` through the resolver |
| C1-P0 native package shell | core + new native PM crate | Node-free plan/lockfile read/`why`; versioned provider protocol |
| C1-P1 pnpm bridge | core | pinned fallback, frozen CI, JSON diagnostics, differential oracle |
| C1-P2 native store/linker | native PM crate | clean frozen install with no Node/pnpm/Corepack on `PATH` |
| C1-T1 toolchain selector | core + runtime | native project/global precedence, verified install, rollback, offline doctor |
| B0 conformance fixtures | core + Nasti | same target/artifact diagnostics through in-process and process adapters |
| T0/T1 executor boundary | core + Lightning + runtime | `kunlun test` plus one JSC worker fixture with honest capability output |

The first runtime packet is implemented as a resolver library and unit tests. It remains open in the
roadmap until the JSC module callbacks consume it.

## Primary references

- [Yarn Plug'n'Play API](https://yarnpkg.com/advanced/pnpapi)
- [pnpm symlinked `node_modules` structure](https://pnpm.io/symlinked-node-modules-structure)
- [Corepack distribution and supported Node versions](https://github.com/nodejs/corepack#how-to-install)
- [SWC Rust usage](https://swc.rs/docs/usage-core)
- [Nasti source](https://github.com/zixiao-labs/Nasti)
- [Lightning source](https://github.com/zixiao-labs/Lightning)
- [Kunlun Core source](https://github.com/kunlunengine/core)
