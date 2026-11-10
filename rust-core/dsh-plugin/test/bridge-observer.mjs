import http from 'node:http';
import { createHash } from 'node:crypto';

const upstream = process.env.SIC_RUST_UPSTREAM;
if (!upstream) throw new Error('SIC_RUST_UPSTREAM is required');
const observations = [];
const server = http.createServer(async (req, res) => {
  if (req.method === 'GET' && req.url === '/status') {
    res.writeHead(200, { 'content-type': 'application/json', 'cache-control': 'no-store' });
    res.end(JSON.stringify({ observations }));
    return;
  }
  const chunks = [];
  for await (const chunk of req) chunks.push(chunk);
  const raw = Buffer.concat(chunks);
  let body;
  try { body = JSON.parse(raw.toString('utf8')); } catch { body = {}; }
  const tools = Array.isArray(body.tools) ? body.tools : [];
  const isBind = req.url.endsWith('/sic/bind');
  const sessionIdentity = isBind ? body.session_id : req.headers['x-sic-bridge-session'];
  const summary = {
    route: req.url,
    authorizationPresent: /^Bearer .+/.test(req.headers.authorization ?? ''),
    bridgeSessionPresent: typeof req.headers['x-sic-bridge-session'] === 'string'
      && req.headers['x-sic-bridge-session'].trim().length > 0,
    bridgeSessionDigest: typeof req.headers['x-sic-bridge-session'] === 'string'
      ? createHash('sha256').update(req.headers['x-sic-bridge-session']).digest('hex').slice(0, 12) : null,
    requestSessionDigest: typeof sessionIdentity === 'string'
      ? createHash('sha256').update(sessionIdentity).digest('hex').slice(0, 12) : null,
    bindChainDigest: isBind && typeof body.chain_id === 'string'
      ? createHash('sha256').update(body.chain_id).digest('hex').slice(0, 12) : null,
    model: typeof body.model === 'string' ? body.model : null,
    maxTokens: Number.isInteger(body.max_tokens) ? body.max_tokens : null,
    toolNames: tools.map(tool => tool?.function?.name).filter(name => typeof name === 'string').sort(),
    toolSchemaHash: createHash('sha256').update(JSON.stringify(tools)).digest('hex').slice(0, 16),
    messageCount: Array.isArray(body.messages) ? body.messages.length : 0,
    responseStatus: null,
    errorCode: null,
  };
  observations.push(summary);
  try {
    const response = await fetch(new URL(req.url, upstream), {
      method: req.method,
      headers: {
        authorization: req.headers.authorization ?? '',
        'content-type': req.headers['content-type'] ?? 'application/json',
        'x-sic-bridge-session': req.headers['x-sic-bridge-session'] ?? '',
      },
      body: raw,
    });
    summary.responseStatus = response.status;
    res.writeHead(response.status, {
      'content-type': response.headers.get('content-type') ?? 'application/json',
      'cache-control': response.headers.get('cache-control') ?? 'no-store',
    });
    const responseBytes = Buffer.from(await response.arrayBuffer());
    try { summary.errorCode = JSON.parse(responseBytes.toString('utf8')).error?.code ?? null; } catch {}
    res.end(responseBytes);
  } catch {
    summary.responseStatus = 502;
    res.writeHead(502, { 'content-type': 'application/json' });
    res.end(JSON.stringify({ error: { message: 'local observer could not reach the Rust test service' } }));
  }
});
server.listen(0, '127.0.0.1', () => {
  process.stdout.write(JSON.stringify({ ready: true, port: server.address().port }) + '\n');
});
