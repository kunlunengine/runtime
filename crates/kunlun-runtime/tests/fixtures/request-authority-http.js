// One unchanged async function body for native JSC and the real Core Node provider.
// The host supplies only an ephemeral loopback URL, never a replacement permission API.
const base = requestAuthorityHttpInputs.base;
const handle = env.http['127.0.0.1'];
const privateUrl = base.replace('127.0.0.1', 'localhost') +
  '/forbidden?auth=auth-private&provider=provider-private&billing=billing-private';
const observations = {};
async function denial(operation) {
  try {
    const response = await operation();
    if (response && typeof response.status === 'number') {
      await response.body?.cancel();
      return 'returned:' + response.status;
    }
    return 'allowed';
  } catch (error) {
    const diagnostic = String(error);
    for (const marker of [
      'auth-private', 'provider-private', 'billing-private',
      'private-host.example.test', '/forbidden', 'localhost',
    ]) {
      if (diagnostic.includes(marker)) throw Error('HTTP denial diagnostic leaked private input');
    }
    return 'denied';
  }
}
observations.projection = Object.isFrozen(env) && Object.isFrozen(env.http) &&
  Object.isFrozen(handle) && Object.getPrototypeOf(handle) === null &&
  Object.keys(env.http).sort().join(',') === '127.0.0.1,localhost' &&
  Object.keys(env.fs).length === 0;
observations.serialization = await denial(() => JSON.stringify(handle));
const response = await handle.fetch(base + '/ok');
observations.allowed = {
  status: response.status,
  header: response.headers.get('x-authority-probe'),
  body: await response.text(),
};
observations.other_admitted_host = await denial(() => handle.fetch(privateUrl));
observations.undeclared_host = await denial(() =>
  handle.fetch('http://private-host.example.test/forbidden?auth=auth-private'));
observations.credentials = await denial(() =>
  handle.fetch(base.replace('http://', 'http://auth-private:provider-private@') + '/forbidden'));
observations.unsupported_scheme = await denial(() => handle.fetch('file:///forbidden'));
const aborted = new AbortController();
aborted.abort();
observations.pre_aborted = await denial(() => handle.fetch(base + '/aborted', { signal: aborted.signal }));
const manual = await handle.fetch(base + '/redirect', { redirect: 'manual' });
observations.manual_redirect = {
  status: manual.status,
  location_preserved: manual.headers.get('location') === privateUrl,
  body: await manual.text(),
};
// Returning a 302 for redirect:'follow' is an observable mismatch, not a denial.
observations.redirect_escape = await denial(() => handle.fetch(base + '/redirect'));
observations.previous_request = globalThis.retainedHttpAuthority
  ? await denial(() => globalThis.retainedHttpAuthority.fetch(base + '/forbidden'))
  : 'not-retained';
globalThis.retainedHttpAuthority = handle;
return JSON.stringify(observations);
