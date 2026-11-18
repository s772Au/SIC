# Native compaction composition

Apply `compaction-external-state.patch` and rebuild the reviewed Harness. It
changes the original compaction service/Basic backend, pruner, `/compact`
consumer and ReactLoopAgent. It neither installs a second compactor nor changes
image-offload. Selected sessions retain their complete native message archive.
Unselected sessions retain native summary, pruning, pressure and manual paths.
Their positional region API also retains its original execution and result.
The selected positional-region rejection is a compatibility boundary, not a
Core defect or an invitation to import the session's whole history.

```js
const compactionRuntime = await installCoreCompactionRuntime(ctx, {
  bindingFor,
  schedule: existingHostRecursiveScheduler,
  appendRecord: sessionAPI.appendPluginRecord,
});
```

The host must provide actual `compaction`, `agents`, `sessions` and optional
`toolResultPruner` services. Missing Basic/pruner source handshakes fail
explicitly. A pruner loaded later receives the same selected-session guard.
The compaction and pruner source handshakes are version 2. Rebuild the source
patch and Node module together; older version 1 patches are rejected. Once an
Agent/session has selected SIC, removing or replacing its chain/scope binding
fails closed. Native source guards retain that selection across runtime unload,
including an Agent that has not yet requested compaction. A late scheduler
result cannot acknowledge a binding that changed while it was running.
Native selected `pruneSession()` keeps its synchronous interface and reports
zero native replacements; it does not launch another task. Its enclosing
Basic pressure/overflow entry reaches the one external callback first.

The callback receives `{agent,binding,request_id,trigger,signal,source_event_seq,
resolveCall,sourceCommandId?}`. Triggers are `pressure`, `context-overflow` and
`manual`. The request ID binds the real native session, latest non-plugin
event, trigger and optional human command ID. `resolveCall()` invokes the
agent's new public `resolveContextEditCall(signal)`: it reuses the original
`agent/request` waterfall and `llm.prepareCall`, returning
`{config,preparedCall?,options}` with actual archive messages, tool history,
tools and signal. It appends no request header/context and dispatches no Worker.
Use already available SDK options at an SDK entry point; no Provider/profile
is inferred or configured here.

The callback must await the existing SIC recursive driver and return an actual
durable `{recorded:true,receipt_id,progressed:boolean,publication_ids?:string[],
text?:string}` receipt. This port starts no model call and implements no token
threshold, reduction loop or higher-order policy. Only actual external progress
permits native overflow retry, bounded by the Harness's existing overflow cap.
No-progress/error preserves the native request failure. A pressure callback
failure prevents Worker admission instead of falling back to native summary.

Automatic/manual methods preserve their names and accept an additive
`CompactionOutcome` union. Native `CompactionResult` is unchanged; an external
outcome contains `{external:{recorded,request_id,receipt_id,progressed,
publication_ids},text}`. `/compact` renders that real receipt without invented
native summary seqs or replacement counts. Manual compaction retains native
idle maintenance, FIFO wake-up, cancellation and session flush. Receipt records
are appended using the actual native PluginRecord API and flushed before
returning to native admission.

The positional `compactRegion()` API retains its original result type. A
selected positional region is rejected before any scheduler dispatch because
there is no trusted mapping from those surface positions to complete SIC CB
ranges. The rejection explicitly states that the archive is preserved; it
never substitutes global compaction for a region request.

The main plugin separately verifies fresh-session enrollment before installing
its lifecycle modules. A native history binding or local compaction request is
not a migration operation. Existing history without a matching host-persisted
SIC enable receipt is rejected; explicit historical migration is not implemented.
See `INSTALL.md` for enrollment, cold-restore and completely disabled-host
composition boundaries. The low-level compaction port assumes its caller has
performed that enrollment check; its fixtures separately verify port behavior.

Run:

```powershell
node rust-core/dsh-plugin/test/check-compaction-source.mjs
node --test rust-core/dsh-plugin/test/transparent-compaction-runtime.spec.mjs
python rust-core/dsh-plugin/transparent/build-compaction-patch.py
```

The fixture uses real patched source, AgentLoop, Tools, BasicCompaction,
pruner, Commands, LlmRuntime/prepareCall, native adapter errors and original
image-offload. Its scheduler is an explicit host receipt spy: it proves the
native port contract and absence of legacy replacements, not the final Core
recursive-driver/main-plugin composition, which requires host integration.
