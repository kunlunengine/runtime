import assert from 'node:assert/strict';
import { runtimeIdentity, dependencyClosure } from './node-provenance.mjs';
import { readFile, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { parseArgs } from 'node:util';
import { createContext, Script } from 'node:vm';
import { bounded, moduleHashes, sha256, sourceCommit } from './check-authority-http.mjs';

const fixtures = fileURLToPath(new URL('../../../crates/kunlun-runtime/tests/fixtures/', import.meta.url));
const fixtureName = 'request-authority-lifecycle.js';
const contractName = 'request-authority-lifecycle.contract.json';

export function validateLifecycleReport(report, contract, hashes, adapter) {
  assert.equal(Object.hasOwn(report, 'error'), false);
  assert.equal(contract.schema_version, 1);
  assert.equal(contract.suite, 'request-authority-lifecycle/v1');
  for (const field of ['expected_progress', 'expected_lifecycle', 'expected_traffic',
    'expected_concurrency', 'expected_concurrent_traffic', 'expected_native_assertions']) {
    assert.ok(contract[field] && typeof contract[field] === 'object', `Missing contract ${field}`);
  }
  assert.equal(report.schema_version, 1);
  assert.equal(report.suite, contract.suite);
  assert.equal(report.adapter, adapter);
  assert.equal(report.status, 'development');
  assert.equal(report.qualification, false);
  for (const field of ['fixture_sha256', 'contract_sha256']) {
    assert.match(hashes[field], /^[0-9a-f]{64}$/);
    assert.equal(report[field], hashes[field]);
  }
  assert.deepStrictEqual(report.observations, contract.expected_progress);
  assert.deepStrictEqual(report.lifecycle, contract.expected_lifecycle);
  assert.deepStrictEqual(report.traffic, contract.expected_traffic);
  assert.deepStrictEqual(report.concurrency, contract.expected_concurrency);
  assert.deepStrictEqual(report.concurrent_traffic, contract.expected_concurrent_traffic);
  // Native counters/context ownership are real additional assertions, not
  // invented equivalents for Node, which has no public host-context/counter API.
  if (adapter === 'native') {
    assert.deepStrictEqual(report.native_assertions, contract.expected_native_assertions);
  } else {
    assert.equal(report.native_assertions, undefined);
  }
}

function createFixture() {
  const traffic = [];
  let acknowledge;
  const receipt = new Promise(resolve => { acknowledge = resolve; });
  let serverError;
  const server = createServer((request, response) => {
    traffic.push(`${request.method} ${request.url}`);
    if (request.method !== 'GET') {
      serverError = 'Unexpected lifecycle method';
      response.destroy();
    } else if (request.url === '/body') {
      response.writeHead(200, { 'transfer-encoding': 'chunked', connection: 'close' });
      response.write('first');
      // No final chunk: both the body read and /ready must be cancelled by the
      // real authority, not completed by fixture teardown.
    } else if (request.url === '/ready?pending=true') {
      acknowledge();
      // Withhold headers as well; reaching this route is the progress barrier.
    } else if (request.url === '/fresh') {
      response.writeHead(200, { connection: 'close' });
      response.end('fresh');
    } else {
      serverError = 'Unexpected lifecycle destination';
      response.destroy();
    }
  });
  return { server, traffic, receipt, check: () => assert.equal(serverError, undefined) };
}

async function listen(server) {
  await bounded(new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  }));
}

async function closeServer(server, force = false) {
  if (!server.listening) {
    server.closeAllConnections();
    return;
  }
  await bounded(new Promise((resolve, reject) => {
    server.close(error => error ? reject(error) : resolve());
    if (force) server.closeAllConnections();
  }));
}

async function runPhase(createRequestAuthority, source, contract, phase) {
  const fixture = createFixture();
  const { server, traffic, receipt } = fixture;
  let authority;
  const result = {};
  try {
    authority = await bounded(createRequestAuthority(contract.setup.declarations, {
      http: contract.setup.deployment_hosts,
    }));
    await listen(server);
    const realm = createContext({
      TextDecoder,
      requestAuthorityLifecycleInputs: Object.freeze({
        base: `http://127.0.0.1:${server.address().port}`, phase,
      }),
    });
    const probe = new Script(`(async function(env) {\n${source}\n})`, {
      filename: fixtureName,
    }).runInContext(realm);
    const controller = new AbortController();
    const pending = authority.invoke(env => probe(env), { signal: controller.signal })
      .then(() => 'resolved', () => 'rejected');
    await bounded(Promise.race([
      receipt,
      pending.then(() => { throw new Error('Invocation ended before post-header receipt'); }),
    ]));
    assert.equal(new Script(
      'requestAuthorityLifecycleProgress.nextReadPending',
    ).runInContext(realm), true);
    result.progress = JSON.parse(new Script(
      'requestAuthorityLifecycleBarrier',
    ).runInContext(realm));
    assert.deepStrictEqual(result.progress, contract.expected_progress[phase]);

    if (phase === 'application') {
      await bounded(authority.close());
      assert.equal(await bounded(pending), 'rejected');
      result.invocation = 'rejected';
      let admitted = false;
      await bounded(assert.rejects(authority.invoke(() => { admitted = true; })));
      result.laterAdmissionDenied = !admitted;
    } else {
      controller.abort();
      assert.equal(await bounded(pending), 'rejected');
      assert.equal(controller.signal.aborted, true);
      result.invocation = 'cancelled';
      realm.requestAuthorityLifecycleInputs = Object.freeze({
        base: `http://127.0.0.1:${server.address().port}`, phase: 'survivor',
      });
      const survivor = await bounded(authority.invoke(env => probe(env)));
      assert.equal(typeof survivor, 'string');
      result.survivor = JSON.parse(survivor);
      assert.deepStrictEqual(result.survivor, contract.expected_progress.survivor);
    }
    await bounded(authority.close());
    await bounded(authority.close());
    result.repeatedCloseCompleted = true;
    // On the success path, require the adapter to release its owned sockets.
    // Do not force-close fixture sockets and then call that successful teardown.
    await closeServer(server);
    fixture.check();
    assert.deepStrictEqual(traffic, contract.expected_traffic[phase]);
    result.traffic = traffic;
    return result;
  } finally {
    try {
      await bounded(authority?.close());
    } finally {
      // Failure cleanup must interrupt stalled sockets even if the adapter
      // failed. This path does not generate a successful observation.
      await closeServer(server, true);
    }
  }
}

async function runConcurrency(createRequestAuthority, source, contract) {
  const blocked = createFixture();
  const independent = createFixture();
  const authorities = [];
  try {
    for (let owner = 0; owner < 2; owner++) {
      authorities.push(await bounded(createRequestAuthority(contract.setup.declarations, {
        http: contract.setup.deployment_hosts,
      })));
    }
    await Promise.all([listen(blocked.server), listen(independent.server)]);
    const makeProbe = (server, phase) => {
      const realm = createContext({
        TextDecoder,
        requestAuthorityLifecycleInputs: Object.freeze({
          base: `http://127.0.0.1:${server.address().port}`, phase,
        }),
      });
      return new Script(`(async function(env) {\n${source}\n})`, {
        filename: fixtureName,
      }).runInContext(realm);
    };
    const aProbe = makeProbe(blocked.server, 'application');
    const bProbe = makeProbe(independent.server, 'independent');
    let aSettled = false;
    const pending = authorities[0].invoke(env => aProbe(env))
      .then(() => 'resolved', () => 'rejected')
      .then(result => { aSettled = true; return result; });
    await bounded(Promise.race([
      blocked.receipt,
      pending.then(() => { throw new Error('Concurrent invocation ended before receipt'); }),
    ]));
    const invokeB = async () => {
      const result = await bounded(authorities[1].invoke(env => bProbe(env)));
      assert.equal(typeof result, 'string');
      return JSON.parse(result);
    };
    const whilePending = await invokeB();
    assert.equal(aSettled, false, 'Independent result must precede blocked invocation completion');
    await bounded(authorities[0].close());
    const aResult = await bounded(pending);
    const afterRevoked = await invokeB();
    const concurrency = {
      while_other_pending: whilePending,
      after_other_revoked: afterRevoked,
      other_invocation_rejected: aResult === 'rejected',
      independent_admission_preserved: afterRevoked.fresh === 'fresh',
    };
    assert.deepStrictEqual(concurrency, contract.expected_concurrency);
    await bounded(authorities[1].close());
    await Promise.all([closeServer(blocked.server), closeServer(independent.server)]);
    blocked.check();
    independent.check();
    const traffic = { blocked: blocked.traffic, independent: independent.traffic };
    assert.deepStrictEqual(traffic, contract.expected_concurrent_traffic);
    return { concurrency, traffic };
  } finally {
    try {
      await Promise.all(authorities.map(authority => bounded(authority.close())));
    } finally {
      await Promise.all([closeServer(blocked.server, true), closeServer(independent.server, true)]);
    }
  }
}

export async function checkLifecycleAuthority(core, nativePath) {
  const [sourceBytes, contractBytes, metadataBytes] = await Promise.all([
    readFile(path.join(fixtures, fixtureName)),
    readFile(path.join(fixtures, contractName)),
    readFile(path.join(core, 'packages/runtime-node/package.json')),
  ]);
  const contract = JSON.parse(contractBytes);
  const metadata = JSON.parse(metadataBytes);
  assert.equal(metadata.name, '@kunlun-js/runtime-node');
  const before = await moduleHashes(core);
  const report = {
    schema_version: 1, suite: contract.suite, adapter: 'node',
    status: 'failed', qualification: false,
    fixture_sha256: sha256(sourceBytes), contract_sha256: sha256(contractBytes),
    node: {
      package: metadata.name, package_version: metadata.version,
      version: process.version, source_commit: sourceCommit(core), modules: before,
      runtime: runtimeIdentity(), dependencies: dependencyClosure(core),
    },
  };
  try {
    const { createRequestAuthority } = await bounded(import(pathToFileURL(
      path.join(core, 'packages/runtime-node/dist/index.js'),
    )));
    assert.equal(typeof createRequestAuthority, 'function');
    const application = await runPhase(createRequestAuthority, sourceBytes.toString('utf8'), contract, 'application');
    const request = await runPhase(createRequestAuthority, sourceBytes.toString('utf8'), contract, 'request');
    report.observations = {
      application: application.progress, request: request.progress, survivor: request.survivor,
    };
    report.lifecycle = {
      application_invocation: application.invocation,
      request_invocation: request.invocation,
      later_application_admission_denied: application.laterAdmissionDenied,
      surviving_request_admitted: request.survivor.fresh === 'fresh',
      repeated_close_completed: application.repeatedCloseCompleted && request.repeatedCloseCompleted,
    };
    report.traffic = { application: application.traffic, request: request.traffic };
    const concurrent = await runConcurrency(createRequestAuthority, sourceBytes.toString('utf8'), contract);
    report.concurrency = concurrent.concurrency;
    report.concurrent_traffic = concurrent.traffic;
    assert.deepStrictEqual(await moduleHashes(core), before);
    assert.equal(sourceCommit(core), report.node.source_commit);
    assert.equal(sha256(await readFile(path.join(fixtures, fixtureName))), report.fixture_sha256);
    assert.equal(sha256(await readFile(path.join(fixtures, contractName))), report.contract_sha256);
    assert.deepStrictEqual(dependencyClosure(core), report.node.dependencies);
    report.status = 'development';
    validateLifecycleReport(report, contract, report, 'node');
    if (nativePath) {
      const native = JSON.parse(await readFile(nativePath));
      validateLifecycleReport(native, contract, report, 'native');
      for (const field of ['observations', 'lifecycle', 'traffic', 'concurrency', 'concurrent_traffic']) {
        assert.deepStrictEqual(report[field], native[field]);
      }
    }
  } catch {
    report.status = 'failed';
    // Never reflect private context, request identities, URLs or transport causes.
    report.error = 'Real Node lifecycle execution, cleanup, identities, or observations failed';
  }
  return report;
}

async function main() {
  const { values } = parseArgs({
    options: {
      'core-root': { type: 'string' }, native: { type: 'string' }, output: { type: 'string' },
      help: { type: 'boolean', default: false },
    },
  });
  if (values.help) {
    console.log('Usage: node check-authority-lifecycle.mjs --core-root <existing Core build> [--native <native JSON>] [--output <new JSON>]');
    console.log('Read-only development check; no host-context API or physical-platform qualification claim.');
    return;
  }
  assert.ok(values['core-root'], '--core-root is required');
  const report = await checkLifecycleAuthority(path.resolve(values['core-root']), values.native);
  if (values.output) await writeFile(values.output, JSON.stringify(report, null, 2) + '\n', { flag: 'wx' });
  console.log(JSON.stringify(report, null, 2));
  if (report.status !== 'development') process.exitCode = 1;
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  await main();
}
