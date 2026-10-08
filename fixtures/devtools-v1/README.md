# Draft v1 business-contract fixtures — MOCK ONLY

`wire.json` is an independently consumable frame vocabulary catalogue, not an ordered session or
native Inspector transcript. Session responses carry version/request/target/session/epoch correlation;
the old initial-slice uncorrelated `source` frame is intentionally replaced while v1 is still draft.
`module.js` is immutable adapter-provided virtual source content. The deliberately synthetic
`kunlun:fixture/module` URL uses the runtime's existing `kunlun:` virtual-source convention;
it is not a file authority or a URL to fetch.

## Portable executable cases

- `workflows.json`: six independent cases with explicit grants, commands and exact result/error
  expectations. Before each case, explicitly attach `mock-runtime`/`session-1` at epoch 1 using
  request ID `setup-attach` and `inspect`. Construct each command request with version `"1"`,
  the case identity and the step's epoch/request ID. Compare the correlated response's `result`
  to `expect`. `queue` is a test-driver pre-dispatch hold; `completions` checks original cancelled
  responses; `events` checks complete event ordering/reasons/handles; `attach` checks the common
  target metadata, new epoch/state and next-sequence cursor. `disconnect`/`replace` are adapter
  notifications, not remote commands.
- `module.js.map` and `original.ts`: actual v3-map/generated/original fixture bytes. The
  `source-map-cases.json` corpus has 25 mapped/unmapped, stale/foreign/unregistered, association, map-bound and
  UTF-8 range cases. See `source-map-cases.md` for identities and lookup rules, including UTF-16 columns.
- `bounded-cases.json`: three finite local recording cases. Feed contiguous validated resumed
  events at sequences 1 through `frames`; enforce the stated byte budget and the 64-frame limit.
  `expected: null` means success; overflow stops without evicting or replaying the valid prefix.

Executable checks live in `crates/kunlun-devtools-protocol/tests/`. The shared lifecycle `Mock`
in `tests/support/` and the separate registered-source-map adapter are test-only. They never execute
expressions, capture a native heap, advertise native capability or translate WIP. Schema-only value/
capture examples test vocabulary, not successful adapter execution. See `docs/devtools-protocol-v1.md`
for bounds, remaining owner/product gates and the separation from native/application qualification.

```sh
cargo test --locked -p kunlun-devtools-protocol
cargo run --locked -p kunlun-devtools-protocol --example schema
```
