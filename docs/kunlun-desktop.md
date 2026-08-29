# Kunlun Desktop Presentation Architecture

Status: accepted for the DevTools showcase

Decision date: 2026-08-29

## Decision

The first production-qualified Kunlun Desktop presentation backend uses a **pinned CEF/Chromium
distribution**. Chromium is a presentation engine only: it does not become the Kunlun application
runtime, and Kunlun Desktop does not adopt Electron's Node-in-the-renderer architecture.

Application and tool logic runs behind a versioned, capability-checked boundary in Kunlun Engine and
the standalone DevTools service. The presentation process renders HTML/CSS, handles input, and sends
typed commands and events across that boundary. It receives no ambient filesystem, subprocess,
socket, secret, or native-addon access.

Kunlun Desktop keeps a deliberately small renderer-backend interface so a platform WebView can be
evaluated later. CEF is nevertheless the sole reference renderer and the only renderer that the
initial release matrix promises. Backend abstraction is not a commitment to browser-feature or
pixel parity.

Building a private WebCore+JSC presentation runtime is out of scope. The pinned JSC used by
`kunlun-runtime` remains an application/server engine; it must not force the Desktop presentation
stack to carry WebCore, graphics, media, accessibility, networking, and browser-process ownership.

## Why this boundary

DevTools is the first Kunlun Desktop showcase because it exercises native windows and menus,
high-volume streaming data, local IPC, crash recovery, profiling, accessibility, secure capability
brokering, and self-inspection. It should prove the Kunlun host and application model, not prove that
the project can maintain a browser engine before it can ship a debugger.

The reference renderer is pinned and distributed with each Desktop release. This costs download and
installed size, but gives CI, release engineering, and support one web-platform version per release
channel. Installed Chrome is never discovered and substituted at runtime.

This choice also keeps the two JavaScript roles honest:

- Chromium/V8 executes less-privileged presentation code inside the renderer sandbox.
- Kunlun/JSC executes application and tool logic in a separately contained service process.
- The DevTools core joins Chromium/CDP, Kunlun/JSC/WIP, and later native targets without pretending
  that they share an engine or wire protocol.

Shipping both V8 and JSC is an intentional cost of isolating presentation from application
semantics. Replacing one with a bespoke combined engine would trade a visible package-size cost for
a much larger browser-maintenance and security-response obligation.

## Process architecture

```text
signed Kunlun Desktop application bundle
                 |
        native bootstrap / updater
                 |
        Desktop host (browser process)
        - windows, menus, lifecycle
        - renderer-backend adapter
        - capability and navigation policy
        - typed IPC routing and audit
          /            |              \
         /             |               \
CEF renderer(s)   Kunlun service     DevTools service/core
HTML/CSS/input    kunlun-runtime/JSC  targets/sessions/maps
sandboxed V8      app/tool logic      WIP/CDP/native adapters
         \             |               /
          \______ versioned local RPC_/
```

CEF's browser, renderer, GPU, and utility processes retain their upstream process boundaries and
sandbox. Kunlun does not collapse them into a single process to save memory. On platforms where CEF
requires a helper executable or app-bundle helpers, those helpers are signed and versioned as part
of the same atomic Desktop installation.

The Kunlun service is a separate process rather than a privileged object injected into the renderer.
A renderer crash can be recovered without corrupting application state; a service crash is reported
and restarted according to an explicit lifecycle policy. Per-window and per-profile isolation is
chosen from security requirements, not hidden behind the backend interface.

## Renderer-backend contract

The native backend interface is limited to operations Kunlun Desktop itself owns:

- report backend identity, exact version, capabilities, and health;
- create and close profiles, windows, and views;
- load a signed application bundle into an isolated secure origin;
- bind a versioned bidirectional message channel with bounded queues and backpressure;
- enforce navigation, download, popup, permission, and external-link policy;
- report lifecycle, focus, loading, renderer-crash, and accessibility events;
- capture diagnostics and screenshots under explicit permission; and
- expose an authenticated inspection endpoint in development builds.

It does not wrap DOM, CSS, canvas, Web APIs, or DevTools protocol domains. Presentation code targets
the documented Kunlun Desktop Web Profile. Optional backends report unsupported capabilities rather
than receiving emulation layers that grow into another browser.

Application assets load from an internal, per-application secure origin backed by the signed bundle,
not from `file:` URLs and not from an unauthenticated localhost server. Arbitrary top-level
navigation and popups are denied by default. External URLs leave the application only through a
host policy decision. Default CSP blocks remote code, inline script without an approved hash or
nonce, and embedding by unrelated origins.

IPC schemas are versioned independently of the renderer implementation. Messages carry the
application, window, session, and capability identity needed for host-side authorization. Payloads
are owned data; large streams use bounded flow control rather than unbounded JSON events. The host
validates every privileged request even when the UI already disabled the corresponding control.

## Options considered

| Option | Release control and visual stability | Main cost | Decision |
| --- | --- | --- | --- |
| Pinned CEF/Chromium | One qualified engine build per Desktop channel; small residual OS/font/GPU variation | Large package, multiple processes, Kunlun owns Chromium security updates | **Reference backend for the first release** |
| Platform WebViews | Smaller application bundle and vendor-managed engine on some platforms, but engine and rollout differ by OS | Three integration stacks, wider visual/API matrix, Linux WebKitGTK dependency and API fragmentation | Deferred as optional backends; not part of the initial compatibility promise |
| Private WebCore+JSC runtime | A pinned fork could freeze behavior, but Kunlun would own the entire rendering and platform integration matrix | Browser-scale engineering, security response, WPT/layout/Test262, graphics, media, IME, accessibility, sandbox, and updater work | Rejected for the foreseeable roadmap |

### Why not make platform WebViews the default

The phrase "platform WebView" hides different products and lifecycle contracts. Windows WebView2
offers Evergreen and Fixed Version distribution; the former changes outside the application release
and the latter restores a bundled-runtime cost. macOS WKWebView follows the OS WebKit version. Linux
WebKitGTK spans GTK and API generations and brings GTK, GLib/GIO, libsoup, and distribution-specific
packaging constraints. A UI can be portable across these backends, but its release qualification is
not one matrix.

An optional platform backend may be admitted later only if it passes the same Desktop host contract,
security policy, Web Profile, accessibility, input/IME, crash-recovery, and performance gates. Linux
release builds must never bind opportunistically to an arbitrary system WebKitGTK version; such a
backend would need an explicitly supported and reproducible distribution policy.

### Why not combine WebCore with Kunlun's JSC

JavaScriptCore is only one browser component. A shippable WebCore embedding also needs a supported
graphics/compositing stack, font and text behavior, networking and storage processes, media,
accessibility, input methods, sandboxing, crash handling, developer tools, packaging, and rapid
security updates. Private glue against WebKit C++ internals would also contradict the runtime
repository's narrow C-ABI boundary.

Upstream Test262 coverage is necessary for JavaScript but says little about DOM, CSS, layout,
compositing, accessibility, or navigation security. WPT and upstream layout tests would become a
product-owned qualification matrix in addition to, not instead of, visual regression tests. The
result would delay the showcase while testing a different product.

## Qualification strategy

Kunlun tests the behavior it owns and relies on the chosen browser project's upstream engine suites:

1. Backend-independent contract tests cover lifecycle, IPC version negotiation, authorization,
   navigation policy, backpressure, crash recovery, and update rollback.
2. A small, published Desktop Web Profile suite covers only the Web APIs and security semantics the
   UI requires. It may reuse focused WPT cases; Kunlun does not fork the full WPT corpus.
3. Accessibility-tree, DOM-semantic, and interaction tests are the default UI regression signal.
4. Pixel goldens run on one hermetic canonical lane with a pinned CEF build, fonts, locale, scale,
   color profile, and graphics mode. Supported OS release lanes add a small set of platform smoke
   images for native text, input, menus, and compositing seams.
5. A CEF revision cannot ship until the Web Profile, security, IPC, accessibility/input, crash, and
   visual gates pass. The exact CEF revision and hashes are recorded in the release manifest.
6. Kunlun/JSC conformance and Test262 work remains a separate runtime gate. A renderer upgrade does
   not silently upgrade JSC, and a JSC upgrade does not silently change presentation output.

This reduces visual-regression breadth without claiming pixel identity across operating systems.
Canonical goldens are stable because the complete rendering input is pinned; platform smoke coverage
remains because fonts, window integration, GPU drivers, and accessibility stacks are still native.

## Distribution and security consequences

- The Desktop installer includes CEF libraries, resources, locales, helpers, and license notices;
  size is tracked as a release budget rather than hidden.
- Desktop and CEF are updated atomically, with signed manifests, staged rollout, crash telemetry, and
  rollback. Chromium security releases require an explicit response-time policy.
- The renderer sandbox is enabled in production. Disabling it is a development-only diagnostic mode
  and must be visible in process diagnostics.
- Renderer, host, Kunlun service, and DevTools service versions participate in startup compatibility
  negotiation. An incompatible partial installation fails closed.
- DevTools can inspect its Chromium presentation target through CDP and its Kunlun application target
  through WIP, but production inspection remains disabled unless explicitly authorized.

## Revisit conditions

The reference renderer decision should be revisited only with measured evidence, such as package
size blocking a target distribution channel, CEF no longer meeting support or sandbox requirements,
or a platform WebView backend passing the complete qualification gates at materially lower total
cost.

A private WebCore+JSC runtime requires a separate charter and staffed browser-engine program. Before
it can be reconsidered, Kunlun must be prepared to own upstream rebases and embargoed security
response, cross-platform graphics and accessibility, IME/media/networking behavior, sandbox and
process architecture, WPT/layout/Test262 infrastructure, and a multi-year maintenance budget. It is
not an optimization task inside the Desktop showcase.

## Primary references

- [CEF general usage and multi-process architecture](https://github.com/chromiumembedded/cef/blob/master/docs/general_usage.md)
- [CEF C API embedding lifecycle](https://github.com/chromiumembedded/cef/blob/master/docs/using_the_capi.md)
- [WebKitGTK stable API reference](https://webkitgtk.org/reference/webkitgtk/stable/)
- [Migrating WebKitGTK applications to GTK 4 / WebKitGTK 6.0](https://webkitgtk.org/reference/webkitgtk/stable/migrating-to-webkitgtk-6.0.html)
- [Microsoft WebView2 runtime distribution](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/distribution)
- [Apple WKWebView](https://developer.apple.com/documentation/webkit/wkwebview)
