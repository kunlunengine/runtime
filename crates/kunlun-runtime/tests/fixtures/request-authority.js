// Adapter-neutral authority slice; setup and expectations live in the adjacent
// contract. Both requests execute these exact bytes, without adapter rewriting.
const handle = env.fs['public-data'];
const absolutePath = globalThis.requestAuthorityInputs.absolutePath;
if (typeof absolutePath !== 'string' || !absolutePath.startsWith('/'))
  throw new Error('missing absolute escape-path test input');
async function rejects(operation) {
  try { await operation(); return false; } catch (_) { return true; }
}
const observations = {
  optional_binding_absent: !Object.hasOwn(env.fs, 'missing-optional'),
  undeclared_binding_absent: !Object.hasOwn(env.fs, 'undeclared'),
  environment_frozen: Object.isFrozen(env),
  filesystem_map_frozen: Object.isFrozen(env.fs),
  http_map_frozen: Object.isFrozen(env.http),
  environment_null_prototype: Object.getPrototypeOf(env) === null,
  filesystem_map_null_prototype: Object.getPrototypeOf(env.fs) === null,
  http_map_null_prototype: Object.getPrototypeOf(env.http) === null,
  handle_frozen: Object.isFrozen(handle),
  environment_serialization_denied: await rejects(() => JSON.stringify(env)),
  handle_serialization_denied: await rejects(() => JSON.stringify(handle)),
  traversal_read_denied: await rejects(() => handle.readTextFile('../escape.txt')),
  absolute_read_denied: await rejects(() => handle.readTextFile(absolutePath)),
  missing_read_denied: await rejects(() => handle.readTextFile('missing.txt')),
  permitted_read: await handle.readTextFile('message.txt'),
  stale_handle_read_denied: globalThis.previousRequestHandle
    ? await rejects(() => globalThis.previousRequestHandle.readTextFile('message.txt'))
    : null
};
globalThis.previousRequestHandle = handle;
return JSON.stringify(observations);
