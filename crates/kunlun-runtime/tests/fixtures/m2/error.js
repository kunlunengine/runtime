sleep(1000000);
const fs = await kunlun.import('kunlun:fs');
await fs.readTextFile(m2Path);
throw new Error('m2 expected failure');
