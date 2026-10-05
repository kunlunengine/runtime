// This function body is shared verbatim by both executors. No host identity,
// provider API, secret API, timers, or native bridge is used here.
const inputs = requestAuthorityLifecycleInputs;
const denied = async operation => {
  try { await operation(); return false; } catch (_) { return true; }
};
if (inputs.phase === 'survivor') {
  const staleHandleDenied = await denied(() =>
    requestAuthorityLifecycleRetained.handle.fetch(inputs.base + '/forbidden'));
  const staleBodyDenied = await denied(() =>
    requestAuthorityLifecycleRetained.reader.read());
  const response = await env.http['127.0.0.1'].fetch(inputs.base + '/fresh');
  return JSON.stringify({
    phase: 'survivor', staleHandleDenied, staleBodyDenied,
    fresh: await response.text()
  });
}
let serializationDenied = false;
try { JSON.stringify(env); } catch (_) { serializationDenied = true; }
const keys = Object.keys(env).sort();
const forbidden = ['auth', 'provider', 'billing', 'context', 'identity', 'scope'];
let contextAbsent = true;
const exposedStrings = [];
// Inspect reachable data and prototypes without calling getters or toJSON.
// Functions' closures are intentionally not a public identity/context API.
const seen = new WeakSet();
const pending = [env];
let visited = 0;
while (pending.length) {
  const object = pending.pop();
  if (typeof object === 'string' || typeof object === 'symbol') {
    exposedStrings.push(String(object));
    continue;
  }
  if (object === null || !['object', 'function'].includes(typeof object) || seen.has(object)) continue;
  seen.add(object);
  if (++visited > 1024) throw Error('unbounded authority metadata');
  for (const key of Reflect.ownKeys(object)) {
    exposedStrings.push(String(key));
    if (forbidden.includes(key)) contextAbsent = false;
    const descriptor = Object.getOwnPropertyDescriptor(object, key);
    if ('value' in descriptor) pending.push(descriptor.value);
  }
  pending.push(Object.getPrototypeOf(object));
}
const secretStringsAbsent = exposedStrings.every(value => !value.includes('private-'));
if (inputs.phase === 'independent') {
  const retainedAbsent = !('requestAuthorityLifecycleRetained' in globalThis);
  const progressAbsent = !('requestAuthorityLifecycleProgress' in globalThis) &&
    !('requestAuthorityLifecycleBarrier' in globalThis);
  const frozen = Object.isFrozen(env) && [env.fs, env.http].every(namespace =>
    Object.isFrozen(namespace) && Object.keys(namespace).every(key =>
      Object.isFrozen(namespace[key])));
  const response = await env.http['127.0.0.1'].fetch(inputs.base + '/fresh');
  return JSON.stringify({
    phase: 'independent', keys, contextAbsent, secretStringsAbsent,
    serializationDenied, frozen, retainedAbsent, progressAbsent,
    fresh: await response.text()
  });
}
const observation = {
  phase: inputs.phase, keys, contextAbsent, secretStringsAbsent,
  serializationDenied, headers: false, firstChunk: '', nextReadPending: false
};
globalThis.requestAuthorityLifecycleProgress = observation;
const handle = env.http['127.0.0.1'];
const response = await handle.fetch(inputs.base + '/body');
observation.headers = response.status === 200;
const reader = response.body.getReader();
globalThis.requestAuthorityLifecycleRetained = { handle, reader };
const first = await reader.read();
observation.firstChunk = new TextDecoder().decode(first.value);
let settled = false;
const next = reader.read().then(
  value => {
    settled = true;
    observation.nextReadPending = false;
    return { rejected: false, done: value.done };
  },
  () => {
    settled = true;
    observation.nextReadPending = false;
    return { rejected: true };
  }
);
// Let an already-settled read run its reaction before claiming a pending read.
await Promise.resolve();
if (settled) throw Error('body read settled before cancellation barrier');
observation.nextReadPending = true;
globalThis.requestAuthorityLifecycleBarrier = JSON.stringify(observation);
// Starting the read before sending this ACK makes receipt a host-side barrier.
// The server never sends a second chunk until the executor has finished.
const ack = await Promise.race([
  handle.fetch(inputs.base + '/ready?pending=true'),
  next.then(() => { throw Error('body read settled before cancellation barrier'); })
]);
await ack.text();
observation.nextReadRejected = (await next).rejected;
return JSON.stringify(observation);
