# Draft v1 business-contract fixtures — MOCK ONLY

`wire.json` contains independently consumable JSON frames, not a native Inspector transcript.
`module.js` is immutable adapter-provided virtual source content. The deliberately synthetic
`kunlun:fixture/module` URL uses the runtime's existing `kunlun:` virtual-source convention;
it is not a file authority or a URL to fetch.

Executable lifecycle, authorization, source, and bounded event checks live in
`crates/kunlun-devtools-protocol/tests/conformance.rs`. Their `Mock` is test-only and never
executes expressions or advertises native capability. No sourcemap lookup or real WIP translation
is claimed. See `docs/devtools-protocol-v1.md` for pending acceptance and normative rules.
