/// <reference path="./index.d.ts" />
/** Fetch entry contract; native invocation is tracked by Runtime #51. */

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
