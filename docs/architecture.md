# Runtime Architecture

## Repository boundary

Kunlun Engine has three separate planes:

```text
authoring / orchestration              build plane                         runtime plane

@kunlun-js/core + kunlun CLI     ->    BuildEngine adapter          ->     kunlun-runtime
application + targets                  server bundle + assets              JSC isolate host
capability declarations                runtime manifest                    Fetch + capabilities
```

`kunlunengine-core` owns public TypeScript application semantics, build adapters, and the user-facing
`kunlun` CLI. This repository owns the native host, JSC lifecycle, native capability brokers,
isolation, and Inspector backend. The CLI invokes the runtime; the runtime does not absorb project
generation or bundler configuration.

## Missing executable boundary in the current core

The existing `ApplicationManifest` describes service names, routes, and capability requirements.
Route handlers remain JavaScript functions inside `kunlun.config.mjs`; they are not serialized into
that manifest. A native process therefore cannot execute the manifest alone.

The build plane must produce two related artifacts:

```text
.kunlun/runtime/
├── manifest.json     # kunlun.runtime-manifest/v1 metadata and integrity
├── server.mjs        # executable ESM server entry
├── chunks/*.mjs
└── maps/*.map
```

The server entry exports a Fetch-shaped handler:

```js
export default {
  async fetch(request, env, executionContext) {
    return new Response('Hello from Kunlun')
  },
}
```

The exact JavaScript API will be finalized with cross-runtime conformance tests. The manifest is
data; the server bundle is code. Mixing the two would either lose handlers or require unsafe source
serialization.

## Native layers

```text
kunlun-runtime (Tokio isolate loop, process, HTTP, lifecycle, CLI protocol)
        |
kunlun-host (Fetch objects, event loop, module loader, capability handles)
        |
kunlun-jsc (safe RAII wrapper; !Send values; typed exceptions)
        |
kunlun-jsc-sys + kunlun_jsc C ABI shim
        |
pinned JavaScriptCore / WTF / bmalloc
```

Only the bottom shim may include WebKit C++ headers. Rust code binds a deliberately small C ABI, not
WebKit's unstable C++ ABI. High-level host code never handles an unrooted raw `JSValueRef`.

## Isolate and concurrency model

- One isolate owns one context group/VM and remains on its polling thread.
- The caller supplies the Tokio runtime and awaits the isolate's thread-affine evaluation future.
- Contexts, values, callbacks, and module records are `!Send + !Sync`.
- Work crosses isolate boundaries through owned byte buffers and structured-clone messages.
- Host async operations return opaque request IDs; completion is posted to the isolate queue.
- Microtask checkpoints occur at specified host boundaries, never from arbitrary foreign threads.
- Execution deadlines require an engine watchdog in the pinned JSC shim; the bootstrap API does not
  claim that Tokio timers can interrupt synchronous JavaScript.

This makes illegal cross-thread JSC access difficult to express in safe Rust.

The first executable proof is `sleep(ms)`: `kunlun-jsc` creates and protects a JSC Deferred Promise,
`kunlun-runtime` queues its deadline, and the evaluation future invokes the Promise resolver on the
same isolate thread when that deadline is due. Filesystem and HTTP operations extend the pattern:

```text
JSC host callback
  -> pending[id] = DeferredPromise            # isolate-local, !Send
  -> Tokio task(operation, JSON, id, sender)  # no JSC pointer
  -> Completion { id, Result<String, String> }
  -> isolate evaluation future drains completion queue
  -> pending.remove(id).resolve/reject()
```

Tests verify native Promise jobs, timer ordering across `await`, asynchronous exception propagation,
filesystem/network capability denial, and real filesystem/local-HTTP completions. A `JSValueRef`
must never enter a Tokio `Send` task or completion message.

## Capability model

The core manifest declares desired capabilities. Deployment policy grants a subset as unforgeable
host handles. Every host call resolves:

```text
(extension, tenant, request, capability, operation, resource) -> allow / deny
```

JavaScript receives no ambient filesystem, subprocess, native-addon, or unrestricted network access.
The bootstrap grants read roots and exact network hosts explicitly. HTTP redirects are disabled so a
granted origin cannot redirect to an ungranted host. These path checks are a development capability
prototype, not a replacement for handle-based filesystem brokering in the hostile-code sandbox.
The capability layer limits host authority, but it does not claim to contain engine exploits; worker
process/container/microVM isolation is a separate layer.

## Compatibility

The runtime manifest carries a schema version and required engine ABI. The runtime rejects unknown
major schemas and unsupported required features before executing code. Node and JSC implementations
share conformance fixtures for Fetch behavior, routing, errors, streaming, aborts, and shutdown.

The bootstrap in this repository intentionally stops below this contract. It supports async function
bodies and capability-gated built-ins, but it is not yet a native ESM/Fetch production interface.
