// The host revokes application authority only after its server receives this call.
// It must observe invocation rejection before allowing the server to send headers.
await env.http['127.0.0.1'].fetch(requestAuthorityHttpInputs.base + '/pending');
return 'unexpected-success';
