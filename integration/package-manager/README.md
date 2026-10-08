# Kunlun P0 optional Core process adapter

This directory is an independent handoff; **no edits to Core were performed**.
Import `adapter.mjs` directly (Node 20+); it has no dependencies, package install,
ambient binary lookup, shell, fallback provider, or second workflow CLI.
`adapter.d.mts` supplies readonly, operation-discriminated TypeScript contracts.

The native `kunlun-pm` executable **requires no Node**. This JavaScript module is
only Core's optional Node orchestration path, not a native launcher.

## Copyable Core wiring

```js
import { createProvider, TransportError } from './integration/package-manager/adapter.mjs';

// Supply these from explicitly selected Core configuration, not PATH discovery.
const provider = createProvider('/absolute/path/to/kunlun-pm');
const controller = new AbortController();

try {
  const response = await provider.run({
    operation: 'plan',
    projectRoot: '/absolute/path/to/project',
    frozen: true,
    ignoreScripts: true,
  }, { timeoutMs: 30_000, signal: controller.signal });

  if (response.status === 'error') {
    // Native diagnostics are structured provider outcomes, not transport failures.
    for (const diagnostic of response.diagnostics) {
      console.error(diagnostic.code, diagnostic.message, diagnostic.remediation);
    }
  } else {
    // This is a read-only frozen graph plan, NEVER installation success.
    const plan = response.result;
    console.log(plan.fingerprint, plan.readiness); // readiness: "not-assessed"
  }
} catch (error) {
  if (!(error instanceof TransportError)) throw error;
  console.error(error.code); // fixed public code, no stderr/args/input excerpts
}
```

Other requests:

```js
await provider.run({ operation: 'version' });
await provider.run({ operation: 'detect', projectRoot: '/absolute/project' });
await provider.run({ operation: 'why', projectRoot: '/absolute/project', package: '@scope/name' });
```

Commands use `kunlun-pm <op> --project <root> --json` (no project for version),
with a positional package for why and optional `--frozen`/`--ignore-scripts` for
plan. Frozen validation is always enforced by P0, even if the flag is absent.
No resolution, fetching, installing, mutation, pruning, execution or lifecycle
scripts are available. Script decisions are denied; content evidence is
`not-verified`; known unbuilt package entries identify importer paths, not a
complete inventory of dependency tarball hooks.

## Boundary and limits

`parseResponse(bytes, expectedOperation, actualExitStatus)` is a pure parser and
validator. It accepts only one newline-terminated UTF-8 JSON object with the
exact v1 envelope, result fields, P0 capability and policy literals, bounded
graph/path arrays, consistent exit status and non-mixed success/error fields.
It rejects duplicate JSON keys, invalid UTF-8, extra lines and unknown fields.
Returned objects are deeply frozen.

Plan references are checked against its nodes/importers. Why contains no graph:
the standalone validator checks canonical reference syntax; when comparing a
why response with a plan, Core must additionally check membership and edges
against that same project's validated graph. Do not claim a why response alone
proves graph membership.

Stdout is bounded to the protocol's 1 MiB limit. Stderr is discarded, validated
as fatal UTF-8, and bounded to 64 KiB by default (`maxStderrBytes` allows
0–1 MiB). Child logs are never inherited or written to adapter stdout.
The timeout defaults to 30 seconds and must be an integer in 1–120,000 ms.
Timeout, cancellation or output overflow kills the owned process with SIGKILL;
the promise settles after process close and stream completion. Pre-aborted
requests do not spawn. Missing/truncated replies, termination, spawn and stream
errors are transport failures, separate from native diagnostics.

The PM creates **no child processes**. Killing the owned process relies on that
P0 assumption: this adapter does not provide a process-tree sandbox, filesystem
sandbox, OS sandboxing, or containment for an arbitrary executable. Select a
trusted native binary explicitly.

## Verification

From the runtime repository root:

```sh
node --test integration/package-manager/test/*.test.mjs
tsc -p integration/package-manager/test/tsconfig.json
KUNLUN_PM_BINARY=/absolute/path/to/kunlun-pm \
  node --test integration/package-manager/test/provider.test.mjs
```

TypeScript is only needed to run the consumer check, not to use the adapter.
The real-provider test uses `fixtures/package-manager-v1/workspace` and skips
unless `KUNLUN_PM_BINARY` is explicitly supplied; a configured but unavailable
binary or fixture fails instead of silently skipping. Unit fake processes are
test-only Node scripts executed through an absolute shebang (POSIX process
tests skip on Windows; pure validator tests remain portable).
