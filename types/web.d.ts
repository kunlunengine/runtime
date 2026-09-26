/** M2 Web primitives and M3 Fetch profile. See docs/runtime-profile-v1.md. */
declare class TextEncoder {
  readonly encoding: "utf-8"
  encode(input?: string): Uint8Array
  encodeInto(input: string, destination: Uint8Array): { read: number; written: number }
}
declare class TextDecoder {
  constructor(label?: string, options?: { fatal?: boolean; ignoreBOM?: boolean })
  readonly encoding: "utf-8"
  readonly fatal: boolean
  readonly ignoreBOM: boolean
  decode(input?: ArrayBuffer | ArrayBufferView, options?: { stream?: boolean }): string
}
declare class URLSearchParams implements Iterable<[string, string]> {
  constructor(init?: string | Iterable<Iterable<string>> | Record<string, string>)
  readonly size: number
  append(name: string, value: string): void
  delete(name: string, value?: string): void
  get(name: string): string | null
  getAll(name: string): string[]
  has(name: string, value?: string): boolean
  set(name: string, value: string): void
  sort(): void
  entries(): IterableIterator<[string, string]>
  keys(): IterableIterator<string>
  values(): IterableIterator<string>
  [Symbol.iterator](): IterableIterator<[string, string]>
  forEach(callback: (value: string, key: string, parent: URLSearchParams) => void, thisArg?: unknown): void
  toString(): string
}
declare class URL {
  constructor(input: string | URL, base?: string | URL)
  href: string
  readonly origin: string
  protocol: string
  username: string
  password: string
  host: string
  hostname: string
  port: string
  pathname: string
  search: string
  hash: string
  readonly searchParams: URLSearchParams
  toString(): string
  toJSON(): string
  static canParse(input: string | URL, base?: string | URL): boolean
  static parse(input: string | URL, base?: string | URL): URL | null
}
declare const console: {
  debug(...args: unknown[]): void
  log(...args: unknown[]): void
  info(...args: unknown[]): void
  warn(...args: unknown[]): void
  error(...args: unknown[]): void
}
type KunlunIntegerTypedArray = Int8Array | Uint8Array | Uint8ClampedArray | Int16Array | Uint16Array | Int32Array | Uint32Array | BigInt64Array | BigUint64Array
declare const crypto: {
  getRandomValues<T extends KunlunIntegerTypedArray>(array: T): T
  randomUUID(): string
  readonly subtle: {
    digest(algorithm: string | { name: string }, data: ArrayBuffer | ArrayBufferView): Promise<ArrayBuffer>
  }
}

type KunlunHeadersInit = Headers | Iterable<readonly [string, string]> | Record<string, string>
type KunlunBodyInit = string | ArrayBuffer | ArrayBufferView | URLSearchParams | ReadableStream<Uint8Array>

declare class Headers implements Iterable<[string, string]> {
  constructor(init?: KunlunHeadersInit)
  append(name: string, value: string): void
  delete(name: string): void
  get(name: string): string | null
  getSetCookie(): string[]
  has(name: string): boolean
  set(name: string, value: string): void
  entries(): IterableIterator<[string, string]>
  keys(): IterableIterator<string>
  values(): IterableIterator<string>
  [Symbol.iterator](): IterableIterator<[string, string]>
  forEach(callback: (value: string, name: string, parent: Headers) => void, thisArg?: unknown): void
}

interface KunlunBody {
  readonly body: ReadableStream<Uint8Array> | null
  readonly bodyUsed: boolean
  arrayBuffer(): Promise<ArrayBuffer>
  bytes(): Promise<Uint8Array>
  text(): Promise<string>
  json(): Promise<unknown>
}

interface KunlunRequestInit {
  method?: string
  headers?: KunlunHeadersInit
  body?: KunlunBodyInit | null
  signal?: AbortSignal
  redirect?: "follow" | "error" | "manual"
  duplex?: "half"
}

declare class Request implements KunlunBody {
  constructor(input: string | URL | Request, init?: KunlunRequestInit)
  readonly url: string
  readonly method: string
  readonly headers: Headers
  readonly signal: AbortSignal
  readonly redirect: "follow" | "error" | "manual"
  readonly duplex: "half"
  readonly body: ReadableStream<Uint8Array> | null
  readonly bodyUsed: boolean
  arrayBuffer(): Promise<ArrayBuffer>
  bytes(): Promise<Uint8Array>
  text(): Promise<string>
  json(): Promise<unknown>
  clone(): Request
}

interface KunlunResponseInit {
  status?: number
  statusText?: string
  headers?: KunlunHeadersInit
}

declare class Response implements KunlunBody {
  constructor(body?: KunlunBodyInit | null, init?: KunlunResponseInit)
  readonly status: number
  readonly statusText: string
  readonly headers: Headers
  readonly ok: boolean
  readonly url: string
  readonly redirected: boolean
  readonly body: ReadableStream<Uint8Array> | null
  readonly bodyUsed: boolean
  arrayBuffer(): Promise<ArrayBuffer>
  bytes(): Promise<Uint8Array>
  text(): Promise<string>
  json(): Promise<unknown>
  clone(): Response
  static redirect(url: string | URL, status?: number): Response
}

declare function fetch(input: string | URL | Request, init?: KunlunRequestInit): Promise<Response>

interface KunlunWebExports {
  console: typeof console
  TextEncoder: typeof TextEncoder
  TextDecoder: typeof TextDecoder
  URL: typeof URL
  URLSearchParams: typeof URLSearchParams
  ReadableStream: typeof ReadableStream
  WritableStream: typeof WritableStream
  TransformStream: typeof TransformStream
  ByteLengthQueuingStrategy: typeof ByteLengthQueuingStrategy
  CountQueuingStrategy: typeof CountQueuingStrategy
  crypto: typeof crypto
  AbortController: typeof AbortController
  AbortSignal: typeof AbortSignal
  Headers: typeof Headers
  Request: typeof Request
  Response: typeof Response
  fetch: typeof fetch
}

type KunlunReadableStream<R = any> = ReadableStream<R>
type KunlunWritableStream<W = any> = WritableStream<W>
type KunlunTransformStream<I = any, O = any> = TransformStream<I, O>
declare module "kunlun:web" {
  export type ReadableStream<R = any> = KunlunReadableStream<R>
  export type WritableStream<W = any> = KunlunWritableStream<W>
  export type TransformStream<I = any, O = any> = KunlunTransformStream<I, O>
  export const console: KunlunWebExports["console"]
  export type TextEncoder = InstanceType<KunlunWebExports["TextEncoder"]>
  export const TextEncoder: KunlunWebExports["TextEncoder"]
  export type TextDecoder = InstanceType<KunlunWebExports["TextDecoder"]>
  export const TextDecoder: KunlunWebExports["TextDecoder"]
  export type URL = InstanceType<KunlunWebExports["URL"]>
  export const URL: KunlunWebExports["URL"]
  export type URLSearchParams = InstanceType<KunlunWebExports["URLSearchParams"]>
  export const URLSearchParams: KunlunWebExports["URLSearchParams"]
  export const ReadableStream: KunlunWebExports["ReadableStream"]
  export const WritableStream: KunlunWebExports["WritableStream"]
  export const TransformStream: KunlunWebExports["TransformStream"]
  export const ByteLengthQueuingStrategy: KunlunWebExports["ByteLengthQueuingStrategy"]
  export const CountQueuingStrategy: KunlunWebExports["CountQueuingStrategy"]
  export const crypto: KunlunWebExports["crypto"]
  export const AbortController: KunlunWebExports["AbortController"]
  export const AbortSignal: KunlunWebExports["AbortSignal"]
  export const Headers: KunlunWebExports["Headers"]
  export const Request: KunlunWebExports["Request"]
  export const Response: KunlunWebExports["Response"]
  export const fetch: KunlunWebExports["fetch"]
}
