# Developer Tools and Inspector

## Decision: backend first, web frontend first, IDE adapters second

Kunlun does not implement three debugger engines.

| Surface | Responsibility | Decision |
| --- | --- | --- |
| Runtime Inspector backend | breakpoints, stepping, scopes, evaluation, profiler/heap plumbing | Built into `kunlun-runtime` |
| Browser DevTools | full source debugger UI | WebKit Web Inspector frontend, served/opened by `kunlun inspect` |
| CLI/TUI | targets, process state, logs, requests, capabilities, REPL, shortcuts | Built into `kunlun`; not a full source debugger |
| IDE | editor breakpoints and debug console | Standalone DAP bridge; thin extensions only |
| Native desktop GUI | none initially | Reconsider only if the web frontend has a demonstrated blocker |

The phrase "built-in DevTools" therefore means the runtime ships the backend and can serve/open its
matching web frontend. It does not mean embedding a browser engine or a platform-specific native
window into the runtime process.

## Protocol layers

JSC speaks WebKit Inspector Protocol (WIP), not Chrome DevTools Protocol (CDP). Kunlun keeps that
fact visible instead of claiming protocol compatibility.

```text
JSC Inspector backend
        |
Kunlun inspector session broker (WIP envelopes, targets, auth, pause-loop integration)
        |--------------------------|
WebKit Inspector frontend          kunlun-debug-adapter
                                   |
                                   Debug Adapter Protocol
                                   |
                                   VS Code / JetBrains / other IDEs
```

The session broker owns:

- target discovery and stable target IDs;
- WIP message transport and session multiplexing;
- reconnect rules across rebuild/HMR;
- source-map and virtual-source retrieval;
- backpressure, maximum message sizes, and protocol logging;
- the nested/pause event-loop pump required while JavaScript execution is stopped.

DAP is an adapter over the same WIP session. A broad WIP-to-CDP compatibility promise is out of
scope because the protocols have different domains and semantics.

## CLI experience

```bash
kunlun dev --inspect                 # start runtime inspector on loopback
kunlun inspect                       # list targets and open the web frontend
kunlun inspect --target orders-api   # select a target
kunlun repl                          # attach a runtime-aware REPL
kunlun debug-adapter --stdio         # IDE extension entrypoint
```

`kunlun dev` may show a compact interactive dashboard when attached to a terminal. It must have a
plain log mode for CI and accessibility. The dashboard consumes structured runtime events and is
never required to operate or debug the application.

## Security defaults

- Inspector disabled in production unless explicitly enabled.
- Loopback bind by default.
- Non-loopback sessions require a short-lived token; deployment products add TLS or an authenticated
  reverse proxy.
- Target metadata must not expose secrets, capability values, environment values, or raw headers.
- `Runtime.evaluate` is equivalent to code execution and follows the strongest inspector permission,
  not a read-only observability permission.
- Every remote attach, evaluation, heap dump, and profiler capture emits an audit event.

## Delivery order

1. Local macOS inspection through the platform Web Inspector validates source naming and pause-loop
   behavior during the M0/M1 engine work.
2. The portable broker and WIP web frontend arrive after the pinned engine exposes Inspector
   callbacks.
3. Sourcemaps, virtual modules, HMR reconnection, profiling, and heap snapshots are added to that
   single backend.
4. DAP bridge and a thin VS Code extension ship after protocol fixtures are stable.
5. JetBrains integration should consume DAP or the documented adapter endpoint, not fork the backend.

WebKit's supported inspectable-context API is documented at
<https://webkit.org/blog/13936/enabling-the-inspection-of-web-content-in-apps/>.
