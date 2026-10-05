import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { readFile, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { parseArgs } from 'node:util';
import { createContext, Script } from 'node:vm';

const runtime = fileURLToPath(new URL('../../../', import.meta.url));
const fixtures = path.join(runtime, 'crates/kunlun-runtime/tests/fixtures');
const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');

export function bounded(promise, timeoutMs = 10_000) {
  let timer;
  const deadline = new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error('HTTP authority deadline exceeded')), timeoutMs);
  });
  return Promise.race([promise, deadline]).finally(() => clearTimeout(timer));
}

// This development check must not be mistaken for the reviewed-commit collector.
// No adapter, permission provider, or diagnostic text is simulated or normalized.
export function validateHttpReport(report, contract, hashes, adapter) {
  assert.equal(contract.schema_version, 1);
  assert.equal(contract.suite, 'request-authority-http/v1');
  assert.equal(contract.qualification, false);
  assert.equal(report.schema_version, 1);
  assert.equal(report.suite, contract.suite);
  assert.equal(report.adapter, adapter);
  assert.equal(report.status, 'development');
  assert.equal(report.qualification, false);
  assert.match(hashes.fixture_sha256, /^[0-9a-f]{64}$/);
  assert.match(hashes.contract_sha256, /^[0-9a-f]{64}$/);
  assert.match(hashes.revocation_sha256, /^[0-9a-f]{64}$/);
  assert.equal(report.fixture_sha256, hashes.fixture_sha256);
  assert.equal(report.contract_sha256, hashes.contract_sha256);
  assert.equal(report.revocation_sha256, hashes.revocation_sha256);
  assert.deepStrictEqual(report.observations, contract.expected_observations);
  assert.deepStrictEqual(report.lifecycle, contract.expected_lifecycle);
  assert.deepStrictEqual(report.requests, contract.expected_requests);
}

async function moduleHashes(core) {
  return Object.fromEntries(await Promise.all(
    ['index.js', 'application.js', 'authority.js'].map(async module => [
      module, sha256(await readFile(path.join(core, 'packages/runtime-node/dist', module))),
    ]),
  ));
}

export async function checkHttpAuthority(core, nativePath) {
  const [source, contractBytes, revocation, metadataBytes] = await Promise.all([
    readFile(path.join(fixtures, 'request-authority-http.js')),
    readFile(path.join(fixtures, 'request-authority-http.contract.json')),
    readFile(path.join(fixtures, 'request-authority-http-revocation.js')),
    readFile(path.join(core, 'packages/runtime-node/package.json')),
  ]);
  const contract = JSON.parse(contractBytes);
  const metadata = JSON.parse(metadataBytes);
  assert.equal(metadata.name, '@kunlun-js/runtime-node');
  const before = await moduleHashes(core);
  const report = {
    schema_version: 1,
    suite: contract.suite,
    adapter: 'node',
    status: 'failed',
    qualification: false,
    fixture_sha256: sha256(source),
    contract_sha256: sha256(contractBytes),
    revocation_sha256: sha256(revocation),
    node: {
      package: metadata.name,
      package_version: metadata.version,
      version: process.version,
      source_commit: execFileSync('git', ['rev-parse', 'HEAD'], {
        cwd: core, encoding: 'utf8', timeout: 30_000,
      }).trim(),
      modules: before,
    },
  };
  const requests = [];
  let acknowledgePending;
  const pendingReceipt = new Promise(resolve => { acknowledgePending = resolve; });
  let pendingResponse;
  const server = createServer(async (request, response) => {
    requests.push(request.url);
    try {
      // Consume actual uploads before replying, including non-replayable streams.
      const chunks = [];
      let bytes = 0;
      for await (const chunk of request) {
        bytes += chunk.length;
        assert.ok(bytes <= 1024 * 1024, 'Unbounded HTTP fixture upload');
        chunks.push(chunk);
      }
      if (request.url === '/pending') {
        pendingResponse = response;
        acknowledgePending();
      } else if (request.url === '/redirect') {
        response.writeHead(302, {
          location: `http://localhost:${server.address().port}/forbidden?auth=auth-private&provider=provider-private&billing=billing-private`,
        });
        response.end();
      } else if (request.url === '/same-origin' || request.url === '/loop') {
        response.writeHead(302, { location: request.url === '/loop' ? '/loop' : '/ok' });
        response.end();
      } else if (/^\/(?:rewrite\/30[123]|replay\/30[78])$/.test(request.url)) {
        response.writeHead(Number(request.url.slice(-3)), { location: '/echo' });
        response.end();
      } else if (request.url === '/echo') {
        const observed = {
          method: request.method,
          body: Buffer.concat(chunks).toString('utf8'),
          content_type: request.headers['content-type'] ?? null,
          content_encoding: request.headers['content-encoding'] ?? null,
          content_language: request.headers['content-language'] ?? null,
          content_location: request.headers['content-location'] ?? null,
          authorization: request.headers.authorization ?? null,
          cookie: request.headers.cookie ?? null,
          custom: request.headers['x-authority-custom'] ?? null,
        };
        const body = JSON.stringify(observed);
        // HEAD has no response body: reflect the same actual fields through headers.
        const mirrored = Object.fromEntries(Object.entries(observed)
          .filter(([name, value]) => name !== 'body' && value !== null)
          .map(([name, value]) => ['x-observed-' + name.replaceAll('_', '-'), value]));
        response.writeHead(200, {
          'content-type': 'application/json',
          'content-length': Buffer.byteLength(body),
          ...mirrored,
        });
        response.end(request.method === 'HEAD' ? undefined : body);
      } else {
        assert.equal(request.url, '/ok', 'Unexpected HTTP fixture destination');
        response.writeHead(200, { 'x-authority-probe': 'http' });
        response.end('你好, scoped HTTP');
      }
    } catch {
      report.error = 'HTTP fixture request failed';
      response.destroy();
    }
  });
  let authority;
  try {
    // Import the actual existing Core build without changing the unattached checkout.
    // Formal collectors must force-build it; this read-only check cannot qualify it.
    const { createRequestAuthority } = await bounded(import(pathToFileURL(
      path.join(core, 'packages/runtime-node/dist/index.js'),
    )));
    assert.equal(typeof createRequestAuthority, 'function');
    await bounded(assert.rejects(createRequestAuthority(contract.setup.declarations, { http: [] })));
    authority = await bounded(createRequestAuthority(contract.setup.declarations, {
      http: contract.setup.deployment_hosts,
    }));
    await bounded(new Promise((resolve, reject) => {
      server.once('error', reject);
      server.listen(0, '127.0.0.1', resolve);
    }));
    const realm = createContext({
      AbortController,
      ReadableStream,
      requestAuthorityHttpInputs: Object.freeze({ base: `http://127.0.0.1:${server.address().port}` }),
    });
    const probe = new Script(`(async function(env) {\n${source.toString('utf8')}\n})`, {
      filename: 'request-authority-http.js',
    }).runInContext(realm);
    report.observations = [];
    for (let request = 0; request < 2; request++) {
      const output = await bounded(authority.invoke(env => probe(env), {
        signal: AbortSignal.timeout(10_000),
      }));
      assert.equal(typeof output, 'string');
      report.observations.push(JSON.parse(output));
    }
    const revokeProbe = new Script(`(async function(env) {\n${revocation.toString('utf8')}\n})`, {
      filename: 'request-authority-http-revocation.js',
    }).runInContext(realm);
    const pending = authority.invoke(env => revokeProbe(env), {
      signal: AbortSignal.timeout(10_000),
    }).then(() => false, () => true);
    // If admission or transport fails early, do not hang waiting for server receipt.
    await bounded(Promise.race([
      pendingReceipt,
      pending.then(() => { throw new Error('Invocation ended before server receipt'); }),
    ]));
    await bounded(authority.close());
    const rejected = await bounded(pending);
    let admitted = false;
    await bounded(assert.rejects(authority.invoke(() => { admitted = true; })));
    report.lifecycle = {
      pending_rejected_before_response: rejected,
      later_admission_denied: !admitted,
    };
    pendingResponse.end();
  } catch {
    // Keep raw paths/URLs/credentials out of diagnostics and preserve any actual results.
    report.error = 'Real Node HTTP authority execution failed';
  } finally {
    try {
      await bounded(authority?.close());
    } catch {
      report.error = 'HTTP authority cleanup failed';
    }
    // Server teardown must run even if adapter cleanup fails or exceeds its deadline.
    pendingResponse?.end();
    if (server.listening) {
      try {
        await bounded(new Promise((resolve, reject) => {
          server.close(error => error ? reject(error) : resolve());
          server.closeAllConnections();
        }));
      } catch {
        report.error = 'HTTP fixture server cleanup failed';
      }
    }
  }
  report.requests = requests;
  try {
    assert.deepStrictEqual(await moduleHashes(core), before);
    assert.equal(
      execFileSync('git', ['rev-parse', 'HEAD'], {
        cwd: core, encoding: 'utf8', timeout: 30_000,
      }).trim(),
      report.node.source_commit,
    );
    assert.equal(sha256(await readFile(path.join(fixtures, 'request-authority-http.js'))), report.fixture_sha256);
    assert.equal(sha256(await readFile(path.join(fixtures, 'request-authority-http.contract.json'))), report.contract_sha256);
    assert.equal(sha256(await readFile(path.join(fixtures, 'request-authority-http-revocation.js'))), report.revocation_sha256);
    assert.equal(report.error, undefined);
    report.status = 'development';
    validateHttpReport(report, contract, report, 'node');
    if (nativePath) {
      const native = JSON.parse(await readFile(nativePath));
      validateHttpReport(native, contract, report, 'native');
      assert.deepStrictEqual(report.observations, native.observations);
      assert.deepStrictEqual(report.lifecycle, native.lifecycle);
      assert.deepStrictEqual(report.requests, native.requests);
    }
  } catch {
    report.status = 'failed';
    report.error ??= 'HTTP observations, traffic, source hashes, or native report differ from the contract';
  }
  return report;
}

async function main() {
  const { values } = parseArgs({
    options: {
      'core-root': { type: 'string' },
      native: { type: 'string' },
      output: { type: 'string' },
      help: { type: 'boolean', default: false },
    },
  });
  if (values.help) {
    console.log('Usage: node check-authority-http.mjs --core-root <existing Core build> [--native <native JSON>] [--output <new JSON>]');
    console.log('Read-only development check. No forced build or physical-platform qualification.');
    return;
  }
  assert.ok(values['core-root'], '--core-root is required');
  const report = await checkHttpAuthority(path.resolve(values['core-root']), values.native);
  if (values.output) await writeFile(values.output, JSON.stringify(report, null, 2) + '\n', { flag: 'wx' });
  console.log(JSON.stringify(report, null, 2));
  if (report.status !== 'development') process.exitCode = 1;
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  await main();
}
