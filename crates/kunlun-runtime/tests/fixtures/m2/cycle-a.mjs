import { read } from './cycle-b.mjs';
export let value = 1;
export function bump() { value++; }
export { read };
globalThis.cycleRuns = (globalThis.cycleRuns || 0) + 1;
