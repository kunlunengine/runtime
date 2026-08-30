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

CEF's browser, renderer, GPU, and utility processes retain their upstream process boundaries;
separate processes do not imply that every process is sandboxed. The browser is a privileged broker
outside the Chromium sandbox. Renderer sandboxing is required; GPU and utility sandbox policies
depend on the exact CEF/Chromium revision, platform, and utility service subtype. Kunlun does not
collapse them into a single process to save memory. Required helpers are signed and versioned as
part of the same atomic Desktop installation.

The Kunlun service is a separate process rather than a privileged object injected into the renderer.
A renderer crash can be recovered without corrupting application state; a service crash is reported
and restarted according to an explicit lifecycle policy. Per-window and per-profile isolation is
chosen from security requirements, not hidden behind the backend interface.

### Sandbox qualification record

This repository has no Desktop launcher, pinned CEF revision, or measured sandbox results yet. The
following candidate platform rows are **unqualified**, not a claim of supported production targets.
Before release, expand them for every supported OS version/architecture and exact CEF revision and
Chromium version. Record the actual state (`enabled`, `disabled`, or `not spawned`) and evidence for
each process, with separate utility service subtypes and GPU hardware/software modes.

| Platform / CEF revision | Browser | Renderer | GPU | Utility (each subtype) |
| --- | --- | --- | --- | --- |
| Windows / not selected | Unsandboxed broker by design; unmeasured | Unmeasured; sandbox required | Unmeasured | Unmeasured |
| macOS / not selected | Unsandboxed broker by design; unmeasured | Unmeasured; sandbox required | Unmeasured | Unmeasured |
| Linux / not selected | Unsandboxed broker by design; unmeasured | Unmeasured; sandbox required | Unmeasured | Unmeasured |

Each record must include build flags, executable/helper paths and hashes, effective command lines,
`CefSettings.no_sandbox`, `browser_subprocess_path`, sandbox initialization results, and the
`sandbox_info` value's origin and whether it is null (not its raw address). Check startup against
the selected revision's [CEF sandbox setup](https://github.com/chromiumembedded/cef/blob/master/docs/sandbox_setup.md):

- **Windows:** use the same sandbox-capable executable for browser and child processes; do not set
  `browser_subprocess_path`. For M138 and newer, use the matching CEF bootstrap executable/client
  DLL arrangement, or build the executable within CEF/Chromium. Forward the executable-created,
  non-null `sandbox_info` to both `CefExecuteProcess` and `CefInitialize`. Older revisions use the
  executable-linked `cef_sandbox` static library and `cef_sandbox_info_create()`.
- **macOS:** initialize each helper with `CefScopedSandboxContext::Initialize` before loading the
  CEF framework; failure terminates the helper. M138 and newer dynamically load the bundled
  `libcef_sandbox.dylib`; earlier revisions link the helper with `cef_sandbox`. Record helper bundle
  layout, signatures, entitlements, and effective sandbox profiles. The Windows-only `sandbox_info`
  argument is null here and is not evidence that the macOS sandbox is disabled.
- **Linux:** record the actual namespace or setuid-helper path and seccomp-BPF policy used for each
  process, including kernel capabilities and helper ownership/permissions where applicable.
  `sandbox_info` is Windows-only and null here too; it does not enable or disable Linux sandboxing.

Production requires `no_sandbox = false` and no sandbox-disabling overrides, including `--no-sandbox`
and `--disable-gpu-sandbox`. Configuration alone is not proof: capture runtime restrictions and
denied-operation probes for the packaged build. Apart from the explicitly privileged browser broker,
the renderer and every GPU/utility process required by the production profile must have its required
sandbox enabled. A disabled or unverified required sandbox, missing process evidence, or failed
initialization **must fail production qualification**; `not spawned` is acceptable only for a
feature excluded from that profile. Requalify after revision, launch, packaging, or platform changes.

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
   navigation policy, backpressure, crash recovery, and the update/rollback cases below.
2. A small, published Desktop Web Profile suite covers only the Web APIs and security semantics the
   UI requires. It may reuse focused WPT cases; Kunlun does not fork the full WPT corpus.
3. Accessibility-tree, DOM-semantic, and interaction tests are the default UI regression signal.
4. Pixel goldens run on one hermetic canonical lane with a pinned CEF build, fonts, locale, scale,
   color profile, and graphics mode. Supported OS release lanes add a small set of platform smoke
   images for native text, input, menus, and compositing seams.
5. A CEF revision cannot ship until the Web Profile, security, IPC, accessibility/input, crash, and
   visual gates pass. The exact CEF revision and hashes, plus the completed per-platform sandbox
   qualification record above, are recorded in the release manifest.
6. Kunlun/JSC conformance and Test262 work remains a separate runtime gate. A renderer upgrade does
   not silently upgrade JSC, and a JSC upgrade does not silently change presentation output.

This reduces visual-regression breadth without claiming pixel identity across operating systems.
Canonical goldens are stable because the complete rendering input is pinned; platform smoke coverage
remains because fonts, window integration, GPU drivers, and accessibility stacks are still native.

## Distribution and security consequences

- The Desktop installer includes CEF libraries, resources, locales, helpers, and license notices;
  size is tracked as a release budget rather than hidden.
- Desktop and CEF are updated atomically, with signed manifests, staged rollout, crash telemetry, and
  authorized failure rollback subject to the security policy below. Chromium security releases
  require an explicit response-time policy and review of the minimum safe CEF version.
- Required subprocess sandboxes must pass the per-platform qualification gate above. Disabling one
  is a development-only diagnostic mode, visible in process diagnostics and ineligible for production.
- Renderer, host, Kunlun service, and DevTools service versions participate in startup compatibility
  negotiation. An incompatible partial installation fails closed.
- DevTools can inspect its Chromium presentation target through CDP and its Kunlun application target
  through WIP, but production inspection remains disabled unless explicitly authorized.

### Update and rollback acceptance policy

Release security metadata must specify a **minimum safe CEF version** for each supported platform
and channel, its corresponding Chromium security baseline, and approved exact CEF revisions and
artifact hashes. Compare parsed numeric versions, not version strings or commit hashes. A valid
manifest signature proves authenticity, not current safety: reject any manifest below this floor,
including a previously installed release or an otherwise authorized failure rollback.

The updater must authenticate security metadata independently of the candidate bundle and retain
the highest accepted policy/revocation sequences and security floor outside the installation being
rolled back. Neither an old signed manifest nor rollback authorization may lower that floor. Before
activation and on startup, verify manifest signatures, approved CEF revisions, artifact hashes,
platform/channel and version compatibility, and the current signed revocation list. Reject revoked
CEF revisions, artifacts, manifests, signing keys, or rollback authorizations. Verify metadata
signatures, scope, sequence, and expiry; missing, expired, or invalid metadata or a sequence below the
retained value fails closed. Offline use may rely only on a still-valid authenticated
policy/revocation cache meeting those checks.

A downgrade additionally requires an explicit, signed rollback authorization from a trusted
rollback-authority role; the old release's signature or crash telemetry alone is insufficient.
Validate the authority and its revocation status, authorization expiry and replay protection, and
bindings to the failed source and target manifest digests, platform/channel, and failure condition.
A valid authorization permits atomic rollback to a compatible, non-revoked release at or above the
current floor, even when its release version is older. Recheck policy immediately before activation,
retain the security metadata across rollback/restart, and audit the decision. If no safe authorized
target exists, fail closed and require a safe recovery update instead of starting vulnerable CEF.

### Required security acceptance tests

These are required integration-test cases for the future Desktop updater and launcher. Both the
implementation and executable Desktop tests are absent from this repository. Production qualification
requires them to run against the packaged build on every supported platform. Use signed fixtures
and otherwise valid metadata so negative cases reach the intended check. Let `F` be the installed
security floor and `A` the failed release; candidate `B` is an older compatible release.

| Case | Expected result |
| --- | --- |
| Validly signed update manifest with CEF below `F` | Reject before activation, despite a valid signature. |
| `B` has CEF below `F` and a valid rollback authorization | Reject; authorization cannot bypass the floor. |
| Failure of `A`; `B` has CEF equal to `F` or above it, no revocations, and valid scoped rollback authorization (test both) | Accept and atomically restore the complete Desktop/CEF bundle; startup succeeds and retains `F` and the policy/revocation sequences. |
| Safe `B`, but authorization is missing, invalidly signed, expired, replayed, or bound to another source, target, platform/channel, or failure | Reject each variant; an old signed manifest alone never permits downgrade. |
| Otherwise valid update/rollback with a revoked revision, artifact, manifest, signing key, or authorization | Reject each revocation variant, including revocation after staging but before activation. |
| Missing, invalid, expired security/revocation metadata or a sequence below the retained value; attempt to lower the retained floor through an old bundle | Reject each variant; rollback and restart never restore older security policy. |
| Required subprocess sandbox disabled, initialization fails, or runtime evidence disagrees with configuration | Fail qualification on each platform; include null Windows `sandbox_info`, macOS helper initialization failure, and unavailable required Linux sandbox mechanisms. |
| All required subprocess probes pass with the documented privileged browser broker | Pass the sandbox gate without claiming that the browser itself is sandboxed. |

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
- [CEF sandbox setup and revision-dependent platform requirements](https://github.com/chromiumembedded/cef/blob/master/docs/sandbox_setup.md)
- [CEF Windows sandbox API](https://github.com/chromiumembedded/cef/blob/master/include/cef_sandbox_win.h)
- [CEF macOS sandbox API](https://github.com/chromiumembedded/cef/blob/master/include/cef_sandbox_mac.h)
- [Chromium sandbox and privileged browser broker](https://chromium.googlesource.com/chromium/src/+/main/docs/design/sandbox.md)
- [Chromium Linux sandbox mechanisms](https://chromium.googlesource.com/chromium/src/+/HEAD/sandbox/linux/README.md)
- [WebKitGTK stable API reference](https://webkitgtk.org/reference/webkitgtk/stable/)
- [Migrating WebKitGTK applications to GTK 4 / WebKitGTK 6.0](https://webkitgtk.org/reference/webkitgtk/stable/migrating-to-webkitgtk-6.0.html)
- [Microsoft WebView2 runtime distribution](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/distribution)
- [Apple WKWebView](https://developer.apple.com/documentation/webkit/wkwebview)
