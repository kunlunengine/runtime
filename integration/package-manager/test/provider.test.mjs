import test from 'node:test';
import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import { access, cp, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { createProvider } from '../adapter.mjs';

const binary = process.env.KUNLUN_PM_BINARY;
const projectRoot = fileURLToPath(new URL('../../../fixtures/package-manager-v1/workspace/', import.meta.url));
test('real native provider: version, detect, frozen plan, why and structured errors', {
  skip: !binary ? 'set KUNLUN_PM_BINARY to an absolute native binary path' : false,
}, async t => {
  await access(projectRoot);
  const provider = createProvider(binary);
  const version = await provider.run({ operation: 'version' });
  assert.equal(version.status, 'ok');
  assert.equal(version.result.capabilities.install, false);
  const detect = await provider.run({ operation: 'detect', projectRoot });
  assert.equal(detect.status, 'ok');
  const conformance = JSON.parse(await readFile(new URL('../../../fixtures/package-manager-v1/conformance.json', import.meta.url), 'utf8'));
  assert.deepEqual(detect.result.workspaceImporters, conformance.workspaceImporters);
  const plan = await provider.run({ operation: 'plan', projectRoot, frozen: true, ignoreScripts: true });
  assert.equal(plan.status, 'ok');
  assert.equal(plan.result.frozen, true);
  assert.equal(plan.result.readOnly, true);
  assert.ok(plan.result.nodes.length > 0);
  assert.deepEqual(plan.result.unbuiltPackages, conformance.knownUnbuiltImporters);
  const target = plan.result.nodes[0];
  const why = await provider.run({ operation: 'why', projectRoot, package: target.name });
  assert.equal(why.status, 'ok');
  assert.equal(why.result.package, target.name);
  assert.ok(why.result.paths.length > 0);
  const leaf = await provider.run({ operation: 'why', projectRoot, package: 'leaf' });
  assert.deepEqual(leaf.result.paths, conformance.whyLeafPaths);
  const ids = new Set(plan.result.nodes.map(n => n.id));
  const importers = new Set(plan.result.importers.map(i => i.path));
  for (const p of why.result.paths) {
    assert.ok(importers.has(p.importer));
    p.nodes.forEach(id => assert.ok(ids.has(id)));
    const importer = plan.result.importers.find(i => i.path === p.importer);
    assert.ok(importer.dependencies.some(d => d.target === p.nodes[0]));
    for (let index = 1; index < p.nodes.length; index++) {
      const previous = plan.result.nodes.find(n => n.id === p.nodes[index - 1]);
      assert.ok(previous.dependencies.some(d => d.target === p.nodes[index]));
    }
    assert.equal(plan.result.nodes.find(n => n.id === p.nodes.at(-1)).name, target.name);
  }
  const empty = await mkdtemp(join(tmpdir(), 'kunlun-empty-'));
  t.after(() => rm(empty, { recursive: true, force: true }));
  for (const operation of ['detect', 'plan', 'why']) {
    const response = await provider.run({ operation, projectRoot: empty, ...(operation === 'why' ? { package: 'missing' } : {}) });
    assert.equal(response.status, 'error');
    assert.equal(response.exitStatus, 1);
    assert.ok(response.diagnostics.length > 0);
  }
  for (const fixture of conformance.negativeCases) {
    const root = await mkdtemp(join(tmpdir(), 'kunlun-negative-'));
    t.after(() => rm(root, { recursive: true, force: true }));
    await cp(projectRoot, root, { recursive: true });
    const file = join(root, fixture.file);
    const original = fixture.write === undefined ? await readFile(file, 'utf8') : null;
    await writeFile(file, fixture.write ?? original.replaceAll(...fixture.replace));
    const response = await provider.run({ operation: 'plan', projectRoot: root, frozen: true, ignoreScripts: true });
    assert.equal(response.status, 'error', fixture.name);
    assert.equal(response.exitStatus, 1, fixture.name);
    assert.equal(response.diagnostics[0].code, fixture.code, fixture.name);
    assert.ok(!JSON.stringify(response).includes('TOP_SECRET'));
  }
});
