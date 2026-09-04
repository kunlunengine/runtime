# Native Module Loading

## M2 resolver slice

The first M2 slice establishes canonical module identity and resolution in Rust before adding a JSC
module-loader ABI. `ModuleResolver` currently recognizes three module kinds:

| Kind | URL form | Resolution policy |
| --- | --- | --- |
| application file | `file:///.../server.mjs` | exact file only, canonicalized below one artifact root |
| runtime built-in | `kunlun:fs` | must exist in the built-in registry |
| generated module | `kunlun-generated:///bootstrap/runtime.mjs` | must be registered before graph resolution |

Relative URL references use standard URL joining. File resolution does not probe extensions or
directory indexes. Query strings and fragments remain part of module identity, while the underlying
file path is canonicalized without them. Missing files, directories, remote `file:` authorities,
unsupported schemes, and symbolic-link escapes are rejected.

Bare package specifiers are deliberately outside this layer. Development resolution belongs to the
selected build/package provider, and production server entrypoints must be bundled before native
execution. This keeps pnpm or Yarn internals out of the runtime and makes the production module graph
an artifact property.

Generated modules use an authority-free hierarchical URL so they can import registered siblings:

```text
kunlun-generated:///bootstrap/entry.mjs
  -> ./runtime.mjs
  -> kunlun-generated:///bootstrap/runtime.mjs
```

Registration controls which generated identities exist; resolving an arbitrary URL in that scheme
does not create a module.

This slice is not a native ESM claim. The next slice must add the pinned-JSC shim callbacks for
resolve, fetch, link, evaluate, dynamic import, and `import.meta`, then route every requested identity
through this resolver. Cycles, live bindings, top-level await, rejection tracking, and source-map
integration remain acceptance requirements at the M2 exit gate.
