/// <reference path="./index.d.ts" />
/** Proposed Fetch entry contract; native invocation is tracked by Runtime #51. */
interface Request {
  readonly url: string
  readonly method: string
  readonly signal: AbortSignal
  readonly body: ReadableStream<Uint8Array> | null
}

interface Response {
  readonly status: number
  readonly body: ReadableStream<Uint8Array> | null
}

export interface ServerExecutionContextV1 {
  readonly signal: AbortSignal
  waitUntil(promise: PromiseLike<unknown>): void
}

export type ServerEnvironmentV1 = Readonly<Record<string, unknown>>

export interface ServerEntryV1 {
  fetch(
    request: Request,
    env: ServerEnvironmentV1,
    executionContext: ServerExecutionContextV1,
  ): Response | PromiseLike<Response>
}
