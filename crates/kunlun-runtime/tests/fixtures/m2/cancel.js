globalThis.m2Started = true;
await sleep(1000000);
throw new Error('cancelled continuation ran');
