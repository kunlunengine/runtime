export const MAX_RESPONSE_BYTES: 1048576;
export type Operation = 'version' | 'detect' | 'plan' | 'why';
export type ErrorCode = 'invalid_arguments' | 'project_unreadable' | 'invalid_manifest' | 'invalid_lockfile' | 'unsupported_schema' | 'unsupported_configuration' | 'unsupported_feature' | 'ambiguous_authority' | 'frozen_drift' | 'policy_denied' | 'graph_invalid' | 'limit_exceeded' | 'internal';
export interface Diagnostic { readonly code: ErrorCode; readonly message: string; readonly remediation: string }
export interface VersionResult {
  readonly version: string;
  readonly stage: 'P0';
  readonly lockfileVersions: readonly ['9.0'];
  readonly capabilities: {
    readonly detect: true; readonly plan: true; readonly why: true;
    readonly resolve: false; readonly fetch: false; readonly install: false;
    readonly mutate: false; readonly prune: false; readonly exec: false; readonly lifecycleScripts: false;
  };
  readonly limits: { readonly maxResponseBytes: 1048576; readonly maxInputBytes: 8388608; readonly maxGraphNodes: 10000; readonly maxWhyPaths: 1000 };
}
export interface DetectResult {
  readonly packageManager: string; readonly lockfileVersion: '9.0';
  readonly authority: 'pnpm-lock.yaml'; readonly readOnly: true;
  readonly workspaceImporters: readonly string[];
}
export interface Dependency {
  readonly name: string; readonly target: string;
  readonly kind: 'production' | 'development' | 'optional'; readonly specifier?: string;
}
export interface Importer {
  readonly path: string; readonly name: string; readonly version: string;
  readonly dependencies: readonly Dependency[];
}
export interface GraphNode {
  readonly id: string; readonly kind: 'registry' | 'workspace'; readonly name: string;
  readonly version: string; readonly integrity?: string;
  readonly conditions: { readonly os: readonly string[]; readonly cpu: readonly string[]; readonly libc: readonly string[] };
  readonly peerDependencies: Readonly<Record<string, string>>; readonly dependencies: readonly Dependency[];
}
export interface PlanResult {
  readonly planSchema: 'kunlun.package-manager-plan/v1'; readonly packageManager: string;
  readonly lockfileVersion: '9.0'; readonly frozen: true; readonly readOnly: true;
  readonly ignoreScripts: boolean; readonly defaultScriptPolicy: 'deny';
  readonly evidence: 'not-verified'; readonly fingerprint: string;
  readonly importers: readonly Importer[]; readonly nodes: readonly GraphNode[];
  readonly scriptDecisions: readonly {
    readonly importer: string; readonly hook: string; readonly digest: string;
    readonly decision: 'denied'; readonly reason: 'default-deny' | 'ignore-scripts';
  }[];
  readonly unbuiltPackages: readonly string[]; readonly readiness: 'not-assessed';
  readonly changedManifestPaths: readonly [];
  readonly policyDecisions: readonly ['read-only', 'frozen-graph-validated', 'scripts-denied', 'content-evidence-not-verified'];
}
export interface WhyResult {
  readonly package: string;
  readonly paths: readonly { readonly importer: string; readonly nodes: readonly string[] }[];
}
export interface ResultMap { version: VersionResult; detect: DetectResult; plan: PlanResult; why: WhyResult }
interface Base<O> {
  readonly schema: 'kunlun.package-manager-provider/v1'; readonly provider: 'kunlun-pm';
  readonly requiresNode: false; readonly operation: O;
}
export type Response<O extends Operation | 'unknown' = Operation | 'unknown'> = O extends Operation | 'unknown'
  ? (Base<O> & { readonly status: 'error'; readonly exitStatus: 1 | 2; readonly diagnostics: readonly Diagnostic[] })
    | (O extends Operation ? Base<O> & { readonly status: 'ok'; readonly exitStatus: 0; readonly result: ResultMap[O] } : never)
  : never;
export type Request =
  | { readonly operation: 'version'; readonly projectRoot?: never; readonly package?: never; readonly frozen?: never; readonly ignoreScripts?: never }
  | { readonly operation: 'detect'; readonly projectRoot: string; readonly package?: never; readonly frozen?: never; readonly ignoreScripts?: never }
  | { readonly operation: 'plan'; readonly projectRoot: string; readonly package?: never; readonly frozen?: boolean; readonly ignoreScripts?: boolean }
  | { readonly operation: 'why'; readonly projectRoot: string; readonly package: string; readonly frozen?: never; readonly ignoreScripts?: never };
export interface RunOptions { readonly timeoutMs?: number; readonly maxStderrBytes?: number; readonly signal?: AbortSignal }
export class TransportError extends Error {
  readonly code: 'invalid_configuration' | 'invalid_response' | 'invalid_utf8' | 'spawn_failed' | 'stream_failed' | 'terminated' | 'output_limit' | 'timeout' | 'aborted';
  constructor(code: TransportError['code']);
}
export function parseResponse<O extends Operation | 'unknown'>(bytes: Uint8Array, operation: O, exitStatus: number): Response<O>;
export function createProvider(binaryPath: string): {
  readonly run: <R extends Request>(request: R, options?: RunOptions) => Promise<Response<R['operation']>>;
};
