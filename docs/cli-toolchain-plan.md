# `kunlun` CLI Package, Toolchain, Build, and Test Plan

Status: decision draft, 2026-09-05. Package-management policy and delivery gates are expanded below.

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

The package-management sections below are delivery requirements, not implemented capabilities. This
runtime repository currently has no `kunlun-pm` crate, installer, lifecycle runner, or package policy
engine. The CLI and provider implementation belong to Core and a native PM component; the runtime
supplies execution capabilities only through its declared contracts.

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
kunlun install [--frozen] [--ignore-scripts]
kunlun add <spec...> [--dev]
kunlun remove <name...>
kunlun update [name...]
kunlun why <name>
kunlun exec <command...>
```

`kunlun install --ignore-scripts` still resolves dependencies, populates the store, and completes
workspace linking. Packages whose build hooks were skipped are reported as `unbuilt`; a complete
installation does not claim that their native binaries or generated assets are ready to use.

The internal `PackageManagerProvider/v1` operations are `detect`, `resolve`, `fetch`, `install`,
`mutate`, `prune`, `why`, and `exec`. Results carry a provider ID, `requiresNode` capability,
structured diagnostics, policy/evidence decisions, lifecycle-script decisions, unbuilt packages,
changed manifest paths, and an exit status. Planning has two checkpoints: a static graph/configuration
check before fetching, then a content-verified script/build plan before mutating the installed tree.
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
- Frozen CI rejects lockfile, manifest, or graph-affecting configuration drift and still evaluates
  the active security policy.
- Installation defaults to denying automatic script execution. An explicit allowlist is constrained
  by the caller's trusted policy; non-interactive runs never invent consent.
- Registry credentials stay in provider-native configuration and must not enter plans, logs, or the
  runtime manifest.
- The native provider must declare supported pnpm lockfile versions. It must round-trip unknown
  fields losslessly or refuse to write, and pass install-tree fixtures against real pnpm before being
  advertised as compatible.

### Lockfile authority and format

Start with the documented pnpm lockfile versions and keep `pnpm-lock.yaml` authoritative. A native
installer does not require its own lockfile format. If Kunlun later needs a separate schema, the
canonical file will be reviewable UTF-8 text named `kunlun.lock`, with deterministic serialization.
Do not introduce a committed binary `kunlun.lockb`. Binary parsing/index caches may be added after
profiling demonstrates a benefit; they are disposable, versioned local cache entries keyed by the
canonical lockfile bytes, parser version, and relevant configuration. Cache deletion or corruption
must never change resolution, suppress verification, or require a new network resolution.

Each project has one authoritative dependency graph. A future migration is an explicit command that
shows the graph/source/policy changes, validates semantic equivalence, and replaces authority
atomically. If both lockfiles are present without a recorded migration state, fail with an ambiguity
diagnostic. Imports and exports are snapshots, not two files updated independently; lossy export must
be refused. Introducing `kunlun.lock` is a later decision and is not a P0-P4 delivery requirement.

The internal graph and any future native schema must represent:

- manifest specifiers and workspace importers; exact resolved versions and source identities;
- registry scope/origin and tarball location, content integrity, and immutable Git revision or local
  workspace identity where that source kind is supported;
- all transitive edges, peer-resolution contexts, optional dependencies, platform/CPU/libc
  conditions, workspace links, and graph-affecting overrides or patches with their content digests;
- schema and resolution-semantics versions plus a fingerprint of graph-affecting configuration.

Platform filtering must not discard the other supported targets from the recorded graph. Freeze
validates manifests, configuration, graph completeness, source bindings, and integrity without
re-resolving ranges or rewriting the lockfile. A stale, missing, unsupported, or ambiguous lockfile
fails before installation changes the tree. Stable ordering and normalized paths produce identical
text for equivalent inputs; credentials, absolute machine paths, and installation timestamps never
enter the lockfile. New formats need a versioned schema, documented compatibility rules, and an
explicit migration path. Unknown semantics fail closed; a compatible writer preserves unknown
non-semantic fields losslessly or refuses to write.

### Node independence and lifecycle scripts

Native installation has a hard acceptance gate: with `node`, `pnpm`, and `corepack` absent from
`PATH`, `kunlun install --ignore-scripts --frozen` must resolve or validate the graph, populate the
store, and link a clean workspace on every supported host target.

Dependency lifecycle scripts are a separate capability from installation. Automatic root/workspace
hooks, dependency `preinstall`/`install`/`postinstall`, Git `prepare`, and implicit native rebuilds
default to denied. `--ignore-scripts` skips all such hooks and cannot be overridden by a package's
manifest, project allowlist, or provider fallback. An explicit user-requested `kunlun run` or `exec`
is a separate execution request; it must not implicitly approve future installation hooks.

Script approval binds the package name, exact version, source/content integrity, script digest,
runner identity/version, and requested capabilities. A change to any of those requires a new policy
decision. Plans show the exact command, package, reason, runner, permissions, and blocked/unbuilt
state. Required generated artifacts remain an actionable build/readiness failure until an approved
build succeeds; installation output must not silently mark them usable.

The effective policy is the intersection of project requests and trusted caller/organization
grants. A repository file cannot authorize its own untrusted pull request to execute code in CI.
Protected CI policy is supplied outside the proposed checkout or from a separately trusted revision;
expanding grants requires that authority. `--frozen` does not imply approval, and non-interactive
runs never prompt, expand grants, or change providers to bypass a denial. A project's unreviewed
`kunlun.config.ts` is also executable input: install discovers its settings from static data and must
not evaluate arbitrary project configuration to learn whether execution is allowed.

The pnpm bridge must meet the same policy contract. Lifecycle suppression alone is insufficient:
project/global `.pnpmfile.cjs` or `.pnpmfile.mjs`, hooks, plugins, executable configuration, Git
preparation, implicit rebuilds, and inherited package-manager configuration must not become alternate
execution paths.
The pinned bridge uses explicit configuration and disables executable extension points in safe
install mode. Unsupported settings or a pnpm version that cannot enforce the policy are rejected
before invocation. No fallback can weaken source, script, network, or trust restrictions.

Runner choices remain explicit:

- Shell/native hooks run through the native process broker with declared capabilities.
- JavaScript hooks may select `runtime = "kunlun"` only after the required APIs are supported.
- Packages that require Node may select `compat-node` with a verified, pinned Node toolchain.
  `doctor` and `--json` expose the requirement before tree mutation; there is no ambient or silent
  fallback to Node. The compatibility runner obeys the same execution policy.

Installation and graph operations remain Node-free regardless of the runner selected for a build.

### Script isolation and build outputs

An allowed hook is still untrusted code. Run it in a separate process with OS-enforced filesystem,
network, and child-process restrictions, or a suitable container/VM. A process boundary alone, a
JSC realm, a JavaScript permission object, or changing the working directory is not a sandbox.
Each supported host must report and test the restrictions it can enforce. If a required restriction
is unavailable, deny execution rather than silently label an unrestricted runner safe.

The baseline runner has no network, registry credentials, signing keys, SSH agent, or inherited
secret-bearing environment. It receives an allowlisted environment, temporary home/cache locations,
read-only toolchains and dependencies, and a writable package build directory. Additional network
destinations or outputs require explicit grants. Restrict process creation, execution time, disk,
memory, and captured output, and terminate the child process tree on cancellation or timeout.

Verified source packages in the shared content store are immutable to scripts. Writable build trees
must use copies or a mechanism that cannot modify the underlying store; writable hardlinks into the
store are insufficient. Validate output paths, file types, and symlink targets before atomic
publication. Keep build outputs separate from fetched source content. Their cache key includes
source integrity, the resolved build-dependency graph and its content digests, script digest,
runner/toolchain version, platform/ABI, declared environment/inputs, and effective permissions.
Untrusted jobs cannot publish entries consumed as trusted build
results; outputs need the same trust-domain separation and integrity checks as other cache inputs.

### Registry, artifact, and supply-chain policy

Evaluate policy before fetching and again after content verification, before linking or execution.
Frozen and offline installations follow the same checks; a lockfile pins a choice rather than
exempting it from revocation or malicious-package policy.

- Bind package scopes and locked artifacts to their configured registry/source. Do not fall back to
  a public registry when a private package lookup fails. Reject an unexpected origin, mutable Git
  reference, or unapproved source change; unsupported source protocols fail before running helper
  programs.
- Require HTTPS for registry traffic, except explicitly approved local development endpoints.
  Scope authentication to the configured origin/path, re-evaluate redirects, and never forward
  credentials to a different origin. Redact credentials in URLs, plans, errors, and logs.
- Verify locked content integrity before extraction or store admission. Checksums establish byte
  identity, not publisher authenticity or absence of malicious code. Verify registry signatures
  and provenance against trusted roots and expected identities where policy requires them; report
  unsupported, absent, invalid, or stale required evidence distinctly rather than converting it to
  success. Evaluate signing certificates at the authenticated signing time where the signature
  scheme requires it; a short-lived certificate expiring later does not invalidate historical proof.
- Extract into staging with path containment and file-type checks. Reject traversal, absolute or
  duplicate paths, escaping links, devices, and dangerous permissions; bound compressed and expanded
  sizes, file counts, and resource use. Concurrent downloads, interruption, or validation failure
  must not expose partial packages. Store admission and linking use atomic publication, and cache
  reuse validates content instead of trusting directory names or a writable receipt alone.
- Apply known-malicious-package blocking and configurable vulnerability policy to both newly
  resolved and locked packages. A release-age cooling period constrains selection of new versions,
  including transitives; the baseline is 1,440 minutes with no silent fallback to a younger release.
  Record the registry time evidence used; missing required publish-time evidence fails closed.
  Any emergency override is narrow,
  expires, and is recorded by trusted policy rather than supplied by the dependency being installed.
- Detect trust downgrades across updates: registry/source changes, loss or change of required
  signature/provenance identity, weaker integrity, new scripts, or expanded permissions. Require an
  explicit policy decision, not an automatic retry with weaker verification. Evidence establishes
  claims about origin/build inputs; it does not certify that the package is harmless.
- Offline operation needs the locked content and the verification/policy evidence required by the
  effective policy. Record evidence digests, origin, verification time, freshness/expiry, and policy
  version without secrets. Missing or stale required evidence fails with an actionable diagnostic;
  any approved offline grace window must be explicit. Offline output reports its last-known
  evidence and does not claim a fresh registry, revocation, or vulnerability check.

Static policy checks cover manifest/source declarations, lockfile/configuration drift, executable
configuration, and requested grants before any project code runs. Verified-content checks cover
actual scripts, archive contents, and evidence. Installation records explain allowed/blocked
packages, overrides, script decisions, evidence freshness, and unbuilt results in human and JSON
output. This policy surface must have a versioned static format; executable policy files cannot
self-authorize their evaluation.

### Native delivery plan

- **P0 — native shell and policy contract:** add the Rust `kunlun-pm` binary, provider protocol,
  static manifest/workspace/policy discovery, read-only pnpm lockfile parser, deterministic plan,
  and `why` graph. Acceptance: no Node or project-code execution; stable plans; unsupported schema,
  drift, ambiguous authority, untrusted grant expansion, and executable-config bypass fixtures fail
  before mutation. Freeze and default script denial are defined here, not deferred to P4.
- **P1 — fetch/store and safe bridge:** add registry/source binding, credential handling, integrity
  and evidence checks, bounded extraction, immutable content store, offline cache, store GC roots,
  and concurrent fetches. Acceptance: tampering, traversal/links, credential-leaking redirects,
  private/public source confusion, concurrent/interrupted installs, and missing/stale evidence are
  rejected without partial admission. Cooling-period, malicious-package, and trust-downgrade policy
  fixtures apply to frozen installs too. The pinned pnpm bridge passes the same no-execution and
  policy-bypass tests before it can serve as fallback or differential oracle.
- **P2 — resolver/linker:** implement peer/optional/platform resolution, workspace protocols, the
  pnpm-compatible virtual store, bins, and atomic linking. Acceptance: a clean frozen install and
  cache-backed offline repeat succeed on each supported host without Node/pnpm/Corepack on `PATH`;
  graph/tree fixtures match supported pnpm semantics, policy failures preserve the prior tree, and
  skipped hooks leave accurate unbuilt diagnostics. This is the native default-provider switch.
- **P3 — mutation:** write declared pnpm lockfile versions and implement add/remove/update/prune.
  Acceptance: byte-stable no-op writes, lossless supported round trips, safe unknown-field refusal,
  cross-platform/peer/patch graph coverage, and differential tree/lockfile tests against pinned pnpm
  releases. Updates report source/trust/script changes for policy review. A separate native text
  format, if later approved, needs migration and single-authority tests before release.
- **P4 — scripts:** add OS-constrained native/Kunlun lifecycle runners and explicit `compat-node`.
  Acceptance: artifact-bound grants; denied network/secrets/store writes and escaped output paths;
  resource limits and child-process cancellation; isolated build-cache trust domains; unchanged
  Node-free `--ignore-scripts` behavior. Unsupported sandbox capabilities fail closed. Only report
  a package built after its permitted hooks and output validation succeed.

The pnpm process bridge may be the default only through P1. P2 is the default-provider switch gate;
P3 is the “pnpm-compatible” write claim; P4 does not block Node-free `--ignore-scripts` installs.

Initial non-goals are Yarn PnP, arbitrary package-manager plugins or executable hooks during
resolution, automatic approval by package popularity, a binary authoritative lockfile, and complete
Node API compatibility. Security controls reduce exposure; neither a frozen lockfile, skipped
installation hooks, integrity/signatures, nor a successful advisory scan guarantees benign package
code when the application later imports or executes it.

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

M2-R1 (#28) owns the engine-independent resolver, contextual errors, URL/cache identity contract,
and adversarial resolver fixtures. M2-R2 (#29) owns the JSC callbacks that consume this contract and
prove actual static/dynamic import behavior. Resolver completion does not claim native ESM support;
the native loader and overall M2 exit gate remain open until their integration tests pass.

## Primary references

- [Yarn Plug'n'Play API](https://yarnpkg.com/advanced/pnpapi)
- [pnpm symlinked `node_modules` structure](https://pnpm.io/symlinked-node-modules-structure)
- [pnpm build and lifecycle settings](https://pnpm.io/settings/build)
- [pnpm dependency-resolution and trust settings](https://pnpm.io/settings/dependency-resolution)
- [pnpm supply-chain security](https://pnpm.io/supply-chain-security)
- [Bun text lockfile format](https://bun.com/docs/pm/lockfile)
- [Bun text lockfile design rationale](https://bun.com/blog/bun-lock-text-lockfile)
- [npm provenance statements and their limits](https://docs.npmjs.com/generating-provenance-statements/)
- [Corepack distribution and supported Node versions](https://github.com/nodejs/corepack#how-to-install)
- [SWC Rust usage](https://swc.rs/docs/usage-core)
- [Nasti source](https://github.com/zixiao-labs/Nasti)
- [Lightning source](https://github.com/zixiao-labs/Lightning)
- [Kunlun Core source](https://github.com/kunlunengine/core)
