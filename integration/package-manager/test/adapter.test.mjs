import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { createProvider, parseResponse, TransportError, MAX_RESPONSE_BYTES } from '../adapter.mjs';

const version = {
  version: '0.1.0', stage: 'P0', lockfileVersions: ['9.0'],
  capabilities: { detect: true, plan: true, why: true, resolve: false, fetch: false, install: false, mutate: false, prune: false, exec: false, lifecycleScripts: false },
  limits: { maxResponseBytes: MAX_RESPONSE_BYTES, maxInputBytes: 8388608, maxGraphNodes: 10000, maxWhyPaths: 1000 },
};
const envelope = (operation = 'version', result = version) => ({
  schema: 'kunlun.package-manager-provider/v1', provider: 'kunlun-pm',
  requiresNode: false, operation, exitStatus: 0, status: 'ok', result,
});
const bytes = v => Buffer.from(JSON.stringify(v) + '\n');
const rejected = (code) => error => {
  assert.ok(error instanceof TransportError);
  assert.equal(error.code, code);
  assert.ok(!error.message.includes('SECRET'));
  return true;
};

test('pure parser accepts and deeply freezes valid results and structured diagnostics', () => {
  const response = parseResponse(bytes(envelope()), 'version', 0);
  assert.equal(response.result.capabilities.install, false);
  assert.ok(Object.isFrozen(response.result.capabilities));
  const error = { ...envelope(), operation: 'unknown', status: 'error', exitStatus: 2 };
  delete error.result;
  error.diagnostics = [{ code: 'invalid_arguments', message: 'invalid provider arguments', remediation: 'use documented options' }];
  assert.equal(parseResponse(bytes(error), 'unknown', 2).status, 'error');
});

test('rejects schema, provider, operation, exits, mixed fields and unsafe capabilities', () => {
  const bad = [
    { ...envelope(), schema: 'other' }, { ...envelope(), provider: 'other' },
    { ...envelope(), operation: 'detect' }, { ...envelope(), exitStatus: 1 },
    { ...envelope(), requiresNode: true }, { ...envelope(), diagnostics: [] },
    { ...envelope(), result: { ...version, capabilities: { ...version.capabilities, install: true } } },
    { ...envelope(), result: null }, null, [], { ...envelope(), status: 'error' },
  ];
  for (const value of bad) assert.throws(() => parseResponse(bytes(value), 'version', 0), rejected('invalid_response'));
});

test('rejects framing, malformed JSON, UTF-8 and oversized replies', () => {
  for (const value of [
    Buffer.from('{}'), Buffer.from('{}\n{}\n'), Buffer.from('{}\r\n'),
    Buffer.from('SECRET\n'), Buffer.from([0xff, 10]),
    Buffer.alloc(MAX_RESPONSE_BYTES + 1, 32),
    Buffer.from('{"schema":1,"schema":2}\n'),
  ]) assert.throws(() => parseResponse(value, 'version', 0), rejected('invalid_response'));
});

test('validates graph references and relative importer paths', () => {
  const detect = { packageManager: 'pnpm@9.15.0', lockfileVersion: '9.0', authority: 'pnpm-lock.yaml', readOnly: true, workspaceImporters: ['.', 'packages/a'] };
  assert.equal(parseResponse(bytes(envelope('detect', detect)), 'detect', 0).status, 'ok');
  for (const path of ['../a', '/a', 'a\\b', 'a//b']) {
    assert.throws(() => parseResponse(bytes(envelope('detect', { ...detect, workspaceImporters: ['.', path] })), 'detect', 0), rejected('invalid_response'));
  }
  for (const packageManager of ['pnpm@9.15.0+', 'pnpm@^9.15.0', 'pnpm@9.15.0+arbitrary', 'pnpm@09.15.0', 'pnpm@9.15.0-beta']) {
    assert.throws(() => parseResponse(bytes(envelope('detect', { ...detect, packageManager })), 'detect', 0), rejected('invalid_response'));
  }
  assert.throws(() => parseResponse(bytes(envelope('why', { package: 'a', paths: [{ importer: '.', nodes: ['a', 'a'] }] })), 'why', 0), rejected('invalid_response'));
});

test('plan validates complete references, evidence and deny-only policy literals', () => {
  const plan = {
    planSchema: 'kunlun.package-manager-plan/v1', packageManager: 'pnpm@9.15.0',
    lockfileVersion: '9.0', frozen: true, readOnly: true, ignoreScripts: false,
    defaultScriptPolicy: 'deny', evidence: 'not-verified', fingerprint: 'a'.repeat(64),
    importers: [{ path: '.', name: 'app', version: '1.0.0', dependencies: [{ name: 'dep', target: 'dep@1.0.0', kind: 'production', specifier: '^1.0.0' }] }],
    nodes: [
      { id: 'dep@1.0.0', kind: 'registry', name: 'dep', version: '1.0.0', integrity: `sha512-${Buffer.alloc(64).toString('base64')}`, conditions: { os: [], cpu: [], libc: [] }, peerDependencies: {}, dependencies: [] },
      { id: 'workspace:.', kind: 'workspace', name: 'app', version: '1.0.0', conditions: { os: [], cpu: [], libc: [] }, peerDependencies: {}, dependencies: [{ name: 'dep', target: 'dep@1.0.0', kind: 'production' }] },
    ],
    scriptDecisions: [{ importer: '.', hook: 'install', digest: 'b'.repeat(64), decision: 'denied', reason: 'default-deny' }],
    unbuiltPackages: ['.'], readiness: 'not-assessed', changedManifestPaths: [],
    policyDecisions: ['read-only', 'frozen-graph-validated', 'scripts-denied', 'content-evidence-not-verified'],
  };
  assert.equal(parseResponse(bytes(envelope('plan', plan)), 'plan', 0).status, 'ok');
  for (const patch of [
    { evidence: 'verified' }, { readiness: 'installed' }, { frozen: false },
    { policyDecisions: ['scripts-approved'] }, { unbuiltPackages: ['missing'] },
    { importers: [{ ...plan.importers[0], dependencies: [{ ...plan.importers[0].dependencies[0], target: 'missing@1.0.0' }] }] },
    { nodes: [{ ...plan.nodes[0], integrity: 'sha1-unsafe' }] },
    { scriptDecisions: [{ ...plan.scriptDecisions[0], decision: 'allow' }] },
    { changedManifestPaths: ['package.json'] },
  ]) assert.throws(() => parseResponse(bytes(envelope('plan', { ...plan, ...patch })), 'plan', 0), rejected('invalid_response'));
  const valid = bytes(envelope());
  const duplicate = Buffer.from(valid.toString().replace('"provider":"kunlun-pm"', '"provider":"SECRET","provider":"kunlun-pm"'));
  assert.throws(() => parseResponse(duplicate, 'version', 0), rejected('invalid_response'));
});

test('peer-context IDs retain bounded nested identity and reject invented syntax', () => {
  const why = id => envelope('why', { package: 'dep', paths: [{ importer: '.', nodes: [id] }] });
  assert.equal(parseResponse(bytes(why('dep@1.0.0(peer@2.0.0(@scope/other@3.0.0))')), 'why', 0).status, 'ok');
  for (const id of ['dep@1.0.0(SECRET)', 'dep@1.0.0(peer@2.0.0)tail', 'dep@1.0.0(peer@2.0.0)(peer@2.0.0)', 'dep@01.0.0']) {
    assert.throws(() => parseResponse(bytes(why(id)), 'why', 0), rejected('invalid_response'));
  }
});

async function fake(t, scenario) {
  const dir = await mkdtemp(join(tmpdir(), 'kunlun-adapter-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const source = await readFile(new URL('./fake.mjs', import.meta.url), 'utf8');
  const executable = join(dir, 'fake.mjs');
  await writeFile(executable, `#!${process.execPath}\n${source}`, { mode: 0o700 });
  await writeFile(join(dir, 'scenario.json'), JSON.stringify(scenario));
  return createProvider(executable);
}
const encoded = value => Buffer.from(value).toString('base64');
const nativeFake = { skip: process.platform === 'win32' ? 'executable shebang fixtures require POSIX' : false };

test('process waits for full output and returns native errors, not transport exceptions', nativeFake, async t => {
  const provider = await fake(t, { stdout: encoded(bytes(envelope())) });
  assert.equal((await provider.run({ operation: 'version' })).status, 'ok');
  const error = { ...envelope(), status: 'error', exitStatus: 1, diagnostics: [{ code: 'invalid_manifest', message: 'invalid static package manifest', remediation: 'repair manifest' }] };
  delete error.result;
  const failing = await fake(t, { stdout: encoded(bytes(error)), exit: 1 });
  assert.equal((await failing.run({ operation: 'version' })).status, 'error');
});

test('process rejects mismatched exit, missing reply, signal and malformed streams', nativeFake, async t => {
  for (const [scenario, code] of [
    [{ stdout: encoded(bytes(envelope())), exit: 1 }, 'invalid_response'],
    [{}, 'invalid_response'], [{ signal: true }, 'terminated'],
    [{ stdout: encoded('SECRET\n') }, 'invalid_response'],
    [{ stdout: encoded(bytes(envelope())), stderr: encoded(Buffer.from([0xff])) }, 'invalid_utf8'],
    [{ stdout: encoded(bytes(envelope())), stderr: encoded(Buffer.from([0xc2])) }, 'invalid_utf8'],
  ]) {
    const provider = await fake(t, scenario);
    await assert.rejects(provider.run({ operation: 'version' }), rejected(code));
  }
});

test('bounds both streams and kills timed out or cancelled processes', nativeFake, async t => {
  for (const scenario of [
    { stdout: encoded(Buffer.alloc(MAX_RESPONSE_BYTES + 1, 65)) },
    { stderr: encoded(Buffer.alloc(65537, 65)) },
  ]) {
    const provider = await fake(t, scenario);
    await assert.rejects(provider.run({ operation: 'version' }), rejected('output_limit'));
  }
  const provider = await fake(t, { hang: true });
  await assert.rejects(provider.run({ operation: 'version' }, { timeoutMs: 50 }), rejected('timeout'));
  const controller = new AbortController();
  const pending = provider.run({ operation: 'version' }, { signal: controller.signal });
  setTimeout(() => controller.abort('SECRET'), 50);
  await assert.rejects(pending, rejected('aborted'));
  await assert.rejects(provider.run({ operation: 'version' }, { signal: controller.signal }), rejected('aborted'));
});

test('explicit binary, explicit roots and bounded configuration; spawn errors are safe', async () => {
  assert.throws(() => createProvider('kunlun-pm'), rejected('invalid_configuration'));
  const provider = createProvider(resolve('/missing/SECRET/kunlun-pm'));
  await assert.rejects(provider.run({ operation: 'version' }), rejected('spawn_failed'));
  await assert.rejects(provider.run({ operation: 'detect', projectRoot: '.' }), rejected('invalid_configuration'));
  await assert.rejects(provider.run({ operation: 'version' }, { timeoutMs: Infinity }), rejected('invalid_configuration'));
});
