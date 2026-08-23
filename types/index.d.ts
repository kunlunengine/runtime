declare module "kunlun:fs" {
  /** Reads a UTF-8 file within a root granted to this isolate. */
  export function readTextFile(path: string): Promise<string>
}

declare module "kunlun:http" {
  export interface HttpRequestInit {
    method?: string
    headers?: Readonly<Record<string, string>>
    body?: string
  }

  export interface HttpResponse {
    readonly status: number
    readonly headers: Readonly<Record<string, string>>
    readonly body: string
  }

  /** Sends a request to a host granted to this isolate. Redirects are not followed. */
  export function request(url: string, init?: HttpRequestInit): Promise<HttpResponse>
}

interface KunlunBuiltinModules {
  "kunlun:fs": typeof import("kunlun:fs")
  "kunlun:http": typeof import("kunlun:http")
}

interface KunlunRuntimeBootstrap {
  /**
   * Bootstrap loader used until native JSC ESM loading is enabled. Native
   * `import` resolves the same module specifiers and exports.
   */
  import<K extends keyof KunlunBuiltinModules>(
    specifier: K,
  ): Promise<KunlunBuiltinModules[K]>
}

declare const kunlun: KunlunRuntimeBootstrap
