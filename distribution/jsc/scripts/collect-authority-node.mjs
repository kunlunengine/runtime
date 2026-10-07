// Transport only: execution and validation live in the real developer executors.
import { writeFile } from 'node:fs/promises';
import assert from 'node:assert/strict';
import { checkHttpAuthority } from './check-authority-http.mjs';
import { checkLifecycleAuthority } from './check-authority-lifecycle.mjs';
import { rejectAmbientLoaders } from './node-provenance.mjs';

rejectAmbientLoaders();
const [core, output, suite] = process.argv.slice(2);
if (!core || !output || !suite || process.argv.length !== 5) {
  throw new Error('Usage: node collect-authority-node.mjs <Core root> <new raw JSON> <suite>');
}
const executors = {
  'request-authority-http/v1': checkHttpAuthority,
  'request-authority-lifecycle/v1': checkLifecycleAuthority,
};
assert.ok(Object.hasOwn(executors, suite), 'Unsupported authority suite');
const report = await executors[suite](core);
await writeFile(output, `${JSON.stringify(report, null, 2)}\n`, { flag: 'wx' });
if (report.status !== 'development' || report.qualification !== false || Object.hasOwn(report, 'error')) {
  process.exitCode = 1;
}
