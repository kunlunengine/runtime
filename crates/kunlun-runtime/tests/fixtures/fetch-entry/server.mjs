let calls = 0;
let completed = 0;
let previousHandle;
let previousContext;
export default {
  async fetch(request, env, context) {
    calls++;
    if (request.signal !== context.signal) throw Error('signal mismatch');
    if ('auth' in env || 'context' in env) throw Error('caller context leaked');
    if (previousContext) {
      let closed = false;
      try { previousContext.waitUntil(Promise.resolve()); } catch (_) { closed = true; }
      if (!closed) throw Error('previous context still open');
    }
    previousContext = context;
    const path = new URL(request.url).pathname;
    if (path === '/throw') throw Error('private application error');
    if (path === '/invalid') return { status: 200 };
    if (path === '/background-error') {
      context.waitUntil(Promise.reject(Error('private task error')));
      return new Response('unreleased');
    }
    if (path === '/wait') {
      context.signal.addEventListener('abort', () => { globalThis.observedAbort = true; });
      context.waitUntil(new Promise(() => {}));
      return new Response('never released');
    }
    if (path === '/body-error') {
      return new Response(new ReadableStream({ pull(controller) { controller.error(Error('private body error')); } }));
    }
    if (path === '/oversized') return new Response(new Uint8Array(1048577));
    if (path === '/head') {
      return new Response(new ReadableStream({
        pull() { return new Promise(() => {}); },
        cancel() { completed++; },
      }), { status: 201, headers: [['x-test', 'head']] });
    }
    if (path === '/redirect') return Response.redirect('https://service.test/target', 307);
    if (path === '/state') return new Response(`${calls}:${completed}`);
    if (path === '/scoped') {
      if (previousHandle) {
        let denied = false;
        try { await previousHandle.readTextFile('message.txt'); } catch (_) { denied = true; }
        if (!denied) throw Error('previous request handle still active');
      }
      previousHandle = env.fs['public-data'];
      context.waitUntil((async () => {
        await sleep(0);
        if (await previousHandle.readTextFile('message.txt') !== 'scoped fixture') throw Error('bad scope');
        completed++;
      })());
      return new Response(await previousHandle.readTextFile('message.txt'));
    }
    context.waitUntil((async () => { await sleep(0); completed++; })());
    return new Response(new Uint8Array([0, 255, 1]), {
      status: 202,
      statusText: 'Accepted',
      headers: [['set-cookie', 'a=1'], ['set-cookie', 'b=2'], ['x-route', request.url]],
    });
  },
};
