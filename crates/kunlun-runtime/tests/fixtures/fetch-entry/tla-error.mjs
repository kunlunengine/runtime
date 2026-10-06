await Promise.reject(Error('private startup error'));
export default { fetch() { return new Response('not reached'); } };
