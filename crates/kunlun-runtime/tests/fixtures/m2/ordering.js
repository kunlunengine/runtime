const events = [];
const fs = await kunlun.import('kunlun:fs');
const timer = sleep(0).then(() => {
    events.push('timer');
    return Promise.resolve().then(() => events.push('timer-microtask'));
});
Promise.resolve().then(() => {
    events.push('microtask');
    Promise.resolve().then(() => events.push('nested'));
});
await timer;
// Deliberately serialize the host request after the timer. Unrelated disk and
// timer readiness has no portable total ordering.
const text = await fs.readTextFile(m2Path);
events.push(text.trim());
await Promise.resolve().then(() => events.push('host-microtask'));
return events.join(',');
