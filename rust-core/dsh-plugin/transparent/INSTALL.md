# Transparent SDK integration: primary entry

Bind a fresh native session before its first user/assistant/tool history. The
binding command reserves an exact session/chain relationship; `--create` creates
an empty Core chain and does **not** import existing native history. Its output
reports `history_admission:'fresh-only'` and `migration_performed:false`.
The actual plugin mount verifies enrollment before taking over lifecycle ports.
Legitimate native system/init/plugin initialization is allowed. Prior native
turns, steps, model attempts, request headers, inherited history and compaction
records require a previously admitted boundary. Historical migration is a separate, currently unconnected
operation; `--migrate` is explicitly rejected. A region or `/compact` request
never triggers migration or rewrites the original archive to simulate it.

First enrollment fsyncs an independent receipt under
`projectionRoot/session-admissions/` and a matching native
`plugin:sic/enable-boundary` record. Restore the receipt directory with the
native JSONL, bindings and Core state. Cold reopening checks the immutable
native prefix, host actor/endpoint, exact session/chain/scope/role and actual
Core chain access. Trusted host code may also enroll a separately scoped
Editor Agent. A marker copied into a native archive alone grants no enrollment.
Once enrolled, removing or replacing the binding blocks that Agent and a cold
reopen under SIC. Restore its exact binding and recreate the Agent with the
reviewed composition.

Earlier SIC versions did not write this enrollment receipt. An older SIC
archive with execution history needs separately verified migration or recovery.
This release has no automatic upgrade path for that archive.

Unselected sessions, and a newly started Harness that does not load SIC at all,
retain native behavior including positional region compaction. This plugin
cannot enforce a guard in a completely different process/composition where it
is absent. Restore an enrolled SIC archive with its matching enabled
composition; switching that archive to another composition is an explicit
host operation, not an implicit effect of deleting a binding.

Install `transparent/` as **sic-transparent-host**. Its default `cordis-entry.mjs` accepts JSON/YAML configuration; `plugin.mjs` is the lower-level JavaScript composition factory. Reviewed native/append-only/fused prompts and original schemas ship in `bundle/`; runtime does not read evidence files. Existing running profiles are not migrated.

The default `transport:'sdk'` edits the original Harness **GenerateOptions/RequestMessage** protocol before its original Provider adapter. No API format or Provider alias is required. Registration, prepared calls, model/options, serializers, retries, choices, admission and stream parsing remain with the Harness.

Apply and rebuild `harness-sdk-context.patch`, `goal-external-state.patch` and `jobs-external-lifecycle.patch`. If the Harness has native compaction, also apply `compaction-external-state.patch`. LlmRuntime must report `contextDispatchVersion===1`; GoalService/its sole driver and JobRegistry/ToolJobs require their native port handshakes. Missing patches fail explicitly, instead of silently omitting SIC. Core requires the real `dsh-messages` adapter, preview/I projection and host-mode recursive scheduler from this source revision.

```json
{
  "enabled": true,
  "mode": "edit",
  "actor": "your-existing-Core-HostAuth-actor",
  "coreURL": "http://127.0.0.1:18890",
  "credentialEnv": "SIC_CORE_HOST_TOKEN",
  "projectionRoot": "D:/your-durable-host-state/projections",
  "bindingsFile": "D:/your-durable-host-state/native-bindings.json"
}
```

The named environment reference is the **Core HostAuth credential**, resolved only in the host process. Provider credentials remain in the existing Harness Provider configuration and never enter Core context payloads. Save this configuration as a JSON file for the binding command, then apply the same fields to the Cordis plugin configuration:

```powershell
node <plugin-directory>/bind-session.mjs --config <host-config.json> --session <actual-native-session-id> --create
# Or bind an already owned Core chain instead of creating one:
node <plugin-directory>/bind-session.mjs --config <host-config.json> --session <actual-native-session-id> --chain <Core-chain-id>
```

The command explicitly creates/binds only Core context state and writes the exact native session/chain/scope mapping to `bindingsFile`; it does not create DSH Agents, change profiles or choose a model. The plugin validates actor/Core identity and chain access on startup and loads this mapping after a restart. Use actual native session identities; install before a selected Agent starts. To change a frozen binding, stop/recreate that Agent and reload a reviewed composition. Unknown sessions remain unselected. A failed/uncertain `chain.create` leaves a creation intent for reconciliation, rather than blindly creating a second chain. A stale `.lock` requires operator verification that no binding command is active before removing it; inspect `chain.list` and bind its known chain after an uncertain creation. Binding-store credentials are references, never token values.

For advanced host code, call `await import('./plugin.mjs').then(module => module.apply(ctx, trustedOptions))` directly and retain its controller. `sic-core-rust-dsh/transparent/plugin` and `sic-transparent-host/plugin` expose this same factory: do not pass them to `ctx.plugin()`, because the returned controller is not a Cordis disposal effect. Use the default package entry (or `sic-core-rust-dsh/transparent`) for Cordis loading; that wrapper performs the real lifecycle composition and publishes its controller as `ctx.sicTransparent`.

Required host services are the actual `llm`, `systemPrompt`, `tools`, `agents`, `sessions`, `sessionProjections`, `connection`, `goals` and `jobs` services with the source-patch handshakes above (including native executor-join support in current Jobs). A persistence plugin must make `sessions.flush()` return true after a real durability listener. The final native prompt must match the reviewed `bundle/native.md` composition: an unknown complete prompt is rejected and must first receive a reviewed fusion/bundle update. The bootstrap does not overwrite an unknown prompt to manufacture a match. Optional native compaction needs its patch when present. Neither this package nor its bootstrap patches an installed application automatically.

SDK mode preserves the original Harness model/Provider admission and call chain. Optional `admitFinal` is available only through the advanced factory as an existing native observation point. Default Editor uses actual `llm.prepareCall`, original route/model/controls and BlockAssembler. Explicit `mode:'passthrough'` is readonly, never fake editing.

Core current selected CBs and immutable recursive source identities determine the shared SDK prefix, including higher-order CBs and tails. Unknown legacy origin mappings require migration. Worker/Editor share public system/tools/CBs; mode/environment/exact readonly I follow them. New text anchors are assigned explicitly; IDs, tool-call arguments, signed reasoning/replay, attachments and control metadata remain protected.

After publication, the same Worker request adopts canonical new CB/I layout. `candidate_request`, actual `adopted` and `layout_receipt` are distinct. Actual SDK messages pass to the original adapter. Final guards and persisted delivery/Session PluginRecord+flush acknowledgements protect retry and adoption correlation.

To enable the optional native image compression path, set `"imageCompression": true` in SDK edit mode. Provide the existing DSH attachment service and Sharp through the native loader. See [IMAGE-COMPRESSION.md](IMAGE-COMPRESSION.md) for display choices, the I receipt, and archive recovery.

The host Editor defaults to at most 16 tool continuation rounds and a
120-second deadline per assignment, including its original native model
calls. Set `editorLimits.maxToolRounds` and `editorLimits.timeoutMs` to
change them. A limit aborts the attempt before publishing a partial batch.
The model's original request options and Worker limits remain host owned.

The entry composes `createHostHierarchyRuntime` automatically. After low-level publication it drains Core's persistent higher-level jobs through the original Harness Editor, then reads the final selected cut and refreshes I before dispatching that same Worker. Rust retains grouping, thresholds, levels, publication and automatic adoption. `hierarchyPolicy` can supply actual Core policy overrides; `drainHigher` remains an optional Harness extension point. Higher-level V is enabled only when actual Core get/claim capabilities advertise `core-same-level-source-group-v1`. The Core binds the earliest pending V to the next same-level WB with fresh lower CBs (or an explicit flush), preserving original sources and provenance. Waiting for new lower material does not deadlock subsequent Worker input. This is separate from the main-level-1 native continuation helper.

Capabilities intersect actual Core candidate and host binding. Real `native.preview` supplies mapped edits; older candidates retain an explicit legacy subset. Default SDK mode consumes the earliest actual main-level-1 pending V sources into separate native SDK messages and bindings on the next WB; it does not reappend consumed V. Native-wire compatibility defers V.

Primary entry installs the single Goal authority port/facade and actual native Jobs intent/claim/producer/settle/recovery ports. Native job schema/rendering/wait clamping remain unchanged. See `GOAL-RUNTIME.md` and `JOBS-RUNTIME.md`.

In edit mode, an existing or later-loaded native compaction service installs the external port automatically. Basic pressure, context overflow and manual `/compact` share the same Core host scheduler; selected sessions do not run a second native summary/pruner policy. A Harness without compaction needs no extra service or configuration. Passthrough preserves its existing compaction behavior. See `COMPACTION-RUNTIME.md`; selected positional region compaction currently rejects requests without a trusted CB-range mapping.

Editor paging defaults to assigned source/units. Extra reads need `editor_readable_sources:[{kind:'native-source',source_id,unit_ids}]`. Independent Editor uses fresh `editor_scope`; Worker needs explicit typed `readable_sources`. Core archive IDs are not native-source IDs. Native read retains original Editor-agent tool guards.

Editor input contract `authenticated-wb-origins-v1` links assigned native tool-result leaves to their original immutable SDK role/call/status through Core's host-message origin map. Higher CB units carry the same authenticated retained/derived tool relationships from the shared source view. These facts describe earlier execution, distinct from what this Editor is currently authorized to execute. The Editor preserves that distinction in I without discarding Board/Host constraints or changing original tool/error/signature data. Unknown or user-claimed tool labels do not become execution receipts.

Editor trace is per-source append JSONL with boundary fsync and paged UI. Layout receipts include higher-level execution; manual hierarchy receipts use their own authorized trace root in the same browser API. Provider SSE is not buffered. Optional `deepseek-native-fetch.patch` adds raw proof only; explicit `transport:'native-wire'` selects that compatibility path. Unloading an installation cancels its selected active Agents and leaves Agent-owned dispatch/tool guards in place; recreate frozen Agents under a reviewed composition instead of silently falling back to native execution. Core control RPCs, including bodies, have a 30-second deadline without automatic mutation retries; Provider timeouts remain with the Harness. The old package-root Chat gateway remains separately named `sic-core-rust-dsh/legacy`.

Fixtures use actual isolated LlmRuntime/prepareCall/adapterStream and local model stubs. They do not claim external Provider/cache experiments. Installed-source deployment and UI visual validation are separate; these files change no running service/profile/Agent/DB.
