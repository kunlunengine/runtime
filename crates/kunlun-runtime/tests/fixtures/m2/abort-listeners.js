// Observe the public listener lifecycle without adding a production JS test API.
// Retain every observed signal until the assertion, so GC cannot hide a leak.
const listeners = new Map();
const originalAdd = AbortSignal.prototype.addEventListener;
const originalRemove = AbortSignal.prototype.removeEventListener;
AbortSignal.prototype.addEventListener = function(type, listener, options) {
    originalAdd.call(this, type, listener, options);
    if (type === 'abort') {
        if (!listeners.has(this)) listeners.set(this, new Set());
        listeners.get(this).add(listener);
    }
};
AbortSignal.prototype.removeEventListener = function(type, listener) {
    originalRemove.call(this, type, listener);
    if (type === 'abort') listeners.get(this)?.delete(listener);
};
try {
    const fs = await kunlun.import('kunlun:fs');
    for (let round = 0; round < 16; round++) {
        for (const phase of ['success', 'error', 'before', 'pending', 'stream', 'eof']) {
            const controller = new AbortController();
            const signal = controller.signal;
            const reason = new Error('m2 listener abort');
            if (phase === 'before') controller.abort(reason);
            try {
                if (phase === 'stream' || phase === 'eof') {
                    const source = await fs.openReadStream(m2Path, { signal });
                    const reader = source.toReadableStream().getReader();
                    await reader.read();
                    if (phase === 'stream') {
                        controller.abort(reason);
                        await reader.closed.catch(error => {
                            if (error !== reason) throw error;
                        });
                    } else {
                        while (!(await reader.read()).done) {}
                        await reader.closed;
                    }
                } else {
                    const pending = fs.readTextFile(
                        phase === 'error' ? m2Path + '.missing' : m2Path, { signal });
                    if (phase === 'pending') controller.abort(reason);
                    await pending;
                    if (phase !== 'success') throw new Error('expected operation failure');
                }
            } catch (error) {
                if (phase === 'before' || phase === 'pending') {
                    if (error !== reason) throw error;
                } else if (phase !== 'error') throw error;
            }
            await Promise.resolve();
            for (const entries of listeners.values()) {
                if (entries.size !== 0) throw new Error('retained abort listener: ' + phase);
            }
        }
    }
    return 'listeners-ok';
} finally {
    AbortSignal.prototype.addEventListener = originalAdd;
    AbortSignal.prototype.removeEventListener = originalRemove;
}
