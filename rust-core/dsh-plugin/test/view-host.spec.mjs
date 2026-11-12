import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createOverviewHandler, createContextHandler, createDecisionHandler, mountSicOverview } from '../view-host.mjs';

test('overview serves only the current session through a host-only bearer', async () => {
  const requests = [];
  const handler = createOverviewHandler({ baseURL: 'http://127.0.0.1:18890/v1', credential: () => 'host-token',
    fetchImpl: async (url, options) => {
      requests.push({ path: new URL(url).pathname, auth: options.headers.authorization, body: JSON.parse(options.body) });
      const part = new URL(url).pathname.split('/').at(-1);
      return Response.json(part === 'status' ? { chain_id: 'fixture-chain' }
        : part === 'accounting' ? { groups: [{ role: 'editor' }] } : { items: [] });
    },
  });
  const response = await handler(new Request('http://dsh.test/api/sic-core/overview', {
    method: 'POST', body: JSON.stringify({ sessionId: 'session-1' }),
  }));
  assert.equal(response.status, 200);
  assert.equal(response.headers.get('cache-control'), 'no-store');
  const value = await response.json();
  assert.equal(value.value.status.value.chain_id, 'fixture-chain');
  assert.equal(value.value.accounting.value.groups[0].role, 'editor');
  assert.equal(requests.length, 3);
  assert.ok(requests.every(r => r.auth === 'Bearer host-token' && r.body.session_id === 'session-1'));
  assert.ok(!JSON.stringify(value).includes('host-token'));
  const invalid = await handler(new Request('http://dsh.test/api/sic-core/overview', {
    method: 'POST', body: JSON.stringify({ sessionId: 'session-1', chain_id: 'forged' }),
  }));
  assert.equal(invalid.status, 400);
  assert.equal(requests.length, 3);
});

test('overview reports partial failure and refuses remote token destinations', async () => {
  assert.throws(() => createOverviewHandler({ baseURL: 'https://remote.example/v1', credential: () => 'private' }));
  const handler = createOverviewHandler({ baseURL: 'http://127.0.0.1/v1', credential: () => 'x',
    fetchImpl: async url => new URL(url).pathname.endsWith('/status')
      ? new Response('', { status: 404 }) : Response.json({ items: [] }),
  });
  const response = await handler(new Request('http://dsh.test', { method: 'POST', body: '{"sessionId":"s"}' }));
  const result = await response.json();
  assert.equal(result.value.status.error, 'session_not_found');
  assert.ok(result.value.accounting.value);
});

test('overview route mounts only through the DSH connection service', () => {
  const registered = [];
  const connection = { fetch: { register(route) { registered.push(route); return () => {}; } } };
  const child = { get: () => connection, effect: callback => callback() };
  const ctx = { inject: (deps, callback) => { assert.deepEqual(deps, ['connection']); callback(child); } };
  mountSicOverview(ctx, { baseURL: 'http://localhost/v1' }, () => 'token');
  assert.deepEqual(registered.map(r => r.path), ['/api/sic-core/overview', '/api/sic-core/context', '/api/sic-core/decision-event']);
  assert.ok(registered.every(r => r.methods.join() === 'POST'));
});

test('decision write requires same-origin JSON and forwards no caller-supplied chain', async () => {
  let calls = 0;
  const handler = createDecisionHandler({ baseURL: 'http://127.0.0.1:1234/v1', credential: () => 'private-token',
    fetchImpl: async (_url, options) => { calls++; const body = JSON.parse(options.body); assert.equal(body.session_id, 's');
      assert.equal(body.expected_revision, 1); assert.equal(body.kind, 'work'); assert.equal(body.chain_id, undefined);
      return Response.json({ decision: { revision: 2 } }); },
  });
  const request = (origin, body) => new Request('http://dsh.test/api/sic-core/decision-event', {
    method: 'POST', headers: { origin, host: 'dsh.test', 'content-type': 'application/json', 'sec-fetch-site': origin === 'http://dsh.test' ? 'same-origin' : 'cross-site' },
    body: JSON.stringify(body),
  });
  const body = { sessionId: 's', expectedRevision: 1, eventId: 'event-1', kind: 'work', text: 'next task' };
  assert.equal((await handler(request('http://evil.test', body))).status, 403);
  const forged = new Request('http://dsh.test/api/sic-core/decision-event', { method: 'POST',
    headers: { origin: 'http://evil.test', host: 'dsh.test', 'content-type': 'application/json', 'sec-fetch-site': 'same-origin' }, body: JSON.stringify(body) });
  assert.equal((await handler(forged)).status, 403);
  assert.equal((await handler(request('http://dsh.test', { ...body, chain_id: 'forged' }))).status, 400);
  const accepted = await handler(request('http://dsh.test', body));
  assert.equal(accepted.status, 200);
  assert.equal((await accepted.json()).value.decision.revision, 2);
  assert.equal(calls, 1);
});

test('context route is on-demand, session-scoped and does not expose its bearer', async () => {
  let forwarded;
  const handler = createContextHandler({ baseURL: 'http://127.0.0.1:1234/v1', credential: () => 'private-token',
    fetchImpl: async (url, options) => { forwarded = { path: new URL(url).pathname, auth: options.headers.authorization, body: JSON.parse(options.body) };
      return Response.json({ action: 'timeline', result: { rows: [] } }); },
  });
  const request = body => new Request('http://dsh.test/api/sic-core/context', { method: 'POST', body: JSON.stringify(body) });
  const response = await handler(request({ sessionId: 'session-1', action: 'timeline', params: { level: 1, kind: 'cb', offset: 0, limit: 20 } }));
  assert.equal(response.status, 200);
  assert.deepEqual(forwarded.body, { session_id: 'session-1', action: 'timeline', params: { level: 1, kind: 'cb', offset: 0, limit: 20 } });
  assert.equal(forwarded.auth, 'Bearer private-token');
  assert.ok(!JSON.stringify(await response.json()).includes('private-token'));
  assert.equal((await handler(request({ sessionId: 'session-1', action: 'chain.create', params: {} }))).status, 400);
});
