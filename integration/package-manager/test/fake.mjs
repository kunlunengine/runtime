// Only invoked by tests via an explicitly selected executable wrapper.
import { readFileSync } from 'node:fs';
const config = JSON.parse(readFileSync(new URL('./scenario.json', import.meta.url)));
if (config.hang) setInterval(() => {}, 1000);
else if (config.signal) process.kill(process.pid, 'SIGTERM');
else {
  if (config.stderr) process.stderr.write(Buffer.from(config.stderr, 'base64'));
  if (config.stdout) process.stdout.write(Buffer.from(config.stdout, 'base64'));
  process.exitCode = config.exit ?? 0;
}
