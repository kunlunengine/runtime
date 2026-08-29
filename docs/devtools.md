# Developer Tools Platform

## Decision: one platform, usable without an IDE

Debugging must still work when a developer has removed VS Code, WebStorm, Zed, or Logos and works
only through Claude Code, Codex, another coding agent, or a terminal such as Warp. Kunlun therefore
does not make an IDE extension—or the browser-hosted WebKit Web Inspector—the product boundary.

The runtime owns only its inspectable execution endpoint. A separately distributed and eventually
separately named DevTools product (working codename: **Chunyang / 纯阳剑 DevTools**) owns sessions,
tools, user interfaces, agent integration, and cross-runtime adapters.

| Layer or surface | Responsibility | Decision |
| --- | --- | --- |
| `kunlun-runtime` Inspector endpoint | JSC/WIP messages, pause-loop integration, runtime diagnostics | Lives in this repository |
| DevTools service/core | targets, sessions, source maps, normalized events, tools, auth, persistence | One standalone platform shared by every client |
| Desktop app | full visual debugger and optional embedded agent | First showcase for the prospective Kunlun Desktop framework |
| CLI/TUI | full headless workflow and interactive terminal client | Must work with no browser or IDE installed |
| MCP + Skill | semantic debugger tools, resources, and debugging playbooks for coding agents | Portable baseline for Codex and other MCP clients |
| Claude Code plugin | Skills, agents, hooks, and packaged MCP integration | Deeper Claude-native integration where it adds value |
| DAP/IDE clients | editor breakpoints, scopes, and debug consoles | Compatibility adapters, not required frontends |
| Browser frontend | WebKit Web Inspector compatibility/bootstrap UI | Supported fallback, not the long-term product center |

"One platform" does not mean pretending that every target speaks one wire protocol. JSC uses WebKit
Inspector Protocol (WIP); browsers and Node-family targets commonly use CDP; native debuggers and
Xcode have different control and data models. Target adapters preserve those facts while the shared
service provides one target graph, session lifecycle, source model, authorization model, and tool
surface. Kunlun does not maintain a different debugger product for each UI or agent.

## Architecture and repository boundary

```text
runtime/backend adapters
  JSC/WIP   Web/CDP   native/Xcode   framework/state adapters
      \        |          |                    /
       standalone DevTools service/core (versioned local RPC)
          |          |           |          |          |
       Desktop     CLI/TUI    MCP + Skill   DAP/IDE   browser fallback
                                   |
                         Claude Code plugin
```

This repository implements the JSC/WIP edge and the runtime diagnostics required by the shared
contract. The standalone product owns the general platform, client applications, MCP server, DAP
bridge, and adapters for other runtimes. Keeping that boundary prevents `kunlun-runtime` from
absorbing a desktop shell or agent SDK while still making its debugger first-class.

The DevTools core initially owns:

- target discovery, stable target IDs, and session multiplexing;
- reconnect rules across rebuild and HMR;
- source maps, virtual sources, and source retrieval;
- breakpoint, pause, scope, evaluation, console, request, profiler, and heap events;
- backpressure, maximum message sizes, event recording, and protocol diagnostics;
- the nested/pause event-loop pump required while JavaScript execution is stopped;
- explicit permissions and audit events for state-changing or code-executing operations.

The common model is not a promise of lossless WIP-to-CDP or Web-to-native protocol translation.
Protocol-specific capabilities remain discoverable, and clients must show unsupported operations
honestly.

## Agent-facing debugger

MCP is the portable agent boundary, not the high-frequency debugger transport. The local MCP server
maps semantic operations onto the DevTools service—for example discovering targets, setting a
breakpoint, continuing to the next pause, reading scopes, evaluating an expression, inspecting a
request, or capturing a diagnostic artifact. Resources expose bounded snapshots and artifacts;
prompts or a companion Skill teach repeatable debugging workflows. Bulk event streams and UI state
remain on the direct local service protocol.

The MCP server should use stdio for a client-owned local process by default. A long-lived Desktop or
shared daemon may expose an authenticated local transport, but must not silently create a remotely
reachable debugger. Tool schemas distinguish read-only inspection from operations that resume a
process, mutate state, execute code, or capture sensitive artifacts.

The same MCP implementation may run as a CLI-owned stdio child or be hosted by the Desktop service;
these are distribution modes, not separate tool contracts.

The Claude Code plugin can package more of the experience than MCP alone: Skills, specialist agents,
hooks, and the MCP server definition can ship together. Other agents receive the same underlying
capability through MCP plus a vendor-neutral Skill. The plugin must not gain a second private
debugger implementation.

The Desktop app may include an agent, but that agent consumes the same published tool and permission
contract as external agents. UI embedding is not a privileged backdoor into the debugged process.

## Desktop showcase

The standalone Desktop app is intended to be the first demanding showcase for a future **Kunlun
Desktop** framework: Kunlun Engine for application/runtime logic plus CEF or a platform WebView for
the presentation layer. Debugging exercises the parts a desktop framework must prove—multiple
processes, native menus and windows, local IPC, streaming data, crash recovery, profiling, secure
capability brokering, and self-inspection—so it can make a concrete case for choosing Kunlun Desktop
instead of Electron or Tauri.

That showcase goal must not make the CLI a reduced companion. Every correctness-critical operation
remains available headlessly; Desktop adds visualization, navigation, and integrated-agent ergonomics.

## Unified Web and native direction

The JSC debugger is the first vertical slice, not the final scope. The general platform is the path
to removing Logos' embedded `vscode-js-debug` and replacing fragmented debug surfaces with one tool
that can coordinate:

- browser, worker, server, and Kunlun/JSC targets;
- React DevTools, Vue DevTools, and Nuxt DevTools adapters;
- Kunlun's own state-management and runtime diagnostics;
- native processes and Xcode-oriented build/launch/debug workflows;
- correlated JavaScript, native, network, state, log, and performance timelines.

Framework DevTools remain adapters with their own capability namespaces; "integration" does not
mean copying their UIs or flattening every framework concept into the core protocol.

## Entry points

The final standalone product name and command are intentionally undecided. Until that naming work is
done, runtime-facing commands are integration entry points rather than ownership claims:

```bash
kunlun dev --inspect                 # expose a loopback-only inspectable target
kunlun inspect                       # discover/launch the installed DevTools client
kunlun inspect --target orders-api   # select a target
kunlun repl                          # attach a runtime-aware headless REPL
```

The standalone CLI must also support target listing, attach/launch, breakpoints, stepping,
evaluation, structured logs, request traces, capability audit events, and heap/CPU diagnostic
capture without opening a window. Machine-readable output is required for agents and automation.

## Security defaults

- Inspection is disabled in production unless explicitly enabled.
- Runtime endpoints and the DevTools service bind to loopback or a restricted local IPC mechanism by
  default.
- Non-loopback sessions require a short-lived token; deployment products add TLS or an authenticated
  reverse proxy.
- Target metadata and agent resources must not expose secrets, capability values, environment
  values, raw headers, or unbounded memory by default.
- Expression evaluation, process control, state mutation, heap dumps, and profiler captures require
  explicit high-trust permissions.
- Every remote attach, evaluation, mutation, heap dump, and profiler capture emits an audit event.
- Desktop, CLI, MCP, plugins, and IDE adapters all pass through the same authorization layer.

## Delivery order

1. Use the platform Web Inspector locally to validate JSC source naming, WIP messages, and pause-loop
   behavior during the engine work.
2. Define the versioned runtime-to-DevTools contract, then implement the portable broker, source maps,
   HMR reconnection, diagnostics, and protocol fixtures.
3. Ship the standalone CLI and MCP + Skill vertical slice so terminal-only and agent-only developers
   can complete a source-debugging session without an IDE.
4. Ship the Kunlun Desktop client on the same service/core, including an optional embedded agent;
   retain the browser frontend as a bootstrap/fallback surface.
5. Package the Claude Code plugin and DAP adapters after the semantic tool and session contracts are
   stable.
6. Add Web/CDP, framework/state, and native/Xcode adapters incrementally, using the platform to
   replace Logos' `vscode-js-debug` dependency rather than moving that dependency into the runtime.

Protocol and integration references:

- WebKit inspectable-context API:
  <https://webkit.org/blog/13936/enabling-the-inspection-of-web-content-in-apps/>.
- MCP tools and local-server security guidance:
  <https://modelcontextprotocol.io/specification/2025-11-25/server/tools> and
  <https://modelcontextprotocol.io/specification/2025-11-25/basic/security_best_practices>.
- Claude Code plugin components and packaging:
  <https://code.claude.com/docs/en/plugins-reference> and
  <https://code.claude.com/docs/en/agent-sdk/plugins>.
