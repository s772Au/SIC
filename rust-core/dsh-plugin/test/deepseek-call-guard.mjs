import http from 'node:http';

const upstream = new URL(process.env.SIC_LIVE_UPSTREAM ?? 'https://amr-link.open-design.ai/v1/chat/completions');
if (upstream.protocol !== 'https:' || upstream.hostname !== 'amr-link.open-design.ai'
    || upstream.pathname !== '/v1/chat/completions' || upstream.search || upstream.hash
    || upstream.username || upstream.password) {
  throw new Error('Live guard only permits the fixed DeepSeek Chat endpoint');
}
const expectedModel = 'deepseek-v4.1-flash-eco';
const expectedTokens = 4096;
const calls = [];
let editorCount = 0;
let workerCount = 0;
let invalidModel = false;
let invalidBudget = false;

function safeErrorCode(status) {
  if (status === 429) return 'provider_rate_limited';
  if (status >= 500) return 'provider_server_error';
  if (status >= 400) return 'provider_rejected';
  return null;
}

const server = http.createServer(async (req, res) => {
  if (req.method === 'GET' && req.url === '/status') {
    res.writeHead(200, { 'content-type': 'application/json', 'cache-control': 'no-store' });
    res.end(JSON.stringify({ editorCount, workerCount, invalidModel, invalidBudget, calls }));
    return;
  }
  if (req.method !== 'POST' || req.url !== '/v1/chat/completions') {
    res.writeHead(404).end();
    return;
  }
  const chunks = [];
  for await (const chunk of req) chunks.push(chunk);
  let body;
  try { body = JSON.parse(Buffer.concat(chunks).toString('utf8')); }
  catch {
    res.writeHead(400, { 'content-type': 'application/json' });
    res.end(JSON.stringify({ error: { code: 'invalid_json', message: 'Fixture guard rejected invalid JSON' } }));
    return;
  }
  if (body.model !== expectedModel) {
    invalidModel = true;
    res.writeHead(403, { 'content-type': 'application/json' });
    res.end(JSON.stringify({ error: { code: 'model_not_allowed', message: 'Live test permits one fixed model only' } }));
    return;
  }
  if (body.max_tokens !== expectedTokens) {
    invalidBudget = true;
    res.writeHead(403, { 'content-type': 'application/json' });
    res.end(JSON.stringify({ error: { code: 'budget_mismatch', message: 'Live test permits one fixed output budget only' } }));
    return;
  }
  const isEditor = JSON.stringify(body.messages).includes('Return exactly one sic_edit_blocks call');
  const role = isEditor ? 'editor' : 'worker';
  if (role === 'editor' ? ++editorCount > 3 : ++workerCount > 4) {
    calls.push({ role, model: expectedModel, maxTokens: expectedTokens, status: 429, errorCode: 'local_call_cap' });
    res.writeHead(429, { 'content-type': 'application/json', 'cache-control': 'no-store' });
    res.end(JSON.stringify({ error: { code: 'local_call_cap', message: 'Live test call cap reached; no provider request sent' } }));
    return;
  }
  const authorization = req.headers.authorization;
  if (typeof authorization !== 'string' || !authorization.startsWith('Bearer ') || authorization.length <= 7) {
    calls.push({ role, model: expectedModel, maxTokens: expectedTokens, status: 401, errorCode: 'credential_missing' });
    res.writeHead(401, { 'content-type': 'application/json' });
    res.end(JSON.stringify({ error: { code: 'credential_missing', message: 'Provider credential was not supplied' } }));
    return;
  }
  const record = { role, model: expectedModel, maxTokens: expectedTokens, status: null, errorCode: null, returnedModel: null, finishReason: null, usageKeys: [], durationMs: null };
  calls.push(record);
  const started = Date.now();
  try {
    // One outgoing request only. Redirects and retries are disabled.
    const response = await fetch(upstream, {
      method: 'POST',
      headers: { authorization, 'content-type': 'application/json' },
      body: JSON.stringify(body),
      redirect: 'manual',
      signal: AbortSignal.timeout(180_000),
    });
    const responseBytes = Buffer.from(await response.arrayBuffer());
    record.status = response.status;
    record.errorCode = safeErrorCode(response.status);
    record.durationMs = Date.now() - started;
    try {
      const result = JSON.parse(responseBytes.toString('utf8'));
      record.returnedModel = typeof result.model === 'string' ? result.model : null;
      record.finishReason = typeof result.choices?.[0]?.finish_reason === 'string' ? result.choices[0].finish_reason : null;
      record.usageKeys = result.usage && typeof result.usage === 'object' ? Object.keys(result.usage).sort() : [];
    } catch {}
    res.writeHead(response.status, {
      'content-type': response.headers.get('content-type') ?? 'application/json',
      'cache-control': 'no-store',
    });
    res.end(responseBytes);
  } catch (error) {
    record.status = 502;
    record.errorCode = error?.name === 'TimeoutError' ? 'provider_timeout' : 'transport';
    record.durationMs = Date.now() - started;
    res.writeHead(502, { 'content-type': 'application/json', 'cache-control': 'no-store' });
    res.end(JSON.stringify({ error: { code: record.errorCode, message: 'Local provider guard could not complete the single upstream attempt' } }));
  }
});

server.listen(0, '127.0.0.1', () => {
  process.stdout.write(JSON.stringify({ ready: true, port: server.address().port }) + '\n');
});

