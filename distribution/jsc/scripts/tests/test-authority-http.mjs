import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { bounded, validateHttpReport } from '../check-authority-http.mjs';

const contract = JSON.parse(await readFile(new URL(
  '../../../../crates/kunlun-runtime/tests/fixtures/request-authority-http.contract.json',
  import.meta.url,
)));
const hashes = {
  fixture_sha256: 'a'.repeat(64), contract_sha256: 'b'.repeat(64),
  revocation_sha256: 'c'.repeat(64),
};
function report() {
  return {
    schema_version: 1,
    suite: contract.suite,
    adapter: 'native',
    status: 'development',
    qualification: false,
    ...hashes,
    observations: structuredClone(contract.expected_observations),
    lifecycle: structuredClone(contract.expected_lifecycle),
    requests: [...contract.expected_requests],
  };
}

test('accepts exact development observations, not qualification', () => {
  validateHttpReport(report(), contract, hashes, 'native');
});

test('rejects a returned redirect instead of denial', () => {
  const actual = report();
  actual.observations[0].redirect_escape = 'returned:302';
  assert.throws(() => validateHttpReport(actual, contract, hashes, 'native'));
});

// Synthetic report tests validate comparison policy, not adapter compatibility.
for (const [name, corrupt] of [
  ['default follow returning a redirect', observation => { observation.default_follow.status = 302; }],
  ['explicit follow allowing an escaped destination', observation => { observation.explicit_redirect_escape = 'returned:302'; }],
  ['error mode returning a response', observation => { observation.redirect_error = 'returned:302'; }],
  ['incorrect final URL', observation => { observation.explicit_follow.final_url = false; }],
  ['HEAD 303 rewritten to GET', observation => { observation.head_303.method = 'GET'; }],
  ['GET 303 losing body metadata headers', observation => { observation.get_303.content_language = null; }],
  ['rewritten POST retaining body headers', observation => { observation.post_302.content_location = '/body-metadata'; }],
  ['buffered upload not replayed', observation => { observation.buffered_308.body = ''; }],
  ['streamed POST incorrectly rewritten on 302', observation => { observation.streamed_302 = 'returned:200'; }],
  ['streamed POST not rewritten on 303', observation => { observation.streamed_303.method = 'POST'; }],
  ['streamed upload replayed', observation => { observation.streamed_307 = 'returned:200'; }],
  ...['content_type', 'content_encoding', 'content_language', 'content_location',
    'authorization', 'cookie', 'custom'].map(field => [
    `HEAD 303 losing ${field}`, observation => { observation.head_303[field] = null; },
  ]),
]) {
  test(`rejects ${name} in either request`, () => {
    for (const request of [0, 1]) {
      const actual = report();
      corrupt(actual.observations[request]);
      assert.throws(() => validateHttpReport(actual, contract, hashes, 'native'));
    }
  });
}

test('rejects redirect-limit off-by-one traffic even with denial observations', () => {
  for (const extra of [false, true]) {
    const actual = report();
    const loop = actual.requests.indexOf('/loop');
    if (extra) actual.requests.splice(loop, 0, '/loop');
    else actual.requests.splice(loop, 1);
    assert.throws(() => validateHttpReport(actual, contract, hashes, 'native'));
  }
});

test('rejects traffic reaching a forbidden destination', () => {
  const actual = report();
  actual.requests.push('/forbidden');
  assert.throws(() => validateHttpReport(actual, contract, hashes, 'native'));
});

test('rejects missing observations, source drift and qualification claims', () => {
  for (const [field, value] of [
    ['observations', []], ['fixture_sha256', 'c'.repeat(64)],
    ['contract_sha256', 'c'.repeat(64)], ['qualification', true],
    ['status', 'passed'], ['adapter', 'node'], ['schema_version', 2],
    ['revocation_sha256', 'd'.repeat(64)], ['lifecycle', {}],
  ]) {
    assert.throws(() => validateHttpReport({ ...report(), [field]: value }, contract, hashes, 'native'));
  }
});

test('watchdog rejects a stalled adapter independently of its AbortSignal', async () => {
  await assert.rejects(bounded(new Promise(() => {}), 1), /deadline exceeded/);
});

test('watchdog preserves completion and rejection', async () => {
  assert.equal(await bounded(Promise.resolve('completed'), 1000), 'completed');
  const failure = new Error('operation failed');
  await assert.rejects(bounded(Promise.reject(failure), 1000), error => error === failure);
});
