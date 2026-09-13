# Kunlun Built-in Modules

## Stable direction and bootstrap status

Kunlun reserves the `kunlun:` URL scheme for runtime-provided modules. The first registry contains:

| Specifier | Bootstrap exports | Required grant |
| --- | --- | --- |
| `kunlun:fs` | `readTextFile`, `openReadStream` | a containing read root |
| `kunlun:http` | `request`, `requestStream` | the exact destination host |

The registry is available through the bootstrap loader on the macOS system backend:

```js
const fs = await kunlun.import('kunlun:fs')
const text = await fs.readTextFile('./README.md')
```

This is explicitly a bootstrap loader, not an ESM polyfill. The pinned backend's native loader
resolves:

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

Read grants pre-open capability directory handles, and each file is opened relative to that handle;
`..` and symbolic-link traversal cannot escape the allowed root, including if paths change during a
read. HTTP supports `http`/`https`, matches the exact host, and does not follow redirects.
Non-streaming responses require UTF-8 and are capped at 1 MiB. `openReadStream` and `requestStream`
expose bounded `Uint8Array` chunks and accept AbortSignal, as documented in
[cancellation, streaming, and shutdown](./lifecycle.md). These constraints are bootstrap defaults;
the final untrusted-code path uses deployment-issued directory/network handles and capability grants.

## Completion ABI

The JSC binding knows only:

```text
HostCall { operation: String, payload: JSON String }
DeferredPromise
```

The Tokio side stores `DeferredPromise` in an isolate-local pending map and sends only request and
stream IDs, owned strings, byte vectors, and response metadata through bounded channels. A worker
cannot obtain or transport a JSC pointer.

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

- `kunlun:fs`: directory handles, metadata, and writes behind separate grants.
- `kunlun:http`: streaming request bodies and Web `Request`/`Response` integration.
- `kunlun:crypto`: Web Crypto-compatible primitives rather than a second incompatible crypto model.
- `kunlun:process`: deployment metadata only; no ambient subprocess or raw environment access.

New built-ins require a module descriptor, Rust operation implementation, capability rule, TypeScript
declaration, denial test, and completion-path test.
