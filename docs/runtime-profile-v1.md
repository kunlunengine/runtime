# M2 JavaScript runtime profile v1

Profile identifier: `kunlun-m2-web/1`. Introduced in runtime 0.1.0. This is the server-side
foundation for M2 Fetch and the M3 application contract, not a browser environment. A change
that removes a supported operation or changes an intentional deviation requires a new profile
version. Additive APIs require new fixtures and declarations. The pinned JSC release is the
production compatibility target; system JSC is a macOS development backend.

## Surface and compatibility targets

| Global / export | Supported surface and reference | Omissions / deviations in v1 |
| --- | --- | --- |
| `console` | `debug`, `log`, `info`, `warn`, `error`; [Console Standard](https://console.spec.whatwg.org/) logging levels | Formatting is the deterministic host format below. No format substitutions (`%s`, `%o`, etc.), groups, counters, timers, tables, assertions, trace or browser inspector object handles. |
| `TextEncoder` | `encoding`, `encode`, `encodeInto`; [Encoding Standard](https://encoding.spec.whatwg.org/#interface-textencoder) | UTF-8, including replacement of unpaired UTF-16 surrogates. `encodeInto` never splits a scalar value and counts UTF-16 input units. No `TextEncoderStream`. |
| `TextDecoder` | Constructor, `encoding`, `fatal`, `ignoreBOM`, `decode` with `stream`; [Encoding Standard](https://encoding.spec.whatwg.org/#interface-textdecoder) | UTF-8 labels only (`utf-8`, `utf8`, `unicode-1-1-utf-8`, ASCII case insensitive and trimmed). Other encodings throw `RangeError`. No `TextDecoderStream`. Invalid input consumes maximal valid prefixes using Rust UTF-8 validation; fatal failures throw `TypeError`. |
| `URL` | Constructor; `href`, `origin`, `protocol`, `username`, `password`, `host`, `hostname`, `port`, `pathname`, `search`, `hash`, `searchParams`; `toString`, `toJSON`, `canParse`, `parse`; [URL Standard](https://url.spec.whatwg.org/) | Parsing and browser-style setters use Rust `url` 2.5.8, including its documented WHATWG differences. No blob/object URL registry (`createObjectURL`, `revokeObjectURL`). `origin` is read-only. |
| `URLSearchParams` | String, record and pair-iterable constructors; `size`, `append`, `delete`, `get`, `getAll`, `has`, `set`, stable `sort`, live `entries`/`keys`/`values`/iteration, `forEach`, `toString`; [URL Standard](https://url.spec.whatwg.org/#urlsearchparams) | Form encoding uses Rust `url::form_urlencoded`. Linked parameters retain identity across URL mutations; their mutations reserialize the query. |
| Streams | `ReadableStream`, `WritableStream`, `TransformStream`, `ByteLengthQueuingStrategy`, `CountQueuingStrategy`; reader/controller classes listed below; [Streams Standard](https://streams.spec.whatwg.org/) | Vendored `web-streams-polyfill` 4.2.0. Default readers, writers, BYOB readers, async iteration, tee, piping, cancellation and queue strategies follow that implementation. Transfer/detachment of BYOB buffers depends on native `ArrayBuffer.prototype.transfer`; no transferable streams or structured-clone support. Host adapters are default streams of `Uint8Array`, not BYOB byte sources. |
| `crypto` | `getRandomValues`, `randomUUID`; [Web Cryptography](https://w3c.github.io/webcrypto/#Crypto-interface) | OS entropy via `getrandom` 0.4.3, no seeded PRNG or fallback. Integer typed arrays only; 65,536-byte per-call limit. UUID v4 uses the same OS entropy. Error names follow the Web API, but errors are `Error` objects, not `DOMException` instances. |
| `crypto.subtle` | Promise-returning `digest` for SHA-256, SHA-384, SHA-512; [Web Cryptography digest](https://w3c.github.io/webcrypto/#SubtleCrypto-method-digest) | RustCrypto `sha2` 0.11; no custom cryptographic algorithms. No SHA-1, keys, import/export, signing, verification, encryption, derivation or wrapping. Digest copies input and computes on the isolate thread before settling its Promise; large inputs can delay other work. Unknown algorithms reject with `NotSupportedError` (an `Error`). |
| `AbortController`, `AbortSignal` | Controller construction, `signal`, `abort`; signal `aborted`, `reason`, `onabort`, `throwIfAborted`, `addEventListener`/`removeEventListener` for `abort`, static `abort`; [DOM abort algorithms](https://dom.spec.whatwg.org/#aborting-ongoing-activities) | Existing minimal event implementation, not an `EventTarget`/`Event` hierarchy. No `any`, `timeout`, dispatch, capture, listener signals or event propagation. Default reason is an `Error` named `AbortError`. See [lifecycle](./lifecycle.md). |

Streams additionally expose `ReadableStreamDefaultReader`, `ReadableStreamBYOBReader`,
`ReadableStreamDefaultController`, `ReadableByteStreamController`, `ReadableStreamBYOBRequest`,
`WritableStreamDefaultWriter`, `WritableStreamDefaultController`, and
`TransformStreamDefaultController`. Their methods/accessors and illegal-constructor behavior
come from the vendored implementation; the checked-in `types/streams.d.ts` enumerates the
complete surface. See the [upstream compatibility notes](https://github.com/MattiasBuelens/web-streams-polyfill/tree/v4.2.0#compatibility).

The hand-written encoding, URL and crypto adapters do not implement the full Web IDL machinery
(e.g. property descriptors, illegal receiver checks, required-argument arity errors and all
exotic-object coercion cases). SharedArrayBuffer is not part of this profile. APIs use the
current isolate's constructors; cross-realm inputs are not a compatibility target. No DOM,
Window, navigator, WebSocket, Node modules, setTimeout or setInterval are introduced by this
profile. Native ECMAScript globals (including Temporal on
the pinned engine) remain governed by the [engine profile](./module-loading.md#temporal).
M3 adds the [application Fetch profile](./fetch-profile-v1.md) without changing this
M2 foundation's identifier. The existing `sleep`, `kunlun.import`, `kunlun:fs`
and `kunlun:http` surfaces and permissions remain specified by
[builtins](./builtins.md) and [lifecycle](./lifecycle.md).

### URL base and referrer behavior

Relative inputs require an explicit valid base. Invalid bases reject even for an absolute input;
opaque bases cannot resolve ordinary relative paths. Unicode hostnames use IDNA, default ports
are removed, IPv6 brackets are retained, paths percent-encode Unicode, and query form encoding
uses `+` for spaces. Query mutation may normalize `%20` to `+` and `~` to `%7E`.
`new URL()` never reads an ambient document, entry-module referrer, cwd, or permission grant.
Native module resolution supplies its own referrer as specified in [module loading](./module-loading.md).
URL parsing does not grant network or filesystem access.

### Console host contract

`TokioIsolate::set_console_sink` installs a synchronous isolate-thread callback receiving an
owned `ConsoleRecord { level, message, source }`. The default sink writes one JSON record per
line to stderr. Applications can route records to their logging system without parsing terminal
formatting. A sink must return promptly and must not reenter the isolate or replace itself.

Arguments are joined by a single space; at most 32 are inspected. Objects use sorted enumerable
own string keys, at most 32 properties per object and three levels of nesting. Accessors are
rendered as `[Getter]` without invocation, cycles as `[Circular]`, and deeper objects as
`[Object]`. Arrays use the same keyed representation. Errors show name and message. Proxies
may run traps; failures become `[Uninspectable]`. Strings and records are truncated, not retained
as engine handles. Message payloads are at most 8,192 UTF-8 bytes and source stacks at most
2,048 bytes. Source contains up to four JSC stack frames (source URL/line/column when available),
not source-mapped or engine-independent stack syntax. No timestamps are inserted by the runtime.

### Host I/O and backpressure

`kunlun:fs.openReadStream` and `kunlun:http.requestStream().body` retain their existing
`ByteStream` contract. Call `toReadableStream()` once to transfer consumption to a standard
`ReadableStream<Uint8Array>`. Direct `read()` then rejects, preventing competing consumers.
The adapter uses high-water mark zero, performs only one host read at a time, and never pulls
until requested. It preserves the host's bounded chunk/channel limits. `pipeTo` propagates
backpressure from asynchronous writes; stream cancellation closes the host producer. An
operation's AbortSignal errors its adapter with the same reason and cancels the host operation,
even while no read is pending. EOF, errors and cancellation remove adapter listeners.

Only plain completion data crosses Tokio channels. Array buffers, stream controllers, readers,
and Promise resolvers remain on the owning JSC thread. Writable/transform sources are JavaScript
callbacks; there is no new filesystem/network write permission or native write API.

### Modules and declarations

`kunlun:web` exports the same objects as the named global APIs in the table, including
AbortController/AbortSignal and both queuing strategies. Reader/controller classes are globals
only. Bootstrap `kunlun.import('kunlun:web')` and native ESM use the same module descriptor.
The types package supports an ES2022-or-later library without `lib.dom`, using TypeScript 5.9+.
The CLI `types` command emits a self-contained concatenation of the declarations shipped in
`types/`; no browser API declarations are borrowed that would imply unsupported runtime APIs.

### Conformance and updates

`tests/fixtures/web-profile.js` is an original, focused equivalent-conformance fixture, not a
claim of passing all WPT. It covers Unicode/replacement/fatal/streaming/BOM/subview decoding,
percent encoding and URL bases, URL/query synchronization, OS-random API constraints, SHA known
answers, reader locks, slow writers, transform piping and abort propagation.
`tests/web_profile.rs` also verifies structured console bounds/source context and real host file
streams with slow consumers, EOF, cancellation and idle abort. The normal workspace suite runs
these on macOS system JSC and on pinned macOS/Linux arm64/x64 JSC in the existing CI matrices.

The vendored streams runtime and license are under `src/web/vendor`; no npm install or network
access is needed at runtime or during a Cargo build. `types/streams.d.ts` is the upstream
ponyfill declaration converted to globals (using the runtime's AbortSignal). When updating the
vendor, record provenance/checksums, retain the license, regenerate that declaration, and run
both backend suites plus the TypeScript fixture. See [vendor provenance](../crates/kunlun-runtime/src/web/vendor/README.md).
