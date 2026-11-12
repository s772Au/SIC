import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createNativeEventForwarder, nativeToolEvent } from '../native-events.mjs';

function event(seq, overrides = {}) {
  return { type: 'tool/result', seq, surfaceOp: 'append', data: {
    message: { role: 'tool', toolCallId: 'call-1', source: { kind: 'tool', callId: 'call-1' },
      content: [{ type: 'text', text: 'PRIVATE_RESULT_SENTINEL' }], ...overrides.message },
    ...overrides.data,
  }, ...overrides.event };
}

function pageReader(events) {
  return async (_sessionId, seq, before) => ({
    startSeq: seq - before, endSeq: seq, events: events.slice(seq - before, seq + 1),
  });
}

test('native event decoding distinguishes repair outcomes without exporting content', () => {
  const ok = nativeToolEvent('session-1', event(3));
  assert.deepEqual(ok, { session_id: 'session-1', call_id: 'call-1', event_seq: 3, outcome: 'ok' });
  assert.equal(nativeToolEvent('session-1', event(4, { message: { isError: true } })).outcome, 'error');
  assert.equal(nativeToolEvent('session-1', event(5, { message: { isError: true }, data: { error: { code: 'TOOL_OUTCOME_UNKNOWN' } } })).outcome, 'unknown');
  assert.equal(nativeToolEvent('session-1', event(6, { message: { isError: true }, data: { error: { code: 'TOOL_NOT_STARTED' } } })).outcome, 'not_started');
  assert.equal(nativeToolEvent('session-1', event(7, { data: { error: { code: 'TOOL_OUTCOME_UNKNOWN' } } })), null);
  assert.equal(nativeToolEvent('session-1', event(8, { message: { toolCallId: 'other' } })), null);
  assert.equal(nativeToolEvent('session-1', event(9, { event: { surfaceOp: { kind: 'replace' } } })), null);
  assert.ok(!JSON.stringify(ok).includes('PRIVATE_RESULT_SENTINEL'));
});

test('only active SIC sessions forward, recover a bounded snapshot and retry failures', async () => {
  const requests = [];
  let fail = true;
  const events = Array.from({ length: 4 }, (_, seq) => ({ seq, type: 'turn/start' })).concat(event(4));
  const forwarder = createNativeEventForwarder({
    baseURL: 'http://127.0.0.1:18890/v1', credential: () => 'host-fixture-key',
    readEventPage: pageReader(events),
    fetchImpl: async (url, options) => {
      requests.push({ url: String(url), headers: options.headers, body: JSON.parse(options.body) });
      if (fail) { fail = false; return new Response('', { status: 503 }); }
      return Response.json({ recorded: true });
    },
  });
  const session = { id: 'session-1', seq: events.length,
    snapshotEvents: () => { throw new Error('deprecated synchronous history must not be called'); } };
  try {
    forwarder.observe(session, event(4));
    assert.equal(requests.length, 0, 'other DSH sessions are never forwarded before SIC activation');
    await forwarder.activateAndFlush('session-1');
    await forwarder.activateAndFlush('session-1');
    assert.equal(requests.length, 2);
    assert.equal(requests[1].url, 'http://127.0.0.1:18890/v1/sic/native-tool-event');
    assert.equal(requests[1].headers.authorization, 'Bearer host-fixture-key');
    assert.deepEqual(requests[1].body, { session_id: 'session-1', call_id: 'call-1', event_seq: 4, outcome: 'ok' });
    assert.ok(!JSON.stringify(requests[1].body).includes('PRIVATE_RESULT_SENTINEL'));
    await forwarder.activateAndFlush('session-1');
    assert.equal(requests.length, 2, 'acknowledged event is not repeatedly forwarded');
  } finally { forwarder.dispose(); }
});

test('native event forwarder refuses a non-loopback credential destination', () => {
  assert.throws(() => createNativeEventForwarder({ baseURL: 'https://remote.example/v1', credential: () => 'private' }));
});

test('bounded recovery gives the newest native results priority', async () => {
  const sent = [];
  const snapshot = Array.from({ length: 600 }, (_, i) => event(i, { message: {
    toolCallId: `call-${i}`, source: { kind: 'tool', callId: `call-${i}` },
  } }));
  const forwarder = createNativeEventForwarder({ baseURL: 'http://127.0.0.1:18890/v1', credential: () => 'fixture',
    readEventPage: pageReader(snapshot),
    fetchImpl: async (_url, options) => { sent.push(JSON.parse(options.body).event_seq); return Response.json({ recorded: true }); },
  });
  const session = { id: 'session-many', seq: snapshot.length };
  try {
    forwarder.observe(session, snapshot[0]);
    await forwarder.activateAndFlush(session.id);
    assert.deepEqual(sent, Array.from({ length: 16 }, (_, i) => 599 - i));
    for (let i = 0; i < 40 && sent.length < 512; i++) await forwarder.activateAndFlush(session.id);
    assert.equal(sent.length, 512);
    assert.equal(new Set(sent).size, 512);
    assert.equal(Math.min(...sent), 88);
  } finally { forwarder.dispose(); }
});

test('an unavailable query service leaves old outcomes unknown without blocking live events', async () => {
  const sent = [];
  const forwarder = createNativeEventForwarder({ baseURL: 'http://127.0.0.1:18890/v1', credential: () => 'fixture',
    readEventPage: async () => { throw new Error('query unavailable'); },
    fetchImpl: async (_url, options) => { sent.push(JSON.parse(options.body).event_seq); return Response.json({ recorded: true }); },
  });
  const session = { id: 'session-no-query', seq: 10 };
  try {
    forwarder.observe(session, event(9));
    await forwarder.activateAndFlush(session.id);
    forwarder.observe(session, event(10));
    await forwarder.activateAndFlush(session.id);
    assert.deepEqual(sent, [10]);
  } finally { forwarder.dispose(); }
});

test('a hung query is bounded and is not duplicated on later continuations', async () => {
  let reads = 0;
  const sent = [];
  const forwarder = createNativeEventForwarder({ baseURL: 'http://127.0.0.1:18890/v1', credential: () => 'fixture',
    readEventPage: () => { reads++; return new Promise(() => {}); },
    fetchImpl: async (_url, options) => { sent.push(JSON.parse(options.body).event_seq); return Response.json({ recorded: true }); },
  });
  const session = { id: 'session-hung-query', seq: 1 };
  try {
    forwarder.observe(session, event(0));
    await forwarder.activateAndFlush(session.id);
    await forwarder.activateAndFlush(session.id);
    forwarder.observe(session, event(1));
    await forwarder.activateAndFlush(session.id);
    assert.equal(reads, 1);
    assert.deepEqual(sent, [1]);
  } finally { forwarder.dispose(); }
});

test('continuation flush processes events queued after an in-flight batch began', async () => {
  let release;
  const gate = new Promise(resolve => { release = resolve; });
  const sent = [];
  const events = [{ seq: 0, type: 'turn/start' }, event(1), event(2)];
  const forwarder = createNativeEventForwarder({ baseURL: 'http://127.0.0.1:18890/v1', credential: () => 'fixture',
    readEventPage: pageReader(events),
    fetchImpl: async (_url, options) => {
      const sequence = JSON.parse(options.body).event_seq;
      sent.push(sequence);
      if (sequence === 1) await gate;
      return Response.json({ recorded: true });
    },
  });
  const session = { id: 'session-race', seq: events.length };
  try {
    await forwarder.activateAndFlush(session.id);
    forwarder.observe(session, event(1));
    forwarder.observe(session, event(2));
    const flush = forwarder.activateAndFlush(session.id);
    release();
    await flush;
    assert.deepEqual(sent, [1, 2]);
  } finally { forwarder.dispose(); }
});
