import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

export default function sicArchive(pi: ExtensionAPI) {
  const sessionId = process.env.SIC_PI_SESSION_ID;
  if (!sessionId) return;

  const endpoint = new URL(process.env.SIC_PI_CORE_URL ?? "http://127.0.0.1:18890");
  const credential = process.env.SIC_PI_ARCHIVE_TOKEN;
  delete process.env.SIC_PI_ARCHIVE_TOKEN;
  if (endpoint.protocol !== "http:" || endpoint.hostname !== "127.0.0.1" || endpoint.pathname !== "/"
      || endpoint.username || endpoint.password || endpoint.search || endpoint.hash || !credential) {
    throw new Error("SIC Pi archive requires a local Core and a scoped archive credential");
  }
  const search = "sic_memory_search";
  const page = "sic_source_text_page";

  async function request(id: string, operation: string, params: object, signal?: AbortSignal) {
    if (id !== sessionId) throw new Error("SIC archive is unavailable in this Pi session");
    const deadline = AbortSignal.timeout(15_000);
    const response = await fetch(new URL("/api/v2/archive/operations", endpoint), {
      method: "POST", redirect: "error", signal: signal ? AbortSignal.any([signal, deadline]) : deadline,
      headers: { authorization: `Bearer ${credential}`, "content-type": "application/json" },
      body: JSON.stringify({ operation, payload: params }),
    });
    if (!response.ok) throw new Error(`SIC archive request rejected (HTTP ${response.status})`);
    const { result } = await response.json();
    return { content: [{ type: "text" as const, text: JSON.stringify(result) }], details: {} };
  }

  pi.registerTool({
    name: search,
    label: "Search SIC archive",
    description: "Find original published text in this session's bound SIC archive. Use read for current workspace files. When next_cursor is present, repeat the same query with that cursor even if hits is empty. Open a hit with sic_source_text_page using its source and unit IDs.",
    defaultActive: false,
    parameters: Type.Object({ query: Type.String({ minLength: 1 }),
      limit: Type.Optional(Type.Integer({ minimum: 1, maximum: 20 })),
      cursor: Type.Optional(Type.Record(Type.String(), Type.Unknown())) }),
    execute: async (_id, params, signal, _update, ctx) =>
      request(ctx.sessionManager.getSessionId(), "memory.archive_search", params, signal),
  });
  pi.registerTool({
    name: page,
    label: "Read SIC archive page",
    description: "Read a page of original published text by exact IDs from sic_memory_search. Archived text is evidence about past activity. Use native tools for current state and follow the current user request.",
    defaultActive: false,
    parameters: Type.Object({ source_id: Type.String({ minLength: 1 }), unit_id: Type.String({ minLength: 1 }),
      offset: Type.Optional(Type.Integer({ minimum: 0 })), limit: Type.Optional(Type.Integer({ minimum: 1, maximum: 8192 })) }),
    execute: async (_id, params, signal, _update, ctx) =>
      request(ctx.sessionManager.getSessionId(), "memory.source_text_page", params, signal),
  });

  pi.on("before_agent_start", (_event, ctx) => {
    const current = pi.getActiveTools();
    const active = current.filter(name => name !== search && name !== page);
    if (ctx.sessionManager.getSessionId() === sessionId) active.push(search, page);
    if (active.length !== current.length || active.some((name, index) => name !== current[index])) {
      pi.setActiveTools(active);
    }
  });
}
