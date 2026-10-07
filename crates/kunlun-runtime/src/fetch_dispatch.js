// Evaluated BEFORE loadEntry() (including entry top-level await). The returned
// control function is rooted only in a native VM slot, never in an app global.
const apply = Reflect.apply;
const define = Object.defineProperty;
const freeze = Object.freeze;
const create = Object.create;
const setPrototype = Object.setPrototypeOf;
const parse = JSON.parse;
const stringify = JSON.stringify;
const P = Promise;
const then = P.prototype.then;
const hasInstance = Function.prototype[Symbol.hasInstance];
const RequestClass = Request;
const ResponseClass = Response;
const Controller = AbortController;
const abort = Controller.prototype.abort;
const signalGetter = Object.getOwnPropertyDescriptor(Controller.prototype, 'signal').get;
const U8 = Uint8Array;
const byteLength = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(U8.prototype), 'length').get;
const getReader = ReadableStream.prototype.getReader;
const readerRead = ReadableStreamDefaultReader.prototype.read;
const readerCancel = ReadableStreamDefaultReader.prototype.cancel;
const readerRelease = ReadableStreamDefaultReader.prototype.releaseLock;
const headerEntries = Headers.prototype.entries;
const headerGetCookies = Headers.prototype.getSetCookie;
const generatorNext = Object.getPrototypeOf(Object.getPrototypeOf(
  apply(headerEntries, new Headers(), []))).next;
const charCodeAt = String.prototype.charCodeAt;
const createEnvironment = globalThis.__kunlunCreateEnvironment;
const TypeErrorClass = TypeError;
const RangeErrorClass = RangeError;
const record = () => create(null);
const array = () => setPrototype([], null);
const invoke = (fn, receiver, args) => apply(fn, receiver, args);

// Neither Promise.resolve/all nor a mutable .then lookup governs settlement.
// Native promises are observed through the captured intrinsic. The returned
// species promise is deliberately not used as evidence of completion.
const observe = (value, fulfilled, rejected) => {
  try { invoke(then, value, [fulfilled, rejected]); }
  catch (error) {
    if (invoke(hasInstance, P, [value])) throw error;
    const promise = new P(resolve => resolve(value));
    invoke(then, promise, [fulfilled, rejected]);
  }
};
let entry, handler, active, loading = false, result = 'pending';
const fail = error => {
  const envelope = record();
  envelope.response = null;
  envelope.error = error;
  return stringify(envelope);
};
const finish = (state, value) => {
  // A late continuation from a closed request cannot touch a newer request.
  if (active !== state) return;
  active = undefined;
  result = value;
};
const close = () => {
  const state = active;
  if (!state) return;
  active = undefined;
  state.open = false;
  invoke(abort, state.controller, []);
  if (state.reader) {
    try { observe(invoke(readerCancel, state.reader, []), () => {}, () => {}); }
    catch (_) {}
  }
};
// All request-owned observations share one fail-closed rule. A synchronous
// installation failure is not an ordinary rejection or settlement evidence.
// Publish the fatal result before cleanup, and never reuse this request scope.
const observeRequest = (state, value, fulfilled, rejected) => {
  try { observe(value, fulfilled, rejected); }
  catch (error) {
    state.observationFailed = true;
    if (active === state) {
      result = fail('engine');
      close();
    }
    rejected(error);
  }
};
// Await only private native promises with an own original constructor, avoiding
// Promise.prototype.constructor/then tampering in Await's PromiseResolve path.
const settled = (state, value) => {
  const promise = new P((resolve, reject) => observeRequest(state, value, resolve, reject));
  define(promise, 'constructor', { value: P });
  return promise;
};
async function dispatch(input, env, state) {
  const executionContext = record();
  executionContext.signal = state.signal;
  executionContext.waitUntil = promise => {
    if (!state.open) throw new TypeErrorClass('Request scope is closed');
    if (state.registered >= 32) throw new RangeErrorClass('waitUntil limit exceeded');
    state.registered++;
    state.pending++;
    let completed = false;
    const complete = ok => {
      if (completed) return;
      completed = true;
      if (!ok) state.backgroundFailed = true;
      if (--state.pending === 0 && state.wake) state.wake();
    };
    // Attach rejection handlers at registration; callback counters, not any
    // caller-overridable aggregate Promise, determine request completion.
    observeRequest(state, promise, () => complete(true), () => complete(false));
  };
  freeze(executionContext);
  let request, failure, output;
  try {
    const init = record();
    init.method = input.method;
    init.headers = input.headers;
    init.signal = state.signal;
    if (input.body !== null) {
      // Numeric indexing avoids application-controlled Symbol.iterator.
      const bytes = new U8(input.body.length);
      for (let i = 0; i < input.body.length; i++) bytes[i] = input.body[i];
      init.body = bytes;
    }
    request = new RequestClass(input.url, init);
  } catch (_) { failure = 'request'; }
  let response;
  if (!failure) {
    try { response = await settled(state, invoke(handler, entry, [request, env, executionContext])); }
    catch (_) { failure = 'handler'; }
  }
  state.open = false;
  if (!failure && !(response instanceof ResponseClass)) failure = 'response';
  if (!failure) {
    try {
      // Snapshot writable/application-defined metadata exactly once.
      const status = response.status;
      const statusText = response.statusText;
      const responseHeaders = response.headers;
      const body = response.body;
      if (typeof status !== 'number' || typeof statusText !== 'string' ||
          statusText.length > 1024) throw new TypeErrorClass('Response metadata');
      const headers = array();
      let headerBytes = 0;
      const addHeader = (name, value) => {
        if (typeof name !== 'string' || typeof value !== 'string')
          throw new TypeErrorClass('Response headers');
        // Count profile Latin-1 UTF-8 without mutable TextEncoder/iterators.
        for (let i = 0; i < name.length; i++) headerBytes += invoke(charCodeAt, name, [i]) < 128 ? 1 : 2;
        for (let i = 0; i < value.length; i++) headerBytes += invoke(charCodeAt, value, [i]) < 128 ? 1 : 2;
        const pair = array(); pair[0] = name; pair[1] = value;
        headers[headers.length] = pair;
        if (headers.length > 256 || headerBytes > 65536)
          throw new RangeErrorClass('Response headers');
      };
      // Captured generator/next, no application Symbol.iterator dispatch.
      const iterator = invoke(headerEntries, responseHeaders, []);
      for (;;) {
        const item = invoke(generatorNext, iterator, []);
        if (item.done) break;
        const name = item.value[0], value = item.value[1];
        if (name !== 'set-cookie') addHeader(name, value);
      }
      const cookies = invoke(headerGetCookies, responseHeaders, []);
      for (let i = 0; i < cookies.length; i++) addHeader('set-cookie', cookies[i]);
      const bytes = array();
      let reads = 0;
      if (body) {
        state.reader = invoke(getReader, body, []);
        if (input.method === 'HEAD') {
          await settled(state, invoke(readerCancel, state.reader, []));
        } else {
          for (;;) {
            const item = await settled(state, invoke(readerRead, state.reader, []));
            if (item.done) break;
            const chunk = item.value;
            const length = invoke(byteLength, chunk, []);
            if (++reads > 4096 || !(chunk instanceof U8) || bytes.length + length > 1048576)
              throw new RangeErrorClass('Response body limit exceeded');
            for (let i = 0; i < length; i++) bytes[bytes.length] = chunk[i];
          }
        }
        invoke(readerRelease, state.reader, []);
        state.reader = undefined;
      }
      output = record();
      output.status = status; output.status_text = statusText;
      output.headers = headers; output.body = bytes;
    } catch (_) {
      failure = 'body';
      if (state.reader) {
        try { await settled(state, invoke(readerCancel, state.reader, [])); } catch (_) {}
        try { invoke(readerRelease, state.reader, []); } catch (_) {}
        state.reader = undefined;
      }
    }
  }
  // The fatal observer path has already closed the request and published an
  // engine failure. Do not delay retirement behind other unresolved tasks.
  if (state.observationFailed) return;
  if (state.pending !== 0) {
    const waiting = new P(resolve => { state.wake = resolve; });
    define(waiting, 'constructor', { value: P });
    await waiting;
  }
  if (!failure && state.backgroundFailed) failure = 'background';
  if (failure) finish(state, fail(failure));
  else {
    const envelope = record();
    envelope.response = output; envelope.error = null;
    finish(state, stringify(envelope));
  }
}
return command => {
  if (command === 'poll') return result;
  if (command === 'close') { close(); return 'closed'; }
  if (command === 'load') {
    if (loading || handler || active) throw new TypeErrorClass('Already loaded');
    loading = true;
    observe(loadEntry(), namespace => {
      try {
        entry = namespace.default;
        if (entry === null || typeof entry !== 'object') { result = 'invalid-entry'; return; }
        handler = entry.fetch;
        result = typeof handler === 'function' ? 'ready' : 'invalid-entry';
      } catch (_) { result = 'invalid-entry'; }
    }, () => { result = 'startup'; });
    return 'pending';
  }
  if (active || !handler) throw new TypeErrorClass('Dispatch is not available');
  const payload = parse(command);
  const state = record();
  state.open = true;
  state.controller = new Controller();
  state.signal = invoke(signalGetter, state.controller, []);
  state.registered = 0; state.pending = 0; state.backgroundFailed = false;
  state.observationFailed = false;
  active = state;
  result = 'pending';
  try {
    observe(dispatch(payload.request, createEnvironment(payload.environment), state),
      () => {}, () => { finish(state, fail('engine')); });
  } catch (_) { close(); throw new TypeErrorClass('Dispatch initialization'); }
  return 'pending';
};
