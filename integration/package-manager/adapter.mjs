import { spawn } from 'node:child_process';
import { isAbsolute } from 'node:path';

export const MAX_RESPONSE_BYTES = 1024 * 1024;
const operations = ['version', 'detect', 'plan', 'why', 'unknown'];
const codes = ['invalid_arguments', 'project_unreadable', 'invalid_manifest', 'invalid_lockfile', 'unsupported_schema', 'unsupported_configuration', 'unsupported_feature', 'ambiguous_authority', 'frozen_drift', 'policy_denied', 'graph_invalid', 'limit_exceeded', 'internal'];

/** Transport errors never contain provider output, arguments, paths or OS errors. */
export class TransportError extends Error {
  constructor(code) {
    super(`Kunlun provider transport failure: ${code}`);
    this.name = 'TransportError';
    this.code = code;
  }
}

const fail = () => { throw new TransportError('invalid_response'); };
const check = (condition) => { if (!condition) fail(); };
const object = (v) => v !== null && typeof v === 'object' && !Array.isArray(v);
const keys = (v, required, optional = []) => {
  check(object(v));
  check(required.every(k => Object.hasOwn(v, k)));
  check(Object.keys(v).every(k => required.includes(k) || optional.includes(k)));
};
const text = v => check(typeof v === 'string' && v.length > 0 && !v.includes('\0'));
const list = (v, fn, max = MAX_RESPONSE_BYTES) => {
  check(Array.isArray(v) && v.length <= max);
  v.forEach(fn);
};
const unique = v => check(new Set(v).size === v.length);
const packageName = v => {
  text(v);
  check(v.length <= 214 && /^(?:@[A-Za-z0-9_-][A-Za-z0-9_.-]*\/)?[A-Za-z0-9_-][A-Za-z0-9_.-]*$/.test(v));
};
const version = v => {
  text(v);
  check(/^(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(?:-[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*)?(?:\+[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*)?$/.test(v));
  const prerelease = v.split('+')[0].split('-').slice(1).join('-');
  check(!prerelease.split('.').some(p => /^\d+$/.test(p) && p.length > 1 && p.startsWith('0')));
};
const path = v => {
  text(v);
  check(v === '.' || (!v.startsWith('/') && !v.includes('\\') && !v.includes(':') &&
    v.split('/').every(p => /^[A-Za-z0-9_.-]+$/.test(p) && p !== '.' && p !== '..')));
};
const registryId = (id, nesting = 0) => {
  text(id); check(id.length <= 1024 && nesting <= 32);
  const suffix = id.indexOf('(');
  const base = suffix < 0 ? id : id.slice(0, suffix);
  const at = base.lastIndexOf('@');
  check(at > 0);
  packageName(base.slice(0, at)); version(base.slice(at + 1));
  const peers = [];
  let depth = 0;
  let start = 0;
  for (let index = suffix < 0 ? id.length : suffix; index < id.length; index++) {
    if (id[index] === '(') {
      if (depth === 0) start = index + 1;
      check(++depth <= 32);
    } else if (id[index] === ')') {
      check(depth > 0);
      if (--depth === 0) peers.push(registryId(id.slice(start, index), nesting + 1).name);
    } else check(depth > 0);
  }
  check(depth === 0); unique(peers);
  return { name: base.slice(0, at), version: base.slice(at + 1) };
};
const nodeId = id => {
  text(id);
  if (id.startsWith('workspace:')) path(id.slice(10));
  else registryId(id);
};
const common = r => {
  check(/^pnpm@(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:\+sha512\.[A-Fa-f0-9]{128})?$/.test(r.packageManager));
  check(r.lockfileVersion === '9.0' && r.readOnly === true);
};
const dependency = d => {
  keys(d, ['name', 'target', 'kind'], ['specifier']);
  packageName(d.name); nodeId(d.target);
  check(['production', 'development', 'optional'].includes(d.kind));
  if (Object.hasOwn(d, 'specifier')) text(d.specifier);
};
const freeze = v => {
  if (object(v) || Array.isArray(v)) {
    Object.values(v).forEach(freeze);
    Object.freeze(v);
  }
  return v;
};

function result(operation, r) {
  if (operation === 'version') {
    keys(r, ['version', 'stage', 'lockfileVersions', 'capabilities', 'limits']);
    version(r.version); check(r.stage === 'P0');
    check(Array.isArray(r.lockfileVersions) && r.lockfileVersions.length === 1 && r.lockfileVersions[0] === '9.0');
    const enabled = ['detect', 'plan', 'why'];
    const disabled = ['resolve', 'fetch', 'install', 'mutate', 'prune', 'exec', 'lifecycleScripts'];
    keys(r.capabilities, [...enabled, ...disabled]);
    enabled.forEach(k => check(r.capabilities[k] === true));
    disabled.forEach(k => check(r.capabilities[k] === false));
    const limits = { maxResponseBytes: MAX_RESPONSE_BYTES, maxInputBytes: 8 * 1024 * 1024, maxGraphNodes: 10000, maxWhyPaths: 1000 };
    keys(r.limits, Object.keys(limits));
    Object.entries(limits).forEach(([k, v]) => check(r.limits[k] === v));
  } else if (operation === 'detect') {
    keys(r, ['packageManager', 'lockfileVersion', 'authority', 'readOnly', 'workspaceImporters']);
    common(r); check(r.authority === 'pnpm-lock.yaml');
    list(r.workspaceImporters, path, 10000); unique(r.workspaceImporters);
    check(r.workspaceImporters.includes('.'));
  } else if (operation === 'plan') {
    keys(r, ['planSchema', 'packageManager', 'lockfileVersion', 'frozen', 'readOnly', 'ignoreScripts', 'defaultScriptPolicy', 'evidence', 'fingerprint', 'importers', 'nodes', 'scriptDecisions', 'unbuiltPackages', 'readiness', 'changedManifestPaths', 'policyDecisions']);
    common(r);
    check(r.planSchema === 'kunlun.package-manager-plan/v1' && r.frozen === true && typeof r.ignoreScripts === 'boolean');
    check(r.defaultScriptPolicy === 'deny');
    check(r.evidence === 'not-verified' && r.readiness === 'not-assessed');
    check(/^[a-f0-9]{64}$/.test(r.fingerprint));
    list(r.importers, i => {
      keys(i, ['path', 'name', 'version', 'dependencies']);
      path(i.path); packageName(i.name); version(i.version); list(i.dependencies, dependency);
      unique(i.dependencies.map(d => d.name));
    }, 10000);
    const importers = r.importers.map(i => i.path);
    unique(importers); check(importers.includes('.'));
    list(r.nodes, n => {
      keys(n, ['id', 'kind', 'name', 'version', 'conditions', 'peerDependencies', 'dependencies'], ['integrity']);
      nodeId(n.id); packageName(n.name); version(n.version);
      check(['registry', 'workspace'].includes(n.kind));
      if (n.kind === 'registry') {
        check(!n.id.startsWith('workspace:') && /^sha512-[A-Za-z0-9+/]{86}==$/.test(n.integrity));
        const decoded = Buffer.from(n.integrity.slice(7), 'base64');
        check(decoded.length === 64 && decoded.toString('base64') === n.integrity.slice(7));
        const identity = `${n.name}@${n.version}`;
        check(n.id === identity || n.id.startsWith(`${identity}(`));
      } else {
        check(n.id.startsWith('workspace:') && !Object.hasOwn(n, 'integrity'));
        check(importers.includes(n.id.slice(10)));
      }
      keys(n.conditions, ['os', 'cpu', 'libc']);
      Object.values(n.conditions).forEach(v => list(v, text));
      check(object(n.peerDependencies));
      Object.entries(n.peerDependencies).forEach(([k, v]) => { packageName(k); text(v); });
      list(n.dependencies, dependency);
      unique(n.dependencies.map(d => d.name));
      if (n.kind === 'registry') check(n.dependencies.every(d => d.kind !== 'development'));
    }, 10000);
    const ids = r.nodes.map(n => n.id);
    unique(ids);
    const nodeIds = new Set(ids);
    [...r.importers, ...r.nodes].forEach(n => n.dependencies.forEach(d => check(nodeIds.has(d.target))));
    const byId = new Map(r.nodes.map(n => [n.id, n]));
    [...r.importers, ...r.nodes].forEach(n => n.dependencies.forEach(d => check(byId.get(d.target).name === d.name)));
    r.importers.forEach(i => {
      const workspace = byId.get(`workspace:${i.path}`);
      check(workspace?.kind === 'workspace' && workspace.name === i.name && workspace.version === i.version);
      check(workspace.dependencies.length === i.dependencies.length);
      i.dependencies.forEach((d, index) => {
        const edge = workspace.dependencies[index];
        check(edge.name === d.name && edge.target === d.target && edge.kind === d.kind);
      });
    });
    r.importers.forEach(i => i.dependencies.forEach(d => check(Object.hasOwn(d, 'specifier'))));
    r.nodes.forEach(n => n.dependencies.forEach(d => check(!Object.hasOwn(d, 'specifier'))));
    list(r.scriptDecisions, d => {
      keys(d, ['importer', 'hook', 'digest', 'decision', 'reason']);
      check(importers.includes(d.importer)); text(d.hook);
      check(/^[a-f0-9]{64}$/.test(d.digest)); check(d.decision === 'denied');
      check(d.reason === (r.ignoreScripts ? 'ignore-scripts' : 'default-deny'));
    });
    list(r.unbuiltPackages, p => { path(p); check(importers.includes(p)); });
    unique(r.unbuiltPackages);
    list(r.changedManifestPaths, path);
    check(r.changedManifestPaths.length === 0);
    check(JSON.stringify(r.policyDecisions) === JSON.stringify(['read-only', 'frozen-graph-validated', 'scripts-denied', 'content-evidence-not-verified']));
  } else if (operation === 'why') {
    keys(r, ['package', 'paths']); packageName(r.package);
    list(r.paths, p => {
      keys(p, ['importer', 'nodes']); path(p.importer);
      list(p.nodes, nodeId, 64); check(p.nodes.length > 0); unique(p.nodes);
    }, 1000);
    unique(r.paths.map(p => JSON.stringify(p)));
  } else fail();
}

/** Parse exactly one native UTF-8 JSON line and validate its operation and exit. */
export function parseResponse(bytes, operation, exitStatus) {
  try { return parse(bytes, operation, exitStatus); }
  catch { throw new TransportError('invalid_response'); }
}

function parse(bytes, operation, exitStatus) {
  check(operations.includes(operation));
  check(bytes instanceof Uint8Array && bytes.byteLength > 0 && bytes.byteLength <= MAX_RESPONSE_BYTES);
  let source;
  try { source = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(bytes); } catch { fail(); }
  check(source.endsWith('\n') && !source.slice(0, -1).includes('\n') && !source.includes('\r'));
  let value;
  try { value = JSON.parse(source); } catch { fail(); }
  // JSON.parse accepts duplicate keys; reject ambiguity at every object depth.
  const stack = [];
  const tokens = source.match(/"(?:[^"\\]|\\.)*"|[{}[\],:]|[^\s{}[\],:]+/g) ?? [];
  tokens.forEach((token, index) => {
    if (token === '{' || token === '[') {
      check(stack.length < 128);
      stack.push(token === '{' ? new Set() : null);
    } else if (token === '}' || token === ']') stack.pop();
    else if (token.startsWith('"') && tokens[index + 1] === ':') {
      const seen = stack.at(-1);
      const key = JSON.parse(token);
      check(seen instanceof Set && !seen.has(key));
      seen.add(key);
    }
  });
  check(object(value));
  keys(value, ['schema', 'provider', 'requiresNode', 'operation', 'exitStatus', 'status'],
    value.status === 'ok' ? ['result'] : ['diagnostics']);
  check(value.schema === 'kunlun.package-manager-provider/v1' && value.provider === 'kunlun-pm' &&
    value.requiresNode === false && value.operation === operation && value.exitStatus === exitStatus);
  if (value.status === 'ok') {
    check(exitStatus === 0 && Object.hasOwn(value, 'result'));
    result(operation, value.result);
  } else {
    check(value.status === 'error' && (exitStatus === 1 || exitStatus === 2));
    list(value.diagnostics, d => {
      keys(d, ['code', 'message', 'remediation']); check(codes.includes(d.code));
      text(d.message); text(d.remediation);
      check(exitStatus === (d.code === 'invalid_arguments' ? 2 : 1));
    });
    check(value.diagnostics.length > 0);
  }
  return freeze(value);
}

/** No shell, PATH lookup, fallback, stdin or inherited child output. */
export function createProvider(binaryPath) {
  if (typeof binaryPath !== 'string' || !isAbsolute(binaryPath) || binaryPath.includes('\0')) {
    throw new TransportError('invalid_configuration');
  }
  return Object.freeze({
    run(request, options = {}) {
      if (!object(request) || !object(options) ||
          Object.keys(request).some(k => !['operation', 'projectRoot', 'package', 'frozen', 'ignoreScripts'].includes(k)) ||
          Object.keys(options).some(k => !['timeoutMs', 'maxStderrBytes', 'signal'].includes(k))) {
        return Promise.reject(new TransportError('invalid_configuration'));
      }
      const { operation, projectRoot, package: packageName, frozen, ignoreScripts } = request;
      const { timeoutMs = 30000, maxStderrBytes = 65536, signal } = options;
      if (!['version', 'detect', 'plan', 'why'].includes(operation) ||
          (operation !== 'version' && (typeof projectRoot !== 'string' || !isAbsolute(projectRoot) || projectRoot.includes('\0'))) ||
          (operation === 'why' && (typeof packageName !== 'string' || !packageName || packageName.startsWith('-') || packageName.includes('\0'))) ||
          (operation !== 'why' && packageName !== undefined) ||
          (operation === 'version' && projectRoot !== undefined) ||
          !Number.isInteger(timeoutMs) || timeoutMs < 1 || timeoutMs > 120000 ||
          !Number.isInteger(maxStderrBytes) || maxStderrBytes < 0 || maxStderrBytes > MAX_RESPONSE_BYTES ||
          (signal !== undefined && !(signal instanceof AbortSignal)) ||
          (frozen !== undefined && (operation !== 'plan' || typeof frozen !== 'boolean')) ||
          (ignoreScripts !== undefined && (operation !== 'plan' || typeof ignoreScripts !== 'boolean'))) {
        return Promise.reject(new TransportError('invalid_configuration'));
      }
      if (signal?.aborted) return Promise.reject(new TransportError('aborted'));
      const args = [operation];
      if (operation === 'why') args.push(packageName);
      if (operation !== 'version') args.push('--project', projectRoot);
      if (frozen) args.push('--frozen');
      if (ignoreScripts) args.push('--ignore-scripts');
      args.push('--json');
      return new Promise((resolve, reject) => {
        let child;
        try { child = spawn(binaryPath, args, { shell: false, stdio: ['ignore', 'pipe', 'pipe'] }); }
        catch { reject(new TransportError('spawn_failed')); return; }
        let failure;
        let stdoutSize = 0;
        let stderrSize = 0;
        const chunks = [];
        const decoder = new TextDecoder('utf-8', { fatal: true });
        const stop = code => {
          if (!failure) failure = new TransportError(code);
          child.kill('SIGKILL');
        };
        const abort = () => stop('aborted');
        const timer = setTimeout(() => stop('timeout'), timeoutMs);
        signal?.addEventListener('abort', abort, { once: true });
        if (signal?.aborted) abort();
        child.on('error', () => stop('spawn_failed'));
        child.stdout.on('error', () => stop('stream_failed'));
        child.stderr.on('error', () => stop('stream_failed'));
        child.stdout.on('data', chunk => {
          stdoutSize += chunk.length;
          if (stdoutSize > MAX_RESPONSE_BYTES) stop('output_limit');
          else if (!failure) chunks.push(chunk);
        });
        child.stderr.on('data', chunk => {
          stderrSize += chunk.length;
          if (stderrSize > maxStderrBytes) stop('output_limit');
          else if (!failure) {
            try { decoder.decode(chunk, { stream: true }); } catch { stop('invalid_utf8'); }
          }
        });
        child.on('close', (code, terminationSignal) => {
          clearTimeout(timer);
          signal?.removeEventListener('abort', abort);
          if (failure) { reject(failure); return; }
          if (terminationSignal || code === null) { reject(new TransportError('terminated')); return; }
          try {
            decoder.decode();
          } catch { reject(new TransportError('invalid_utf8')); return; }
          try {
            const response = parseResponse(Buffer.concat(chunks), operation, code);
            if (response.status === 'ok' && operation === 'why') check(response.result.package === packageName);
            if (response.status === 'ok' && operation === 'plan') check(response.result.ignoreScripts === Boolean(ignoreScripts));
            resolve(response);
          }
          catch (error) { reject(error); }
        });
      });
    },
  });
}
