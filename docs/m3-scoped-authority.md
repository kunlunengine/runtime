# M3 scoped authority

Status: Rust admission, built-in permission projection, and request-scope handles
are implemented. Fetch `env` exposure and inbound HTTP request dispatch remain
[#51](https://github.com/kunlunengine/runtime/issues/51); Node/native parity
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
uses this intersection for all existing `kunlun:fs` and `kunlun:http`
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

## Request lifetime

After one isolate claims the application authority,
`ApplicationAuthority::begin_request` owns a new `RequestContext` and produces
one `RequestEnvironment`. A `ScopedHandle` can be obtained only for an effective
grant, only used with the environment that issued it, and cannot be serialized
or moved to another thread. Context values stay in that request environment;
there is no process-global current caller. Dropping or revoking a request
environment invalidates its retained handles without affecting a concurrent
request. Revoking the application, requesting isolate shutdown, or dropping
the isolate invalidates all related handles and new privileged operations.
The host must cancel and join already started work via the existing bounded
isolate shutdown; revocation does not retroactively undo a completed operation.

Application-level built-in grants intentionally last for the admitted
application. Request-specific caller context and any future `env` handles have
shorter lifetimes. #51 must bind the JavaScript `env` projection and its host
calls to the current request environment; no JavaScript `env` is currently
installed by this code. #49's outbound Fetch implementation must reuse the
same destination checks, including each redirect, rather than introducing an
alternate ambient network route.

M3 scoped filesystem failures omit host paths, and scoped HTTP transport
failures omit request URLs. Request context values and handles have no automatic
Debug or serialization representation. These diagnostics are for operators;
public HTTP error responses and source-map disclosure policy belong to #51.
