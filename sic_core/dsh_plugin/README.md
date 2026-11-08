# DSH route for the SIC core

This plugin connects a real DSH Agent to SIC's full-history Chat Completions bridge. It uses DSH's published PiAi adapter and the native SDK for protocol conversion. It does not replace the Harness Agent loop. The bridge and the operator API must use the **same Core instance and database** so the official Langflow/LangGraph workspaces observe the actual DSH chain.

Tested host: DSH `0.2.1-rc.5`. The package declares the compatible pre-0.3 host range; other releases require a real Harness integration check.

## Isolated installation

Use a new DSH home and profile first. Keep the normal desktop profile unchanged. In the plugin directory run `pnpm install --frozen-lockfile --ignore-scripts`, then use the public DSH CLI:

```powershell
$env:DSH_HOME = '<private isolated directory>'
dsh --profile sic-core-headless --from-default-profile headless --dump-config
dsh plugin --profile sic-core-headless add '<absolute path to this directory>'
```

Set `SIC_CORE_TOKEN` to the bridge's locally configured Bearer credential. Do not put a credential in a profile, a flow, a prompt, or a shared archive. The bridge address must be loopback, and the bridge explicitly authorizes model work. The plugin never discovers or borrows another provider login.

The bundled patch supplies a local fixture route named `sic-core/sic-worker`. **Its four zero prices are fixture values**, not real provider prices. For configured providers, explicitly replace input/output/cache-read/cache-write prices and model capacity in a private DSH patch. Worker prices shown by DSH describe this Worker route; the Core ledger records the separate Editor route and usage. Do not sum DSH totals into the Core ledger again.

Disable DSH's independent automatic compaction and result-pruning plugins for this isolated profile when testing SIC's ownership of context editing. Disable unrelated provider/title/session-telemetry integrations if the fixture must remain entirely offline. Use DSH's normal profile patch mechanism, not edits to the installed application.

## Behavior

- The adapter passes DSH's real session identity as bridge affinity. Independent sessions receive independent bindings even when their messages are identical.
- The bridge verifies the incoming full history, persists input cursors and delivery receipts, and rejects changed historical input. A transport retry does not authorize replay of an uncertain provider call.
- Before exposing model output, the adapter appends the actual Core provider projection and its references to the DSH session log. These projections can differ from the Harness's unedited history.
- A DSH `tools/execute` around-hook reports the actual root tool dispatch and completion, independently of a model proposing a tool call. The shared Core runtime can edit during this real interval.
- Tool-completion events use a durable session-log outbox. Delivery failures preserve the real tool result; unresolved events must be flushed before the next model dispatch.
- SDK and host model retries are disabled for this route. Explicit recovery belongs to the bridge's persisted recovery operation.

`fixtureWaitTool` defaults to false. An isolated integration fixture can enable it and set `fixtureWaitMs`; it registers only a synthetic wait tool for measuring a real tool interval. It is not an application capability or an LLM evaluation.

## Historical peers executing host tools

`peerTools` is an explicit list of native tools already visible to the parent. It defaults to empty. The Core deployment must separately enable `bridge.host_tools_enabled` and allow those names in `bridge.host_tool_allowlist`. Neither a checkpoint nor historical text can authorize a tool. `peerConcurrency` defaults to 2 (maximum 4); `peerMaxDepth` defaults to 4.

The plugin uses the installed host's public subagent helpers and Agent manager to create an isolated native execution carrier with the parent's delegated policies and working directory, filtered to the approved tools. SIC identity remains the peer's CB chain; the native carrier's parent metadata is a host security relationship. Tools execute through the normal DSH runtime, not a shell substitute or a plugin-owned Agent loop.

Reservation is distinct from actual dispatch. Only the native execution hook records `tool_start`. Native results are written to the peer log and the parent's durable outbox before completion delivery. Delivery retries resend the same result, never repeat the tool. Cancellation, expired attempts and uncertain transport outcomes do not automatically reissue execution; unresolved receipts remain visible for explicit recovery. Registration generations replace the same logical host slot atomically. Old pumps cannot renew or deliver; native shutdown aborts active carriers, closes registration and drains. Previously claimed unknown work is never moved to the new generation. Current deployment and route authority are checked again at dispatch and completion.

The isolated fixture runner supports `-Peer`. This additionally enables `fixturePeerTool`, which asks a historical SIC peer to call the real native wait tool and returns its result to the parent. Both fixture tools are disabled by default. See [read-only verification](../../docs/dsh-harness-verification.md).

## Current protocol scope

The bridge currently accepts text Chat Completions with complete, closed tool-call groups. SSE is a **buffered response**, not incremental token streaming. Images, arbitrary request extensions, and `store:true` are rejected rather than silently discarded. This plugin adds one tested DSH route; it does not automatically migrate existing DSH provider profiles or promise support for every API dialect.
