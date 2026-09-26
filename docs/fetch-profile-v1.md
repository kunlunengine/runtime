# M3 application Fetch profile v1

M3 adds global `Headers`, `Request`, `Response`, and `fetch`, also exported by
`kunlun:web`. This is the outbound API and the object shape for the proposed
`kunlun.fetch-entry/v1` handler. Inbound dispatch remains #51 work. The unchanged
`kunlun-m2-web/1` profile is the foundation, not a claim of full browser or Node
Fetch compatibility.

## Compatibility report

| Surface | Supported | Unsupported or bounded |
| --- | --- | --- |
| `Headers` | Record, pair, and Headers constructors; `append`, `set`, `delete`, `has`, `get`, `getSetCookie`, sorted iteration and `forEach`; case-insensitive names and repeated values | Browser header guards are absent. Invalid names and values reject. Transport-controlled request headers such as Host and Content-Length reject. Requests allow at most 256 headers and 64 KiB of names and values. |
| `Request` | Absolute HTTP(S) URL; method, headers, body, signal, redirect mode, `duplex: 'half'`, `clone`, body readers and locks | Credential-bearing URLs, GET/HEAD body, CONNECT/TRACE/TRACK, FormData/Blob, cache/credentials/mode/integrity/referrer options. Unknown init keys throw. Streaming body clone throws. |
| `Response` | Status 200–599, status text, headers, body, `ok`, `url`, `redirected`, `clone`, body readers, `Response.redirect` | `Response.error`, `Response.json` factory, trailers, FormData/Blob. 204/205/304 reject bodies. Streaming body clone throws. Network status text uses the canonical reason. |
| `fetch` | HTTP(S), binary and Unicode buffered bodies, streaming `Uint8Array` upload/download, abort, `follow`/`manual`/`error` redirects | Cookies, cache, CORS, integrity, keepalive, proxy configuration, automatic decompression and browser security context. A streaming body cannot be replayed for a 307/308 redirect. At most 20 redirects. |

Bodies are `ReadableStream<Uint8Array>` values. `arrayBuffer`, `bytes`, `text`, and
`json` consume at most 1 MiB; applications can read larger bodies directly from
the stream. Buffered uploads are limited to 1 MiB. Streaming uploads send at
most 64 KiB per call through a two-chunk channel; downloads use a two-chunk,
64 KiB channel with one pending pull. Upload writes wait for channel capacity.
Response cancellation stops the host producer. The generic stream `tee()` exists,
but Request/Response cloning deliberately rejects a streaming body because an
unevenly consumed tee can buffer without bound.

`bodyUsed` becomes true when consumption starts or a body is locked. Producer
errors reject pending reads. Abort before open rejects with the signal reason
without networking; pending open, pending reads, and post-header abort cancel the
associated host work. Evaluation cancellation and isolate shutdown remove upload
channels, pending calls, and response streams. Tests cover these paths in
`crates/kunlun-runtime/tests/fetch_profile.rs`.
The same `fetch-shared.js` object fixture runs unchanged on Node 24 and JSC;
network and capability cases use the local JSC HTTP fixture until #53's
cross-runtime application corpus is available.

## Authority and redirects

Fetch permissions are distinct from legacy `kunlun:http` `--allow-net` grants.
A trusted embedder can grant an exact host with
`HostPermissions::allow_fetch_host` for direct execution. M3 artifact deployment
uses `AdmissionPolicy::new` with `HostPermissions::allow_net_host` grants; admission
intersects those grants with declared `http.host` capabilities and projects the
result into both legacy HTTP and Fetch for the application's lifetime. A bare
declaration or direct M2 `--allow-net` grant does not enable Fetch. Fetch's direct
grant does not enable `kunlun:http`. Each request and redirect destination is
checked by the host before opening a connection, including revocation of an M3
application authority. Automatic host-client redirects and system proxy discovery
are disabled. Cross-origin redirects remove Authorization, Cookie, and
Proxy-Authorization. The existing `kunlun:http` contract remains separate.
