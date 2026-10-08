# Source-map portable cases — MOCK ONLY

These are deterministic business-contract fixtures, not native Inspector, JSC, CLI/MCP,
or HMR qualification. No JavaScript is executed. No requested URL or path is fetched.

`source-map-cases.json` is an independent table for adapter implementers. Each case
starts with fresh immutable registrations, all at target `fixture`, epoch `1`, revision
`r1`:

| Role | ID | Display URL | Fixture bytes |
| --- | --- | --- | --- |
| Generated | `module` | `kunlun:fixture/module` | unchanged `module.js` |
| Map | `map` | `kunlun:fixture/module.js.map` | `module.js.map` |
| Original | `original` | `kunlun:fixture/original.ts` | `original.ts` |

The generated descriptor associates the generated identity with the map identity.
The map's source name resolves only to the registered original identity, never to a
filesystem/network resource. Artifact-aware registration is optional adapter input.

`lookup` uses the generated identity and the case's zero-based `line` and `column`
(default `0`). `expected` is the original zero-based UTF-16 location, or `null` for no
mapped token. The v3 map contains mappings at generated columns 0, 13 and 17, and an
explicit unmapped segment at 14. The column-17 token points to original UTF-16 column
33, the `é` after a non-BMP emoji (UTF-8 byte offset 35, scalar column 32). Greatest
lower-bound lookup is allowed within a line, never across generated lines.

`read` retrieves the registered original with UTF-8 byte `offset` and `max_bytes`.
`text` and `eof` are expected chunk fields. A too-small limit must fail, not split a
code point. `error` is the protocol error code, never a successful empty mapping.

Lookup mutations:

- `revision`: change requested generated revision to `r2`.
- `epoch`: change requested generated epoch to `2`.
- `target`: change requested generated target to `foreign`.
- `source`: replace requested generated display URL with `file:///etc/passwd`.
- `map` / `original`: remove that registration, keeping the descriptor/map reference.
- `no_association`: clear the generated descriptor's map association.
- `map_revision`: associate the descriptor with an unregistered `r2` map revision.
- `map_epoch` / `map_target`: associate a foreign-epoch/target map; reject before lookup.
- `invalid_map`: replace registered map bytes with `not JSON`.
- `max_size_map`: pad valid map JSON with ASCII spaces to exactly 1,048,576 bytes.
- `oversized_map`: replace map bytes with 1,048,577 ASCII spaces; reject before parsing.
- `unsupported`: adapter does not implement lookup; return typed `Unsupported` with
  operation `lookup_source_map`, reason `not_implemented`.

The test-only Rust adapter uses the pinned `sourcemap` library for every successful
lookup and `source_chunk` for retrieval. The map-input limit is 1 MiB, matching runtime
`SourceMaps`; this is not a production source-map service.
