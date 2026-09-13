interface KunlunAbortEvent {
  readonly type: "abort"
  readonly target: AbortSignal
  readonly currentTarget: AbortSignal
}

interface KunlunAbortEventListener {
  (event: KunlunAbortEvent): void
}

interface KunlunAbortEventListenerObject {
  handleEvent(event: KunlunAbortEvent): void
}

interface KunlunAbortEventListenerOptions {
  once?: boolean
}

interface AbortSignal {
  readonly aborted: boolean
  readonly reason: unknown
  onabort: ((this: AbortSignal, event: KunlunAbortEvent) => unknown) | null
  throwIfAborted(): void
  addEventListener(
    type: "abort",
    listener: KunlunAbortEventListener | KunlunAbortEventListenerObject | null,
    options?: boolean | KunlunAbortEventListenerOptions,
  ): void
  removeEventListener(
    type: "abort",
    listener: KunlunAbortEventListener | KunlunAbortEventListenerObject | null,
  ): void
}

declare const AbortSignal: {
  readonly prototype: AbortSignal
  abort(reason?: unknown): AbortSignal
}

interface AbortController {
  readonly signal: AbortSignal
  abort(reason?: unknown): void
}

declare const AbortController: {
  readonly prototype: AbortController
  new (): AbortController
}

declare module "kunlun:fs" {
  export interface ReadOptions {
    signal?: AbortSignal
  }

  export interface ByteStream extends AsyncIterable<Uint8Array> {
    read(): Promise<IteratorResult<Uint8Array, undefined>>
    cancel(reason?: unknown): Promise<void>
  }

  /** Reads a UTF-8 file within a root granted to this isolate. */
  export function readTextFile(path: string, options?: ReadOptions): Promise<string>

  /** Opens a bounded, pull-driven byte stream within a granted read root. */
  export function openReadStream(path: string, options?: ReadOptions): Promise<ByteStream>
}

declare module "kunlun:http" {
  export interface HttpRequestInit {
    method?: string
    headers?: Readonly<Record<string, string>>
    body?: string
    signal?: AbortSignal
  }

  export interface HttpResponse {
    readonly status: number
    readonly headers: Readonly<Record<string, string>>
    readonly body: string
  }

  export interface StreamingHttpResponse {
    readonly status: number
    readonly headers: Readonly<Record<string, string>>
    readonly body: import("kunlun:fs").ByteStream
  }

  /** Sends a request to a host granted to this isolate. Redirects are not followed. */
  export function request(url: string, init?: HttpRequestInit): Promise<HttpResponse>

  /** Sends a request and exposes its response body as a bounded byte stream. */
  export function requestStream(
    url: string,
    init?: HttpRequestInit,
  ): Promise<StreamingHttpResponse>
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
