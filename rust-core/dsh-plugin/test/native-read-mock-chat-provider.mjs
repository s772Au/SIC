import http from 'node:http';

let requestCount = 0;
let credentialPresent = true;
let invalidRequest = false;
let childFixtureIssued = false;
const calls = [];
const server = http.createServer(async (req, res) => {
  if (req.method === 'GET' && req.url === '/status') {
    res.writeHead(200, { 'content-type': 'application/json', 'cache-control': 'no-store' });
    res.end(JSON.stringify({ requestCount, credentialPresent, invalidRequest, calls }));
    return;
  }
  if (req.method !== 'POST' || req.url !== '/v1/chat/completions') { res.writeHead(404).end(); return; }
  const expectedKey = process.env.SIC_MOCK_PROVIDER_KEY;
  credentialPresent &&= Boolean(expectedKey && req.headers.authorization === `Bearer ${expectedKey}`);
  const chunks = [];
  for await (const chunk of req) chunks.push(chunk);
  let body;
  try { body = JSON.parse(Buffer.concat(chunks).toString('utf8')); }
  catch { invalidRequest = true; res.writeHead(400).end(); return; }
  requestCount += 1;
  if (body.model !== 'mock-model' || !Array.isArray(body.messages) || body.stream === true) invalidRequest = true;
  const serialized = JSON.stringify(body.messages);
  const isEditor = serialized.includes('Return exactly one sic_edit_blocks call');
  const isChildTask = serialized.includes('SIC_PEER_CHILD_TASK_SYNTHETIC');
  const hasFixtureResult = body.messages.some(message => message?.role === 'tool'
    && typeof message.content === 'string' && message.content.includes('SIC_RUST_SYNTHETIC_FIXTURE_ONLY'));
  const toolNames = (body.tools ?? []).map(tool => tool.function?.name).filter(name => typeof name === 'string').sort();
  let message;
  let responseKind;
  if (isEditor) {
    responseKind = requestCount === 1 ? 'requester_editor' : 'peer_editor';
    message = { role: 'assistant', content: null, tool_calls: [{ id: `peer-editor-${requestCount}`, type: 'function', function: { name: 'sic_edit_blocks', arguments: JSON.stringify({ ops: [], pvi: { P: [] } }) } }] };
  } else if (isChildTask && !hasFixtureResult && !childFixtureIssued) {
    childFixtureIssued = true;
    responseKind = 'peer_fixture_tool_call';
    message = { role: 'assistant', content: null, tool_calls: [{ id: 'peer-child-fixture-call', type: 'function', function: { name: 'sic_tool_call', arguments: JSON.stringify({ name: 'read', arguments: { file_path: 'SIC_TOOL_FIXTURE.txt', offset: 1, limit: 10 } }) } }] };
  } else if (isChildTask && hasFixtureResult) {
    responseKind = 'peer_final_text_after_fixture';
    message = { role: 'assistant', content: 'SIC_RUST_DSH_MOCK_ROUNDTRIP_OK' };
  } else {
    responseKind = 'requester_bootstrap_final_text';
    message = { role: 'assistant', content: 'SIC_PARENT_BOOTSTRAP_READY' };
  }
  const functionNames = message.tool_calls?.map(call => call.function.name) ?? [];
  calls.push({ callIndex: requestCount, responseKind, toolNames, functionNames, maxTokens: body.max_tokens ?? null, fixtureResultObserved: hasFixtureResult });
  res.writeHead(200, { 'content-type': 'application/json', 'cache-control': 'no-store' });
  res.end(JSON.stringify({ model: 'mock-model', choices: [{ index: 0, finish_reason: functionNames.length ? 'tool_calls' : 'stop', message }], usage: { prompt_tokens: 10, completion_tokens: 5 } }));
});
server.listen(0, '127.0.0.1', () => process.stdout.write(JSON.stringify({ ready: true, port: server.address().port }) + '\n'));
