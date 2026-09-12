use kunlun_jsc::{JscError, JscVm};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinModuleDescriptor {
    pub specifier: &'static str,
    pub exports: &'static [&'static str],
}

pub const BUILTIN_MODULES: &[BuiltinModuleDescriptor] = &[
    BuiltinModuleDescriptor {
        specifier: "kunlun:fs",
        exports: &["readTextFile", "openReadStream"],
    },
    BuiltinModuleDescriptor {
        specifier: "kunlun:http",
        exports: &["request", "requestStream"],
    },
];

pub const TYPESCRIPT_DECLARATIONS: &str = include_str!("../../../types/index.d.ts");

const BOOTSTRAP_SOURCE: &str = r#"
(() => {
  'use strict';
  const hostCall = globalThis.__kunlunHostCall;
  if (typeof hostCall !== 'function') {
    throw new Error('Kunlun host-call bridge is not installed');
  }

  const asModule = (exports) => {
    Object.defineProperty(exports, Symbol.toStringTag, { value: 'Module' });
    return Object.freeze(exports);
  };

  const abortStates = new WeakMap();
  const defaultAbortReason = () => {
    const error = new Error('This operation was aborted');
    error.name = 'AbortError';
    return error;
  };

  class AbortSignal {
    constructor(token) {
      if (token !== abortStates) throw new TypeError('Illegal constructor');
      abortStates.set(this, { aborted: false, reason: undefined, listeners: [], onabort: null });
    }

    get aborted() { return abortStates.get(this).aborted; }
    get reason() { return abortStates.get(this).reason; }
    get onabort() { return abortStates.get(this).onabort; }
    set onabort(listener) {
      abortStates.get(this).onabort = typeof listener === 'function' ? listener : null;
    }

    throwIfAborted() {
      const state = abortStates.get(this);
      if (state.aborted) throw state.reason;
    }

    addEventListener(type, listener, options = undefined) {
      if (type !== 'abort' || listener == null) return;
      const state = abortStates.get(this);
      if (state.listeners.some(entry => entry.listener === listener)) return;
      state.listeners.push({ listener, once: options === true || options?.once === true });
    }

    removeEventListener(type, listener) {
      if (type !== 'abort' || listener == null) return;
      const state = abortStates.get(this);
      state.listeners = state.listeners.filter(entry => entry.listener !== listener);
    }

    static abort(reason = defaultAbortReason()) {
      const controller = new AbortController();
      controller.abort(reason);
      return controller.signal;
    }
  }

  class AbortController {
    constructor() {
      this.__signal = new AbortSignal(abortStates);
    }

    get signal() { return this.__signal; }

    abort(reason = defaultAbortReason()) {
      const signal = this.__signal;
      const state = abortStates.get(signal);
      if (state.aborted) return;
      state.aborted = true;
      state.reason = reason;
      const event = Object.freeze({ type: 'abort', target: signal, currentTarget: signal });
      const listeners = state.listeners.slice();
      for (const entry of listeners) {
        try {
          if (typeof entry.listener === 'function') entry.listener.call(signal, event);
          else entry.listener.handleEvent(event);
        } catch (error) {
          Promise.reject(error);
        }
        if (entry.once) signal.removeEventListener('abort', entry.listener);
      }
      if (state.onabort !== null) {
        try { state.onabort.call(signal, event); } catch (error) { Promise.reject(error); }
      }
    }
  }

  Object.defineProperty(globalThis, 'AbortSignal', {
    value: AbortSignal, configurable: false, enumerable: false, writable: false,
  });
  Object.defineProperty(globalThis, 'AbortController', {
    value: AbortController, configurable: false, enumerable: false, writable: false,
  });

  let nextRequestId = 1;
  const invoke = (operation, payload, signal) => {
    if (signal !== undefined && !(signal instanceof AbortSignal)) {
      return Promise.reject(new TypeError('signal must be an AbortSignal'));
    }
    if (signal?.aborted) return Promise.reject(signal.reason);
    const requestId = nextRequestId++;
    const encoded = JSON.stringify({ ...payload, requestId });
    const pending = hostCall(operation, encoded);
    if (signal === undefined) return pending;
    return new Promise((resolve, reject) => {
      let settled = false;
      const cleanup = () => signal.removeEventListener('abort', onAbort);
      const onAbort = () => {
        if (settled) return;
        settled = true;
        cleanup();
        hostCall('host.cancel', JSON.stringify({ requestId })).then(() => {}, () => {});
        reject(signal.reason);
      };
      signal.addEventListener('abort', onAbort, { once: true });
      pending.then(
        value => { if (!settled) { settled = true; cleanup(); resolve(value); } },
        error => { if (!settled) { settled = true; cleanup(); reject(error); } },
      );
    });
  };

  const base64Digits = new Int16Array(128);
  base64Digits.fill(-1);
  const base64Alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
  for (let digit = 0; digit < base64Alphabet.length; digit++) {
    base64Digits[base64Alphabet.charCodeAt(digit)] = digit;
  }

  const decodeBase64 = (encoded) => {
    let validDigits = 0;
    for (const character of encoded) {
      if (character === '=') break;
      const code = character.charCodeAt(0);
      if (code < base64Digits.length && base64Digits[code] >= 0) validDigits++;
    }
    const output = new Uint8Array(Math.floor(validDigits * 6 / 8));
    let offset = 0;
    let bits = 0;
    let value = 0;
    for (const character of encoded) {
      if (character === '=') break;
      const code = character.charCodeAt(0);
      const digit = code < base64Digits.length ? base64Digits[code] : -1;
      if (digit < 0) continue;
      value = (value << 6) | digit;
      bits += 6;
      if (bits >= 8) {
        bits -= 8;
        output[offset++] = (value >> bits) & 255;
      }
    }
    return output;
  };

  class HostByteStream {
    constructor(streamId, signal) {
      this.__streamId = streamId;
      this.__signal = signal;
      this.__closed = false;
      this.__aborted = false;
      this.__abortReason = undefined;
      this.__onAbort = undefined;
      if (signal !== undefined) {
        this.__onAbort = () => {
          this.__aborted = true;
          this.__abortReason = signal.reason;
          this.cancel(signal.reason).then(() => {}, () => {});
        };
        signal.addEventListener('abort', this.__onAbort, { once: true });
        if (signal.aborted) this.__onAbort();
      }
    }

    __cleanup() {
      if (this.__signal !== undefined && this.__onAbort !== undefined) {
        this.__signal.removeEventListener('abort', this.__onAbort);
        this.__onAbort = undefined;
      }
    }

    async read() {
      if (this.__aborted) throw this.__abortReason;
      if (this.__closed) return { done: true, value: undefined };
      let encoded;
      try {
        encoded = await invoke('stream.next', { streamId: this.__streamId }, this.__signal);
      } catch (error) {
        this.__closed = true;
        this.__cleanup();
        throw error;
      }
      const message = JSON.parse(encoded);
      if (message.done) {
        this.__closed = true;
        this.__cleanup();
        return { done: true, value: undefined };
      }
      return { done: false, value: decodeBase64(message.value) };
    }

    async cancel(_reason = undefined) {
      if (this.__closed) return;
      this.__closed = true;
      this.__cleanup();
      await hostCall('stream.cancel', JSON.stringify({ streamId: this.__streamId }));
    }

    [Symbol.asyncIterator]() {
      return {
        next: () => this.read(),
        return: async () => { await this.cancel(); return { done: true, value: undefined }; },
      };
    }
  }

  const fs = asModule({
    readTextFile(path, options = {}) {
      return invoke('fs.readTextFile', { path: String(path) }, options.signal);
    },
    async openReadStream(path, options = {}) {
      const opened = JSON.parse(await invoke(
        'fs.openReadStream', { path: String(path) }, options.signal,
      ));
      return new HostByteStream(opened.streamId, options.signal);
    },
  });

  const http = asModule({
    async request(url, init = {}) {
      const headers = {};
      if (init.headers != null) {
        for (const [name, value] of Object.entries(init.headers)) {
          headers[String(name)] = String(value);
        }
      }
      const encoded = await invoke('http.request', {
        url: String(url),
        method: init.method == null ? 'GET' : String(init.method),
        headers,
        body: init.body == null ? null : String(init.body),
      }, init.signal);
      return JSON.parse(encoded);
    },
    async requestStream(url, init = {}) {
      const headers = {};
      if (init.headers != null) {
        for (const [name, value] of Object.entries(init.headers)) {
          headers[String(name)] = String(value);
        }
      }
      const opened = JSON.parse(await invoke('http.requestStream', {
        url: String(url),
        method: init.method == null ? 'GET' : String(init.method),
        headers,
        body: init.body == null ? null : String(init.body),
      }, init.signal));
      return Object.freeze({
        status: opened.status,
        headers: Object.freeze(opened.headers),
        body: new HostByteStream(opened.streamId, init.signal),
      });
    },
  });

  const modules = Object.freeze({
    'kunlun:fs': fs,
    'kunlun:http': http,
  });
  const runtime = Object.freeze({
    import(specifier) {
      const module = modules[String(specifier)];
      return module === undefined
        ? Promise.reject(new TypeError(`Unknown Kunlun built-in module: ${specifier}`))
        : Promise.resolve(module);
    },
  });

  Object.defineProperty(globalThis, 'kunlun', {
    value: runtime,
    configurable: false,
    enumerable: false,
    writable: false,
  });
  delete globalThis.__kunlunHostCall;
})();
"#;

pub(crate) fn install_builtin_modules(vm: &mut JscVm) -> Result<(), JscError> {
    vm.evaluate(BOOTSTRAP_SOURCE, "kunlun:bootstrap/builtins")?;
    Ok(())
}

pub fn is_builtin_specifier(specifier: &str) -> bool {
    BUILTIN_MODULES
        .iter()
        .any(|module| module.specifier == specifier)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declarations_cover_every_builtin_export() {
        for module in BUILTIN_MODULES {
            assert!(
                TYPESCRIPT_DECLARATIONS
                    .contains(&format!("declare module \"{}\"", module.specifier)),
                "missing declaration for {}",
                module.specifier
            );
            for export in module.exports {
                assert!(
                    TYPESCRIPT_DECLARATIONS.contains(export),
                    "missing declaration for {}::{export}",
                    module.specifier
                );
            }
        }
    }

    #[test]
    fn resolves_only_registered_builtin_specifiers() {
        assert!(is_builtin_specifier("kunlun:fs"));
        assert!(is_builtin_specifier("kunlun:http"));
        assert!(!is_builtin_specifier("node:fs"));
        assert!(!is_builtin_specifier("kunlun:unknown"));
    }
}
