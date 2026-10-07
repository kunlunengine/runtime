import { createHash } from 'node:crypto';
import { readFileSync, readdirSync, realpathSync, statSync } from 'node:fs';
import { createRequire } from 'node:module';
import path from 'node:path';

export function rejectAmbientLoaders() {
  const forbidden = new Set(['NODE_OPTIONS', 'NODE_PATH', 'NPM_CONFIG_NODE_OPTIONS',
    'LD_PRELOAD', 'LD_LIBRARY_PATH', 'DYLD_LIBRARY_PATH',
    'DYLD_INSERT_LIBRARIES', 'DYLD_FRAMEWORK_PATH']);
  for (const [key, value] of Object.entries(process.env)) {
    if (value && forbidden.has(key.toUpperCase())) throw new Error(`Ambient loader rejected: ${key}`);
  }
}

export function runtimeIdentity() {
  rejectAmbientLoaders();
  return {
    version: process.version, arch: process.arch, platform: process.platform,
    executable_sha256: createHash('sha256').update(readFileSync(process.execPath)).digest('hex'),
    libc: process.report.getReport().header.glibcVersionRuntime ?? null,
  };
}

// Bind actual undici and its declared runtime dependency closure, not an ignored
// installation directory or absolute machine-specific pnpm paths.
export function dependencyClosure(core) {
  rejectAmbientLoaders();
  const packages = {};
  const visited = new Set();
  function visit(directory) {
    directory = realpathSync(directory);
    if (visited.has(directory)) return;
    visited.add(directory);
    const metadata = JSON.parse(readFileSync(path.join(directory, 'package.json')));
    const files = {};
    function walk(relative = '') {
      for (const name of readdirSync(path.join(directory, relative)).sort()) {
        if (name === 'node_modules') continue;
        const file = path.join(relative, name);
        if (statSync(path.join(directory, file)).isDirectory()) walk(file);
        else files[file.split(path.sep).join('/')] = createHash('sha256')
          .update(readFileSync(path.join(directory, file))).digest('hex');
      }
    }
    walk();
    const key = `${metadata.name}@${metadata.version}`;
    const digest = createHash('sha256').update(JSON.stringify(files)).digest('hex');
    if (packages[key] && packages[key] !== digest) throw new Error(`Conflicting dependency: ${key}`);
    packages[key] = digest;
    const require = createRequire(path.join(directory, 'package.json'));
    for (const name of Object.keys(metadata.dependencies ?? {}).sort()) {
      visit(path.dirname(require.resolve(`${name}/package.json`)));
    }
  }
  const require = createRequire(path.join(core, 'packages/runtime-node/dist/authority.js'));
  visit(path.dirname(require.resolve('undici/package.json')));
  return Object.fromEntries(Object.entries(packages).sort(([a], [b]) => a.localeCompare(b)));
}
