# Runtime provider process contract v0.2

## One user entrance, two implementation layers

Core owns the user-facing `kunlun` workflow, command help, orchestration, presentation, and
user-level exit policy. `kunlun-runtime` is its native executor and an explicit low-level developer
tool, **not a second workflow CLI**. It does not implement `create`, `install`, `dev`, or `build`.
Core invokes the provider instead of duplicating runtime artifact admission or backend capability
logic. The DevTools product remains separate; an inspectability primitive is not a debugger endpoint.

The new `kunlun-runtime-protocol` crate contains only the engine-independent provider argument
grammar and JSON envelope. It is not Core's command/event protocol, an RPC daemon, or a launcher.
Protocol **v0.2** is independent of the Cargo package version (currently **0.1.0**).

## Operations

```text
kunlun-runtime version [--json]
kunlun-runtime doctor [--json]
kunlun-runtime check-artifact <directory> --manifest-sha256 <trusted-digest>
    [--bind-read <binding> <directory>]... [--allow-net <exact-host>]... [--json]
```

Options can precede the artifact directory. `--` permits a literal directory beginning with `-`.
Unknown options, extra positionals, repeated `--json`/digest options, missing values, or a digest
other than 64 lowercase hexadecimal characters are usage errors. Provider arguments are bounded
to 128 tokens and 128 KiB total token bytes. Deployment read roots/network hosts are explicit;
the default grants nothing. Artifact capability declarations do not grant host authority.
`--bind-read` names an artifact `fs.binding` and opens its deployment directory; names are unique,
non-empty ASCII `[A-Za-z0-9_.-]`. The legacy developer `--allow-read` flag is intentionally not
accepted here: a legacy unnamed directory grant cannot authorize an M3 named binding.

- `version` creates no isolate. It reports executable/package version, provider schema, supported
  manifest schema, entry contract, engine ABI, runtime profile, operations, backend revision/target,
  distribution mode, and separate capability flags.
- `doctor` runs the existing classic-script, inspection-toggle, Temporal (when available), and
  async timer smoke tests. `smoke_tests.temporal = false` is honest on an unqualified system engine.
  A hermetic pinned engine missing Temporal fails its smoke test.
- `check-artifact` invokes the existing fail-closed admission implementation: trusted manifest
  digest, schema/ABI/profile, file integrity, closed module graph, source maps, and required
  capability/deployment intersection. It snapshots checked bytes but **never evaluates application
  code**, initializes no isolate, and starts no server. Supply the digest from trusted deployment
  metadata; computing a digest from the same untrusted local manifest does not create trust.

Capability keys use camelCase. `artifactAdmission` is true. `applicationExecution`, `httpServe`,
and `inspectorTransport` are false: this process API cannot yet launch an admitted Fetch app, serve
HTTP, or attach a portable debugger. `nativeModules` and the other JSC primitive flags reflect the
selected backend. Bare `run-module` and an in-process dispatcher do not establish the provider
application-execution capability. Core must reject unavailable capabilities, not silently run Node.

## JSON envelope and exits

With `--json`, stdout contains exactly one newline-terminated UTF-8 JSON object, at most **16 KiB**
including newline. The provider writes no human logs to stdout in this mode. Startup/linker failure,
process termination, and broken stdout are transport failures, not successful protocol replies.
Do not parse cargo/build output as provider JSON; invoke the selected binary directly.

```json
{
  "schema": "kunlun.runtime-provider/v0.2",
  "operation": "check-artifact",
  "status": "error",
  "diagnostics": [{
    "code": "admission_rejected",
    "message": "artifact admission rejected",
    "remediation": "verify the trusted manifest digest, artifact integrity, ABI/profile and deployment grants",
    "admission_kind": "integrity"
  }]
}
```

Success uses `status: "ok"` and `result`; failure uses `status: "error"` and `diagnostics`, never
both. `operation` is one of `version`, `doctor`, `check-artifact`, including version aliases.
Callers validate the exact schema and operation before interpreting results. Unsupported capabilities
must not be treated as success just because a `version` query succeeded.

| Exit | Diagnostic code | Meaning |
| --- | --- | --- |
| 0 | none | Successful operation |
| 2 | `invalid_arguments` | Invalid grammar, missing trusted pin, or unusable deployment read root |
| 1 | `admission_rejected` | Existing admission rejected the artifact; typed `admission_kind` explains the class |
| 1 | `diagnostic_failed` | Runtime smoke test failed |
| 1 | `internal` | Response encoding/output failure |

The machine diagnostics are allowlisted, bounded messages and remediation text. They omit input
fragments, expressions, source, environment, headers, absolute paths, capability resources, and host
grant details. Human mode remains available for local smoke diagnostics. New automation must use
JSON fields/codes, not parse human prose.

Legacy `eval`, `run`, `eval-async`, `run-async`, `run-module`, and `types` remain developer commands,
not v0.2 JSON operations. Their signal, shutdown, and resource-policy behavior is unchanged.
`eval`/`run` now reject extra arguments instead of silently ignoring flags.

## Evidence and next slices

```sh
cargo test --locked -p kunlun-runtime-protocol
cargo clippy --locked -p kunlun-runtime-protocol --all-targets -- -D warnings
cargo test --locked -p kunlun-runtime --test provider_cli
```

The protocol crate tests without JSC. Process tests use `fixtures/runtime-provider-v0.2/errors.json`
to verify real exit codes, structured errors/redaction, capability honesty, clean JSON stdout,
smoke tests, and admission without application execution. The fixture manifest pin is reviewed
test metadata, not an automatic production trust policy.

Run native tests with the documented verified pinned artifact. On macOS, explicit
`--no-default-features --features system-jsc` is developer evidence only; it does not qualify M3,
Inspector, a Core-produced application, or the four-platform release.

Remaining slices: the admitted application launch/server process API, bounded lifecycle events and
control channel, producer/Core integration, and separately qualified Inspector transport. Core
owns mapping those provider operations into the sole user-facing workflow.
