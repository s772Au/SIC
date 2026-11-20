import { createHash } from "node:crypto";
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import type { Message } from "@earendil-works/pi-ai";

// Test bridge binds to an isolated fixture. No production Core credentials enter Pi.
export default function toolWaitFixture(pi: ExtensionAPI) {
  const endpoint = process.env.SIC_TEST_PI_BRIDGE;
  const session = process.env.SIC_TEST_PI_SESSION;
  if (!endpoint || !/^http:\/\/127\.0\.0\.1:[0-9]+$/.test(endpoint) || !session) {
    throw new Error("The Pi tool-wait test requires an isolated bridge and session");
  }
  let workerFrame: Message[] | undefined;
  let publication: Promise<{ original: Message; edited: Message }> | undefined;
  let adopted = false;

  async function post(path: string, body: unknown, signal?: AbortSignal) {
    const response = await fetch(endpoint + path, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
      signal,
    });
    if (!response.ok) throw new Error(`Fixture ${path}: ${response.status} ${await response.text()}`);
    return response.json();
  }

  async function edit(ctx: ExtensionContext, entry: { id: string; message: Message }, sequence: number,
                      frame: Message[]) {
    const model = ctx.model;
    const assistant = entry.message;
    if (!model || assistant.role !== "assistant" || assistant.stopReason !== "toolUse") {
      throw new Error("Expected a finalized Pi tool-calling response on an active route");
    }
    const block = assistant.content.findIndex(part => part.type === "text" && part.text.trim());
    const leaf = assistant.content[block];
    if (leaf?.type !== "text") throw new Error("The test Worker must produce formed progress");
    const request = { provider: model.provider, model: model.id, messages: [...frame, assistant] };
    const anchor = ["messages", frame.length, "content", block, "text"];
    const digest = createHash("sha256").update(JSON.stringify(request)).digest("hex");
    const callIds = assistant.content.filter(part => part.type === "toolCall").map(part => part.id);
    const binding = await post("/freeze", {
      format: "pi-messages",
      request,
      native_step_id: digest,
      idempotency_key: digest,
      editable_anchors: [anchor],
      host_message_origins: [{ message_id: entry.id, block_index: block, native_anchor: anchor }],
      tool_wait: { assistant_message_id: entry.id, assistant_event_seq: sequence, call_ids: callIds },
    }, ctx.signal);
    const result = await ctx.modelRegistry.streamSimple(model, { messages: [...frame, {
      role: "user", timestamp: Date.now(),
      content: JSON.stringify({ role: "SIC_EDITOR_TOOL_WAIT_FIXTURE", scope: "only the frozen Worker progress",
        operation: "COMPRESS", original: leaf.text, response: { replacement: "text" } }),
    }] }, { signal: ctx.signal }).result();
    if (result.stopReason !== "stop" || result.content.length !== 1 || result.content[0]?.type !== "text") {
      throw new Error(`Unexpected test Editor response ${result.stopReason}: ${result.errorMessage ?? ""}`);
    }
    const { replacement } = JSON.parse(result.content[0].text);
    if (typeof replacement !== "string" || !replacement || replacement === leaf.text) {
      throw new Error("Expected one changed text leaf");
    }
    const published = await post("/publish", { source_id: binding.source_id,
      expected_source_digest: binding.source_digest, original: leaf.text, replacement }, ctx.signal);
    return { original: assistant, edited: published.request.messages[frame.length] };
  }

  pi.on("context_with_system", async (event, ctx) => {
    if (ctx.sessionManager.getSessionId() !== session) return;
    try {
      if (!publication) {
        workerFrame = structuredClone(event.messages) as Message[];
        return;
      }
      if (adopted) throw new Error("The isolated test expects exactly one Worker continuation");
      const { original, edited } = await publication;
      const matches = event.messages.map((message, index) =>
        JSON.stringify(message) === JSON.stringify(original) ? index : -1).filter(index => index >= 0);
      if (matches.length !== 1) throw new Error("Frozen original missing or duplicated in Worker continuation");
      const messages = event.messages.slice();
      messages[matches[0]] = edited;
      adopted = true;
      return { messages };
    } catch (error) {
      ctx.abort();
      throw error;
    }
  });

  pi.on("tool_call", (event, ctx) => {
    if (ctx.sessionManager.getSessionId() !== session || publication || !workerFrame) return;
    const branch = ctx.sessionManager.getBranch();
    const index = branch.findLastIndex(entry => entry.type === "message" && entry.message.role === "assistant"
      && entry.message.content.some(part => part.type === "toolCall" && part.id === event.toolCallId));
    const entry = branch[index];
    if (entry?.type !== "message") return { block: true, reason: "No durable Worker assistant for the fixture" };
    publication = edit(ctx, entry, index + 1, workerFrame);
    // The continuation hook owns the join. Keep a rejection handled until it runs.
    void publication.catch(() => {});
  });

  pi.on("tool_result", async (event, ctx) => {
    if (ctx.sessionManager.getSessionId() === session && event.toolName === "bash") {
      await post("/tool-finished", { isError: event.isError, callId: event.toolCallId }, ctx.signal);
    }
  });
}
