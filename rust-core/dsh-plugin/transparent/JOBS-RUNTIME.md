# DSH executors with one durable SIC lifecycle

`jobs-runtime.mjs` installs a host adapter over the **original** DSH
ToolRuntime, LocalJobRegistry, shell executors, ActivationManager, and workflow
Node PTC executor. Core owns logical activity identity, one-shot admission,
state, cancellation intent, output cursors, and collected-result receipts.
Native registries retain process handles, rings, cleanup, and observations.
There is no replacement executor, Provider route, or second scheduler.

Apply `jobs-external-lifecycle.patch` to the reviewed native source and rebuild
normal DSH host/Remote artifacts. This adds explicit native producer/control
ports and identifies launching tools by their **actual registered definition**,
including DSH's configured subagent/workflow tool names. It changes no model
tool name, parameters, output schema, rendering contract, model configuration,
or native job limit. `prepare-jobs-runtime.mjs` builds the test fixture by
applying the shipped patch to an isolated native source copy, checking it
byte for byte against the reviewed transformations, and transpiling that
patched source. It leaves the installed host untouched.

```js
const jobs = await installCoreJobsRuntime(ctx, {
  client: lifecycleClient,
  bindingFor,
  // optional: actual appendPluginRecord; otherwise imported from native DSH
  appendRecord: appendPluginRecord,
  onCollection: record => observeCollectionBinding(record),
});
```

Required services are `jobs`, `agents`, `tools`, and `sessionProjections`.
The patched native tool-jobs controller must have loaded before installation
(`externalControlVersion === 1`).
The native executor resource join must also report
`externalExecutorJoinVersion === 1` and implement `joinExecutor`; an older
source patch is rejected before installation. Optional `subagents` and `workflowEngine`
are attached through their real native ports when present and on later service
availability. The SubagentRuntime must expose `externalPeerResidencyVersion === 2`.
The primary plugin checks already mounted service contracts before session
admission or port mutation. The returned API is
`{facade, refresh(agent), list(ownerSessionId), adoptPublication(), dispose(), drain(),
readState(externalId, ownerSessionId)}`. Unlike the goal adapter, **do not shadow
the three job tools**: their patched original execute bodies call the port,
and preserve native wait-default/cap clamping and content finalization.

Before any wrapped tool body can launch work, the host persists an intent and
obtains the only dispatch permit. The logical identity uses the real session,
open native turn, rootCallId, callId, original tool name, and argument digest.
AsyncLocalStorage carries the receipt to the synchronous native producer.
JobRegistry checks it before `spec.run`; ActivationManager awaits it before
provider/child materialization. Native ids attach to that same Core object.
Native tool retries return the committed original ToolExecutionResult; an
uncertain claimed dispatch returns an error requiring reconciliation and
never calls the original executor again. Native job_output/job_kill retries
likewise replay their original Core receipt without moving the cursor again.

Each local peer activation gets a new lifecycle object and executor identity
while retaining the same durable child Session address. Only the latest epoch
appears at that address. Its original claim identifies settlement, including
notifications delayed into a later epoch. An attached legacy peer without
epoch fields can continue after Core acknowledges it as terminal. The native
alias still resolves to a single current activity after restart.

The v2 local launch identity also survives a restart between claim and
attachment. A repeated attempt reports uncertainty for reconciliation.
Unattached claims made before this stable launch identity was introduced
require separately verified reconciliation before resuming the affected
child. Upgrade the adapter and native host patch together.

Per-execution queues acknowledge Core observations **before** forwarding native
job events, UI updates, or completion notices. The normal job controller and
notice/wakeup policy remain in use. Native controller list/get/readAt use
receipt-derived Core views, including persisted channel/offset metadata.
stdout/stderr/log and native gap/spill notices keep their native rendering.
Core unknown state is shown through the original valid `running` status with
an explicit reconciliation detail, never an invented success/terminal state.
Existing live handles are cancelled through their native control operation;
`stopping` is a request, and only actual executor release records settlement.
`job_output wait` joins the original native handle with its caller signal;
aborting that wait consumes no Core output and does not kill the job. The
subsequent Core collection has zero remaining wait, so an abandoned HTTP wait
cannot consume output later. Same-call receipt replays skip the native wait.
Module unload cancels, joins, flushes, and removes its ports after that boundary.
It closes native launch/control admission before awaiting cancellation. A tool
whose durable claim is still in flight is joined and refused before its original
executor can run; already delegated native executions are joined and re-scanned
for handles before the observation ports are removed.
Core transport/observation failures do not bypass actual cancellation and
native resource settlement. The adapter finishes those cleanup attempts and
then reports the unacknowledged Core facts as an error; it never fabricates a
Core terminal state from a cancellation request or a rejected observer promise.
Physical cancellation starts immediately when unloading, in parallel with the
ordered Core delivery queue. It never waits for a Core cancellation RPC or a
stalled response deadline. A failed delivery queue remains failed: later queued
observations and unload control do not each issue another uncertain RPC. Local
resource joins still complete, and unload reports the original unacknowledged
failure. The same handle is stopped once across both cleanup scans.
Native wait bounds are inherited; Node timer fractional-delay quantization is
preserved at the integer Core transport boundary.

Native subagent results are acknowledged after actual ActivationManager cleanup
and before its existing parent notification. Workflow execution stays in the
original Node PTC process and its native job wrapper. Background workflow jobs
and the workflow tool share one lifecycle; foreground workflow results are
settled on that same admitted run. Child activations receive separate native
activation identities before materialization, without duplicating the parent
workflow's authority.

Collection metadata is generated by the host from actual Core receipts:

```js
{
  lifecycle_object_id, collection_receipt_id, collection_digest, chain_id,
  message_id, call_id,
  material: { block_index: 0, utf8_start: 0, utf8_end }
}
```

It is persisted as `plugin:sic/jobs/collection` only after the real native
`tool/result` supplies its message id. Its final blocks must exactly equal the
original registered `output.render(args, Core.value)` blocks; truncated or
post-edited output receives no material authorization. `utf8_end` comes from
the real renderer's leading collected text, with no search-based attribution.
The same proof is carried in `meta.sic_collection` through the native
`output.presentationMeta` contract. Downstream source binding must use the
verified plugin record plus native message/block identity; model/stdout fields
with the same name confer no authority. `onCollection(record)` receives exactly
this record. Publication/adoption still requires Core's native source and
current-P collection eligibility checks; collection alone is not adoption.
`adoptPublication({agent,publication,request_id,adopted_cut_id?})` probes the
actual adopted cut. It returns separate adopted and skipped entries with Core
P-lineage proofs. Higher deletion/V skips the write, while invalid cuts and
permission failures remain errors. The caller supplies its actual durable
layout's selected cut and invokes this only after adoption persistence.

All custom persisted records use native `appendPluginRecord`, and are safely
ignorable projections/provenance. Core remains authoritative after session
reopen or record omission. On reopen, absent execution handles become unknown
and require observation-based reconciliation; they are not respawned.
Repeated inspection of an already unknown activity does not manufacture new
recovery observations or revisions. Core-acknowledged output remains consumable
once after a real native Session JSONL cold resume. Reattaching lost process
handles and reconstructing output that was never acknowledged are outside this
process-local native registry's contract; neither is reported as successful.

Reproduce in the SIC worktree:

```powershell
python rust-core/dsh-plugin/transparent/build-jobs-patch.py
node rust-core/dsh-plugin/test/check-jobs-source.mjs
node --test rust-core/dsh-plugin/test/transparent-jobs-runtime.spec.mjs
```

The fixture builds source copies under `03-backend-builds`, uses original
compiled native dependencies, copies the selected Core binary by SHA-256 into
its own build directory, and starts private loopback Core/SQLite instances.
The source check applies the shipped Jobs patch to an isolated native source
copy and compares every tested source file byte for byte before TypeScript
checks. Subagent settlement requires the original activation claim and a
native `externalPeerResidencyVersion === 2` port.
It uses real PowerShell/Subprocess execution, native ToolRuntime/JobRegistry,
ActivationManager, actual workflow Node PTC, model tool-call/result messages,
and JobController/Typert owner controls. Only the model/remote child provider
boundary is scripted. It exercises one-shot dispatch, output/collection,
cancel-versus-release, SQLite reopen, unknown crash windows, custom native tool
names, workflow execution, exact message/material collection binding, native
UI streams, unloading, and original ring retention. Raw SQLite, traces,
receipts, message ids, Core logs and test reports live under
`06-evidence/dsh-sic-transparent-comparison-20261117/jobs-runtime`.

This evidence does not switch the existing service/profile, touch user
sessions, call an external model, or claim a packaged browser application
release. The source patch and main-plugin composition must be included in
the normal DSH build before deployment. PTC sub-dispatch collection still
needs its own exact root-output material binding before it may be adopted;
an inner collection receipt alone does not authorize arbitrary outer output.
