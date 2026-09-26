(() => {
  'use strict';
  const { invoke, HostByteStream, upload } = globalThis.__kunlunFetchBridge;
  delete globalThis.__kunlunFetchBridge;
  const headerStates = new WeakMap();
  const bodyStates = new WeakMap();
  const token = /^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/;
  const headerName = value => {
    const name = String(value).toLowerCase();
    if (!token.test(name)) throw new TypeError('Invalid HTTP header name');
    return name;
  };
  const headerValue = value => {
    const text = String(value).replace(/^[\t ]+|[\t ]+$/g, '');
    if (/[\x00-\x08\x0a-\x1f\x7f\u0100-\uffff]/.test(text)) throw new TypeError('Invalid HTTP header value');
    return text;
  };
  class Headers {
    constructor(init = undefined) {
      const pairs = [];
      headerStates.set(this, pairs);
      if (init == null) return;
      if (init instanceof Headers) {
        for (const pair of headerStates.get(init)) pairs.push([...pair]);
      } else if (typeof init[Symbol.iterator] === 'function') {
        for (const pair of init) {
          if (typeof pair !== 'object' || pair == null || pair.length !== 2)
            throw new TypeError('Header pair must contain two values');
          this.append(pair[0], pair[1]);
        }
      } else {
        for (const [name, value] of Object.entries(init)) this.append(name, value);
      }
    }
    append(name, value) { headerStates.get(this).push([headerName(name), headerValue(value)]); }
    delete(name) {
      name = headerName(name);
      const pairs = headerStates.get(this);
      for (let i = pairs.length - 1; i >= 0; i--) if (pairs[i][0] === name) pairs.splice(i, 1);
    }
    get(name) {
      name = headerName(name);
      const values = headerStates.get(this).filter(pair => pair[0] === name).map(pair => pair[1]);
      return values.length ? values.join(', ') : null;
    }
    getSetCookie() { return headerStates.get(this).filter(pair => pair[0] === 'set-cookie').map(pair => pair[1]); }
    has(name) { name = headerName(name); return headerStates.get(this).some(pair => pair[0] === name); }
    set(name, value) { this.delete(name); this.append(name, value); }
    *entries() {
      const names = [...new Set(headerStates.get(this).map(pair => pair[0]))].sort();
      for (const name of names) yield [name, this.get(name)];
    }
    *keys() { for (const [name] of this) yield name; }
    *values() { for (const [, value] of this) yield value; }
    [Symbol.iterator]() { return this.entries(); }
    forEach(callback, thisArg = undefined) { for (const [name, value] of this) callback.call(thisArg, value, name, this); }
  }
  const rawHeaders = headers => headerStates.get(headers).map(pair => [...pair]);
  const forbiddenRequestHeader = name =>
    ['host', 'content-length', 'transfer-encoding', 'connection', 'upgrade',
      'proxy-authorization', 'te', 'trailer', 'keep-alive'].includes(name);
  const MAX_CONSUMED_BODY = 1024 * 1024;
  const copyBytes = input => {
    if (input instanceof Uint8Array) return new Uint8Array(input);
    if (ArrayBuffer.isView(input)) return new Uint8Array(input.buffer.slice(input.byteOffset, input.byteOffset + input.byteLength));
    if (input instanceof ArrayBuffer) return new Uint8Array(input.slice(0));
    return null;
  };
  const trackedStream = (source, state) => {
    let reader;
    return new ReadableStream({
      async pull(controller) {
        state.used = true;
        reader ??= source.getReader();
        try {
          const { done, value } = await reader.read();
          if (done) { reader.releaseLock(); reader = undefined; controller.close(); }
          else controller.enqueue(value);
        } catch (error) {
          reader.releaseLock(); reader = undefined;
          controller.error(error);
        }
      },
      async cancel(reason) {
        state.used = true;
        if (reader) {
          try { await reader.cancel(reason); } finally { reader.releaseLock(); reader = undefined; }
        } else await source.cancel(reason);
      },
    }, { highWaterMark: 0 });
  };
  const makeBody = (value, headers) => {
    if (value == null) return { stream: null, bytes: null, used: false };
    if (value instanceof ReadableStream) {
      if (value.locked) throw new TypeError('Body stream is locked');
      const state = { stream: null, bytes: null, used: false };
      state.stream = trackedStream(value, state);
      return state;
    }
    let bytes = copyBytes(value);
    if (bytes === null && value instanceof URLSearchParams) {
      bytes = new TextEncoder().encode(value.toString());
      if (!headers.has('content-type')) headers.set('content-type', 'application/x-www-form-urlencoded;charset=UTF-8');
    }
    if (bytes === null) {
      if (typeof value !== 'string') throw new TypeError('Unsupported body type');
      bytes = new TextEncoder().encode(value);
      if (!headers.has('content-type')) headers.set('content-type', 'text/plain;charset=UTF-8');
    }
    const snapshot = new Uint8Array(bytes);
    const source = new ReadableStream({ start(controller) { controller.enqueue(bytes); controller.close(); } });
    const state = { stream: null, bytes: snapshot, used: false };
    state.stream = trackedStream(source, state);
    return state;
  };
  const cloneBody = state => {
    if (state.used || state.stream?.locked) throw new TypeError('Body has already been used');
    if (state.stream === null) return null;
    if (state.bytes === null) throw new TypeError('Cloning a streaming body is unsupported');
    return new Uint8Array(state.bytes);
  };
  const transferBody = prior => {
    const reader = prior.stream.getReader();
    let released = false;
    const release = () => { if (!released) { reader.releaseLock(); released = true; } };
    const state = { stream: null, bytes: prior.bytes, used: false };
    const source = new ReadableStream({
      async pull(controller) {
        try {
          const { done, value } = await reader.read();
          if (done) { release(); controller.close(); }
          else controller.enqueue(value);
        } catch (error) { release(); controller.error(error); }
      },
      async cancel(reason) { try { await reader.cancel(reason); } finally { release(); } },
    }, { highWaterMark: 0 });
    state.stream = trackedStream(source, state);
    prior.used = true;
    return state;
  };
  class BodyHolder {
    get body() { return bodyStates.get(this).stream; }
    get bodyUsed() { const state = bodyStates.get(this); return state.used || !!state.stream?.locked; }
    async arrayBuffer() {
      const state = bodyStates.get(this);
      if (state.used || state.stream?.locked) throw new TypeError('Body has already been used');
      state.used = true;
      if (state.stream === null) return new ArrayBuffer(0);
      const reader = state.stream.getReader();
      const chunks = [];
      let length = 0;
      try {
        for (;;) {
          const { done, value } = await reader.read();
          if (done) break;
          if (!(value instanceof Uint8Array)) throw new TypeError('Body chunk must be Uint8Array');
          length += value.byteLength;
          if (length > MAX_CONSUMED_BODY) throw new RangeError('Body exceeds the 1 MiB consumption limit');
          chunks.push(new Uint8Array(value));
        }
      } catch (error) {
        await reader.cancel(error).catch(() => {});
        throw error;
      } finally { reader.releaseLock(); }
      const output = new Uint8Array(length);
      let offset = 0;
      for (const chunk of chunks) { output.set(chunk, offset); offset += chunk.length; }
      return output.buffer;
    }
    async bytes() { return new Uint8Array(await this.arrayBuffer()); }
    async text() { return new TextDecoder().decode(await this.arrayBuffer()); }
    async json() { return JSON.parse(await this.text()); }
  }
  const normalizeMethod = value => {
    const method = String(value);
    if (!token.test(method)) throw new TypeError('Invalid request method');
    const upper = method.toUpperCase();
    if (['CONNECT', 'TRACE', 'TRACK'].includes(upper)) throw new TypeError('Forbidden request method');
    return ['DELETE', 'GET', 'HEAD', 'OPTIONS', 'POST', 'PUT', 'PATCH'].includes(upper) ? upper : method;
  };
  class Request extends BodyHolder {
    constructor(input, init = {}) {
      super();
      for (const name of Object.keys(init)) {
        if (!['method', 'headers', 'body', 'signal', 'redirect', 'duplex'].includes(name))
          throw new TypeError(`Unsupported Request option: ${name}`);
      }
      const previous = input instanceof Request ? input : null;
      const url = new URL(previous ? previous.url : input);
      if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password)
        throw new TypeError('Request requires an absolute HTTP(S) URL without credentials');
      this.url = url.href;
      this.method = normalizeMethod(init.method ?? previous?.method ?? 'GET');
      this.headers = new Headers(init.headers ?? previous?.headers);
      for (const [name] of this.headers) {
        if (forbiddenRequestHeader(name)) throw new TypeError(`Unsupported request header: ${name}`);
      }
      this.redirect = init.redirect ?? previous?.redirect ?? 'follow';
      if (!['follow', 'error', 'manual'].includes(this.redirect)) throw new TypeError('Unsupported redirect mode');
      if (init.duplex !== undefined && init.duplex !== 'half') throw new TypeError('Unsupported duplex mode');
      this.duplex = 'half';
      this.signal = init.signal ?? previous?.signal ?? new AbortController().signal;
      if (!(this.signal instanceof AbortSignal)) throw new TypeError('signal must be an AbortSignal');
      const prior = previous && bodyStates.get(previous);
      const inherited = init.body === undefined && prior && prior.stream !== null;
      if (init.body === undefined && prior && (prior.used || prior.stream?.locked))
        throw new TypeError('Body has already been used');
      const body = init.body === undefined ? null : init.body;
      if ((this.method === 'GET' || this.method === 'HEAD') && (body != null || inherited))
        throw new TypeError('GET and HEAD requests cannot have a body');
      bodyStates.set(this, inherited ? transferBody(prior) : makeBody(body, this.headers));
    }
    clone() {
      return new Request(this.url, {
        method: this.method, headers: this.headers, signal: this.signal,
        redirect: this.redirect, body: cloneBody(bodyStates.get(this)),
      });
    }
  }
  class Response extends BodyHolder {
    constructor(body = null, init = {}) {
      super();
      for (const name of Object.keys(init)) {
        if (!['status', 'statusText', 'headers'].includes(name))
          throw new TypeError(`Unsupported Response option: ${name}`);
      }
      const status = init.status ?? 200;
      if (!Number.isInteger(status) || status < 200 || status > 599) throw new RangeError('Invalid response status');
      this.status = status;
      this.statusText = String(init.statusText ?? '');
      if (/[^\t\x20-\x7e]/.test(this.statusText)) throw new TypeError('Invalid status text');
      this.headers = new Headers(init.headers);
      if ([204, 205, 304].includes(status) && body != null) throw new TypeError('Null-body status cannot have a body');
      this.url = '';
      this.redirected = false;
      bodyStates.set(this, makeBody(body, this.headers));
    }
    get ok() { return this.status >= 200 && this.status <= 299; }
    clone() {
      const copy = new Response(cloneBody(bodyStates.get(this)), {
        status: this.status, statusText: this.statusText, headers: this.headers,
      });
      copy.url = this.url;
      copy.redirected = this.redirected;
      return copy;
    }
    static redirect(url, status = 302) {
      if (![301, 302, 303, 307, 308].includes(status)) throw new RangeError('Invalid redirect status');
      return new Response(null, { status, headers: { location: new URL(url).href } });
    }
  }
  const encodeBase64 = bytes => {
    const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
    let output = '';
    for (let i = 0; i < bytes.length; i += 3) {
      const a = bytes[i], b = bytes[i + 1], c = bytes[i + 2];
      output += alphabet[a >> 2] + alphabet[(a & 3) << 4 | (b ?? 0) >> 4];
      output += i + 1 < bytes.length ? alphabet[(b & 15) << 2 | (c ?? 0) >> 6] : '=';
      output += i + 2 < bytes.length ? alphabet[c & 63] : '=';
    }
    return output;
  };
  let nextUploadId = 1;
  async function sendUpload(stream, uploadId, signal) {
    const reader = stream.getReader();
    const onAbort = () => { reader.cancel(signal.reason).catch(() => {}); };
    signal.addEventListener('abort', onAbort, { once: true });
    let completed = false;
    try {
      for (;;) {
        signal.throwIfAborted();
        const { done, value } = await reader.read();
        if (done) break;
        if (!(value instanceof Uint8Array)) throw new TypeError('Request body chunk must be Uint8Array');
        for (let offset = 0; offset < value.byteLength; offset += 65536) {
          signal.throwIfAborted();
          await upload('fetch.upload.write', {
            uploadId, chunkBase64: encodeBase64(value.subarray(offset, offset + 65536)),
          });
        }
      }
      completed = true;
    } finally {
      signal.removeEventListener('abort', onAbort);
      reader.releaseLock();
      await upload(completed ? 'fetch.upload.close' : 'fetch.upload.abort', { uploadId }).catch(() => {});
    }
  }
  async function fetch(input, init = undefined) {
    let request = new Request(input, init);
    const signal = request.signal;
    signal.throwIfAborted();
    let redirected = false;
    for (let redirects = 0; ; redirects++) {
      if (redirects > 20) throw new TypeError('Too many Fetch redirects');
      const state = bodyStates.get(request);
      if (state.stream?.locked || state.used) throw new TypeError('Request body has already been used');
      state.used = true;
      const streaming = state.stream !== null && state.bytes === null;
      const uploadId = streaming ? nextUploadId++ : undefined;
      const transfer = new AbortController();
      const onAbort = () => transfer.abort(signal.reason);
      signal.addEventListener('abort', onAbort, { once: true });
      if (signal.aborted) onAbort();
      let opened;
      const pending = invoke('fetch.requestStream', {
        url: request.url, method: request.method, headers: rawHeaders(request.headers),
        bodyBase64: state.bytes === null ? null : encodeBase64(state.bytes), uploadId,
      }, transfer.signal).then(encoded => { opened = JSON.parse(encoded); return opened; });
      const pump = streaming ? sendUpload(state.stream, uploadId, transfer.signal) : Promise.resolve();
      try { await Promise.all([pending, pump]); }
      catch (error) {
        transfer.abort(error);
        await pump.catch(() => {});
        if (opened) await new HostByteStream(opened.streamId).cancel().catch(() => {});
        throw error;
      } finally { signal.removeEventListener('abort', onAbort); }
      const hostBody = new HostByteStream(opened.streamId, signal);
      const nullBody = request.method === 'HEAD' || [204, 205, 304].includes(opened.status);
      let response;
      try {
        response = new Response(null, {
          status: opened.status, statusText: opened.statusText, headers: opened.headers,
        });
      } catch (error) { await hostBody.cancel(); throw error; }
      response.url = opened.url;
      response.redirected = redirected;
      if (nullBody) await hostBody.cancel();
      else bodyStates.set(response, makeBody(hostBody.toReadableStream(), response.headers));
      const location = response.headers.get('location');
      if (![301, 302, 303, 307, 308].includes(response.status) || location === null || request.redirect === 'manual')
        return response;
      if (response.body) await response.body.cancel();
      if (request.redirect === 'error') throw new TypeError('Fetch redirect disallowed');
      const next = new URL(location, request.url);
      if (!['http:', 'https:'].includes(next.protocol)) throw new TypeError('Unsupported redirect URL');
      const switchToGet = response.status === 303 || ([301, 302].includes(response.status) && request.method === 'POST');
      if (!switchToGet && state.bytes === null && state.stream !== null)
        throw new TypeError('Cannot replay a streaming request body');
      const headers = new Headers(request.headers);
      if (next.origin !== new URL(request.url).origin) {
        for (const name of ['authorization', 'cookie', 'proxy-authorization']) headers.delete(name);
      }
      if (switchToGet) for (const name of ['content-type', 'content-length', 'transfer-encoding']) headers.delete(name);
      request = new Request(next.href, {
        method: switchToGet ? 'GET' : request.method, headers,
        body: switchToGet ? null : state.bytes, signal, redirect: request.redirect,
      });
      redirected = true;
    }
  }
  for (const [name, value] of Object.entries({ Headers, Request, Response, fetch })) {
    Object.defineProperty(globalThis, name, { value, writable: false, configurable: false });
  }
})();
