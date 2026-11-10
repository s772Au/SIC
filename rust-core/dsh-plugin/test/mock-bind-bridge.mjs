import http from 'node:http';

const expectedToken = process.env.SIC_CORE_TOKEN;
const expectedChain = 'synthetic-migrated-chain';
const order = [];
const bindings = [];
const chats = [];
let authValid = Boolean(expectedToken);
let protocolValid = true;

function sendSse(res, model, message, finishReason) {
  res.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-store' });
  const base = { object: 'chat.completion.chunk', created: Math.floor(Date.now() / 1000), model };
  const frames = [
    { ...base, id: 'mock-bind-chat', choices: [{ index: 0, delta: { role: 'assistant' }, finish_reason: null }] },
    { ...base, id: 'mock-bind-chat', choices: [{ index: 0, delta: message, finish_reason: null }] },
    { ...base, id: 'mock-bind-chat', choices: [{ index: 0, delta: {}, finish_reason: finishReason }] },
  ];
  for (const frame of frames) res.write(`data: ${JSON.stringify(frame)}\n\n`);
  res.end('data: [DONE]\n\n');
}

async function readJson(req) {
  const chunks = [];
  for await (const chunk of req) chunks.push(chunk);
  return JSON.parse(Buffer.concat(chunks).toString('utf8'));
}

const server = http.createServer(async (req, res) => {
  if (req.method === 'GET' && req.url === '/status') {
    res.writeHead(200, { 'content-type': 'application/json', 'cache-control': 'no-store' });
    res.end(JSON.stringify({ authValid, protocolValid, order, bindings, chats }));
    return;
  }
  if (req.method !== 'POST') { res.writeHead(404).end(); return; }
  authValid &&= Boolean(expectedToken && req.headers.authorization === `Bearer ${expectedToken}`);

  if (req.url === '/v1/sic/bind') {
    let body;
    try { body = await readJson(req); }
    catch { protocolValid = false; res.writeHead(400).end(); return; }
    const fields = body && Object.keys(body).sort();
    const valid = fields?.join(',') === 'chain_id,session_id'
      && typeof body.session_id === 'string' && body.session_id.trim().length > 0
      && body.chain_id === expectedChain;
    protocolValid &&= Boolean(valid);
    if (!valid || !authValid) { res.writeHead(403).end(); return; }
    if (bindings.length && bindings[0].sessionId !== body.session_id) {
      res.writeHead(409, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ error: { code: 'chain_already_bound', message: 'synthetic conflict' } }));
      return;
    }
    bindings.push({ sessionId: body.session_id, chainId: body.chain_id });
    order.push('bind');
    res.writeHead(200, { 'content-type': 'application/json', 'cache-control': 'no-store' });
    res.end(JSON.stringify({ session_id: body.session_id, chain_id: body.chain_id, replayed: bindings.length > 1 }));
    return;
  }

  if (req.url === '/v1/chat/completions') {
    let body;
    try { body = await readJson(req); }
    catch { protocolValid = false; res.writeHead(400).end(); return; }
    const sessionId = req.headers['x-sic-bridge-session'];
    const toolNames = Array.isArray(body.tools) ? body.tools.map(tool => tool?.function?.name).filter(Boolean).sort() : [];
    const fixtureResultObserved = Array.isArray(body.messages) && body.messages.some(message =>
      message?.role === 'tool' && typeof message.content === 'string'
        && message.content.includes('SIC_RUST_SYNTHETIC_FIXTURE_ONLY'));
    const valid = authValid && bindings.some(binding => binding.sessionId === sessionId)
      && toolNames.length === 1 && toolNames[0] === 'sic_fixture_read';
    protocolValid &&= Boolean(valid);
    if (!valid) { res.writeHead(409, { 'content-type': 'application/json' }); res.end('{}'); return; }
    order.push('chat');
    const record = { sessionId, toolNames, fixtureResultObserved };
    chats.push(record);
    if (fixtureResultObserved) {
      sendSse(res, body.model, { content: 'SIC_BIND_HEADLESS_ROUNDTRIP_OK' }, 'stop');
    } else {
      sendSse(res, body.model, {
        tool_calls: [{ index: 0, id: 'fixture-call-1', type: 'function', function: { name: 'sic_fixture_read', arguments: '{}' } }],
      }, 'tool_calls');
    }
    return;
  }
  res.writeHead(404).end();
});

server.listen(0, '127.0.0.1', () => {
  process.stdout.write(JSON.stringify({ ready: true, port: server.address().port }) + '\n');
});
