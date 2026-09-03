# `kunlun` CLI: a Vite+-class Kunlun Workflow

## Goal

Match the coherence of Vite+ while preserving Kunlun's architecture: one memorable CLI, explicit
subsystem contracts, and replaceable build engines/runtimes. "Against Vite+" means comparable daily
workflow and scaffolding quality, not copying every implementation choice or hiding capability
differences between Nasti, Vite, Webpack, and Rspack.

The cross-repository implementation decisions for package layout, rustup-style toolchain selection,
native builds, and Lightning execution are recorded in
[cli-toolchain-plan.md](./cli-toolchain-plan.md).

The CLI remains in `kunlunengine-core`. This runtime exposes a machine-readable process protocol and
a `kunlun-runtime` developer binary; it does not become the project generator.

## Command surface

```text
kunlun create [template] [directory]   scaffold a project or run a generator
kunlun install                         delegate dependency installation to pinned pnpm
kunlun dev [project]                   build targets + runtime + HMR + optional inspector
kunlun check [--fix]                   format + lint + type-check
kunlun test [--watch]                  run Lightning (or configured test provider)
kunlun build [project]                 build all selected application targets
kunlun run <task>                      run workspace tasks
kunlun start                           start a built application
kunlun inspect                         open/attach developer tools
kunlun runtime <subcommand>            install/list/use/doctor native runtimes
kunlun doctor                          validate project, engines, runtime, and toolchain
```

Aliases may retain `kunlun new`, but documentation and generator semantics standardize on `create`.

The first coherent release needs `create`, `dev`, `check`, `test`, `build`, `start`, and `doctor`.
Workspace caching and remote execution come later; command names and exit behavior should stabilize
before those internals.

## Package-management provider boundary

| Concern | Default provider | CLI role |
| --- | --- | --- |
| dependency resolution/workspaces | native `kunlun-pm`; pinned pnpm compatibility bridge during bootstrap | resolve/fetch/store/link without Node; select fallback provider and stream diagnostics |
| development/build | Nasti `BuildEngine` | select targets and orchestrate sessions |
| alternative build | Vite/Webpack/Rspack adapters | capability negotiation and clear errors |
| format/lint | Oxfmt/Oxlint | one `check` result and fix policy |
| type checking | project TypeScript | configuration and task orchestration |
| tests | Lightning | consistent watch/CI lifecycle |
| native server execution | `kunlun-runtime` | version selection, manifest handshake, lifecycle |
| reference/fallback execution | `runtime-node` | compatibility and unsupported-host fallback |

This revises the current "CLI never installs" wording. The target implementation is a native
`kunlun-pm` provider that owns resolution, registry access, integrity verification, the content
store, workspace linking, and lockfile updates without starting Node. The first usable releases may
invoke the project's pinned pnpm as a compatibility bridge while native coverage grows. Kunlun's
toolchain manager verifies that pnpm distribution; Corepack is only an optional adapter and is not
assumed to ship with Node. The bridge has explicit retirement gates and does not become the
architecture. Neither the build nor runtime layers understand package-manager internals.

## Generator protocol

The current CLI writes one fixed JavaScript project directly from `packages/cli/src/run.ts`. Replace
that with a versioned generator contract:

```ts
interface KunlunGeneratorV1 {
  metadata: {
    name: string
    version: string
    description: string
    compatibility: string
  }
  prompts(context: GeneratorContext): Promise<Prompt[]>
  plan(answers: Answers, context: GeneratorContext): Promise<FilePlan>
  apply(plan: FilePlan, context: GeneratorContext): Promise<ApplyResult>
}
```

Important properties:

- `plan` is inspectable and supports `--dry-run` and `--json`.
- Writes are atomic; non-empty destinations require an explicit merge/force policy.
- Re-running a code generator is idempotent where possible.
- Templates do not run arbitrary post-install code without showing and confirming it in interactive
  mode; CI requires an explicit allow flag.
- Every generated project records template identity/version so `kunlun migrate` can explain changes.
- Remote/community templates are integrity-pinned in non-interactive workflows.

## First-party template matrix

Start small and test every cell that is published:

| Template | Framework | Default builder | Runtime |
| --- | --- | --- | --- |
| `app:vanilla` | browser + Fetch service | Nasti | Node, then JSC |
| `app:react` | React client + Fetch service | Nasti | Node, then JSC |
| `app:vue` | Vue client + Fetch service | Nasti | Node, then JSC |
| `service` | server-only Fetch application | Nasti server target | Node, then JSC |
| `library` | TypeScript library | Nasti/tsdown integration | none |
| `workspace` | apps + packages monorepo | Nasti | selectable |

Vite/Webpack/Rspack are builder choices, not separate copied template trees. Conditional template
fragments and contract tests keep them from drifting.

Examples:

```bash
kunlun create app:react my-app
kunlun create service orders --runtime jsc --no-install
kunlun create workspace platform --builder nasti --pm pnpm
kunlun create github:org/template my-app --integrity sha256-...
```

## Configuration and discovery

Use one `kunlun.config.ts` as the project-level composition point. Builders retain native options in
their adapters. Tool-specific config remains possible when native features cannot be represented
honestly; `kunlun doctor` reports which files were loaded.

Workspace roots use `pnpm-workspace.yaml`. `kunlun run` later adds filters, parallel execution, and
cache keys over package scripts rather than inventing a second workspace graph.

## Migration from the current CLI

1. Extract the current fixed file writes into `@kunlun-js/create` with golden-file tests.
2. Implement local first-party generator discovery and `--dry-run`; keep `new` as an alias.
3. Add interactive and fully non-interactive modes with identical plan output.
4. Add provider commands (`check`, `test`, `install`) with tool availability reported by `doctor`.
5. Add native runtime artifact/version handshake when runtime M3 is ready.
6. Add remote templates only after integrity, cache, and script-execution policies are enforced.

## Vite+ baseline used for comparison

Current Vite+ documentation presents `vp create`, `install`, `dev`, `check`, `test`, and `build` as a
single workflow; its create command supports official project kinds, package-backed templates, and
remote GitHub/URL templates, while `vp run` covers recursive/parallel/filtered workspace tasks.

- <https://viteplus.dev/guide/>
- <https://viteplus.dev/guide/create>
- <https://viteplus.dev/guide/check>
- <https://viteplus.dev/guide/monorepo>

Those are product benchmarks. Kunlun's acceptance tests must be based on its own documented command,
generator, builder-capability, and runtime contracts.
