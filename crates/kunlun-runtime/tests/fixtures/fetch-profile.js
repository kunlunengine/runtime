const assert = (condition, message) => { if (!condition) throw new Error(message); };
const web = await kunlun.import('kunlun:web');
assert(web.fetch === fetch && web.Request === Request && web.Response === Response && web.Headers === Headers, 'module/global identity');
const headers = new Headers([['X-Thing', 'one'], ['x-thing', 'two']]);
assert(headers.get('X-THING') === 'one, two', 'duplicate header values');
assert([...headers][0][0] === 'x-thing', 'header normalization');
headers.set('x-thing', 'last');
assert(headers.get('x-thing') === 'last', 'header set');
try { headers.append('bad name', 'x'); throw new Error('invalid header accepted'); }
catch (error) { assert(error instanceof TypeError, 'header error'); }
try { headers.append('x', 'one\r\ntwo'); throw new Error('header injection accepted'); }
catch (error) { assert(error instanceof TypeError, 'header value error'); }

const request = new Request('https://example.test/path', { method: 'post', body: '中文' });
assert(request.method === 'POST' && request.headers.get('content-type')?.startsWith('text/plain'), 'request metadata');
assert(await request.clone().text() === '中文', 'request clone');
assert(await request.text() === '中文' && request.bodyUsed, 'request consume');
try { request.clone(); throw new Error('used request cloned'); }
catch (error) { assert(error instanceof TypeError, 'used request error'); }
try { new Request('https://example.test/', { body: 'x' }); throw new Error('GET body accepted'); }
catch (error) { assert(error instanceof TypeError, 'GET body error'); }
try { new Request('https://example.test/', { cache: 'reload' }); throw new Error('unsupported option accepted'); }
catch (error) { assert(error instanceof TypeError, 'unsupported option error'); }
try { new Request('https://example.test/', { headers: { Host: 'other.test' } }); throw new Error('Host header accepted'); }
catch (error) { assert(error instanceof TypeError, 'Host header error'); }

const bytes = new Uint8Array([0, 1, 127, 255]);
const response = new Response(bytes, { status: 201, headers: [['x-test', 'a'], ['x-test', 'b']] });
bytes[1] = 99;
assert(response.ok && response.status === 201 && response.headers.get('x-test') === 'a, b', 'response metadata');
assert((await response.clone().bytes()).join(',') === '0,1,127,255', 'response clone bytes');
assert((await response.bytes()).join(',') === '0,1,127,255', 'response bytes');
const direct = new Response('direct');
const directReader = direct.body.getReader();
await directReader.read();
directReader.releaseLock();
assert(direct.bodyUsed, 'direct stream consumption marks body used');
try { direct.clone(); throw new Error('disturbed body cloned'); }
catch (error) { assert(error instanceof TypeError, 'disturbed body error'); }
try { new Response('bad', { status: 204 }); throw new Error('204 body accepted'); }
catch (error) { assert(error instanceof TypeError, 'null body status error'); }
const streaming = new Response(new ReadableStream({ start(controller) { controller.enqueue(new Uint8Array([1])); controller.close(); } }));
try { streaming.clone(); throw new Error('streaming clone accepted'); }
catch (error) { assert(error instanceof TypeError, 'streaming clone error'); }
await streaming.body.cancel();
return 'fetch-objects-ok';
