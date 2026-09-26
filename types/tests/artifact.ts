/// <reference path="../index.d.ts" />
import type { RuntimeManifestV1 } from "../runtime-manifest-v1"
import type { ServerEntryV1 } from "../server-entry-v1"

const manifest = {
  schema: "kunlun.runtime-manifest/v1",
  engine: { abi: 1, runtime_profile: "kunlun-m2-web/1" },
  entry_contract: "kunlun.fetch-entry/v1",
  entry: "./server.mjs",
  files: [{ url: "./server.mjs", kind: "module", sha256: "0".repeat(64) }],
  required_features: ["closed-module-graph"],
  compatibility_flags: [],
  capabilities: { required: [], optional: [] },
} satisfies RuntimeManifestV1

const entry: ServerEntryV1 = {
  fetch(request, env, executionContext) {
    executionContext.waitUntil(Promise.resolve(request.url))
    return new Response(null, { status: 200 })
  },
}

void manifest
void entry
