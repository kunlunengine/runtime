# M3 scoped authority

Status: Rust admission, built-in permission projection, JavaScript request `env`,
and request-scope handles are implemented. Inbound HTTP request dispatch remains
[#51](https://github.com/kunlunengine/runtime/issues/51); independent Node/native parity
remains [#53](https://github.com/kunlunengine/runtime/issues/53). These APIs do
not claim a JavaScript Fetch server or hostile-code isolation.

## Deployment and admission

The trusted deployer supplies `AdmissionPolicy::new(manifest_sha256,
HostPermissions)`. `HostPermissions::allow_net_host` grants one exact HTTP(S)
hostname; `bind_read_root(name, path)` opens one filesystem directory and
assigns it a manifest-visible binding name. Request handles accept only
binding-relative paths; they never expose the host directory path.
`allow_read_root` remains available
for direct M2 execution but is never interpreted as an M3 `fs.binding` grant.
The manifest contains only names and resource identifiers, never host paths,
tokens, provider credentials, or the authority objects themselves.

Admission accepts only the exact intersection of manifest declarations and
deployment grants. Missing required grants reject before module evaluation.
Missing optional grants produce no handle. Unknown capability names and
noncanonical resources reject, including in optional declarations. Deployment
grants that were not declared are removed. The resulting
`ApplicationAuthority` is owned by `AdmittedArtifact` and must travel with
its validated source and asset snapshot. `TokioIsolate::new_with_authority`
uses this intersection for `kunlun:fs`, `kunlun:http`, and outbound Fetch
operations. An authority may be claimed by only one isolate. The M2 direct
`new_with_permissions` constructor remains a separate trusted-host interface;
the future M3 server must use the authority constructor.

The built-in filesystem operations recheck paths through opened root-scoped
`cap-std` directories. The HTTP client disables automatic redirects, so a
caller must explicitly request a redirect destination and undergo a new host
check. `http.host` scopes the hostname for HTTP(S); v1 does not restrict a
specific port or path. Host permission checks occur on every operation. The
admitted module loader independently limits executable sources to the indexed
snapshot; a filesystem binding does not add module sources.
Both HTTP clients ignore ambient proxy environment variables: a deployment's
`HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY` is not an undeclared network grant.

## Request lifetime

After one isolate claims the application authority,
`ApplicationAuthority::begin_request` owns a new `RequestContext` and produces
one `RequestEnvironment`. A `ScopedHandle` can be obtained only for an effective
grant, only used with the environment that issued it, and cannot be serialized
or moved to another thread. Request environments can only be created on the
claiming isolate's thread. Context values stay in that request environment;
there is no process-global current caller. Dropping or revoking a request
environment invalidates its retained handles without affecting a concurrent
request. Revoking the application, requesting isolate shutdown, or dropping
the isolate invalidates all related handles and new privileged operations.
Revocation wakes pending I/O, aborts response producers, and closes upload/body
channels; queued completions are rechecked before delivery. Request return,
failure, or dropping the invocation future cancels its timers and host work.
The host must still join workers via bounded isolate shutdown: an already
running blocking filesystem syscall cannot be preempted, and revocation does
not retroactively undo a completed operation or retract delivered bytes.

Application-level built-in grants intentionally last for the admitted
application. They do not carry caller auth/provider/billing data. Operations
started during a request invocation additionally inherit its cancellation
scope. Outside a request, the built-ins use only application authority.
[Outbound Fetch](./fetch-profile-v1.md) uses the same admitted `http.host`
intersection and checks each redirect before opening a connection.

## Executable request projection

The trusted adapter consumes a `RequestEnvironment` with
`TokioIsolate::evaluate_request_body(environment, source, source_url)`. The body
has a lexical, read-only `env`:

```js
const text = await env.fs["public-data"].readTextFile("help/欢迎.txt");
const api = env.http["api.example.test"];
if (api) {
  const response = await api.fetch("https://api.example.test/help");
  return await response.text();
}
return text;
```

`env.fs` and `env.http` are frozen, null-prototype binding maps. Missing optional
grants are absent (`undefined`); required grants must have passed admission.
Filesystem handles only read bounded UTF-8 regular files relative to the opened
binding, with the same 1 MiB limit as `kunlun:fs`. HTTP handles expose the same
Fetch profile as global `fetch`, narrowed to that one declared host. A redirect
to another host is denied even when another application-level grant allows it.
Hosts, including ports and paths, retain the v1 semantics above. Handles have no
public identity, constructor, host path, or credential fields. JSON serialization
of an environment or handle throws; copying methods does not mint a new grant.

Every env operation carries a private request identity and resource selection.
The host compares them with its own current request, checks the effective grant,
narrows the existing host permission set, and rechecks the actual path or URL.
The internal projection factory cannot grant authority from invented metadata.
Raw bridge functions are removed before application evaluation. Identity alone
is not the security claim: root-relative opens, destination checks, revocation,
and the admitted module snapshot remain enforced independently.

An environment belongs to exactly one isolate. Multiple environments may coexist
on that thread, but this integration primitive serializes JavaScript invocations
through `&mut TokioIsolate`; concurrent execution uses separate isolates. It does
not infer caller identity from a process-global or async-local variable. A
retained env method, body stream, or upload from an earlier invocation cannot be
used by a later one. Completion, error, and cancellation all end the consumed
environment. Detached application work is not a way to extend that lifetime.

This primitive does not itself dispatch an exported `fetch` handler, implement
`waitUntil`, or stream an inbound HTTP response. #51 must keep the scope alive
for its entire handler/body/background-work contract before adopting those APIs;
it must not return a live response body from this bounded string-result helper.

## Context, secrets, and diagnostics

`RequestContext` is owned by the trusted host, not by JavaScript. The adapter may
read its own context while the request is active; it must not put raw auth,
provider credentials, or billing state into module globals or `env`. V1 has no
secret binding, arbitrary OS-env projection, provider service, credential
issuance, or billing API. Such declarations fail admission, including optional
ones. Future host services must resolve credentials on the host and check the
same request owner and operation scope, not expose a transferable token.

M3 scoped filesystem failures omit host paths, and scoped HTTP transport
failures omit request URLs. Request context values and handles have no automatic
Debug or serialization representation; `HostPermissions` Debug is redacted.
Runtime diagnostics never include the private request identity. Application code
can deliberately log data it was allowed to read; this is not data-loss prevention
or a hostile-code boundary. These diagnostics are for operators;
public HTTP error responses and source-map disclosure policy belong to #51.

## Verification and #53 handoff

`tests/request_environment.rs` exercises real admitted JSC calls, filesystem
escapes, cross-request/isolate denial, frozen/opaque projections, diagnostic
redaction, and synchronized in-flight revocation. `tests/artifact_admission.rs`
continues covering required grants and module snapshots. Dispatcher unit tests
cover stale stream ownership, cancellation notifications, and discarded-stream
teardown; the M0–M2 suite remains required.

`tests/fixtures/request-authority.js` is an adapter-neutral probe consumed without
rewriting its bytes. The harness must bind `public-data` to a directory containing
`message.txt` with `public message`, declare but not grant `missing-optional`,
keep `undeclared` deployment authority outside the manifest, and invoke it in two
successive request environments in one application. The adjacent
`request-authority.contract.json` specifies the complete setup and expected
two-request observations. Denial, successful reads, optional absence, and stale
handle behavior must not be normalized away.

Native tests consume this probe now. Independent runtime-node execution and
pinned four-platform comparison belong to #53 and are **not** replaced by a
mock Node permission layer or a local system-JSC pass. #50's cross-adapter
qualification remains open until that gate runs the real adapters.

The shared HTTP and post-headers lifecycle probes now exercise the real Node
adapter too: ordered network traffic, scoped redirect denials, invocation
cancellation/revocation, stale handles/body readers and concurrent independent
isolates are compared without rewriting probe bytes. Native host-context
ownership and resource-counter assertions are explicitly separate from shared
observations; no Node context/provider service is invented for the comparison.
Four-physical-platform evidence execution is deferred to
[#63](https://github.com/kunlunengine/runtime/issues/63), so implementation can
proceed independently while #53 retains its formal qualification gate.

The [authority conformance evidence slice](./m3-authority-conformance.md) defines
the shared observation contract, native report collection, strict comparison,
and real Core adapter integration / outstanding physical-platform dependencies.
It is not the complete M3 application exit gate.
