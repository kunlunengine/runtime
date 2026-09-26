const equal = (actual, expected, label) => {
  if (actual !== expected) throw new Error(`${label}: ${actual} != ${expected}`);
};
const headers = new Headers([['X-Item', 'one'], ['x-item', 'two']]);
equal(headers.get('X-ITEM'), 'one, two', 'header values');
headers.set('x-item', 'last');
equal(headers.get('x-item'), 'last', 'header replacement');
const request = new Request('https://example.test/路径?q=中文', {
  method: 'POST', headers, body: '武陵 café 😀',
});
equal(request.url, 'https://example.test/%E8%B7%AF%E5%BE%84?q=%E4%B8%AD%E6%96%87', 'URL');
equal(request.method, 'POST', 'method');
equal(await request.clone().text(), '武陵 café 😀', 'request clone');
equal(await request.text(), '武陵 café 😀', 'request body');
equal(request.bodyUsed, true, 'request body used');
const transferable = new Request('https://example.test/transfer', { method: 'POST', body: 'moved' });
const transferred = new Request(transferable);
equal(transferable.bodyUsed, true, 'source request transferred');
equal(transferable.body.locked, true, 'source request locked');
equal(await transferred.text(), 'moved', 'transferred buffered body');
const streamedRequest = new Request('https://example.test/upload', {
  method: 'POST', duplex: 'half',
  body: new ReadableStream({ start(controller) {
    controller.enqueue(new Uint8Array([1, 2, 3])); controller.close();
  } }),
});
const transferredStream = new Request(streamedRequest);
equal(streamedRequest.bodyUsed, true, 'source stream transferred');
equal(streamedRequest.body.locked, true, 'source stream locked');
equal((await transferredStream.arrayBuffer()).byteLength, 3, 'streaming request body');
const response = new Response(new Uint8Array([0, 1, 127, 255]), { status: 201, headers });
equal(response.ok, true, 'response ok');
equal((await response.arrayBuffer()).byteLength, 4, 'binary response');
equal(new Response(null, { status: 204 }).body, null, 'null body');
return 'fetch-shared-ok';
