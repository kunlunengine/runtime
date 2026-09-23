# Wuling ADE host contract v1

Status: **preview RFC and executable contract model**, not a qualified Desktop host.

Scope: [runtime #43](https://github.com/kunlunengine/runtime/issues/43), including its
credential/update and Rust CLI/SDK clarifications. Consumer: 武陵 / Wuling
[#63](https://github.com/zixiao-labs/Wuling-DevOps/issues/63) and
[#69](https://github.com/zixiao-labs/Wuling-DevOps/issues/69). Source review: 2026-09-23.

## Decision and ownership

Extend, do not replace, the accepted [Desktop architecture](./kunlun-desktop.md). Pinned
CEF/Chromium remains the sole initial Desktop reference renderer; Kunlun/JSC and DevTools remain
separate services. The renderer has no ambient process, filesystem, socket, secret, or native
authority. Desktop is an **optional presentation host**, not Qingting's owner or required launcher.
A separately running Qingting CLI/SDK and cloud/headless workers must remain useful with no GUI.

| Owner | Responsibility | Not delegated to Desktop |
| --- | --- | --- |
| Wuling ADE | Session/task/command semantics, read-only subscriptions, write leases, review/diff, cross-device UI state | Application authorization is not replaced by a local user grant |
| Qingting Core/CLI/SDK | Rust execution, cancellation/recovery, approvals, provider authentication and model capability qualification | Desktop secure storage does not grant ChatGPT subscription access |
| Kunlun host | Authenticated renderer boundary, explicit user grants, native resources, containment, lifecycle and platform qualification | Does not implement the Agent or choose its wire protocol |
| Updatemgr / deployment owner | Common release metadata, preflight/drain/status and deployment adapters | Cannot override native update security; Compose/K8s updates run outside application containers |

The retired `wuling-agent` is not a baseline. At review time Wuling main
[`602946082f443a68d3711c5117942583cb4df2dc`](https://github.com/zixiao-labs/Wuling-DevOps/tree/602946082f443a68d3711c5117942583cb4df2dc)
does not contain a frozen ADE/Qingting session schema. Issues #69, #70, #73 and #75 specify
requirements, not implemented protocol APIs. Consequently the checked-in schema below is a
**Kunlun-side normalization proposal**, not a new authoritative Wuling protocol or evidence of
cross-repository conformance. Pin the reviewed upstream schema revision and add bidirectional
mapping tests before advertising interoperability. Unknown upstream versions fail closed.

### Rust integration: compare boundaries before choosing one

The [maintainer clarification](https://github.com/kunlunengine/runtime/issues/43#issuecomment-5759313429)
selects Rust for Qingting CLI/SDK, **not** FFI, IPC, ACP, or a mandatory JavaScript SDK wrapper.

| Option | Benefits | Obligations / cost |
| --- | --- | --- |
| In-process Rust SDK or narrow C ABI in the trusted service | Owned typed calls, fewer copies, straightforward local embedding | Pin crate/ABI compatibility; define allocation/free ownership, callback lifetime/thread affinity, async cancellation and shutdown; contain panics before any C ABI; native faults share the service failure domain |
| Out-of-process CLI/service IPC | Independent lifetime, attach/detach and fault containment; fits headless and remote execution | Authenticate endpoints and peers, length-limit framing before decode, negotiate versions, bound queues, enforce deadlines, reconnect/resync and uncertain-command handling |
| Both through a normalized adapter | One consumer state machine across local SDK and independent service | Two bindings to test; neither may silently fall back or weaken authority |

Recommend proving the independently running service/CLI attach workflow first; leave transport and
protocol selection to the joint Qingting/Wuling review. The renderer-to-host boundary is always
authenticated, capability-checked IPC, even if a trusted service later embeds a Rust SDK. No Rust
pointer, JSC handle, callback, allocator ownership or borrowed buffer crosses the renderer channel.
IPC authentication is not provided by a session ID, a random localhost port, or possession of a
deep link. Local launch uses a protected bootstrap channel/OS peer identity; remote sessions use
the consumer's authenticated transport and audience-scoped delegation.

## Versioned schemas and negotiation

The executable schema source is [`fixtures/wuling-host/contract.rs`](../fixtures/wuling-host/contract.rs);
the JSON consumer conversation lives alongside it. Serde tags and field types define the preview
wire vocabulary. Tests reject unknown fields/operations and incompatible versions. This schema is
not exported from the runtime built-in module/type package.

| Preview schema | Meaning |
| --- | --- |
| `Request` | `protocol: "kunlun_host_preview_v1"`, `profile: "desktop" \| "mobile_remote"`, `request_id`, claimed `binding`, opaque `grant`/`resource`, lease generation and tagged `operation` |
| `Binding` | Application, window, session and renderer connection epoch; compared against the host-supplied peer, never trusted from the message |
| `Operation` | Closed operation union; local start/open consumes a previously resolved resource reference, not arbitrary executable/path/credential arguments |
| `Frame` / `SessionEvent` | Session and history generation, sequence, tagged output/command completion/approval/auth status/terminal observation |
| `Auth` | Pending, authorized, denied, expired, cancelled, reauthorization-required or revoked; no secret-bearing fields |
| `Conversation` | One request and bounded observed events, used as a serialized consumer example, not a Qingting framing protocol |

The small fixture fixes limits at 4,096 bytes per decoded envelope, 1,024 bytes per payload,
four queued stream frames / 4,096 serialized event bytes, with one frame / 1,024 bytes reserved
for control. PTY dimensions are 1–500 columns and 1–300 rows. These are deliberately small test
budgets, not qualified product limits. Stream ACKs are cumulative byte offsets at sent event
boundaries, scoped to one authenticated stream/epoch, and cannot mint additional credit.

The model separates host consent/authorization, session observation, resource state and update
policy decisions; it is not a dispatch loop. Consent binds exact operation parameters in the
fixture. OS resource resolution, trusted consumer snapshots/lease arbitration and actual
authentication are test inputs. A production operation handler must compose these checks and
revalidate at commit; a broker authorization alone is not provider access or native execution.
Negotiation, response/error envelopes, native resource resolution and upstream schema mapping
remain implementation gates, not APIs implemented by this fixture.

The production binding must negotiate host schema version, exact consumer schema revision, profile,
operation set, message/stream limits and recovery capabilities before attaching. Desktop and
mobile advertise distinct profiles; absence means unsupported, not an emulation promise. An
incompatible major version or required feature denies attach. Optional additions require explicit
negotiation, not accepting an arbitrary command tag.

All requests carry correlation and session context and an opaque grant reference. The host compares
that context with the **authenticated peer**; claimed application/window/session fields never
establish identity. Broker-side state binds a grant to:

- application and secure origin/profile, native window, session and connection epoch;
- exact resource and permitted operations (no transitive process/file/network authority);
- issuing user/delegation, purpose, expiry, revocation state and audit correlation;
- consumer authorization/approval and the current input lease for command submission and PTY
  input/resize; other operations have their own permissions and any resource-specific fencing.

Resource, provider-connection, approval and deep-link references are opaque, scoped, short-lived
handles, not paths, credentials or URLs that confer blanket authority. A valid local grant and a
valid consumer operation permission are **both** necessary, not interchangeable. The exclusive
input lease only arbitrates command/terminal input. It neither grants unrelated mutation authority
nor prevents a separately authorized observer from approving, cancelling or revoking a connection
when there is no input owner. A read-only subscription alone permits none of those mutations.

### Operation surface

| Capability family | Requests and responses | Mandatory broker checks |
| --- | --- | --- |
| Session | Attach/detach, snapshot/resume, events, command status, cancel, approval decision | Identity/audience/session, schema, sequence and operation permission; command submission also needs the input lease; cancel/approval have independent authorization |
| PTY / subprocess | Start an explicitly approved command, input, resize, cancel, exit/cleanup status | Executable/argv/cwd/env policy and filesystem handles; PTY ownership, dimensions, lease, limits, process containment |
| Filesystem | Select root/file through native consent UI, open/read/write/stat scoped handles | Requested operation and root; no arbitrary native path access or scope widening |
| Provider connection | Request native auth interaction, observe device-auth/re-auth status, use/revoke connection reference | Account/provider/workspace/audience/scope, expiry, delegation, provider state; never export a refresh token |
| Window / lifecycle | Request focus, observe focus/background/close/crash, native menu action | Owning window and permitted action; an untrusted focus request cannot steal input ownership |
| Deep link | Resolve an allowlisted link into an intent preview, then explicit attach/navigation | Scheme/host/action allowlist, nonce, expiry, authenticated user/audience, single use and confirmation |
| Streams | Subscribe, data/sequence, credit/ack, terminal/gap/resync | Session authorization, exact stream, byte/frame limits and valid acknowledgement range |
| Update adapter | Read release/status, request preflight/drain/stage/activate via trusted native updater | User/admin policy plus all independently enforced native security gates below |

An approval binds the exact operation, target, parameters digest and expiry. A review of one diff
does not authorize a changed patch. Deep links never run a command, approve a mutation, populate a
terminal for automatic submission, or carry provider credentials. Re-resolve and reauthorize their
intent after login; never restore authority from a saved link.

### Authorization order and denial

For **every** privileged operation, including resize, credit, cancel and status:

1. Enforce transport frame and decode limits; validate schema/profile and authenticated peer.
2. Resolve the handle in that peer's current epoch; check binding, operation, expiry and revocation.
3. Check current consumer operation-specific read/write/approval policy. For command/terminal
   input, compare input-lease owner, generation and expiry at dispatch. A workspace write fence
   or approval is separate from this lease; none is replaced by a renderer Boolean.
4. For interactive input/resize, require the active input owner and foreground focus. Validate
   operation-specific payload limits before touching native resources.
5. Recheck authority at side-effect commit after asynchronous work; close/cancel affected resources
   if revocation won the race. Return a bounded result and an audit decision.

Use stable typed denial categories: incompatible/unsupported, malformed/limit, denied/expired/revoked,
stale epoch/lease, read-only/not-input-owner, resync-required and outcome-unknown. A denial must not
include secret values, native paths outside the grant, or evidence that leaks another session's
existence. Audit operation/resource class, correlation, decision and policy generation; redact
tokens, auth user codes, terminal text, environment, file contents and deep-link query values.
Rate-limit denials and consent prompts; the renderer cannot issue its own grants.

### Renderer egress and untrusted content

The Wuling profile defaults renderer networking to **deny**: `connect-src 'none'`, no remote
subresources, form submission, unrestricted navigation or worker/service-worker registration.
Host enforcement must cover Chromium's browser/network service as well as renderer APIs:
fetch/XHR, WebSocket, WebRTC, DNS-mediated requests, redirects, downloads and localhost/private
network targets. A CSP header is defense in depth, not a substitute for network-service policy.
Remote session/provider traffic goes through the authenticated trusted service; a separately
approved external browser login is not renderer egress permission. If a later profile needs
direct network access, version an explicit origin/method/resource allowlist and qualify it.
Test denied public/loopback/private targets, redirects and alternate APIs in the packaged build.

Treat repository files, terminal/log text, Markdown and Diff as hostile data. Render escaped text
or a reviewed sanitizer's restricted output; never inject it into script-capable HTML or unsafe
DOM sinks. Allowlist terminal control sequences; suppress clipboard writes, title changes,
automatic links/downloads and other escapes unless a separate user action and broker grant allow
them. Preview content needing executable HTML runs in a separate unprivileged origin with no
bridge, grants or application session storage. Test XSS/Markdown/link/control-sequence payloads:
a signed application origin or CSP alone does not make its embedded content trustworthy.

### Filesystem and credential implementation boundary

The fixture uses resource handles and does **not** implement path containment or a credential vault.
The native adapter must obtain grants via explicit user interaction, pin root identity, resolve
relative paths beneath the opened directory, and prevent symlink/reparse-point/rename and TOCTOU
escapes. Do not authorize a string prefix and then reopen an unchecked absolute path. Separate read,
write, create and delete authority; deny executable/plugin loading and network shares unless
explicitly qualified. Revocation stops new opens and closes/invalidate outstanding handles.

Use an OS secure-storage adapter (for example macOS Keychain) only in the trusted process, with
access-control and lock/unlock handling tested per platform. Qingting owns provider exchanges,
refresh concurrency and provider terms/capability qualification; a successful vault write is not
successful provider authorization. The renderer observes redacted states such as pending,
authorized, denied, expired, cancelled, re-authentication-required or revoked through bounded
session/auth events. Device codes and verification interactions belong in trusted UI; the host
validates the provider destination before opening it. Public CI never inherits a user's subscription
refresh token. An authorization completion for an obsolete attempt/epoch cannot resurrect a revoked
connection. A fresh login creates a new connection scope.

Revocation blocks new provider use/refresh immediately at the responsible authority, invalidates
local references and cancels pending work. Disconnected clients remain without authority until
status is refreshed; a cached "authorized" UI is not permission. Native secret deletion, remote
provider revocation and cancellation are separate results: report partial failure without granting
continued access.

## Session adapter and input ownership

Wuling owns session/workspace/task/command identities, durable command outcomes and sequencing.
The host normalizes those events without renumbering history or deriving authority from UI state.

1. Attach to the independently running CLI/SDK or remote task as **read-only**. Obtain a snapshot
   and its cursor, then accept contiguous events for that same session/history epoch.
2. Ignore already-applied sequence numbers. A gap, incompatible history epoch or expired retention
   cursor freezes incremental application and requests a fresh authoritative snapshot. No speculative
   application across gaps. Only an authenticated snapshot may reset the cursor.
3. A user explicitly requests a writer lease. The consumer arbitrates exactly one input owner per
   session across Desktop, CLI and phones. Transfer/revoke/expiry fences the old owner before the new
   writer acts. Local focus alone does not mint a lease.
4. Dispatch a user command once with its consumer identity/idempotency context. After disconnect,
   query its status; an unknown outcome remains **unknown**, never automatically retried. Sequence
   resume replays observations, not executed commands, approvals or terminal input.
5. Cancel targets the original task/command and is idempotent. "Cancel requested" is not "cancelled":
   accept a racing successful completion once, retain terminal outcomes, and report cleanup separately.
   Cancellation also stops new side effects/credential refresh according to consumer policy.
6. Detach or GUI exit drops subscriptions and local grants, not the independently owned Qingting
   task or its provider credentials. Explicit remote cancellation is a separate authorized action.

Draft, selected diff/range, filter and scroll/read position are presentation state keyed by stable
session/document identifiers, not queue entries to resubmit. Preserve them across resize/reconnect,
but mark stale selection against a changed revision. Restoring a draft never presses Enter.
Read-only viewers may observe state and request a new lease but cannot input, resize the shared PTY,
apply a diff or approve by reusing an old writer reference.

### PTY, cancellation and lifecycle

| Transition | Required behavior |
| --- | --- |
| PTY start | Separate local host-owned process groups from independently owned Qingting tasks; obtain explicit execution grant. No shell-string interpretation unless that exact shell invocation was approved. Filter environment and inherited descriptors. |
| Resize | Positive bounded rows/columns for the owned PTY; reject zero/oversized or stale-owner requests. Coalesce pending resizes to the latest geometry without reordering input. |
| Input | One owner; bounded bytes; do not send incomplete IME composition as a command. Enter during composition commits text, not an Agent action. |
| Background / focus loss | Clear local input ownership immediately; disable input/resize, clear unsent input, release or explicitly renew the consumer lease. Do not cancel a remote task merely because its window lost focus. |
| Foreground | Refresh auth/lease/session status, resync outputs; explicit reacquisition before writing. Never flush queued keystrokes. |
| Local cancel / grant revoke | Close input, request graceful process-group termination, enforce a bounded deadline then kill the contained tree, reap children, close PTY/streams. Report exit and cleanup completion separately. |
| Renderer crash / navigation / detach | Revoke its epoch/grants, abandon unsent mutations, clean up its host-owned children; independently owned Qingting continues. Recreate only a read-only presentation after authentication. |
| Host crash / restart | Durable execution supervisor/OS containment cleans up host-owned children; a new host epoch invalidates all renderer handles. Reload durable security/revocation state, then query remote outcomes and reconnect read-only. |
| Service crash | Present outcome-unknown and reconnect/status query; do not re-execute the last command. A failed cleanup is visible and blocks resource reuse, not a false success. |
| Update drain | Stop new mutations, checkpoint UI/read cursor, release leases and clean up local children before activation. Preserve independent remote execution; never serialize renderer grants into restoration state. |

POSIX process groups alone do not contain a daemon that escapes the group. Qualify an OS
supervisor/containment policy that prevents or accounts for escape; Windows needs the corresponding
Job Object/process-tree policy. Test grandchildren, forced termination, ignored signals and host
death. The pure fixture proves transition decisions only, not native kill/reap or crash containment.

Cleanup containment is also **not a resource sandbox**. Local commands must execute in a qualified
OS-sandboxed worker/container profile that exposes only approved filesystem roots, scoped network
destinations and required IPC endpoints, prevents ambient credential/home-directory access, and
propagates restrictions to descendants. A child with the broker user's unrestricted OS identity
can bypass handle grants with ordinary file/socket syscalls; such execution is unsupported by
this profile, even after command consent. Missing confinement fails the local-execution capability
closed. Test unrelated-file reads, network/private-service access, inherited credentials and
escape attempts independently of process-tree cleanup.

### Bounded streams

Negotiate maximum frame bytes, unacknowledged bytes, queued frames and per-peer/session/stream
budgets; cap the number of streams and requests too. Count encoded bytes, not characters. Apply
limits **before allocation/decode** in the eventual transport, not only after parsing JSON.

Credit is consumed when sending; only an in-range, monotonic acknowledgement of sent data releases
it. Reject overflow, forged future ACKs, duplicates that mint credit, and ACKs from another
stream/epoch. Stop reading a local PTY when its bounded queue fills; do not accumulate an unbounded
side queue. A remote task's lifetime must not depend on a slow GUI: the execution service provides
bounded retention and explicit gap/resync, or detaches that subscriber on lag. Never silently drop
output and imply a complete transcript.

Reserve bounded control capacity for cancel/revoke/terminal notifications. A data flood cannot
block cancellation forever. If even control capacity/its deadline is exhausted, close the
subscription, revoke local authority and require resync; do not drop a terminal outcome silently.
Consumer event sequence and per-stream byte offsets are different cursors. ACKing rendered bytes
does not acknowledge command execution.

## Native material, accessibility and input

macOS owns window chrome, menus, titlebar/toolbar and native permission/auth/file dialogs. Optional
native controls must use the same session state/actions and one defined focus/accessibility order.
CEF owns task content, terminal blocks, diff/review and their semantic DOM. Do not overlay duplicate
native and DOM controls with independent values or expose two accessibility nodes for one action.

Native system material uses the supported AppKit/SwiftUI APIs for the qualified OS (for example
[`NSVisualEffectView`](https://developer.apple.com/documentation/appkit/nsvisualeffectview)).
CSS blur/translucency is a **visual fallback**, not native system material or a claim of Apple's
brand-layer behavior. Keep content and navigation legible without transparency, respect reduced
motion and increased contrast, and observe preference changes at runtime in both native chrome and
CEF content. No screenshot-only accessibility acceptance.

The native adapter must test the composed OS accessibility tree and keyboard route, not just DOM
unit tests: VoiceOver roles/names/state/actions, terminal output announcement throttling, diff
line/range semantics, menu shortcuts, focus ring and tab order across the native/CEF seam.
Chinese Pinyin composition/candidate placement, marked text, selection, clipboard and Enter/Escape
must work at mixed scale, with native dialogs, focus changes and renderer recovery. Global shortcuts
must not consume IME composition. Test reduced motion, transparency and contrast independently and
together, light/dark and dynamic text/zoom without losing actions or selection.

## Update adapter: status is not authority

The [accepted update/rollback policy](./kunlun-desktop.md#update-and-rollback-acceptance-policy)
is normative. A common adapter may expose a release ID, component versions/compatibility,
platform/channel, native manifest digest, maintenance requirement, preflight reasons and states
such as available, draining, staged, activating, healthy, failed, recovery-required. These are a
normalization proposal until Updatemgr's own schema is reviewed; no updater API is claimed.

The trusted native updater, not renderer or consumer metadata, authenticates:

- independent signed security policy and revocations, sequences and expiry;
- numeric minimum-safe CEF floor and approved exact revision/artifact hashes;
- complete Desktop/CEF/helpers/resources/host/service compatibility and signatures;
- source/target/platform/channel/failure-bound, unexpired, non-replayed rollback authorization;
- revocation of revisions, artifacts, manifests, keys and rollback authorizations.

Retain the highest policy/revocation sequences and floor outside the replaced installation.
Recheck at activation and startup, including revocation after staging. Old signed metadata cannot
lower the floor; an otherwise authorized rollback below it still fails. An updater crash restores
only a **compatible complete bundle**, never just CEF or just the host, while keeping current
security state. If neither installed nor recovery bundle passes, fail closed for presentation and
require a safe recovery update. Remote/headless execution does not require starting unsafe CEF.

The adapter has no `ignore_security`, unsigned-install or forced-downgrade escape hatch. An
Updatemgr failure cannot turn a failed preflight into approval. Authentication/revocation events
that arrive during drain must survive the restart; all previous renderer grants remain invalid.
Compose/K8s installation belongs to the external deployment controller, not a self-mutating
application container. Mobile uses its platform distribution mechanism, not this Desktop updater.

## Evidence and acceptance

Run the platform-independent consumer fixture with:

```sh
cargo test -p xtask --test wuling_host
```

The fixture exercises schema/denial, scoped grants, lease/input rules, sequence/cancel/reconnect,
stream bounds, auth revocation and update-policy decisions with deterministic trusted test facts.
Restart is modeled by retained/cloned state, not disk durability; remote execution is separate
model state, not a running service. Reconnect returns uncertain command IDs to query, not network
requests. It does not invoke CEF, native PTYs, secure storage, real Qingting or an installer.
Mock signature validity and complete/compatible-bundle facts are **not** signature or installation
verification evidence. Tests and the JSON conversation are reviewable under
[`fixtures/wuling-host/`](../fixtures/wuling-host/). The existing `cargo test -p xtask` CI lane
discovers this integration test without requiring a JSC artifact.

| Gate | Required evidence before a supported-host claim | Current status |
| --- | --- | --- |
| Host contract model | Serialized consumer conversation and denial/revoke/reconnect/stream/update tests | Executable fixture in this repository; not native integration |
| Consumer binding | Pinned Wuling/Qingting schema, Rust ABI or authenticated IPC review; independent CLI and SDK attach/detach with GUI absent | Open; upstream boundary not frozen |
| Local native resources | Real PTY resize/IME/cancel/grandchild cleanup/crash tests; handle/path escape and vault lock/revoke probes | Not implemented / unqualified |
| Desktop renderer | Exact pinned CEF/Chromium, signing/helper hashes, Web Profile, per-process sandbox runtime denial probes on every supported OS/architecture | No launcher or pin; unqualified |
| Native UI | OS accessibility tree, keyboard/Chinese IME, native/CEF focus seam and reduced motion/transparency/contrast results for each host profile | No supported native profile yet; untested |
| Distribution | Signed complete-bundle tests from the accepted Desktop matrix, retained policy and revocation, rollback/crash/activation race tests | Model only; packaged tests absent |
| Mobile | Separate viewport/lifecycle/accessibility and distribution gates, simulator vs device evidence | [Separate preview RFC](./wuling-mobile.md); not Desktop qualification |

Do not mark #43's platform acceptance gates complete from a model pass. Record OS/build/architecture,
SDK/toolchain, renderer revision/hash, profile, test command, result and artifact links for each
future native lane. Keep unmeasured rows visible. This work is independent of M2 closeout and M3
server compatibility; it neither broadens their support claims nor adds a Desktop prerequisite.

## References

- [Accepted Desktop architecture](./kunlun-desktop.md), including its sandbox and signed update matrix.
- [ADE / session and cross-device requirements #69](https://github.com/zixiao-labs/Wuling-DevOps/issues/69).
- [Standalone Qingting / model auth #70](https://github.com/zixiao-labs/Wuling-DevOps/issues/70).
- [Agent/workload authorization #73](https://github.com/zixiao-labs/Wuling-DevOps/issues/73).
- [Updatemgr deployment adapters #75](https://github.com/zixiao-labs/Wuling-DevOps/issues/75).
- [AppKit accessibility](https://developer.apple.com/documentation/appkit/accessibility)
  and [text input client](https://developer.apple.com/documentation/appkit/nstextinputclient).
- [Apple Keychain services](https://developer.apple.com/documentation/security/keychain-services).
