/** Data-only producer/consumer contract; see docs/runtime-manifest-v1.md. */
export interface RuntimeManifestV1 {
  readonly schema: "kunlun.runtime-manifest/v1"
  readonly engine: {
    readonly abi: 1
    readonly runtime_profile: "kunlun-m2-web/1"
  }
  readonly entry_contract: "kunlun.fetch-entry/v1"
  readonly entry: string
  readonly files: readonly RuntimeManifestFileV1[]
  readonly required_features: readonly ("closed-module-graph")[]
  readonly compatibility_flags: readonly ("source-map-v3")[]
  readonly capabilities: {
    readonly required: readonly CapabilityDeclarationV1[]
    readonly optional: readonly CapabilityDeclarationV1[]
  }
}

export interface RuntimeManifestFileV1 {
  /** Canonical `./` root-relative, percent-encoded file URL. */
  readonly url: string
  readonly kind: "module" | "asset" | "source_map"
  /** Lowercase SHA-256 of the exact file bytes. */
  readonly sha256: string
  /** Required only for source_map records. */
  readonly for?: string
}

export interface CapabilityDeclarationV1 {
  readonly name: string
  readonly resource: string
}
