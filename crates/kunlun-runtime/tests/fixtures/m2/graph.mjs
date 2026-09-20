import * as a from './cycle-a.mjs';
a.bump();
for (let round = 0; round < 32; round++) {
    const modules = await Promise.all(Array.from({ length: 8 }, () => import('./cycle-a.mjs')));
    if (modules.some(module => module !== a)) throw new Error('dynamic import identity');
    if (a.read() !== 2 || a.value !== 2 || cycleRuns !== 1) throw new Error('cycle live binding');
}
globalThis.m2GraphResult = 'graph-ok';
