import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { dependencyClosure, rejectAmbientLoaders, runtimeIdentity } from '../node-provenance.mjs';

test('runtime identity comes from the actual observation process', () => {
  const identity = runtimeIdentity();
  assert.equal(identity.version, process.version);
  assert.equal(identity.arch, process.arch);
  assert.equal(identity.platform, process.platform);
  assert.match(identity.executable_sha256, /^[0-9a-f]{64}$/);
  assert.equal(identity.libc, process.report.getReport().header.glibcVersionRuntime ?? null);
});

test('ambient loader controls fail closed without changing the environment', () => {
  for (const key of ['NODE_OPTIONS', 'NODE_PATH', 'npm_config_node_options',
    'LD_PRELOAD', 'LD_LIBRARY_PATH', 'DYLD_LIBRARY_PATH', 'DYLD_INSERT_LIBRARIES']) {
    const previous = process.env[key];
    try {
      process.env[key] = 'synthetic-loader';
      assert.throws(() => rejectAmbientLoaders(), /Ambient loader rejected/);
      assert.throws(() => runtimeIdentity(), /Ambient loader rejected/);
      assert.equal(process.env[key], 'synthetic-loader');
    } finally {
      if (previous === undefined) delete process.env[key];
      else process.env[key] = previous;
    }
  }
});

test('dependency provenance hashes recursive runtime files, not only package versions', t => {
  // Synthetic package layout tests hashing mechanics, not adapter permission policy.
  const target = path.join(process.cwd(), 'target');
  mkdirSync(target, { recursive: true });
  const core = mkdtempSync(path.join(target, 'authority-provenance-'));
  t.after(() => rmSync(core, { recursive: true, force: true }));
  const dist = path.join(core, 'packages/runtime-node/dist');
  const undici = path.join(core, 'packages/runtime-node/node_modules/undici');
  const dependency = path.join(undici, 'node_modules/transport-helper');
  mkdirSync(dist, { recursive: true });
  mkdirSync(dependency, { recursive: true });
  writeFileSync(path.join(undici, 'package.json'), JSON.stringify({
    name: 'undici', version: '1.0.0', dependencies: { 'transport-helper': '1.0.0' },
  }));
  writeFileSync(path.join(undici, 'index.js'), 'export const transport = true;\n');
  writeFileSync(path.join(dependency, 'package.json'), JSON.stringify({
    name: 'transport-helper', version: '1.0.0',
  }));
  writeFileSync(path.join(dependency, 'index.js'), 'export const allowed = true;\n');

  const before = dependencyClosure(core);
  assert.deepStrictEqual(Object.keys(before), ['transport-helper@1.0.0', 'undici@1.0.0']);
  for (const digest of Object.values(before)) assert.match(digest, /^[0-9a-f]{64}$/);
  assert.deepStrictEqual(dependencyClosure(core), before);
  const metadata = readFileSync(path.join(dependency, 'package.json'), 'utf8');
  writeFileSync(path.join(dependency, 'index.js'), 'export const allowed = false;\n');
  const after = dependencyClosure(core);
  assert.equal(after['undici@1.0.0'], before['undici@1.0.0']);
  assert.notEqual(after['transport-helper@1.0.0'], before['transport-helper@1.0.0']);
  assert.equal(readFileSync(path.join(dependency, 'package.json'), 'utf8'), metadata);
});
