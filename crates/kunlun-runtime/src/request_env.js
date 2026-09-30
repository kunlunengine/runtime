(() => {
  'use strict';
  const invoke = globalThis.__kunlunRequestBridge;
  const createFetch = globalThis.__kunlunCreateScopedFetch;
  delete globalThis.__kunlunRequestBridge;
  delete globalThis.__kunlunCreateScopedFetch;
  const create = Object.create;
  const freeze = Object.freeze;
  const defineProperty = Object.defineProperty;
  const string = String;
  const TypeErrorClass = TypeError;

  const opaque = object => {
    defineProperty(object, 'toJSON', {
      value() { throw new TypeErrorClass('Request authority cannot be serialized'); },
    });
    return freeze(object);
  };
  // This is a projection, not a grant-minting API. Rust selects the current
  // request, checks its private identity, and rechecks operation/resource scope
  // on every call. Calling this factory with invented metadata grants nothing.
  defineProperty(globalThis, '__kunlunCreateEnvironment', {
    value({ scope, capabilities }) {
      const fs = create(null);
      const http = create(null);
      for (let i = 0; i < capabilities.length; i++) {
        const { name, resource } = capabilities[i];
        const authority = create(null);
        authority.scope = scope;
        authority.capabilityName = name;
        authority.capabilityResource = resource;
        freeze(authority);
        const handle = create(null);
        if (name === 'fs.binding') {
          handle.readTextFile = (path, options = undefined) => invoke(
            'fs.readTextFile', { path: string(path) }, options?.signal, authority,
          );
          fs[resource] = opaque(handle);
        } else if (name === 'http.host') {
          handle.fetch = createFetch(authority);
          http[resource] = opaque(handle);
        }
      }
      const env = create(null);
      env.fs = freeze(fs);
      env.http = freeze(http);
      return opaque(env);
    },
    writable: false,
    configurable: false,
  });
})();
