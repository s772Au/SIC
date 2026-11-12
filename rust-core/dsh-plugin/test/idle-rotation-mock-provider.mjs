import http from 'node:http';

// Local, synthetic DSH integration fixture. It never contacts a real Provider.
let workerCount = 0;
let invalid = false;
let invalidChoiceCount = 0;
const injectInvalidChoice = process.env.SIC_INJECT_INVALID_CHOICE === '1';
const cycle = Number(process.env.SIC_ROTATION_CYCLE ?? 8);
if (!Number.isSafeInteger(cycle) || cycle < 2 || cycle > 32) process.exit(2);
const calls = [];
const server = http.createServer(async (req, res) => {
  if (req.method === 'GET' && req.url === '/status') {
    res.writeHead(200, { 'content-type': 'application/json', 'cache-control': 'no-store' });
    res.end(JSON.stringify({ invalid, invalidChoiceCount, calls }));
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
  catch { invalid = true; res.writeHead(400).end(); return; }
  if (req.headers.authorization !== `Bearer ${process.env.SIC_MOCK_PROVIDER_KEY}` ||
      body.model !== 'mock-model' || body.stream === true || !Array.isArray(body.messages)) {
    invalid = true; res.writeHead(400).end(); return;
  }
  const editor = JSON.stringify(body.messages).includes('Return exactly one sic_edit_blocks call');
  let message;
  if (!editor && injectInvalidChoice && invalidChoiceCount === 0) {
    invalidChoiceCount++;
    res.writeHead(200, { 'content-type': 'application/json', 'cache-control': 'no-store' });
    res.end(JSON.stringify({ model: 'mock-model', usage: { prompt_tokens: 10, completion_tokens: 5 }, choices: [
      { index: 0, message: { role: 'assistant', content: 'synthetic ambiguous candidate' } },
      { index: 1, finish_reason: 'tool_calls', message: { role: 'assistant', tool_calls: [
        { id: 'ambiguous-fixture-call', type: 'function', function: { name: 'sic_tool_call',
          arguments: JSON.stringify({ name: 'sic_fixture_read', arguments: {} }) } },
      ] } },
    ] }));
    return;
  }
  if (editor) {
    message = { role: 'assistant', content: null, tool_calls: [{ id: 'rotation-editor', type: 'function',
      function: { name: 'sic_edit_blocks', arguments: JSON.stringify({ ops: [], pvi: { P: [] } }) } }] };
  } else {
    workerCount++;
    const directory = body.messages.flatMap(m => {
      if (typeof m?.content !== 'string') return [];
      try { return JSON.parse(m.content)?.sic_editable_I_views?.dynamic_tool_directory ?? []; }
      catch { return []; }
    });
    calls.push({ worker: workerCount, fixtureExpanded: directory.some(t => t?.name === 'sic_fixture_read') });
    if (workerCount % cycle === 0) {
      message = { role: 'assistant', content: 'SIC_ROTATION_ROUND_OK' };
    } else {
      const expand = workerCount === 1;
      const name = expand ? 'sic_tool_expand' : 'sic_tool_search';
      const args = expand ? { name: 'sic_fixture_read' } : { query: 'fixture' };
      message = { role: 'assistant', content: null, tool_calls: [{ id: `rotation-${workerCount}`, type: 'function',
        function: { name, arguments: JSON.stringify(args) } }] };
    }
  }
  const finish = message.tool_calls ? 'tool_calls' : 'stop';
  res.writeHead(200, { 'content-type': 'application/json', 'cache-control': 'no-store' });
  res.end(JSON.stringify({ model: 'mock-model', choices: [{ index: 0, finish_reason: finish, message }],
    usage: { prompt_tokens: 10, completion_tokens: 5 } }));
});
server.listen(0, '127.0.0.1', () => process.stdout.write(JSON.stringify({ port: server.address().port }) + '\n'));
