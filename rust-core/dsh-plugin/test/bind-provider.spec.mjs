import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { test } from 'node:test';
import { createProvider } from '../index.mjs';

const hostToken = 'test-host-bearer-only';
const providerToken = 'test-provider-token-only';

function deferred() {
  let resolve;
  const promise = new Promise(done => { resolve = done; });
  return { promise, resolve };
}

async function listen(handler) {
  const server = createServer(handler);
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  const address = server.address();
  return {
    server,
    baseURL: `http://127.0.0.1:${address.port}/v1`,
    close: async () => {
      server.closeAllConnections();
      await new Promise(resolve => server.close(resolve));
    },
  };
}

function readJson(req) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    req.on('data', chunk => chunks.push(chunk));
    req.on('end', () => {
      try { resolve(JSON.parse(Buffer.concat(chunks).toString('utf8'))); }
      catch (error) { reject(error); }
    });
    req.on('error', reject);
  });
}

function config(baseURL, extra = {}) {
  return {
    provider: 'sic-test',
    baseURL,
    apiKeyEnv: 'SIC_TEST_HOST_TOKEN',
    model: 'sic-worker',
    bindChainId: 'chain-migrated-fixture',
    bindTimeoutMs: 500,
    timeoutMs: 2000,
    maxTokens: 4096,
    inputPricePerMillion: 0,
    outputPricePerMillion: 0,
    cacheReadPricePerMillion: 0,
    cacheWritePricePerMillion: 0,
    allowedTools: [],
    ...extra,
  };
}

const model = {
  id: 'sic-worker',
  name: 'SIC Rust Worker',
  provider: 'sic-test',
  api: 'openai-completions',
  baseUrl: '',
  reasoning: false,
  input: ['text'],
  contextWindow: 1_000_000,
  maxTokens: 393_216,
  cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
  compat: { supportsStore: false, supportsDeveloperRole: false, supportsUsageInStreaming: true, maxTokensField: 'max_tokens' },
};

function sendChatStream(res) {
  res.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-store' });
  const frames = [
    { id: 'mock-completion', object: 'chat.completion.chunk', created: 1, model: 'sic-worker', choices: [{ index: 0, delta: { role: 'assistant' }, finish_reason: null }] },
    { id: 'mock-completion', object: 'chat.completion.chunk', created: 1, model: 'sic-worker', choices: [{ index: 0, delta: { content: 'local mock response' }, finish_reason: null }] },
    { id: 'mock-completion', object: 'chat.completion.chunk', created: 1, model: 'sic-worker', choices: [{ index: 0, delta: {}, finish_reason: 'stop' }] },
  ];
  for (const frame of frames) res.write(`data: ${JSON.stringify(frame)}\n\n`);
  res.end('data: [DONE]\n\n');
}

async function collect(stream) {
  const events = [];
  for await (const event of stream) events.push(event.type);
  return { events, result: await stream.result() };
}

test('bind is an awaited one-per-session gate for stream and streamSimple', async () => {
  process.env.SIC_TEST_HOST_TOKEN = hostToken;
  const releaseBind = deferred();
  const order = [];
  const binds = [];
  const chats = [];
  const local = await listen(async (req, res) => {
    if (req.url === '/v1/sic/bind') {
      const body = await readJson(req);
      order.push('bind-start');
      binds.push({ body, authorization: req.headers.authorization });
      await releaseBind.promise;
      order.push('bind-end');
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ session_id: body.session_id, chain_id: body.chain_id, replayed: false }));
      return;
    }
    if (req.url === '/v1/chat/completions') {
      const body = await readJson(req);
      order.push('chat');
      chats.push({ body, authorization: req.headers.authorization, session: req.headers['x-sic-bridge-session'] });
      sendChatStream(res);
      return;
    }
    res.writeHead(404).end();
  });
  try {
    const instance = createProvider(config(local.baseURL));
    const selected = { ...model, baseUrl: local.baseURL };
    const options = { sessionId: 'dsh-concurrent-1', apiKey: providerToken, maxTokens: 4096 };
    const first = instance.provider.stream(selected, { messages: [{ role: 'user', content: 'one' }] }, options);
    const second = instance.provider.streamSimple(selected, { messages: [{ role: 'user', content: 'two' }] }, options);

    for (let i = 0; i < 50 && binds.length === 0; i++) await new Promise(resolve => setTimeout(resolve, 2));
    assert.equal(binds.length, 1);
    assert.equal(chats.length, 0, 'chat must wait until bind acknowledgement');
    assert.deepEqual(binds[0], {
      body: { session_id: 'dsh-concurrent-1', chain_id: 'chain-migrated-fixture' },
      authorization: `Bearer ${hostToken}`,
    });

    releaseBind.resolve();
    const [firstResult, secondResult] = await Promise.all([collect(first), collect(second)]);
    assert.equal(chats.length, 2);
    assert.ok(order.indexOf('bind-end') < order.indexOf('chat'));
    assert.ok(chats.every(call => call.authorization === `Bearer ${providerToken}`));
    assert.ok(chats.every(call => call.session === 'dsh-concurrent-1'));
    assert.ok(firstResult.events.includes('done') && secondResult.events.includes('done'));
    assert.equal(firstResult.result.stopReason, 'stop');
    assert.equal(secondResult.result.stopReason, 'stop');
  } finally {
    delete process.env.SIC_TEST_HOST_TOKEN;
    await local.close();
  }
});

test('bind rejection produces a terminal stream error and never reaches chat', async () => {
  process.env.SIC_TEST_HOST_TOKEN = hostToken;
  let bindCalls = 0;
  let chatCalls = 0;
  const local = await listen(async (req, res) => {
    if (req.url === '/v1/sic/bind') {
      bindCalls++;
      await readJson(req);
      res.writeHead(409, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ error: { code: 'chain_already_bound', message: 'private detail must not escape' } }));
      return;
    }
    if (req.url === '/v1/chat/completions') {
      chatCalls++;
      sendChatStream(res);
      return;
    }
    res.writeHead(404).end();
  });
  try {
    const instance = createProvider(config(local.baseURL));
    const stream = instance.provider.stream({ ...model, baseUrl: local.baseURL }, { messages: [] }, {
      sessionId: 'dsh-conflict', apiKey: providerToken,
    });
    const observed = await collect(stream);
    assert.equal(bindCalls, 1, 'the host bind itself is not retried');
    assert.equal(chatCalls, 0);
    assert.ok(observed.events.includes('error'));
    assert.equal(observed.result.stopReason, 'error');
    assert.match(observed.result.errorMessage, /HTTP 409/);
    assert.doesNotMatch(observed.result.errorMessage, /private detail|test-host-bearer/);
  } finally {
    delete process.env.SIC_TEST_HOST_TOKEN;
    await local.close();
  }
});

test('a cancelled waiter on a shared bind does not start its Chat stream', async () => {
  process.env.SIC_TEST_HOST_TOKEN = hostToken;
  const releaseBind = deferred();
  const binds = [];
  const chats = [];
  const local = await listen(async (req, res) => {
    if (req.url === '/v1/sic/bind') {
      const body = await readJson(req);
      binds.push(body);
      await releaseBind.promise;
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ session_id: body.session_id, chain_id: body.chain_id, replayed: false }));
      return;
    }
    if (req.url === '/v1/chat/completions') {
      chats.push(req.headers['x-sic-bridge-session']);
      sendChatStream(res);
      return;
    }
    res.writeHead(404).end();
  });
  try {
    const instance = createProvider(config(local.baseURL));
    const selected = { ...model, baseUrl: local.baseURL };
    const first = instance.provider.stream(selected, { messages: [] }, {
      sessionId: 'dsh-shared-cancel', apiKey: providerToken,
    });
    const controller = new AbortController();
    const second = instance.provider.streamSimple(selected, { messages: [] }, {
      sessionId: 'dsh-shared-cancel', apiKey: providerToken, signal: controller.signal,
    });
    for (let i = 0; i < 50 && binds.length === 0; i++) await new Promise(resolve => setTimeout(resolve, 2));
    assert.equal(binds.length, 1);
    assert.equal(chats.length, 0);

    controller.abort();
    releaseBind.resolve();
    const [firstResult, secondResult] = await Promise.all([collect(first), collect(second)]);
    assert.equal(firstResult.result.stopReason, 'stop');
    assert.equal(secondResult.result.stopReason, 'error');
    assert.match(secondResult.result.errorMessage, /cancelled/);
    assert.deepEqual(chats, ['dsh-shared-cancel']);
  } finally {
    delete process.env.SIC_TEST_HOST_TOKEN;
    await local.close();
  }
});

test('a second DSH session cannot claim the configured chain in the same process', async () => {
  process.env.SIC_TEST_HOST_TOKEN = hostToken;
  const binds = [];
  const chats = [];
  const local = await listen(async (req, res) => {
    if (req.url === '/v1/sic/bind') {
      const body = await readJson(req);
      binds.push(body);
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ session_id: body.session_id, chain_id: body.chain_id, replayed: false }));
      return;
    }
    if (req.url === '/v1/chat/completions') {
      chats.push(req.headers['x-sic-bridge-session']);
      sendChatStream(res);
      return;
    }
    res.writeHead(404).end();
  });
  try {
    const instance = createProvider(config(local.baseURL));
    const selected = { ...model, baseUrl: local.baseURL };
    const call = sessionId => collect(instance.provider.stream(selected, { messages: [] }, { sessionId, apiKey: providerToken }));
    const first = await call('dsh-owner');
    const second = await call('dsh-other');
    assert.equal(first.result.stopReason, 'stop');
    assert.equal(second.result.stopReason, 'error');
    assert.match(second.result.errorMessage, /different DSH session/);
    assert.deepEqual(binds.map(binding => binding.session_id), ['dsh-owner']);
    assert.deepEqual(chats, ['dsh-owner']);
  } finally {
    delete process.env.SIC_TEST_HOST_TOKEN;
    await local.close();
  }
});

test('bind timeout and caller cancellation both prevent chat', async t => {
  await t.test('bounded timeout', async () => {
    process.env.SIC_TEST_HOST_TOKEN = hostToken;
    let chatCalls = 0;
    const local = await listen(async (req, res) => {
      if (req.url === '/v1/sic/bind') {
        await readJson(req);
        setTimeout(() => {
          if (!res.destroyed) {
            res.writeHead(200, { 'content-type': 'application/json' });
            res.end(JSON.stringify({ session_id: 'late-session', chain_id: 'chain-migrated-fixture' }));
          }
        }, 100);
        return;
      }
      if (req.url === '/v1/chat/completions') {
        chatCalls++;
        sendChatStream(res);
        return;
      }
      res.writeHead(404).end();
    });
    try {
      const instance = createProvider(config(local.baseURL, { bindTimeoutMs: 20 }));
      const observed = await collect(instance.provider.stream({ ...model, baseUrl: local.baseURL }, { messages: [] }, {
        sessionId: 'late-session', apiKey: providerToken,
      }));
      assert.equal(observed.result.stopReason, 'error');
      assert.match(observed.result.errorMessage, /timed out/);
      assert.equal(chatCalls, 0);
    } finally {
      delete process.env.SIC_TEST_HOST_TOKEN;
      await local.close();
    }
  });

  await t.test('caller cancellation', async () => {
    process.env.SIC_TEST_HOST_TOKEN = hostToken;
    let bindCalls = 0;
    let chatCalls = 0;
    const local = await listen(async (req, res) => {
      if (req.url === '/v1/sic/bind') {
        bindCalls++;
        await readJson(req);
        return;
      }
      if (req.url === '/v1/chat/completions') {
        chatCalls++;
        sendChatStream(res);
        return;
      }
      res.writeHead(404).end();
    });
    try {
      const instance = createProvider(config(local.baseURL, { bindTimeoutMs: 1000 }));
      const controller = new AbortController();
      const stream = instance.provider.stream({ ...model, baseUrl: local.baseURL }, { messages: [] }, {
        sessionId: 'cancel-session', apiKey: providerToken, signal: controller.signal,
      });
      for (let i = 0; i < 50 && bindCalls === 0; i++) await new Promise(resolve => setTimeout(resolve, 2));
      controller.abort();
      const observed = await collect(stream);
      assert.equal(bindCalls, 1);
      assert.equal(chatCalls, 0);
      assert.equal(observed.result.stopReason, 'error');
      assert.match(observed.result.errorMessage, /cancelled/);
    } finally {
      delete process.env.SIC_TEST_HOST_TOKEN;
      await local.close();
    }
  });
});
