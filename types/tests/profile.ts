/// <reference path="../index.d.ts" />
import { URL as WebURL, ReadableStream as WebReadable, TextEncoder as WebEncoder, crypto as webCrypto } from 'kunlun:web'
import { openReadStream } from 'kunlun:fs'

const url: WebURL = new WebURL('../path', 'https://example.test/a/')
url.searchParams.set('unicode', '中文')
const encoder: WebEncoder = new WebEncoder()
const stream: WebReadable<Uint8Array> = new WebReadable({
  start(controller) { controller.enqueue(encoder.encode(url.href)); controller.close() },
})
const abort = new AbortController()
const output = new WritableStream<Uint8Array>({
  async write(bytes) { await webCrypto.subtle.digest('SHA-256', bytes) },
})
void stream.pipeTo(output, { signal: abort.signal })
void openReadStream('input').then(source => source.toReadableStream().getReader().read())
void kunlun.import('kunlun:web').then(web => new web.TextDecoder().decode())
// @ts-expect-error Floating point arrays are not accepted for OS random filling.
webCrypto.getRandomValues(new Float32Array(1))
// @ts-expect-error Fetch is intentionally outside profile v1.
fetch('https://example.test')
