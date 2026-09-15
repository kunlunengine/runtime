(() => {
  'use strict';
  const host = globalThis.__kunlunWeb;
  delete globalThis.__kunlunWeb;
  const call = (op, payload) => {
    const result = JSON.parse(host(op, JSON.stringify(payload)));
    if (result.error !== undefined) throw new TypeError(result.error);
    return result.value;
  };
  const usv = value => String(value).toWellFormed();
  const bytes = value => {
    if (ArrayBuffer.isView(value)) return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
    if (value instanceof ArrayBuffer) return new Uint8Array(value);
    throw new TypeError('Expected an ArrayBuffer or ArrayBufferView');
  };
  class TextEncoder {
    get encoding() { return 'utf-8'; }
    encode(input = '') {
      const output = [];
      for (const character of usv(input)) {
        const c = character.codePointAt(0);
        if (c < 128) output.push(c);
        else if (c < 2048) output.push(192 | c >> 6, 128 | c & 63);
        else if (c < 65536) output.push(224 | c >> 12, 128 | c >> 6 & 63, 128 | c & 63);
        else output.push(240 | c >> 18, 128 | c >> 12 & 63, 128 | c >> 6 & 63, 128 | c & 63);
      }
      return new Uint8Array(output);
    }
    encodeInto(input, destination) {
      if (!(destination instanceof Uint8Array)) throw new TypeError('Expected Uint8Array');
      let read = 0, written = 0;
      for (const character of String(input)) {
        const encoded = this.encode(character);
        if (written + encoded.length > destination.length) break;
        destination.set(encoded, written);
        written += encoded.length;
        read += character.length;
      }
      return { read, written };
    }
  }
  class TextDecoder {
    #fatal; #ignoreBOM; #pending = []; #bomSeen = false;
    constructor(label = 'utf-8', options = {}) {
      if (!['utf-8', 'utf8', 'unicode-1-1-utf-8'].includes(String(label).replace(/^[\t\n\f\r ]+|[\t\n\f\r ]+$/g, '').toLowerCase())) {
        throw new RangeError('This profile supports only UTF-8');
      }
      this.#fatal = Boolean(options.fatal); this.#ignoreBOM = Boolean(options.ignoreBOM);
    }
    get encoding() { return 'utf-8'; }
    get fatal() { return this.#fatal; }
    get ignoreBOM() { return this.#ignoreBOM; }
    decode(input = new Uint8Array(), options = {}) {
      const stream = Boolean(options.stream);
      try {
        const result = call('decode', { bytes: this.#pending.concat(Array.from(bytes(input))), fatal: this.#fatal, stream });
        this.#pending = result.pending;
        let text = result.text;
        if (text.length && !this.#bomSeen) {
          this.#bomSeen = true;
          if (!this.#ignoreBOM && text.charCodeAt(0) === 0xfeff) text = text.slice(1);
        }
        if (!stream) { this.#pending = []; this.#bomSeen = false; }
        return text;
      } catch (error) { this.#pending = []; this.#bomSeen = false; throw error; }
    }
  }
  const parameterStates = new WeakMap();
  class URLSearchParams {
    constructor(init = '') {
      let pairs;
      if (init !== null && typeof init === 'object') {
        if (init[Symbol.iterator]) {
          pairs = Array.from(init, pair => {
            const values = Array.from(pair);
            if (values.length !== 2) throw new TypeError('Expected a pair');
            return values.map(usv);
          });
        } else pairs = Object.keys(init).map(key => [usv(key), usv(init[key])]);
      } else pairs = call('params.parse', usv(init).replace(/^\?/, ''));
      parameterStates.set(this, { pairs, update: () => {} });
    }
    get size() { return parameterStates.get(this).pairs.length; }
    append(name, value) { const s = parameterStates.get(this); s.pairs.push([usv(name), usv(value)]); s.update(); }
    delete(name, value = undefined) {
      name = usv(name); if (value !== undefined) value = usv(value);
      const s = parameterStates.get(this);
      s.pairs = s.pairs.filter(p => p[0] !== name || (value !== undefined && p[1] !== value)); s.update();
    }
    get(name) { return parameterStates.get(this).pairs.find(p => p[0] === usv(name))?.[1] ?? null; }
    getAll(name) { name = usv(name); return parameterStates.get(this).pairs.filter(p => p[0] === name).map(p => p[1]); }
    has(name, value = undefined) { name = usv(name); if (value !== undefined) value = usv(value); return parameterStates.get(this).pairs.some(p => p[0] === name && (value === undefined || p[1] === value)); }
    set(name, value) {
      name = usv(name); value = usv(value); const s = parameterStates.get(this); let found = false;
      s.pairs = s.pairs.filter(p => { if (p[0] !== name) return true; if (found) return false; found = true; p[1] = value; return true; });
      if (!found) s.pairs.push([name, value]); s.update();
    }
    sort() { const s = parameterStates.get(this); s.pairs.sort((a, b) => a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0); s.update(); }
    *entries() { for (let i = 0; i < parameterStates.get(this).pairs.length; i++) yield [...parameterStates.get(this).pairs[i]]; }
    *keys() { for (const [key] of this) yield key; }
    *values() { for (const [, value] of this) yield value; }
    [Symbol.iterator]() { return this.entries(); }
    forEach(callback, thisArg = undefined) { for (const [key, value] of this) callback.call(thisArg, value, key, this); }
    toString() { return call('params.serialize', parameterStates.get(this).pairs); }
  }
  const urlStates = new WeakMap();
  class URL {
    constructor(input, base = undefined) {
      const data = call('url', { input: usv(input), base: base === undefined ? null : usv(base) });
      const params = new URLSearchParams(data.search);
      urlStates.set(this, { data, params });
      parameterStates.get(params).update = () => {
        const state = urlStates.get(this);
        state.data = call('url', { input: state.data.href, field: 'search', value: params.toString() });
      };
    }
    get searchParams() { return urlStates.get(this).params; }
    get origin() { return urlStates.get(this).data.origin; }
    toString() { return this.href; }
    toJSON() { return this.href; }
    static canParse(input, base = undefined) { try { new URL(input, base); return true; } catch { return false; } }
    static parse(input, base = undefined) { try { return new URL(input, base); } catch { return null; } }
  }
  for (const field of ['href', 'protocol', 'username', 'password', 'host', 'hostname', 'port', 'pathname', 'search', 'hash']) {
    Object.defineProperty(URL.prototype, field, {
      enumerable: true, configurable: true,
      get() { return urlStates.get(this).data[field]; },
      set(value) {
        const state = urlStates.get(this);
        state.data = call('url', field === 'href' ? { input: usv(value) } : { input: state.data.href, field, value: usv(value) });
        parameterStates.get(state.params).pairs = call('params.parse', state.data.search.replace(/^\?/, ''));
      },
    });
  }
  const namedError = (name, message) => Object.assign(new Error(message), { name });
  const crypto = Object.freeze({
    getRandomValues(array) {
      if (!ArrayBuffer.isView(array) || ![Int8Array, Uint8Array, Uint8ClampedArray, Int16Array, Uint16Array, Int32Array, Uint32Array, BigInt64Array, BigUint64Array].some(Type => array instanceof Type)) throw namedError('TypeMismatchError', 'Expected an integer TypedArray');
      if (array.byteLength > 65536) throw namedError('QuotaExceededError', 'Random request exceeds 65536 bytes');
      new Uint8Array(array.buffer, array.byteOffset, array.byteLength).set(call('random', array.byteLength)); return array;
    },
    randomUUID() {
      const b = new Uint8Array(call('random', 16)); b[6] = (b[6] & 15) | 64; b[8] = (b[8] & 63) | 128;
      const h = Array.from(b, x => x.toString(16).padStart(2, '0')).join('');
      return `${h.slice(0,8)}-${h.slice(8,12)}-${h.slice(12,16)}-${h.slice(16,20)}-${h.slice(20)}`;
    },
    subtle: Object.freeze({ async digest(algorithm, data) {
      const name = String(typeof algorithm === 'string' ? algorithm : algorithm.name).toUpperCase();
      if (!['SHA-256', 'SHA-384', 'SHA-512'].includes(name)) throw namedError('NotSupportedError', 'Unsupported digest');
      return new Uint8Array(call('digest', { algorithm: name, bytes: Array.from(bytes(data)) })).buffer;
    } }),
  });
  const inspect = (value, seen = new Set(), depth = 0) => {
    if (typeof value === 'string') return value.slice(0, 8192);
    if (value === null || typeof value !== 'object') return String(value);
    if (seen.has(value)) return '[Circular]';
    if (depth >= 3) return '[Object]';
    seen.add(value);
    try {
      if (value instanceof Error) return `${value.name}: ${value.message}`.slice(0, 8192);
      const entries = Object.keys(value).sort().slice(0, 32).map(key => {
        const descriptor = Object.getOwnPropertyDescriptor(value, key);
        return `${key}: ${descriptor && 'value' in descriptor ? inspect(descriptor.value, seen, depth + 1) : '[Getter]'}`;
      });
      return `{ ${entries.join(', ')} }`.slice(0, 8192);
    } catch { return '[Uninspectable]'; } finally { seen.delete(value); }
  };
  const console = {};
  for (const level of ['debug', 'log', 'info', 'warn', 'error']) console[level] = (...args) => {
    const message = args.slice(0, 32).map(value => inspect(value)).join(' ').slice(0, 8192);
    const source = String(new Error().stack ?? '').split('\n').slice(1, 5).join('\n').slice(0, 2048);
    call('console', { level, message, source });
  };
  for (const [name, value] of Object.entries({ TextEncoder, TextDecoder, URL, URLSearchParams, crypto, console: Object.freeze(console) })) {
    Object.defineProperty(globalThis, name, { value, writable: false, configurable: false });
  }
})();
