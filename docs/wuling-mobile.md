# Wuling Mobile Presentation Profile

Status: proposed RFC / contract preview; no shipping mobile native host or qualified platform

Reference review date: 2026-09-23

## Recommendation and scope

Start with a **remote-execution-first preview**: observe task sessions, read logs and Diff, keep
drafts, annotate, approve authorized requests, and explicitly resume input on a remote workspace.
Validate the remote workflow in Web/PWA first, as requested by [Wuling ADE #69][ade]. Mobile is a
separate presentation profile, not an extension of Desktop's renderer support matrix.

Recommend a small SwiftUI/UIKit + WKWebView evaluation for the iOS native shell, compared against
the existing React Native/Chen direction using the same session scenarios. This is a recommendation
for evaluation, **not an implementation selection**. No native host, device support, SDK test result,
or App Store acceptance follows from this document. The accepted [Desktop architecture](kunlun-desktop.md)
retains its pinned CEF reference backend and qualification gates; it implies no iOS CEF support.
This work is independent of M3 server compatibility.

Wuling owns ADE application/session authorization and workflow. Qingting owns its standalone
Core/CLI/SDK and provider integration; a remotely running CLI/SDK must be usable without any GUI,
and ADE attaches as a consumer, not its mandatory launcher. The [runtime #43 clarification][runtime]
specifies Rust for Qingting, while FFI versus IPC (or both), message schemas, and protocol ownership
remain open. Do not assume ACP, a JavaScript SDK wrapper, or an in-process mobile Agent. Reusing
TypeScript session reducers and application state does not select a Qingting binding or transport.

## Host options

The public [Chen README][chen] documents Mobile (React Native / Expo) scaffolding, a
`chen-the-dawnstreak/native` entry point, React Navigation integration, shared data/state hooks,
and native form bindings. That establishes an existing direction, not evidence of an ADE iOS
host, native material parity, or a qualified foldable experience. Its Web/Desktop scaffolds do
not change the runtime repository's accepted Desktop backend.

| Consideration | SwiftUI/UIKit + WKWebView | React Native / Chen |
| --- | --- | --- |
| Native shell | Direct ownership of scenes, navigation, safe areas, keyboard guides, system controls and materials. UIKit interoperation remains necessary where SwiftUI alone is insufficient. | Reuses React/TypeScript navigation and state patterns; requires a reviewed native adapter for platform-specific geometry, lifecycle, materials and credentials. Chen alone is not that adapter. |
| Existing ADE content | WKWebView can render shared web log/Diff/editor content; a narrow authenticated host bridge still needs implementation and qualification. | Shared pure TypeScript state is valuable, but DOM/CSS components are not native views. Choose native implementations or separately qualify embedded web surfaces; do not promise automatic UI reuse. |
| Accessibility and text | Must integrate native and web accessibility/focus trees and preserve text composition across their boundary. | Native accessibility and text APIs exist, but native modules, virtualized content and any WebView introduce their own focus/IME seams. |
| Lifecycle | Platform scene state and system-managed WebKit processes; web content termination is recoverable, not an application-state store. | React Native AppState distinguishes active, inactive and background; JS state alone is not durable restoration or permission to keep running. |
| Maintenance | Apple-only shell plus shared web/state code; OS WebKit updates expand the qualification matrix. | Broader cross-platform reuse; React Native, Chen, navigation, modules and any Expo/WebView dependencies add compatibility and release work. |
| Decision evidence | Measure real native/web seams, accessibility, IME, restoration, size and memory. | Run the same scenarios and measure reuse and module maintenance rather than assuming lower cost. |

The native Apple evaluation is preferable for testing the most platform-sensitive requirements
directly. Retain React Native/Chen as a viable candidate; final selection requires measured
results and product review. Android would require its own backend, lifecycle and distribution
qualification and is not qualified by either candidate's iOS results.

### Web/PWA baseline before native selection

The first remote-workflow lane is mobile Safari and an installed Web/PWA where the platform
supports installation; qualify these contexts separately. It uses browser authentication and
consumer-authorized remote session APIs, not the native bridge, Keychain adapter, native materials
or local execution. Complete its attach/observe/draft/approve/cancel/reconnect scenarios before
using the native comparison to select a host.

Use bounded account-scoped browser storage; clear private caches on sign-out/account switch.
Never cache provider credentials or restore grants/write leases from storage. A service worker,
if used, cannot queue commands or approvals for background replay; scope/version its assets,
isolate private responses and prove update/cache migration cannot restore another account's state.
Test browser/device-auth return, browser process death, suspension, expired sessions, network loss,
keyboard/viewport changes and cold restoration. Missing required browser capabilities are explicit
baseline gaps, not evidence supplied by an unrelated native candidate.

## Proposed capability profile

“Included” below means required behavior for a future preview, **not implemented support**.
Negotiate a versioned profile and explicit capabilities with the host and remote service.
Unknown, unavailable or denied operations fail closed; a hidden UI control is not authorization.

| Capability | Preview boundary |
| --- | --- |
| Session attach/detach and read-only subscription | Included, authenticated and scoped to account, organization, workspace and session; remote execution outlives the phone UI. |
| Logs, command blocks, Diff and annotations | Included with bounded streams, sequence recovery and server-side authorization. Cached content is explicitly stale/offline. |
| Command submission and PTY input/resize | Remote only, under the current server-issued exclusive write lease. Read-only observers cannot resize the shared PTY. Resize uses measured terminal cells, coalesces layout changes and never submits input. |
| Cancel, pause and approval | Explicit remote requests only where advertised and authorized. Pause is unsupported if the service has no defined pause operation; disconnect is neither pause nor cancellation. |
| Credentials and device authorization | Native trusted interaction and scoped short-lived references; no provider refresh tokens in presentation state. |
| Drafts, navigation and restoration | Included as bounded, versioned app-private data subject to account isolation and retention policy; never restore authority from a saved UI snapshot. |
| Files and sharing | Remote workspace access through explicit service grants. Optional user-selected import/export or OS share UI requires separate grants and qualification, not arbitrary local filesystem access. |
| Deep links and external navigation | Validate routes and destinations in the host; resolve opaque, expiring references after authentication and confirmation as needed. No token-bearing URLs or command execution on link open. |
| Local shell, PTY, arbitrary Actions, subprocesses or native addons | Unsupported. A desktop grant cannot be relabeled as a mobile capability. |
| App-provided JIT or persistent background Runner | Unsupported; no promise to embed the Desktop Kunlun/JSC service or keep the mobile process/network stream alive indefinitely. |
| CEF on iOS, Desktop IPC endpoints or production inspection | Unsupported by this profile. Browser inspection in a future development build would require separate explicit policy. |

The native adapter is a privileged boundary even in a React Native application. Do not expose
general native-module invocation, arbitrary fetch-to-native proxies, secrets, files, or process
authority to presentation content. WKWebView bridges must validate source/frame identity,
application/session binding, schema version, size, grant scope and expiry on every call.
Use narrowly allowed destinations, navigation and message operations, bounded queues, and redacted
audits. App assets and remote content must have explicit origin/CSP policy; untrusted log or Diff
content cannot become bridge-authorized script. Native capability denial remains authoritative.

## Session continuity and lifecycle

Consume the versioned session contract owned by [Wuling ADE][ade], rather than defining a second
mobile event protocol. Its required semantics include stable session/workspace/task/command
identities, ordered event sequences, cancellation, read-only subscriptions and one active input
lease. Agree the actual wire schema with Qingting/Wuling before implementing the adapter.

- On attach, authenticate and negotiate versions/capabilities before receiving scoped state.
  Resume from the last applied sequence; discard duplicates, detect gaps, and request a bounded
  snapshot/resynchronization when retained history is unavailable. Do not silently omit output.
- Bound stream bytes, event sizes and queued work. Apply backpressure while visible; disconnect
  or suspend subscriptions when necessary rather than buffering an unbounded background log.
  Remote retention limits must be visible when they prevent complete recovery.
- Inactive, background, scene disconnect, network loss, memory pressure and termination are normal
  states. Persist draft/navigation checkpoints incrementally, not only in a final callback that
  may never arrive. Apple can suspend or terminate the app; OS background opportunities are
  time-limited and purpose-specific, not a Runner contract.
- On loss of foreground or input ownership, stop sending input and best-effort release the lease;
  the server must also enforce expiry when release cannot arrive. Restore read-only first and
  require explicit user intent plus a fresh valid lease to resume input.
- A remote task continues under server policy when the phone disconnects or closes. Cancellation
  is an acknowledged remote command with server-side cleanup, not “close the socket.” Show
  pending/unknown outcomes honestly and reconcile status after reconnect.
- Reconnect, process restart, credential renewal, renderer recovery and updates never replay
  submitted commands or approvals automatically. Keep unsent drafts separate from submitted
  requests; reconcile uncertain results by request identity before offering a new user action.
- A WebKit content-process crash or React tree remount must not own or destroy the session.
  Restore presentation from app-owned state and remote sequence history, with no inherited bridge
  grants or input lease. Offline mode permits explicit cached reads/draft edits, not silently
  queued execution.

## Adaptive geometry, input and restoration

### Effective viewport is the source of layout

Compute the usable content region from the **current scene/view bounds**, native chrome,
top/right/bottom/left safe-area insets, keyboard overlap, and any actual reported occlusion.
Keep coordinate spaces explicit: native points, web CSS pixels, visual-viewport offsets/scale,
and terminal cells are not interchangeable. Transform intersections before applying them.
Assign one owner for each inset so the native shell and web content do not subtract it twice.
Safe-area values are view-relative and may be zero before attachment; remeasure after layout.
Do not substitute physical screen size for the app's effective viewport.

Fold/unfold, outer/inner display transitions, Split View, rotation, window resize, keyboard
movement and Dynamic Type changes all trigger the same adaptive-layout path. Use a compact
single-task view when space is tight and list/detail or log/Diff panes only when content minimums,
text size and touch targets fit. Collapse panes without destroying their state. Split View is
tested wherever the target platform exposes it, not assumed for every iPhone/OS combination.
Capabilities and measured available space select layout; device-name tables do not.

Only avoid a physical occlusion when real platform data identifies one. A fold, posture hint,
wide aspect ratio or a marketing image is not evidence of an unusable strip. Never invent a hinge
gap in a continuous display. If segment/occlusion APIs are absent, use the ordinary continuous
viewport and safe areas; record that limitation rather than synthesizing geometry. Detect API
availability and validate returned rectangles at runtime. Chrome's [Viewport Segments API][segments]
documentation establishes Chrome behavior, **not Safari or WKWebView support**. Do not infer
`window.viewport.segments` support in either Apple engine context, or treat web visual viewport
data alone as a physical-occlusion report.

### Text, keyboard and selection

Use current platform keyboard/layout data (for example UIKit's [keyboard layout guide][keyboard])
to keep the composer, caret, selection handles and actions reachable. Test software, hardware,
floating and undocked keyboards where available; do not assume every keyboard is a full-width
bottom inset. Web visual-viewport updates can complement native geometry but need coordinate
conversion and deduplication. Scrolling an obscured editor is preferable to destroying it.

Preserve Chinese IME marked/composing text during ordinary geometry transitions. Enter used to
confirm a candidate must not submit a command. Do not remount the editor on fold/rotation/resize,
rewrite its value mid-composition, or derive PTY input from layout events. Qualify candidate
selection, punctuation, selection replacement, hardware-keyboard shortcuts and focus movement
across native/web boundaries. Do not promise to serialize OS-owned IME candidate state across
process death; preserve committed drafts and report any unavailable transient composition.

Honor Dynamic Type/text scaling, large accessibility sizes, contrast, touch targets and readable
code/log presentation. Reflow controls and allow intentional code scrolling rather than clipping
actions or forcing tiny text. Font metrics changes can require a remote PTY resize only when the
client owns the write lease.

### State survives presentation changes

Keep session identity, unsent draft, selected task/command/Diff, filters, pane choice, scroll
anchor, text selection and focus target outside layout-specific views. Use stable content
identifiers and revision-aware text ranges rather than screen coordinates or DOM-node pointers.
On reflow restore the logical selection and reading anchor; if remote content changed, reconcile
or visibly invalidate an unavailable range instead of selecting unrelated text.

Use a versioned, bounded, account-scoped checkpoint and platform restoration mechanisms as
complements, not substitutes. Restoration is best effort after OS termination, not guaranteed by
the scene API. Validate schema/account/session availability, reauthenticate and obtain a fresh
snapshot before resuming. Do not persist provider secrets, capability grants, live leases or
command-send queues in restoration data. On sign-out or account switch clear sensitive cached
content and isolate drafts according to retention policy. Test both scene restoration and a
cold start with missing, corrupt, outdated or revoked state.

### iPhone Duo qualification

[Apple's announcement][duo] is product context, not evidence that this application works on the
device or that a particular geometry API exists. Record exact Xcode/SDK version, simulator runtime,
available device profile, host build and observed behavior for any SDK/simulator experiment.
A generic resize/fold simulation is useful contract evidence, but must be labeled simulated and
must not be reported as an iPhone Duo run if that SDK/device profile is unavailable.

Eventual **physical iPhone Duo qualification is a separate gate**: record hardware, OS build,
outer/inner transitions, touch, real safe areas, keyboard/IME, thermal/memory behavior, accessibility
and lifecycle observations. No such runs are recorded here. Simulator success cannot close this
gate, and no unverified device dimensions, hinge geometry, Safari features or new Apple API names
are part of this contract.

## Accessibility and material boundary

Native navigation, chrome and optional native controls own platform materials and their
accessibility behavior. Web content owns semantic DOM, content structure, selection and accessible
names; a native wrapper does not automatically make an editor or terminal accessible. React Native
must meet the same outcome through its accessibility APIs and any native adapters. Verify one
coherent VoiceOver reading/focus order across surfaces, keyboard traversal, Switch Control,
labels, selected/disabled/error states and announcements. Streamed logs must not flood speech or
steal focus. Provide accessible command-block/text alternatives to a visual terminal when needed.

Follow [Wuling's design boundary][design]: brand color, geometry and typography belong in content;
code, logs and Diff remain on readable solid surfaces. CSS blur/glass is a visual fallback, not
native Liquid Glass or evidence of platform integration. Respect reduced motion, reduced
transparency, increased contrast and appearance changes in both native and web layers. Native
material use never excuses poor text contrast or obscured controls.

## Credentials and independent remote execution

Wuling identity and provider connections are distinct. The consumer displays connection source,
model, organization/budget, pending approval, device-authorization progress, expiry/revocation
and reauthentication status without receiving provider refresh tokens. Qingting owns and
qualifies provider access, including subscription integrations; Keychain storage alone proves
neither entitlement nor provider compatibility.

Use trusted native/system authorization interaction and broker-issued opaque short-lived
references bound to audience, account, session and allowed operations. Validate expiry/revocation
on use and redact references from logs. Deep links may carry only narrowly scoped short-lived
handoff references, never credentials or executable authority; validate routing and user context
before resolving them. OS secure storage, if used for the native client's own authentication,
is accessed only by the trusted adapter, not renderer scripts or generic JS modules.

Revocation, account changes, remote authorization denial, app crash and restore invalidate local
handles and stop privileged traffic. Reauthentication does not silently restore a write lease
or execute pending approvals. The remote Qingting Core/CLI/SDK owns its execution and provider
credential lifecycle independently of ADE attach/detach. Transport authentication, framing,
backpressure and cancellation still require review; choosing remote execution does not claim
that the open Qingting protocol is already implemented.

## Distribution and updates

Mobile follows its approved **platform distribution mechanism**, not the Desktop CEF updater.
Record the selected channel, signing, entitlements, minimum OS, privacy declarations and applicable
review requirements before release. App Store/TestFlight or other eligible platform channels
must be evaluated under their current rules; no alternate-channel entitlement or approval is
assumed. Apple's [App Review Guidelines][review], including executable code, background services
and browser-engine requirements, are release inputs, not a promise of approval.

Do not use Wuling Updatemgr to hot-swap a native host, inject arbitrary executable Actions, bypass
platform signing, or implement a private native rollback channel. A consumer adapter may expose
common release metadata, preflight/status and an approved update destination. Remote services
update independently and negotiate compatibility; external deployment owners handle Compose/K8s
updates outside application containers.

WKWebView follows platform WebKit rather than the pinned Desktop CEF build. Record OS/engine
information available through supported APIs and qualify required web features on each supported
OS; never invent an exact WebKit build number. RN/Chen dependencies and native modules need a
locked, reproducible release record. Platform or dependency updates require regression gates.
An unsupported security baseline or protocol combination must disable affected privileged
operations and direct users to an approved update, without resurrecting revoked credentials,
restoring stale authority or replaying commands. Desktop's independent security metadata,
minimum-safe CEF floor and rollback policy remain unchanged.

## Qualification checklist and evidence

All platform gates below are **unrun / unqualified**. This RFC and desktop contract fixtures are
not mobile device test evidence. For each future run retain host/candidate commit, SDK/toolchain,
OS/runtime, hardware or simulator identity, channel, capability observations, test steps and
artifacts. Separate automated contract results, simulated geometry, simulator UI tests and
physical-device results; never merge them into a single “iOS passed” claim.

- [ ] Compare both host candidates on the same ADE scenario; publish maintenance/reuse,
  startup/memory, input and accessibility findings before choosing an implementation.
- [ ] Verify authorization denial, unknown capabilities, expired/revoked references, malicious
  navigation/bridge requests, account isolation and secret/log redaction on the native boundary.
- [ ] Attach to an independently running Qingting session; detach/close the app without stopping
  the task. Prove ordered resume, duplicates/gaps, bounded backpressure and read-only behavior.
- [ ] Test write-lease contention across devices, remote PTY resize ownership, cancel outcome
  reconciliation, dropped acknowledgements and reconnect/update without command replay.
- [ ] Test 375-CSS-pixel compact content, tablet/wide content, rotation, resize, Split View where
  supported, all four safe areas, and keyboard overlap without double insets.
- [ ] Simulate fold/unfold and display transitions with absent/present reported segments and
  occlusion. Preserve session/draft/selection and never invent an occluded hinge.
- [ ] Record actual SDK/simulator availability and evidence separately for iPhone Duo scenarios;
  leave unsupported simulator scenarios explicitly blocked, not passed.
- [ ] Complete eventual physical iPhone Duo qualification separately; no current device claim.
- [ ] Test Chinese IME, software/hardware keyboards, candidate confirmation, selection/caret,
  floating keyboards where available, Dynamic Type and focus across native/web seams.
- [ ] Test VoiceOver, Switch Control, keyboard traversal, semantic trees, reduced motion and
  transparency, contrast and solid log/Diff surfaces; include live-stream announcement behavior.
- [ ] Exercise inactive/background/suspension/termination, network changes, memory pressure,
  web-content crash, scene discard and cold launch; verify durable drafts and fresh authorization.
- [ ] Test restoration migration, missing/corrupt data, revoked sessions, account switching and
  retention cleanup without persisting authority or transient IME state promises.
- [ ] Complete the Safari and supported installed-PWA baseline above before native selection;
  record authentication, storage/cache isolation, update, suspension and remote-session evidence.
- [ ] Qualify required web features independently in WKWebView and Safari/PWA; Chrome
  viewport-segment results do not count for either. Record unsupported features and fallbacks.
- [ ] Qualify signed distribution/update flow, current platform policies, service compatibility
  and safe failure. Unavailable channels, minimum OS decisions and provider approvals stay open.

## Primary references

Project direction and ownership:

- [Runtime #43, including the Rust/protocol clarification][runtime]
- [Wuling ADE session/mobile consumer #69][ade]
- [Wuling UI/material boundary #64][design]
- [Qingting independent CLI/SDK #70](https://github.com/zixiao-labs/Wuling-DevOps/issues/70)
- [Wuling authorization #73](https://github.com/zixiao-labs/Wuling-DevOps/issues/73)
- [Wuling update adapters #75](https://github.com/zixiao-labs/Wuling-DevOps/issues/75)
- [Chen public README and native direction][chen]

Platform contracts (consulted for this preview; these are not qualification records):

- [Apple WKWebView](https://developer.apple.com/documentation/webkit/wkwebview)
- [Apple app lifecycle](https://developer.apple.com/documentation/uikit/managing-your-app-s-life-cycle)
- [Apple safeAreaInsets][safearea]
- [Apple UIKeyboardLayoutGuide][keyboard]
- [Apple state restoration](https://developer.apple.com/documentation/uikit/restoring-your-app-s-state)
- [React Native AppState](https://reactnative.dev/docs/appstate)
- [React Native AccessibilityInfo](https://reactnative.dev/docs/accessibilityinfo)
- [Chrome Viewport Segments API][segments]
- [Apple iPhone Duo announcement][duo]
- [Apple App Review Guidelines][review]

[runtime]: https://github.com/kunlunengine/runtime/issues/43
[ade]: https://github.com/zixiao-labs/Wuling-DevOps/issues/69
[design]: https://github.com/zixiao-labs/Wuling-DevOps/issues/64
[chen]: https://github.com/zixiao-labs/Chen-the-Dawnstreak#react-native
[safearea]: https://developer.apple.com/documentation/uikit/uiview/safeareainsets
[keyboard]: https://developer.apple.com/documentation/uikit/uikeyboardlayoutguide
[segments]: https://developer.chrome.com/blog/viewport-segments-api-shipped
[duo]: https://www.apple.com/newsroom/2026/09/apple-unveils-iphone-duo/
[review]: https://developer.apple.com/app-store/review/guidelines/
