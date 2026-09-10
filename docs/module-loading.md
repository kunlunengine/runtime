# Native Module Loading

## Resolver and loader ownership

The engine-independent resolver contract belongs to [#28](https://github.com/kunlunengine/runtime/issues/28).
The JSC module ABI, module records, fetch/link/evaluate callbacks, and actual static and dynamic
imports belong to [#29](https://github.com/kunlunengine/runtime/issues/29). The pinned backend now
connects those operations to JSC's native module loader. `supports_native_modules` is true for
`bundled-jsc`; `system-jsc` returns `Unsupported` for module installation.

`ModuleResolver` recognizes three module kinds:

| Kind | URL form | Resolution policy |
| --- | --- | --- |
| application file | `file:///.../server.mjs` | exact file only, canonicalized below one artifact root |
| runtime built-in | `kunlun:fs` | must exist in the built-in registry |
| generated module | `kunlun-generated:///bootstrap/runtime.mjs` | must be registered before graph resolution |

Bare package specifiers are deliberately outside this layer. Development resolution belongs to the
selected build/package provider, and production server entrypoints must be bundled before native
execution. This keeps pnpm or Yarn internals out of the runtime and makes the production module graph
an artifact property.

No extension probing, directory indexes, package manifest lookup, or network fallback is performed.

## Input and URL rules

`resolve_entry(path)` accepts a native filesystem path. Relative entry paths are relative to the
artifact root, never the process working directory. This API encodes literal filename characters
such as spaces, `%`, `?`, and `#` when producing a file URL; callers must not pre-encode native paths.

`resolve(specifier, referrer)` accepts a URL specifier and a previously resolved `ModuleUrl`:

- `./`, `../`, `/`, `?`, and `#` references join against the referrer URL. Absolute URLs select their
  own scheme. URL joining and dot-segment removal use the WHATWG URL implementation in `url`.
- Empty input, leading/trailing whitespace, raw Unicode control characters (including ASCII
  controls/DEL), and backslashes fail closed
  before parsing. The parser must not silently trim or repair these inputs.
- Every percent escape must contain two hexadecimal digits. Percent-encoded ASCII controls/DEL
  are rejected. Encoded `/` and `\` in a pathname are rejected rather than treated as separators.
  A literal percent in a filename is expressed as `%25` in the URL API.
- Malformed absolute URLs have an invalid-specifier error; ordinary unmapped package names have
  an unsupported-bare-specifier error. Unsupported valid URL schemes have a distinct error.
- Query strings and fragments participate in identity, including an explicitly empty `?` or `#`.
  Escape hex digits are normalized to uppercase, but query/fragment escapes are not decoded:
  `?x=A` and `?x=%41` remain different identities.

### File URLs on macOS and Linux

File paths are converted from URL encoding, canonicalized by the host filesystem, checked against
the canonical artifact root, and converted back into a file URL. This unifies relative paths,
dot segments, equivalent filename escapes, and in-root symlinks. The canonical target must be a
regular file. Missing paths, directories, and targets outside the root are errors. Containment uses
path components, so a sibling named `project-other` does not belong to a root named `project`.

`file://localhost/...` normalizes to an authority-free local file URL. Other file authorities and
credentials are rejected. Query and fragment are removed only for filesystem lookup and then
restored on the canonical module URL. They never select a different file on disk.

Unix absolute paths are supported on both platforms. Windows drive/UNC import spellings are not a
compatibility feature on these hosts. Unicode filenames are encoded as file URLs; the resolver does
not apply a separate Unicode normalization or case-folding algorithm. Filesystem behavior therefore
still applies. Hard links with different canonical paths are distinct module identities.

### Built-ins and generated modules

Built-ins use exact registry names. Adding query/fragment suffixes or spelling aliases does not
create additional built-ins. Knowing a built-in identity does not grant permission to its host APIs.

Generated identities require authority-free hierarchical `kunlun-generated:///` URLs, so registered
modules can resolve siblings with ordinary relative references:

```text
kunlun-generated:///bootstrap/entry.mjs
  -> ./runtime.mjs
  -> kunlun-generated:///bootstrap/runtime.mjs
```

Generated pathname escapes use uppercase hex and decode ASCII unreserved characters; dot segments
are normalized. Registering an equivalent canonical URL is idempotent. Registration reserves an
identity, not source content: resolving an arbitrary generated URL does not register it, and this
API does not replace module source. A future source registry must reject conflicting content for an
already-bound identity. The separate scheme prevents generated identities from impersonating file
or built-in modules.

## Shared import and cache contract

Both static import requests and dynamic import requests must call the same
`ModuleResolver::resolve(specifier, referrer)` entry point. There is no import-kind-specific
resolution mode. `ModuleUrl::cache_key()` returns the canonical URL string, including query and
fragment, and agrees with `ModuleUrl` equality and hashing. Raw specifiers and native filesystem
paths must not be used as alternate module-record keys.

Caches belong to one isolate/module graph and its resolver policy; a URL is not a globally reusable
authorization token or a cross-isolate module handle. A referrer from another resolver is revalidated
against the receiving resolver's root and generated registry. This prevents a foreign referrer from
expanding the receiving resolver's authority.

For `A -> B -> A`, resolving the final edge yields the original key for A. The future loader must
reserve a module record under that key before traversing dependencies, and reuse that record for
subsequent static or dynamic requests. Query/fragment variants intentionally reserve different
records. The resolver tests establish identity convergence and map/set deduplication; execution
order, live bindings, repeated fetch suppression, and Promise settlement require the native loader
tests in #29.

## Errors and authority

`ModuleResolutionError` retains the original `specifier`, optional canonical `referrer`, and a typed
`ModuleResolutionErrorKind`. Every failure from `resolve` includes both request fields, including
URL validation, unknown registry entries, invalid referrers, and filesystem-policy failures.
Construction, entry-path resolution, and generated registration have no importing module and use
`referrer: None`; entry diagnostics preserve the supplied native path. The display message includes
the request context, while callers can branch on `kind` without parsing text.

Constructing a resolver is a trusted host operation that selects an artifact root. Resolution only
inspects filesystem paths and metadata; it performs no source reads or implicit network requests.
It does not grant ambient filesystem, network, or process capabilities. Host API permissions remain
enforced by the host broker.

A successful resolution does not protect a later path-based open from concurrent filesystem
changes. `ModuleSources` preserves containment with root-scoped directory handles, rejects
non-regular files, and bounds source reads. An independently verified immutable artifact tree can
also be used.
The resolver must not be presented as a hostile-code filesystem sandbox.

## Verification and remaining integration

The resolver's table-driven tests cover Unicode/escaping, absolute and relative paths, dot segments,
query/fragment identity, registry restrictions, generated aliases, cycles, typed diagnostic context,
and denial cases. Unix fixtures include symlink and root-prefix escapes. They run in the workspace
test corpus on macOS and Linux; the existing pinned-artifact workflows run the same corpus on each
supported architecture.

The local macOS developer-backend check is:

```sh
cargo test -p kunlun-runtime --no-default-features --features system-jsc
```

For pinned macOS/Linux backends, use the verified artifact setup described in
[JSC distribution](./jsc-distribution.md) and the workspace test commands in the platform workflows.
Local developer-backend success is not evidence that pinned platform jobs have run.

The native corpus covers cycles with live bindings, TLA with timers and host I/O, dynamic-import
cache identity and rejection, generated modules, permission denial, source maps, repeated teardown,
Rust callback panics and reentry. The native sanitizer harness additionally checks C++ callback
exceptions, revocation during graph loading, wrong-thread operations and module root balance.
Explicit microtask and rejection policy is documented in [microtasks](./microtasks.md); the overall M2 gate remains open.

## Executing modules

After installing a verified pinned artifact as described in the distribution guide:

```sh
cargo run -p kunlun-runtime -- run-module dist/server.mjs
cargo run -p kunlun-runtime -- run-module dist/server.mjs --allow-read ./data
```

The CLI uses the canonical entry's parent directory as its module root. It does not grant built-in
filesystem or network access merely because a module can be loaded. Imports of `kunlun:fs` and
`kunlun:http` expose genuine module namespaces over the existing capability-gated exports.
`run` and `run-async` retain their classic-script behavior.

Embedders construct `ModuleSources::new(artifact_root)`, optionally register generated sources and
maps, then call `TokioIsolate::install_module_sources` once and await `evaluate_module(entry)`.
Native entry paths are relative to the configured root. Absolute module URLs can also be entries.
The resolver/source policy and JSC cache are fixed for the VM lifetime; replacing a loader is an
error. Equal canonical URLs share one module even across static/dynamic requests, while queries
and fragments distinguish module identities.

`JscVm::load_module` resolves an entry before touching the cache, then returns a rooted
`ModuleRecord<'vm>`. Its first phase fetches and parses the graph; after `poll` reports `Fulfilled`,
`evaluate` starts native linking and evaluation. TLA can leave that phase pending. The caller must
continue driving timers, host completions and explicit JSC microtask checkpoints. Polling reads native
Promise state without consulting replaceable JavaScript properties. A rejected phase yields a
structured `JscError`; evaluation cannot be started twice on a handle. Dropping a handle releases
its root, while JSC retains cached module records until context teardown.

Module records borrow their VM and are `!Send + !Sync`. Rust callback state is retained for each
invocation with no registry borrow held across user code. Revocation disconnects callbacks before
Rust state is dropped. C++ exceptions and Rust panics become JavaScript errors. Reentrant
evaluate/poll/release on an active native handle returns `InvalidState`.

The Tokio driver runs explicit JSC microtask checkpoints and settles host work on the owning thread.
Dropping a pending module future cancels its timers and host operations and retires the isolate;
subsequent evaluation is rejected until the embedder creates a new isolate. Dropping a handle
alone is not cancellation of JSC's graph. Synchronous JS still has no execution deadline (#32).

## Source fetching and diagnostics

File fetching opens relative to a retained `cap_std::fs::Dir`, preserving root containment across
symlink races. Nonblocking open plus a regular-file check rejects a path raced into a FIFO or
device. Sources must be UTF-8, at most 8 MiB each, with at most 1024 fetched module identities and
64 MiB of sources per VM. Generated registration has the same aggregate bounds. No fetch performs
network requests, package resolution, extension probing, or implicit permission grants.

JSC receives the canonical URL as both the module key and source origin. `import.meta.url` is that
key. Error diagnostics preserve the generated URL and engine stack, then append mapped locations
when a v3 source map is available. Throwing `stack` getters cannot replace the original error.

Supported maps are explicit `register_source_map` registrations, trailing
`//# sourceMappingURL=relative-file.map` comments, and trailing base64 JSON data URLs (with optional
`charset=utf-8`). External maps must be regular files below the artifact root. Original source names
are resolved relative to the map URL and displayed without fetching their contents, including
HTTP URLs. Regular maps and indexes containing embedded maps are supported; external index-section
URLs are not fetched. Maps are bounded to 1 MiB each and 8 MiB per VM. Missing, malformed or denied
automatic maps leave the original generated diagnostics intact; invalid explicit registrations
return an error. This metadata layer does not implement the M4 debugger transport.

## Temporal

The pinned revision's `useTemporal` option defaults to true. Its native implementation supplies
`Duration`, `Instant`, `PlainDate`, `PlainDateTime`, `PlainMonthDay`, `PlainTime`, `PlainYearMonth`,
`ZonedDateTime`, and `Now`; no JavaScript polyfill is installed. The pinned test corpus checks leap
days, nanosecond precision, calendar/time arithmetic, DST transitions and invalid-input rejection
in a fresh process without a `JSC_useTemporal` override.

`.cargo/config.toml` enables the same option before Cargo starts child processes, including for
system-JSC development. It respects an existing environment value. Standalone system-backend
binaries can be launched with `JSC_useTemporal=true`; their actual supported surface depends on
the host OS. Options freeze at the first VM, so the runtime never mutates the environment after
threads or JSC have started. `doctor` probes the actual global and runs a leap-day smoke test; a
pinned backend with Temporal disabled fails this check.

References: [WHATWG URL Standard](https://url.spec.whatwg.org/),
[`url` crate documentation](https://docs.rs/url/2.5.8/url/struct.Url.html).
