# Kunlun Built-in Modules

## Stable direction and bootstrap status

Kunlun reserves the `kunlun:` URL scheme for runtime-provided modules. The first registry contains:

| Specifier | Bootstrap exports | Required grant |
| --- | --- | --- |
| `kunlun:fs` | `readTextFile(path)` | a containing read root |
| `kunlun:http` | `request(url, init?)` | the exact destination host |

The current macOS framework backend has no native JSC module-loader API. Until the pinned WebKit shim
lands, the same registry is available through:

```js
const fs = await kunlun.import('kunlun:fs')
const text = await fs.readTextFile('./README.md')
```

This is explicitly a bootstrap loader, not an ESM polyfill. It does not rewrite `import`/`export` and
does not claim cyclic-module or live-binding semantics. The native loader will resolve:

```js
import { readTextFile } from 'kunlun:fs'
const text = await readTextFile('./README.md')
```

to the same Rust module descriptor and HostCall operations.

## Permissions

No filesystem or network capability is ambient:

```bash
kunlun-runtime run-async script.js --allow-read ./data
kunlun-runtime run-async script.js --allow-net api.example.com
```

Read paths are canonicalized and must remain under an allowed root. HTTP supports `http`/`https`,
matches the exact host, does not follow redirects, requires UTF-8 response bodies, and currently caps
responses at 1 MiB. These constraints are bootstrap defaults; the final untrusted-code path uses
brokered directory/network handles and deployment-issued capability grants.

## Completion ABI

The JSC binding knows only:

```text
HostCall { operation: String, payload: JSON String }
DeferredPromise
```

The Tokio side stores `DeferredPromise` in an isolate-local pending map and sends only request IDs and
owned strings through the MPSC completion channel. A worker cannot obtain or transport a JSC pointer.

## TypeScript types

Ambient declarations live in the types-only package directory `types/` and are intended to publish as
`@kunlun-js/runtime-types`. Projects enable them with a development dependency and either automatic
type discovery or:

```json
{
  "compilerOptions": {
    "types": ["@kunlun-js/runtime-types"]
  }
}
```

The declarations support both future native imports and the bootstrap `kunlun.import()` API. A Rust
test verifies that every registered module and export has a matching declaration. The CLI can print
the exact shipped declarations with `kunlun-runtime types`.

## Next ABI additions

- `kunlun:fs`: byte reads, directory handles, metadata, writes behind separate grants, AbortSignal.
- `kunlun:http`: streaming request/response bodies and Web `Request`/`Response` integration.
- `kunlun:crypto`: Web Crypto-compatible primitives rather than a second incompatible crypto model.
- `kunlun:process`: deployment metadata only; no ambient subprocess or raw environment access.

New built-ins require a module descriptor, Rust operation implementation, capability rule, TypeScript
declaration, denial test, and completion-path test.
