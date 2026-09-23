# Runtime artifact manifest v1

Status: **artifact parsing and native admission implemented**; Fetch entry invocation,
request `env`, `executionContext`, and HTTP serving are **contract proposals** for
[#50](https://github.com/kunlunengine/runtime/issues/50) and
[#51](https://github.com/kunlunengine/runtime/issues/51). Core and runtime-node producer
integration belongs to [#52](https://github.com/kunlunengine/runtime/issues/52). The
[portable fixture](../fixtures/runtime-manifest/v1/manifest.json) is a consumer contract
example, not evidence that a BuildEngine emits it yet.

## Layout and manifest

The artifact root is the directory containing `manifest.json`, normally
`.kunlun/runtime/`. The JSON file contains **data only**. The versioned
[JSON schema](../schemas/runtime-manifest-v1.schema.json) and
[TypeScript declarations](../types/runtime-manifest-v1.d.ts) describe its shape.
The fixture includes `server.mjs`, a Unicode chunk, a v3 source map, and a subpath asset.

| Field | v1 meaning |
| --- | --- |
| `schema` | Exact `kunlun.runtime-manifest/v1`. No minor-version guessing. |
| `engine.abi` | Kunlun JSC shim ABI `1`, independent of WebKit version or CPU triple. |
| `engine.runtime_profile` | Exact `kunlun-m2-web/1`; this states the minimum API surface, not Fetch implementation status. |
| `entry_contract` | Exact `kunlun.fetch-entry/v1`, versioning the exported handler shape independently of the artifact format. |
| `entry` | Canonical artifact-relative URL for the single server ESM entry. |
| `files` | Complete allowlist of executable module sources and indexed assets/maps. Each record has a kind and raw-byte SHA-256. Source maps identify their module with `for`. |
| `required_features` | Required admission features. v1 recognizes `closed-module-graph`; unknown values fail. |
| `compatibility_flags` | Optional behavior chosen by the producer. v1 recognizes `source-map-v3`; unknown values fail, even if seemingly optional. |
| `capabilities` | Required and optional `{name, resource}` declarations. They request authority and never create it. |

All fields are required, including empty arrays. Unknown JSON fields, missing fields,
unknown kinds, unsupported schema/ABI/profile/features/flags, and duplicate canonical
file identities fail admission. A producer needing a new field or incompatible meaning
must use a new schema version. A consumer may support multiple versions explicitly;
this implementation accepts exactly v1. No Node fallback, network fetch, package
resolution, extension probing, or implicit engine downgrade occurs during admission.

The manifest is not listed in `files`, because that would make its own digest
self-referential. The deployer supplies the **expected SHA-256 of the raw manifest
bytes** from a trusted release/deployment record to `AdmissionPolicy`. Each file hash
covers the exact bytes read from its named regular file, including UTF-8 bytes,
newlines, map JSON, and asset data. Hashes do not cover a decoded string, a
transformed bundle, other unindexed files, or filesystem metadata. A changed file
requires updating its digest and the manifest digest. SHA-256 detects alteration
relative to the trusted manifest digest; a self-authored manifest and its self-authored
hashes do **not** establish publisher identity or authorization. A signature or
trusted deployment record must bind the expected manifest digest to an approved
publisher and release. Native JSC's separate
[distribution manifest](./jsc-distribution.md) pins the engine binary/toolchain and
its provenance. It is neither part of this application artifact nor a substitute for
application publisher trust.

## URL, source, and snapshot rules

Manifest URLs use `./` followed by the canonical URL-encoded path relative to the
artifact root. For example, `./assets/help/%E6%AC%A2%E8%BF%8E%20%E7%BB%84%E4%BB%B6.svg`
names `assets/help/欢迎 组件.svg`. Literal `%`, `?`, and `#` in filenames are encoded
as `%25`, `%3F`, and `%23`. Raw Unicode, unencoded spaces, redundant dot segments,
query/fragment suffixes, encoded separators, absolute URLs, and symlink aliases are
not canonical manifest identities. Paths are resolved through the M2
[`ModuleResolver`](./module-loading.md); its file URL, containment, and import rules
also apply to static and dynamic module loading. A source import can use any M2
syntax, but its resulting canonical file URL must be in the indexed `module` set.
Runtime built-ins retain their separate host permission checks. Generated modules
are not artifact files and cannot be declared by this format.

Admission reads every indexed regular file through a root-scoped directory,
checks the digest, validates UTF-8 module sources and v3 maps, and enforces limits:
1 MiB manifest, 1 MiB per map / 8 MiB maps total, 8 MiB per module / 64 MiB
module total, 16 MiB per asset, 128 MiB total snapshot, and 1024 files.
Each external `sourceMappingURL` trailer must point to an indexed map bound to
that module; inline or undeclared map trailers fail. Original source names inside
maps are diagnostic metadata and are never fetched. Assets are served only from
the `AdmittedArtifact` snapshot; its module loader fetches only the same checked
module bytes. No post-admission file read can replace a checked source or asset.
Resolution still rechecks the M2 path identity; removal or symlink replacement can
fail closed, while ordinary content replacement cannot change executed bytes.
The application must be loaded and its exports checked before opening traffic;
that final startup step belongs to #51. An undeclared dynamic import fails at
resolve/fetch and never obtains unindexed source bytes.

## Capabilities and admission errors

`AdmissionPolicy` is supplied by the deployment, separately from the artifact.
It names supported capability types and exact grants. Every required declaration
must be present in those grants before admission succeeds. Optional declarations
may be absent; their corresponding `env` binding is omitted. Unknown capability
types fail, including optional ones, so a misspelled or future security feature
cannot silently disappear. Declarations are not host permissions. #50 must derive
the policy from real scoped handles and enforce every privileged operation; a
caller constructing matching strings alone does not authorize filesystem,
network, or secrets access. A JSC realm is not a hostile-code security boundary.

`AdmissionErrorKind` has stable categories: `manifest` (syntax/shape/digest
spelling), `schema`, `compatibility`, `capability`, `path`, `identity`,
`integrity`, `missing_file`, `invalid_file`, `limit`, and `source_map`.
Diagnostics include a manifest-relative location and no file contents or secret
values. Callers should branch on the typed category, not parse the display text.
The [admission tests](../crates/kunlun-runtime/tests/artifact_admission.rs) cover
tampering, missing files, schema/profile/ABI changes, denied grants, escaped
paths, Unicode, duplicate identities, maps, and snapshot replacement.

## Fetch entry contract (proposed; not yet executable)

The module at `entry` must export one default object with a callable `fetch`:

```js
export default {
  fetch(request, env, executionContext) {
    return new Response('Hello from Kunlun');
  },
};
```

The [server-entry types](../types/server-entry-v1.d.ts) describe the proposal.
`fetch` receives one standard `Request`, a per-request read-only `env` projection,
and an `executionContext` bound to that request. It may return a `Response`
synchronously or a promise/thenable that fulfills with one. A thrown exception,
rejected promise, or other return value is a typed request failure; if headers
have not been sent, #51 produces a bounded 500 response, otherwise it terminates
the response stream. It never coerces an invalid value into a response. Entry
absence, malformed default export, and failed top-level await are startup
failures and prevent traffic admission. Specific HTTP status/header/cancellation
conformance remains #51 work; this is the shared producer/consumer shape.

`env` contains only declared, explicitly granted opaque bindings. Required
bindings must exist; absent optional bindings are omitted. Its values are
request-scoped unless #50 explicitly defines an application-scoped service;
caller auth/provider/billing data cannot be shared between requests. No
deployment secret value appears in the manifest or browser artifact.

The proposed v1 `executionContext` has `signal: AbortSignal` and
`waitUntil(promise)`. The signal aborts on client disconnect, request deadline,
or shutdown. `waitUntil` tracks bounded background work accepted during the
request; it cannot extend a request or isolate indefinitely. Calling it after
the request scope closes fails. `passThroughOnException`, untracked detached
tasks, and other Cloudflare/Node extensions are unsupported. #51 must define
the exact budget, drain, and post-headers behavior in executable tests before
advertising Fetch entry support.

Core owns manifest/server bundle emission and producer examples; Runtime owns
admission, native loading, and execution. Runtime-node owns an independent
consumer of these same bytes. Core #6 and Runtime #42 are fixture consumers,
not alternate manifest formats. Producer and consumer maintainers should review
the schema, entry shape, grant names, and fixture before #52 labels emission
implemented.
