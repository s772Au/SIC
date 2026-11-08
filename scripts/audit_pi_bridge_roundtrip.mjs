/** Offline evidence generator; reads installed pi-ai, never edits DSH or uses network. */
import { pathToFileURL } from "node:url";
import { readFile, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import path from "node:path";

const [piRoot, output, dshRoot] = process.argv.slice(2);
if (!piRoot || !output) throw new Error("Usage: node audit_pi_bridge_roundtrip.mjs PATH_TO_PI_AI OUTPUT.json");
const source = path.join(piRoot, "dist/api/openai-completions.js");
const { stream, convertMessages } = await import(pathToFileURL(source));
const pkg = JSON.parse(await readFile(path.join(piRoot, "package.json"), "utf8"));
const baseModel = { id: "mock", name: "mock", provider: "openai", api: "openai-completions",
  baseUrl: "http://127.0.0.1:9/v1", reasoning: false, input: ["text"],
  cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }, contextWindow: 10000, maxTokens: 1000 };
const samples = [
  { name: "empty-metadata", assistant: { role: "assistant", content: "hello", refusal: null, annotations: [], tool_calls: null }, accept: true },
  { name: "empty-tool-array", assistant: { role: "assistant", content: "hello", tool_calls: [] }, accept: true },
  { name: "tool-json-reformat", assistant: { role: "assistant", content: null, refusal: null, annotations: [],
      tool_calls: [{ id: "call_test", type: "function", function: { name: "demo", arguments: '{ "b" : 2, "a": 1.0 }' } }] }, accept: true },
  { name: "tool-empty-content", assistant: { role: "assistant", content: null,
      tool_calls: [{ id: "call_test", type: "function", function: { name: "demo", arguments: "{}" } }] },
    compat: { requiresAssistantAfterToolResult: true }, accept: true },
  { name: "inserted-empty-reasoning", assistant: { role: "assistant", content: "hello" },
    reasoning: true, compat: { requiresReasoningContentOnAssistantMessages: true }, accept: true },
  { name: "lost-nonempty-reasoning", assistant: { role: "assistant", content: "hello", reasoning_content: "meaningful reasoning" }, accept: false },
  { name: "lost-nonempty-annotation", assistant: { role: "assistant", content: "hello", annotations: [{ type: "url_citation", url: "https://example.invalid/evidence" }] }, accept: false },
  { name: "unsafe-integer-rounding", assistant: { role: "assistant", content: null,
      tool_calls: [{ id: "call_test", type: "function", function: { name: "demo", arguments: '{"count":9007199254740993}' } }] }, accept: false },
];
const results = [];
for (const sample of samples) {
  const model = { ...baseModel, reasoning: sample.reasoning ?? false };
  // This is the exact field whitelist emitted by the v0.1 Chat bridge SSE.
  const delta = Object.fromEntries(Object.entries(sample.assistant).filter(([key]) => ["role", "content", "tool_calls", "refusal"].includes(key)));
  if (delta.tool_calls) delta.tool_calls = delta.tool_calls.map((call, index) => ({ ...call, index }));
  const reason = delta.tool_calls?.length ? "tool_calls" : "stop";
  const chunks = [{ id: "audit", choices: [{ index: 0, delta, finish_reason: null }] },
    { id: "audit", choices: [{ index: 0, delta: {}, finish_reason: reason }] }];
  const sse = chunks.map(chunk => "data: " + JSON.stringify(chunk) + "\n\n").join("") + "data: [DONE]\n\n";
  let parsed;
  let fetchCalls = 0;
  for await (const event of stream(model, { messages: [{ role: "user", content: "hi", timestamp: 0 }] }, {
    apiKey: "local-audit-placeholder", fetch: async () => {
      fetchCalls++;
      return new Response(sse, { status: 200, headers: { "Content-Type": "text/event-stream" } });
    },
  })) {
    if (event.type === "error") throw new Error(event.error.errorMessage);
    if (event.type === "done") parsed = event.message;
  }
  if (!parsed || fetchCalls !== 1) throw new Error("Unexpected local pi-ai stream completion");
  const toolResults = (parsed.content ?? []).filter(block => block.type === "toolCall").map(block => ({
    role: "toolResult", toolCallId: block.id, toolName: block.name,
    content: [{ type: "text", text: "local tool result" }], isError: false, timestamp: 0,
  }));
  const converted = convertMessages(model, { messages: [parsed, ...toolResults, { role: "user", content: "next", timestamp: 0 }] }, {
    supportsMidConvoSystemMessages: true, supportsDeveloperRole: false, requiresAssistantAfterToolResult: false, ...sample.compat,
  });
  results.push({ name: sample.name, upstream_assistant: sample.assistant,
    pi_replayed_assistant: converted[0], expected_compatible: sample.accept });
}
const evidence = { schema: "sic-pi-replay-audit-v1", package: pkg.name, version: pkg.version,
  source_file: "dist/api/openai-completions.js", source_sha256: createHash("sha256").update(await readFile(source)).digest("hex"),
  network_requests: 0, mechanism: "actual installed stream() with injected local fetch, followed by convertMessages(); bridge SSE field whitelist reproduced", samples: results };
if (dshRoot) {
  evidence.dsh_source_inspection = {};
  for (const relative of ["packages/llm/llm-pi-ai/src/adapter.ts", "packages/llm/llm-pi-ai/src/stream.ts", "packages/llm/llm-pi-ai/src/replay.ts"]) {
    evidence.dsh_source_inspection[relative] = createHash("sha256").update(await readFile(path.join(dshRoot, relative))).digest("hex");
  }
}
evidence.dsh_host_executed = false;
await writeFile(output, JSON.stringify(evidence, null, 2) + "\n");
console.log(`Wrote ${results.length} actual pi-ai replay fixtures; network requests: 0`);
