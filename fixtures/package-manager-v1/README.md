# Package-manager v1 shared conformance inputs

`workspace/` is a synthetic static pnpm v9 project with:

- three admitted importers and a workspace exclusion;
- registry/transitive/dev/optional edges, nested exact peer contexts and a dependency cycle;
- all-target platform conditions and deny-only policy;
- deliberately executable hook declarations that must **never run**.

Every integrity declaration is syntactically valid SHA-512 SRI with an all-zero placeholder digest;
no registry artifact is available or considered verified. Do not install this fixture with pnpm or use it to
claim native fetch/linker interoperability.

The native and optional Core adapter tests both consume this project. Copy it to an owned
temporary directory for rejection/mutation tests; the provider itself remains read-only.
`conformance.json` provides reusable positive expectations and negative static-input mutations.

See [`docs/package-manager-provider-v1.md`](../../docs/package-manager-provider-v1.md) for supported
semantics, deny-only policy, limits, exits and honest P0 capability/readiness claims.
