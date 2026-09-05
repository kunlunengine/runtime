# Native Module Loading

## Resolver and loader ownership

The engine-independent resolver contract belongs to [#28](https://github.com/kunlunengine/runtime/issues/28).
The JSC module ABI, module records, fetch/link/evaluate callbacks, and actual static and dynamic
imports belong to [#29](https://github.com/kunlunengine/runtime/issues/29). Completing the resolver
does not enable native ESM execution. `supports_native_modules` remains false until that integration
is implemented and validated against the pinned engine.

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
changes. The #29 fetch layer must preserve containment when opening the source (for example through
root-scoped handles and verification), or use an independently verified immutable artifact tree.
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

#29 must add the pinned-JSC shim callbacks for resolve, fetch, link, evaluate, dynamic import, and
`import.meta`, then consume this resolver and key contract. Cycles with live bindings, top-level
await, rejection tracking, and source-map integration remain M2 exit requirements.

References: [WHATWG URL Standard](https://url.spec.whatwg.org/),
[`url` crate documentation](https://docs.rs/url/2.5.8/url/struct.Url.html).
