# Package-manager provider v1: C1-P0

## Implemented boundary, not an installer

The independent `crates/kunlun-pm` component implements **read-only P0** from
[#77](https://github.com/kunlunengine/runtime/issues/77). It is hosted in this Cargo workspace
for native development and conformance, but is not part of `kunlun-runtime`, its JavaScriptCore
host, or its production ESM resolver. Core retains the sole `kunlun` workflow entrance, provider
selection, help and presentation. No Core files are modified here.

P0 reads static files, validates an explicitly supported frozen pnpm graph, emits a deterministic
plan, and explains dependency paths. It has no network client, subprocess broker, writer,
installer, registry policy evaluator, content store or lifecycle runner. It neither locates nor
executes Node, pnpm or Corepack. A valid plan is **not** installation success, verified package
content, source authenticity, script approval, sandbox qualification or application readiness.
Native-default switching still requires the P2 gate.

## Process grammar

```text
kunlun-pm version [--json]
kunlun-pm detect [--project <root>] [--json]
kunlun-pm plan [--project <root>] [--frozen] [--ignore-scripts] [--json]
kunlun-pm why <package-name> [--project <root>] [--json]
```

Project defaults to the process working directory for direct native developer use. Core's JS
adapter requires an explicitly selected **absolute binary path** and absolute project root.
Options may precede the why package name. `--` ends option parsing. Unknown options, repeated
options, extra positionals, missing values and unavailable operations are usage failures.
At most 128 argument tokens / 128 KiB of token bytes are accepted.

- `version` does not read a project; it reports P0, lockfile `9.0`, resource limits and separate
  capabilities. Only `detect`, `plan` and `why` are true. `resolve`, `fetch`, `install`, `mutate`,
  `prune`, `exec` and `lifecycleScripts` are false.
- `detect` validates the same project/graph as `plan`, then reports the exact `packageManager`,
  lockfile authority and importer paths. It does not report unsupported projects as covered.
- `plan` always enforces frozen validation, even without `--frozen`. There is no re-resolution.
- `why` validates that same frozen graph, then returns all bounded simple paths from each
  importer's direct dependencies to the requested package. Workspace links, transitives,
  optional/dev edges and exact peer-context identities are retained. Cycles terminate. No match
  returns `paths: []`; resource exhaustion fails the entire operation, not truncated success.

## Supported static subset

This is deliberately smaller than complete pnpm interoperability.

| Input | P0 accepts |
| --- | --- |
| `package.json` | Unique JSON keys; valid package name; semver version (default `0.0.0`); exact root `pnpm@x.y.z` pin, optionally with Corepack `+sha512.<128 hex>` suffix; disjoint production/dev/optional dependency groups |
| Registry specifiers | Exact full semver, `^x.y.z`, `~x.y.z`, `*`; no partial versions, tags, unions, aliases or unsupported source protocols |
| Workspace discovery | Root importer `.` plus `pnpm-workspace.yaml#packages`: literal slash-separated safe ASCII paths, a final whole-segment `*`, and `!` exclusions; no `**` or other glob syntax |
| Workspace dependency | `workspace:*`, `workspace:^`, `workspace:~` or a supported full-semver range; matching package name/version and contained `link:` reference to a discovered importer |
| `pnpm-lock.yaml` | String `lockfileVersion: '9.0'`, importer specifier/version entries, registry packages with canonical SHA-512 SRI, snapshots with exact peer suffixes, production/optional edges, peer declarations, platform/CPU/libc conditions and supported metadata |
| Lockfile settings | `autoInstallPeers: true`, `excludeLinksFromLockfile: false`; optional `injectWorkspacePackages: false` |

Every importer and manifest must agree, all resolved edges must exist, every snapshot must have
matching package metadata and valid integrity syntax, and every package record must have a
snapshot. Peer contexts must agree with resolved peer edges, and unreachable registry records
are rejected. Metadata conditions are retained for all targets; P0 does not apply host filtering.
Integrity is only a **locked declaration**: no package bytes have been fetched or checked.
The shared graph fixture is synthetic, including syntactically valid all-zero digest placeholders;
it is not a real-pnpm install-tree or registry compatibility certificate.

Unknown lockfile/workspace fields and unsupported graph-affecting manifest fields fail closed.
Examples include catalogs, overrides, patches, injected/local/Git/tarball sources, workspace
aliases, hoisted/linker configuration and manifest peer-resolution configuration. No lossy writer
exists. Cosmetic package metadata is not executed or projected into machine results.

`pnpm-lock.yaml` is the sole graph authority. Presence of `kunlun.lock`, `kunlun.lockb`, npm,
Yarn or Bun lockfiles is an ambiguity failure, not an implicit migration. Lockfile absence,
manifest/importer drift and incompatible workspace links fail before any tree changes.
Explicit `--project` selects a root; P0 does not search ancestors for a different root.

Project/workspace `.npmrc`, `.pnpmfile.cjs`, `.pnpmfile.mjs` and executable Kunlun configuration
are rejected without reading/evaluating their contents. Ambient environment, user/global pnpm
configuration and Corepack state are never consulted. Static inputs must be contained regular
files; symlinked input files or workspace paths are rejected.

## Deny-only policy and honest readiness

Optional project/workspace `kunlun-pm-policy.json` has exactly:

```json
{
  "schema": "kunlun.package-manager-policy/v1",
  "lifecycleScripts": "deny"
}
```

Absent policy also denies. Unknown keys, grant lists, `allow` or self-declared trust are rejected.
There is **no approved runner or grant expansion path** in P0. Future execution must intersect
artifact-bound project requests with external trusted caller/CI policy; a repository cannot
authorize itself. This narrow deny-only contract does not claim to implement P1 evidence or P4
approval/sandboxing.

Plans report known importer `preinstall`/`install`/`postinstall`/`prepare` hooks, the
[root-only `pnpm:devPreinstall`](https://pnpm.io/scripts#pnpmdevpreinstall) hook and implicit
`binding.gyp` native builds as denied, with SHA-256 script digests rather than potentially
secret-bearing commands. `--ignore-scripts` cannot be weakened by policy or fallback. Without it,
the default is still denied. `unbuiltPackages` identifies only **known importer paths**;
dependency tarball hooks are not known yet. Consequently every plan reports
`readiness: "not-assessed"` and `evidence: "not-verified"`, not installed or usable.

Plan fields use camelCase. `changedManifestPaths` is always `[]`; `readOnly` and `frozen` are
always true. Importers are sorted by normalized path, nodes by exact ID, dependency edges by
name and script decisions by importer/hook. Workspace node IDs are `workspace:<importer>`;
registry IDs retain full peer contexts. Every importer has a matching workspace node, enabling
paths across workspace links.

`fingerprint` is SHA-256 over the protocol semantics identifier and length-framed, sorted
project-relative filenames/raw static bytes. It does not include machine paths, timestamps,
environment, credentials or `node_modules` state. It is a reproducibility key, **not trusted
approval** or a package-content digest. Formatting changes can change it; semantically equivalent
input files are not normalized into a new authoritative lockfile.

## Machine envelope and limits

With `--json`, stdout is exactly one newline-terminated UTF-8 JSON object, no prose:

```json
{
  "schema": "kunlun.package-manager-provider/v1",
  "provider": "kunlun-pm",
  "requiresNode": false,
  "operation": "plan",
  "exitStatus": 1,
  "status": "error",
  "diagnostics": [{
    "code": "frozen_drift",
    "message": "manifest, workspace or configuration differs from the frozen graph",
    "remediation": "regenerate and review the authoritative pnpm lockfile using the pinned package manager"
  }]
}
```

Success instead has `status: "ok"`, `exitStatus: 0`, and operation-specific `result`, never
`diagnostics`. Error has diagnostics, never result. Unknown verbs use `operation: "unknown"`.
Exit 2 means `invalid_arguments`; exit 1 means project/schema/configuration/policy/graph/limit
failure. Parser/OS errors and user input excerpts are never emitted; diagnostics have fixed,
allowlisted public messages. No absolute paths, URL credentials, script text or configuration
contents enter failure output. Versioned readonly contracts are in
[`adapter.d.mts`](../integration/package-manager/adapter.d.mts); native structures are in
[`protocol.rs`](../crates/kunlun-pm/src/protocol.rs).

Response limit is **1 MiB including newline**. Input limits are 8 MiB per file, 32 MiB total,
100,000 parsed values, 64 nesting levels, 8,192-byte strings and 512 importers. Graph limit is
10,000 nodes. Why allows 1,000 paths, depth 64 and 100,000 visits. Exhaustion is an error.
Startup failures, signals and broken stdout are transport failures, not successful replies.
Invoke the selected binary directly: Cargo build output is not provider JSON.

## Core handoff and validation

[`integration/package-manager`](../integration/package-manager/README.md) contains the dependency-free
optional Node process adapter, readonly declarations, strict TypeScript consumer test and
fake-process boundary tests. `createProvider(absoluteBinaryPath).run(request, options)` resolves
structured native success/errors and rejects separate secret-safe `TransportError`s. It validates
exact envelopes, capabilities, policy literals and graph references, bounds both output streams,
and handles timeout/AbortSignal. It is not a native launcher or OS sandbox.
Core should vendor or package these files with a recorded immutable source revision, rather than
invent a second native parser or parse human output. Existing pnpm workflows remain independent;
native plan success must not select an unavailable installer or silently fall back.

Shared fixtures are in [`fixtures/package-manager-v1`](../fixtures/package-manager-v1/README.md).

```sh
cargo build --locked -p kunlun-pm
cargo test --locked -p kunlun-pm
cargo clippy --locked -p kunlun-pm --all-targets -- -D warnings
KUNLUN_PM_BINARY="$PWD/target/debug/kunlun-pm" \
  node --test integration/package-manager/test/*.test.mjs
npx --yes -p typescript@5.9.3 tsc -p integration/package-manager/test/tsconfig.json
PATH="" ./target/debug/kunlun-pm plan \
  --project fixtures/package-manager-v1/workspace --frozen --ignore-scripts --json
```

Rust process tests clear the child's environment/PATH and compare the full prior file inventory
after success and rejection. These are engine-independent P0 checks, not the four-platform JSC
release gate, real-pnpm differential install evidence or completion of P1–P4.
