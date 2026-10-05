import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { createContext, Script } from 'node:vm';
import { validateLifecycleReport } from '../check-authority-lifecycle.mjs';
import { bounded, sha256 } from '../check-authority-http.mjs';

const fixtures = new URL('../../../../crates/kunlun-runtime/tests/fixtures/', import.meta.url);
const source = await readFile(new URL('request-authority-lifecycle.js', fixtures));
const contractBytes = await readFile(new URL('request-authority-lifecycle.contract.json', fixtures));
const contract = JSON.parse(contractBytes);
const hashes = { fixture_sha256: sha256(source), contract_sha256: sha256(contractBytes) };
const valid = adapter => ({
  schema_version: 1, suite: contract.suite, adapter,
  status: 'development', qualification: false, ...hashes,
  observations: structuredClone(contract.expected_progress),
  lifecycle: structuredClone(contract.expected_lifecycle),
  traffic: structuredClone(contract.expected_traffic),
  concurrency: structuredClone(contract.expected_concurrency),
  concurrent_traffic: structuredClone(contract.expected_concurrent_traffic),
  ...(adapter === 'native' ? { native_assertions: structuredClone(contract.expected_native_assertions) } : {}),
});

test('accepts exact independent Node/native development observations', () => {
  for (const adapter of ['node', 'native']) {
    validateLifecycleReport(valid(adapter), contract, hashes, adapter);
  }
});

const mutations = {
  'wrong suite': report => { report.suite = 'request-authority-http/v1'; },
  'wrong adapter': report => { report.adapter = 'native'; },
  'failed execution': report => { report.status = 'failed'; },
  'invented qualification': report => { report.qualification = true; },
  'wrong probe bytes': report => { report.fixture_sha256 = 'a'.repeat(64); },
  'wrong contract bytes': report => { report.contract_sha256 = 'b'.repeat(64); },
  'missing observations': report => { delete report.observations; },
  'extra observation': report => { report.observations.extra = true; },
  'confidential env field': report => { report.observations.request.contextAbsent = false; },
  'early body EOF': report => { report.observations.application.nextReadPending = false; },
  'wrong first chunk': report => { report.observations.request.firstChunk = 'private'; },
  'stale body allowed': report => { report.observations.survivor.staleBodyDenied = false; },
  'stale handle allowed': report => { report.observations.survivor.staleHandleDenied = false; },
  'fresh request denied': report => { report.lifecycle.surviving_request_admitted = false; },
  'application admission allowed': report => { report.lifecycle.later_application_admission_denied = false; },
  'numeric boolean': report => { report.lifecycle.repeated_close_completed = 1; },
  'unexpected traffic': report => { report.traffic.request.push('GET /forbidden'); },
  'reordered traffic': report => { report.traffic.application.reverse(); },
  'missing traffic': report => { report.traffic.request.pop(); },
  'cross-realm global exposed': report => { report.concurrency.while_other_pending.retainedAbsent = false; },
  'independent invocation denied': report => { report.concurrency.independent_admission_preserved = false; },
  'blocked invocation survived revocation': report => { report.concurrency.other_invocation_rejected = false; },
  'extra concurrent traffic': report => { report.concurrent_traffic.independent.push('GET /forbidden'); },
  'fake Node counters': report => { report.native_assertions = contract.expected_native_assertions; },
};
for (const [name, mutate] of Object.entries(mutations)) {
  test(`rejects ${name}`, () => {
    const report = valid('node');
    mutate(report);
    assert.throws(() => validateLifecycleReport(report, contract, hashes, 'node'));
  });
}

test('native cleanup and context assertions remain mandatory and exact', () => {
  const report = valid('native');
  delete report.native_assertions;
  assert.throws(() => validateLifecycleReport(report, contract, hashes, 'native'));
  report.native_assertions = structuredClone(contract.expected_native_assertions);
  report.native_assertions.resources_empty_after_shutdown = false;
  assert.throws(() => validateLifecycleReport(report, contract, hashes, 'native'));
});

test('invalid expected digest fails rather than hiding corpus changes', () => {
  assert.throws(() => validateLifecycleReport(valid('node'), contract, {
    ...hashes, fixture_sha256: 'not-a-sha256',
  }, 'node'));
});

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

// These deliberately synthetic readers test the probe itself. They are not
// adapters, grant providers, or evidence of Node/native security compatibility.
function executeProbe(secondRead, phase = 'application', metadata = {}) {
  const ready = deferred();
  const ack = deferred();
  const requests = [];
  let reads = 0;
  const reader = {
    read: () => ++reads === 1
      ? Promise.resolve({ value: new TextEncoder().encode('first'), done: false })
      : secondRead(),
  };
  const handle = Object.assign(Object.create(null), metadata, {
    fetch: async url => {
      const route = url.slice('http://fixture.test'.length);
      requests.push(route);
      if (route === '/body') return { status: 200, body: { getReader: () => reader } };
      if (route === '/fresh') return { text: async () => 'fresh' };
      assert.equal(route, '/ready?pending=true');
      ready.resolve();
      return ack.promise;
    },
  });
  const env = Object.assign(Object.create(null), {
    fs: Object.freeze(Object.create(null)),
    http: Object.freeze(Object.assign(Object.create(null), { '127.0.0.1': Object.freeze(handle) })),
  });
  Object.defineProperty(env, 'toJSON', { value: () => { throw Error('opaque'); } });
  Object.freeze(env);
  const realm = createContext({
    TextDecoder, env,
    requestAuthorityLifecycleInputs: Object.freeze({ base: 'http://fixture.test', phase }),
  });
  const execution = new Script(`(async function(env) {\n${source}\n})(env)`).runInContext(realm);
  return { realm, execution, requests, ready, ack };
}

for (const [name, read] of [
  ['immediate EOF', () => Promise.resolve({ done: true })],
  ['immediate rejection', () => Promise.reject(Error('synthetic read failure'))],
]) {
  test(`the actual probe cannot claim a pending barrier after ${name}`, async () => {
    const probe = executeProbe(read);
    await bounded(assert.rejects(probe.execution, /body read settled before cancellation barrier/));
    assert.deepStrictEqual(probe.requests, ['/body']);
    assert.equal(probe.realm.requestAuthorityLifecycleBarrier, undefined);
    assert.equal(probe.realm.requestAuthorityLifecycleProgress.nextReadPending, false);
  });
}

test('the actual probe snapshots a pending read and rejects settlement during acknowledgement', async () => {
  const read = deferred();
  const probe = executeProbe(() => read.promise);
  const ended = assert.rejects(probe.execution, /body read settled before cancellation barrier/);
  await bounded(probe.ready.promise);
  const snapshot = JSON.parse(probe.realm.requestAuthorityLifecycleBarrier);
  assert.deepStrictEqual(snapshot, contract.expected_progress.application);
  assert.equal(probe.realm.requestAuthorityLifecycleProgress.nextReadPending, true);
  read.resolve({ done: true });
  await bounded(ended);
  assert.equal(probe.realm.requestAuthorityLifecycleProgress.nextReadPending, false);
  assert.deepStrictEqual(JSON.parse(probe.realm.requestAuthorityLifecycleBarrier), snapshot);
});

for (const [name, metadata, field] of [
  ['nested secret', { metadata: { owner: 'private-A-auth' } }, 'secretStringsAbsent'],
  ['symbol secret', { [Symbol('private-provider')]: 'opaque' }, 'secretStringsAbsent'],
  ['symbol value', { metadata: { owner: Symbol('private-B-billing') } }, 'secretStringsAbsent'],
  ['nested context', { metadata: { auth: 'opaque' } }, 'contextAbsent'],
  ['prototype secret', { metadata: Object.create({ owner: 'private-B-billing' }) }, 'secretStringsAbsent'],
]) {
  test(`the actual opacity traversal detects ${name}`, async () => {
    const probe = executeProbe(() => new Promise(() => {}), 'independent', metadata);
    const observation = JSON.parse(await bounded(probe.execution));
    assert.equal(observation[field], false);
    assert.deepStrictEqual(probe.requests, ['/fresh']);
  });
}

test('opacity traversal handles cycles without invoking getters', async () => {
  let getterCalls = 0;
  const metadata = {};
  metadata.cycle = metadata;
  Object.defineProperty(metadata, 'auth', {
    get() { getterCalls++; return 'private-A-auth'; },
  });
  const probe = executeProbe(() => new Promise(() => {}), 'independent', { metadata });
  const observation = JSON.parse(await bounded(probe.execution));
  assert.equal(observation.contextAbsent, false);
  assert.equal(getterCalls, 0);
});
