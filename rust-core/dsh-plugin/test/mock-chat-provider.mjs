import http from 'node:http';
let requestCount = 0;
let failureCount = 0;
let credentialPresent = true;
let invalidRequest = false;
let workerFixtureIssued = false;
const calls = [];
const server = http.createServer(async (req, res) => {
  if (req.method === 'GET' && req.url === '/status') {
    res.writeHead(200, { 'content-type': 'application/json', 'cache-control': 'no-store' });
    res.end(JSON.stringify({ requestCount, failureCount, credentialPresent, invalidRequest, calls }));
    return;
  }
  if (req.method !== 'POST' || req.url !== '/v1/chat/completions') {
    res.writeHead(404).end();
    return;
  }
  const expectedKey = process.env.SIC_MOCK_PROVIDER_KEY;
  credentialPresent &&= Boolean(expectedKey && req.headers.authorization === `Bearer ${expectedKey}`);
  const chunks = [];
  for await (const chunk of req) chunks.push(chunk);
  let body;
  try { body = JSON.parse(Buffer.concat(chunks).toString('utf8')); }
  catch { invalidRequest = true; res.writeHead(400).end(); return; }
  requestCount += 1;
  if (body.model !== 'mock-model' || !Array.isArray(body.messages) || body.stream === true) invalidRequest = true;
  if (JSON.stringify(body.messages).includes('SIC_DSH_NO_RETRY_PROBE')) {
    failureCount += 1;
    res.writeHead(503, { 'content-type': 'application/json', 'cache-control': 'no-store' });
    res.end(JSON.stringify({ error: { message: 'local synthetic failure' } }));
    return;
  }
  const toolNames = (body.tools ?? []).map(tool => tool.function?.name);
  // The frozen model tool schema is shared across roles; distinguish Editor
  // calls by its static protocol instruction, without logging prompt contents.
  const isEditor = JSON.stringify(body.messages).includes('Return exactly one sic_edit_blocks call');
  const hasFixtureResult = Array.isArray(body.messages) && body.messages.some(message =>
    message?.role === 'tool' && typeof message.content === 'string'
      && message.content.includes('SIC_RUST_SYNTHETIC_FIXTURE_ONLY'));
  let message;
  let responseKind;
  if (isEditor) {
    responseKind = 'editor_edit_blocks';
    message = { role: 'assistant', content: null, tool_calls: [{ id: 'fixture-editor-1', type: 'function', function: { name: 'sic_edit_blocks', arguments: JSON.stringify({ ops: [], pvi: { P: ['local synthetic fixture'] } }) } }] };
  } else if (!workerFixtureIssued) {
    workerFixtureIssued = true;
    responseKind = 'worker_fixture_tool_call';
    message = { role: 'assistant', content: null, tool_calls: [{ id: 'fixture-worker-1', type: 'function', function: { name: 'sic_tool_call', arguments: JSON.stringify({ name: 'sic_fixture_read', arguments: {} }) } }] };
  } else {
    responseKind = 'worker_final_text';
    message = { role: 'assistant', content: 'SIC_RUST_DSH_MOCK_ROUNDTRIP_OK' };
  }
  const functionNames = message.tool_calls?.map(call => call.function.name) ?? [];
  calls.push({ model: body.model, maxTokens: body.max_tokens ?? null, toolNames: toolNames.filter(name => typeof name === 'string').sort(), responseKind, functionNames, fixtureResultObserved: hasFixtureResult });
  res.writeHead(200, { 'content-type': 'application/json', 'cache-control': 'no-store' });
  res.end(JSON.stringify({ model: 'mock-model', choices: [{ index: 0, finish_reason: functionNames.length ? 'tool_calls' : 'stop', message }], usage: { prompt_tokens: 10, completion_tokens: 5 } }));
});
server.listen(0, '127.0.0.1', () => {
  const address = server.address();
  process.stdout.write(JSON.stringify({ ready: true, port: address.port }) + '\n');
});
