import { createProvider, parseResponse, type Response } from '../adapter.mjs';
const provider = createProvider('/opt/kunlun-pm');
const plan = await provider.run({ operation: 'plan', projectRoot: '/project', frozen: true });
if (plan.status === 'ok') {
  const frozen: true = plan.result.frozen;
  // @ts-expect-error plan is not a version result
  plan.result.capabilities;
  // @ts-expect-error readonly nested arrays
  plan.result.nodes.push({});
  // @ts-expect-error readonly fields
  plan.result.readOnly = false;
  void frozen;
} else {
  plan.diagnostics[0]?.code;
  // @ts-expect-error error has no result
  plan.result;
}
declare const response: Response;
if (response.status === 'ok' && response.operation === 'version') {
  const noInstall: false = response.result.capabilities.install;
  void noInstall;
}
const unknown = parseResponse(new Uint8Array(), 'unknown', 2);
const error: 'error' = unknown.status;
void error;
// @ts-expect-error project root required
provider.run({ operation: 'detect' });
// @ts-expect-error package required
provider.run({ operation: 'why', projectRoot: '/project' });
// @ts-expect-error install unavailable
provider.run({ operation: 'install', projectRoot: '/project' });
// @ts-expect-error flags only for plan
provider.run({ operation: 'detect', projectRoot: '/project', frozen: true });
