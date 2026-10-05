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
observations.explicit_redirect_escape = await denial(() =>
  handle.fetch(base + '/redirect', { redirect: 'follow' }));
async function follow(init) {
  const followed = await handle.fetch(base + '/same-origin', init);
  return {
    status: followed.status,
    redirected: followed.redirected,
    final_url: followed.url === base + '/ok',
    body: await followed.text(),
  };
}
observations.default_follow = await follow();
observations.explicit_follow = await follow({ redirect: 'follow' });
observations.redirect_error = await denial(() =>
  handle.fetch(base + '/same-origin', { redirect: 'error' }));
observations.redirect_limit = await denial(() => handle.fetch(base + '/loop'));

const headers = {
  'content-type': 'text/plain;charset=UTF-8',
  'content-encoding': 'identity',
  'content-language': 'en',
  'content-location': '/body-metadata',
  'authorization': 'probe-auth',
  'cookie': 'probe-cookie',
  'x-authority-custom': 'retained',
};
async function echo(path, method, body) {
  const echoed = await handle.fetch(base + path, { method, headers, body });
  return await echoed.json();
}
// Observe the destination's actual method, bytes and headers, not just its status.
observations.post_301 = await echo('/rewrite/301', 'POST', 'replay-你好');
observations.post_302 = await echo('/rewrite/302', 'POST', 'replay-你好');
observations.put_303 = await echo('/rewrite/303', 'PUT', 'replay-你好');
observations.get_303 = await echo('/rewrite/303', 'GET');
const head = await handle.fetch(base + '/rewrite/303', { method: 'HEAD', headers });
observations.head_303 = {
  status: head.status,
  redirected: head.redirected,
  method: head.headers.get('x-observed-method'),
  content_type: head.headers.get('x-observed-content-type'),
  content_encoding: head.headers.get('x-observed-content-encoding'),
  content_language: head.headers.get('x-observed-content-language'),
  content_location: head.headers.get('x-observed-content-location'),
  authorization: head.headers.get('x-observed-authorization'),
  cookie: head.headers.get('x-observed-cookie'),
  custom: head.headers.get('x-observed-custom'),
  body: await head.text(),
};
observations.buffered_307 = await echo('/replay/307', 'POST', 'replay-你好');
// Binary buffered bodies must retain their source and be replayable too.
observations.buffered_308 = await echo('/replay/308', 'POST',
  new Uint8Array([114, 101, 112, 108, 97, 121, 45, 228, 189, 160, 229, 165, 189]));
function streamedBody() {
  return new ReadableStream({
    start(controller) {
      controller.enqueue(new Uint8Array([115, 116, 114, 101, 97, 109]));
      controller.close();
    },
  });
}
observations.streamed_307 = await denial(() => handle.fetch(base + '/replay/307', {
  method: 'POST', body: streamedBody(), duplex: 'half',
}));
observations.streamed_308 = await denial(() => handle.fetch(base + '/replay/308', {
  method: 'POST', body: streamedBody(), duplex: 'half',
}));
// Fetch rejects source-null bodies before the POST-to-GET rewrite for 301/302.
observations.streamed_301 = await denial(() => handle.fetch(base + '/rewrite/301', {
  method: 'POST', body: streamedBody(), duplex: 'half',
}));
observations.streamed_302 = await denial(() => handle.fetch(base + '/rewrite/302', {
  method: 'POST', body: streamedBody(), duplex: 'half',
}));
const streamed303 = await handle.fetch(base + '/rewrite/303', {
  method: 'POST', headers, body: streamedBody(), duplex: 'half',
});
observations.streamed_303 = await streamed303.json();
observations.previous_request = globalThis.retainedHttpAuthority
  ? await denial(() => globalThis.retainedHttpAuthority.fetch(base + '/forbidden'))
  : 'not-retained';
globalThis.retainedHttpAuthority = handle;
return JSON.stringify(observations);
