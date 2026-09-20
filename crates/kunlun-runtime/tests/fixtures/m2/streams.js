const fs = await kunlun.import('kunlun:fs');
let bytes = 0;
const stream = (await fs.openReadStream(m2Path)).toReadableStream();
await stream.pipeTo(new WritableStream({
    async write(chunk) {
        // Yield through the host timer dispatcher rather than assume an I/O
        // operation finishes within a wall-clock sleep.
        await sleep(0);
        if (chunk.some(byte => byte !== 42)) throw new Error('corrupt stream');
        bytes += chunk.length;
    }
}));
if (bytes !== 1048576) throw new Error('truncated stream');
for (const phase of ['before', 'pending', 'after']) {
    const abort = new AbortController();
    const reason = new Error('m2 abort ' + phase);
    if (phase === 'before') abort.abort(reason);
    try {
        const opening = fs.openReadStream(m2Path, { signal: abort.signal });
        if (phase === 'pending') abort.abort(reason);
        const source = (await opening).toReadableStream();
        if (phase !== 'after') throw new Error('aborted open fulfilled');
        const reader = source.getReader();
        await reader.read();
        abort.abort(reason);
        try {
            await reader.read();
            throw new Error('missing read abort');
        } catch (error) { if (error !== reason) throw error; }
        await reader.closed.catch(() => {});
    } catch (error) { if (error !== reason) throw error; }
}
const unused = (await fs.openReadStream(m2Path)).toReadableStream();
await unused.cancel('unused');
return 'streams-ok';
