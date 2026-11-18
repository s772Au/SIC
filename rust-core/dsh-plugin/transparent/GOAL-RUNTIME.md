# Native DSH goal state and SIC Core

The model keeps `create_goal`, `get_goal`, and `update_goal` and their original
schemas, output rendering, and calling style. Core owns the persistent Goal;
the original DSH `goal-round-driver` remains the only continuation scheduler.
Provider/model invocation stays in the original AgentLoop.

Apply `goal-external-state.patch` to the reviewed DSH source and rebuild its
normal host/remote artifacts. The patch adds a selected-agent external state
port to GoalService, an explicit single-driver handshake, and awaited Core
checkpoints in the original driver. It keeps the original followup, Inbox,
competing-human-message fences, descendant wait, pre-step admission, cancellation,
durability flush, and unloading flow. There is no second scheduler.

Compose the module before selected Agents run:

```js
const runtime = await installCoreGoalRuntime(ctx, {
  client: lifecycleClient,
  bindingFor,
  resolveGrant: authority,
  emitGoalChanged: (agent, change) =>
    agentEvents(ctx, agent).emit('goal/changed', { change }),
});
// Pass runtime.facade to the existing per-agent installGoalCompatibility().
```

The patched native driver must already be active when a selected Agent is
attached. The adapter rejects an unpatched GoalService/driver. Unselected Agents
retain their original service behavior. The public `defaultMaxGoalRounds`
getter exposes DSH's resolved native cap; the bridge inherits it and adds no
model/provider policy or new cap configuration.

The driver refreshes authoritative state at scheduling and admission boundaries.
Core records an exact round start before native admission and an exact finish
after whole-Agent quiescence. Abort, model failure, output limit, driver unload,
and recovered unfinished rounds close as interrupted and stay disarmed. Session
creation/resume never rearms a Goal; a real human resume is required. Three
consecutive native rounds reporting the same blocked tool condition produce
Core outcome receipts before `blocked` can succeed.

Native `/goal` mutations use their committed `command/run` user source.
Native UI Remote methods use the actual Gateway `this.ctx.invocation`, whose
owner principal and peer are fixed by the host. The bridge validates the Goal
namespace/service/method and live cancellation signals, persists the observed
RPC intent, and issues the human grant. No wire/model flag grants authority.
GoalService's synchronous methods reject local mutations for selected Agents;
native commands and Remote wrappers await the port instead. `clear` maps to
internal `clear_goal`, preserving the original model tool schemas, retaining a
Core tombstone, and clearing the native UI view.

GoalService.get() reads only the latest receipt-derived Core view; Remote get
refreshes Core first. The normal native `goal` projection is fed by
`plugin:sic/goal/receipt` via DSH's actual appendPluginRecord/pluginRecordOf.
These records are safely ignorable read caches, survive JSONL restore, and
never grant mutation/continuation authority. The adapter reconstructs them from
Core after resume; it does not maintain a second authoritative Goal log.

Reproduce without editing the DSH checkout or using a live service/session:

```powershell
python rust-core/dsh-plugin/transparent/build-goal-patch.py
node rust-core/dsh-plugin/test/check-goal-source.mjs
$env:SIC_CORE_TEST_BIN = '<selected-build>/sic-core-rust.exe'
node --test rust-core/dsh-plugin/test/transparent-goal-runtime.spec.mjs
```

The fixture transpiles only copies of real DSH source into `03-backend-builds`,
loads its actual compiled dependencies, starts private loopback Core/SQLite,
and uses the actual native tools, AgentLoop, GoalService, driver, command
registry, Typert Gateway, peer identity, projection registry, and JSONL backend.
Only the model adapter is scripted. No fixture manually submits round observations.
It checks two-round completion, host pause/cancellation and resume, inherited
cap, clear, persisted session resume plus Core reopen, repeated blocked audit,
and owner UI RPC with non-owner rejection. Logs, persisted JSONL, SQLite,
native message identities, queue/claim events, and derived UI snapshots are
kept under `06-evidence/dsh-sic-transparent-comparison-20261117/goal-runtime`.

Boundary: this is executable isolated integration evidence. It does not switch
the running DSH/Core service, modify user sessions, exercise an external model,
or claim a full packaged application/browser release test. Normal DSH rebuild
must regenerate Typert artifacts after applying the source patch; the fixture
reuses the actual generated contracts with only the patched implementation
method names adjusted to match those decorators.
