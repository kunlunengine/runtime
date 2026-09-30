// Adapter-neutral #53 probe. The harness admits a "public-data" filesystem
// binding containing message.txt ("public message"), omits "missing-optional",
// and invokes these same bytes in two successive request environments.
const handle = env.fs['public-data'];
if (!handle || env.fs['missing-optional'] !== undefined || env.fs.undeclared !== undefined)
  throw new Error('incorrect declaration/grant intersection');
if (!Object.isFrozen(env) || !Object.isFrozen(env.fs) || !Object.isFrozen(handle))
  throw new Error('mutable request projection');
async function mustDeny(operation) {
  let denied = false;
  try { await operation(); } catch (_) { denied = true; }
  if (!denied) throw new Error('authority escalation');
}
for (const value of [env, handle])
  await mustDeny(() => JSON.stringify(value));
for (const path of ['../message.txt', '/message.txt', 'missing.txt'])
  await mustDeny(() => handle.readTextFile(path));
if (globalThis.previousRequestHandle)
  await mustDeny(() => globalThis.previousRequestHandle.readTextFile('message.txt'));
if (await handle.readTextFile('message.txt') !== 'public message')
  throw new Error('incorrect scoped read');
globalThis.previousRequestHandle = handle;
return 'request-authority-ok';
