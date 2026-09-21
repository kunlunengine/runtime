const fs = await kunlun.import('kunlun:fs');
// Deliberately abandon producers and a read. Evaluation cleanup, not a JS
// cancel/finally block, owns these resources on both success and rejection.
globalThis.m2AbandonedStreams = await Promise.all(
    Array.from({ length: 3 }, () => fs.openReadStream(m2Path))
);
m2AbandonedStreams[0].read();
if (m2Throw) throw new Error('m2 stream failure');
return 'abandoned';
