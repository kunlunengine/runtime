/// <reference path="./index.d.ts" />
/** Fetch entry contract; native invocation is tracked by Runtime #51. */

export interface ServerExecutionContextV1 {
  readonly signal: AbortSignal
  waitUntil(promise: PromiseLike<unknown>): void
}

/** A read-only, request-owned filesystem binding; paths are binding-relative. */
export interface ServerFileBindingV1 {
  readTextFile(path: string, options?: { signal?: AbortSignal }): Promise<string>
}

/** Each destination, including redirects, must match this binding's exact host. */
export interface ServerHttpBindingV1 {
  fetch(input: string | URL | Request, init?: KunlunRequestInit): Promise<Response>
}

/**
 * Only the declaration/deployment intersection is projected. Absent optional
 * bindings are undefined. Handles are not serializable and expire with the
 * request. Caller credentials and OS environment variables are never projected.
 */
export interface ServerEnvironmentV1 {
  readonly fs: Readonly<Record<string, ServerFileBindingV1 | undefined>>
  readonly http: Readonly<Record<string, ServerHttpBindingV1 | undefined>>
}

export interface ServerEntryV1 {
  fetch(
    request: Request,
    env: ServerEnvironmentV1,
    executionContext: ServerExecutionContextV1,
  ): Response | PromiseLike<Response>
}
