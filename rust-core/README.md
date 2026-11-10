# SIC Rust core — development build

This is an independent Rust implementation with its own SQLite store. It does
not open a Python Core database for writing. Current evidence includes local
Core/SDK tests and fixed-model live smoke runs. A real DSH headless process also
completed a read-only fixture tool roundtrip through the Rust Core and the
fixed OpenDesign model. These are limited acceptance results; this build is not
a production replacement for the Python service.

## Local control service

From the directory containing this README, build with `cargo build --release`.
Create a new database path (the parent directory must already exist), then
provide the control-plane bearer through a host environment variable:

```powershell
$SicExe = '.\release\sic-core-rust.exe' # use .\target\release\sic-core-rust.exe after a local build
New-Item -ItemType Directory -Force .\sic-state | Out-Null
& $SicExe init .\sic-state\core.db
$env:SIC_RUST_BEARER = '<generate-a-unique-local-bearer>'
& $SicExe serve .\sic-state\core.db 127.0.0.1:18930 local-owner SIC_RUST_BEARER
```

The server accepts loopback addresses only and requires the bearer for
`/health`, `/api/v2/capabilities`, and `/api/v2/operations`. The
authenticated host fixes the actor and permission ceiling. See
`tests/server.rs` for the exact operation envelope. Query capabilities at
runtime rather than assuming that all contract operations are implemented.
Stop this control-only server before starting the model-enabled alternative
below; only one service can own a database at a time.

To enable fixed model calls, the host supplies `--allow-model --model-config
PATH`. The JSON file uses `roles.worker` and `roles.editor` profiles;
credentials must be referenced by environment-variable name, not embedded.
`config/opendesign.example.json` gives the configured 1,000,000-token context,
393,216-token output ceiling and a 4,096-token actual output cap for the
user-selected OpenDesign model. Its `credential_origin` pins the configured
key to the OpenDesign HTTPS origin: an accidental change of `api_base` to a
different scheme, host or port is rejected before the key is read. Other
profiles may set their own origin; profiles without this optional field do not
get that check. Set `SIC_PROVIDER_KEY` and the independent
`SIC_RUST_BEARER` in the host environment, then for example start:

```powershell
& $SicExe serve .\sic-state\core.db 127.0.0.1:18930 local-owner SIC_RUST_BEARER --allow-model --model-config .\config\opendesign.example.json --dsh-alias sic-worker
```

For that command set the DSH plugin's `baseURL` to
`http://127.0.0.1:18930/v1`, `apiKeyEnv` to `SIC_RUST_BEARER`, and
`maxTokens` to the same 4,096-token actual cap. The example contains no
credential or price. Configure the real DSH price values separately; local
usage counters are not a bill.
Local mock-provider tests exercise the real Core provider path, including
bounded hierarchy scheduling and dynamic host tool dispatch. The recorded live
smoke runs used the fixed `deepseek-v4.1-flash-eco` profile: one completed an
Editor → CB → Worker text cycle, and one hierarchy run reached level 3 with one
third-order CB. The log records observed calls, not provider billing. A
surrogate tokenizer counts the complete serialized request plus a margin; it
is not provider-native usage or an exact billing record.

`snapshot-python SOURCE DEST` creates a separate, consistent SQLite snapshot
and verifies it without opening the old database for writing. A snapshot is
an archive, not a converted, executable Rust Agent. Do not point `serve` at
a Python database: the store intentionally rejects it.

## Current implementation and limits

- The configured runtime has one Editor per chain; Worker waits while an
  Editor is scheduled. Worker and Editor requests share frozen public
  instructions, CB messages, tool schemas and tool choice, then receive
  separately authorized I sections. Worker and Editor profiles may name
  different models; the supplied profile uses the same model. This proves
  logical request structure, not a physical provider cache hit.
- Core supports ordered WB/CB publication, provenance, text editing and replay,
  P/V/I routing, decision views, bounded higher-order formation and scheduling,
  Socket checkpoints/read ACLs, Message P/V, Signal and Board operations.
  Mock-provider tests exercise real Core calls. A single live hierarchy smoke
  reached a third-order CB; it does not establish every hierarchy/role path or
  sustained automatic scheduling.
- Board state is persisted separately with ACL checks, revision CAS, task
  claims/progress and atomic latest-I refresh at CB publication. Model tools can
  claim and report progress but cannot attest completion. A bound Agent's Board
  access is rechecked on requests; a fork gets no parent-chain Board grant by
  inheritance.
- Dynamic tool catalog and ACLs are host-owned. Model requests can search,
  expand, reclaim or request execution; the host performs the external action
  and records its result. A host must atomically `tool.claim` a pending task
  before executing it; `tool.complete` requires the same claim ID. Replaying
  a claim is a receipt, not permission to execute again. In-flight leases
  survive restart, partial completion blocks Worker continuation, and
  `tool.complete` is idempotent for the same result while rejecting changed
  results. A host may reconcile an unclaimed task as unknown without claiming
  execution. Catalog removal/schema sync is
  tested separately from a live external executor. Unused tools are eligible
  for eviction after 15 completed Worker rounds. The 128 complete-schema cap
  counts both fixed SIC tools and expanded dynamic tools; the current 22 fixed
  tools leave 106 dynamic slots. The discoverable catalog can be larger;
  Worker I shows at most 20 names and `sic_tool_search` provides authorized pages.
- A real DSH headless process completed a synthetic read-only tool roundtrip
  using OpenDesign `deepseek-v4.1-flash-eco`, including after the multi-turn
  bridge fix. A separately tested, explicitly host-started Rust supervisor
  and its opt-in serial watcher discovered a peer task, ran a DSH fixture
  against a local mock Provider and returned its actual Worker response over
  an authorized Socket. The watcher also passed with DSH's built-in `read` on
  an isolated synthetic file using a mock model. A one-shot supervisor
  separately completed a synthetic tool roundtrip with the fixed OpenDesign
  model and real DSH. See `PEER-SUPERVISOR.md`. This does not establish
  real-model watcher operation, arbitrary application tools, unattended
  recovery or production readiness.
- Python-store handling is isolated: `snapshot-python` is archival,
  `import-stopped-text` accepts only a narrow stopped, primary-level plain-text
  backlog, and `upgrade-active-python` quarantines unsupported legacy work.
  These commands do not provide general executable conversion of Python
  history, active tools, bridges or permissions. For a supported upgraded
  single-owner store, `legacy-audit`, `legacy-activate-board`, and
  `legacy-start` verify and record a narrow staged activation; quarantined
  runtime events and old tool grants remain disabled. The authenticated
  `/v1/sic/bind` route attaches a new DSH session to an existing idle Agent.
- A separate offline `legacy-config-audit` can inspect only the archived
  `task_goal` and `editor_feedback` subset. It prints the target revision guard,
  field names and value digests, never the config values. Restoration requires
  an explicit target, actor, idempotency key and the exact guard returned by
  audit; it accepts only values validated by Rust config rules and refuses a
  different value already set by the target host. It does not run or complete
  any quarantined event. Example:

  ```powershell
  $db = '.\rust-target.sqlite'
  $chain = '<target chain ID>'
  $actor = '<authenticated local host actor>'
  $audit = .\sic-core-rust.exe legacy-config-audit $db $chain $actor | ConvertFrom-Json
  if ($audit.eligible) {
      $g = $audit.target_guard
      .\sic-core-rust.exe legacy-config-restore $db $chain $actor '<idempotency key>' `
          $g.run_state $g.cut_seq $g.control_epoch $g.input_epoch $g.config_sha256
  }
  ```
- `runtime.abandon` is explicit host reconciliation for an uncertain Worker or
  Editor claim. It does not retry a provider call; an old response cannot be
  adopted after claim invalidation. The authenticated `/v1/sic/status` and
  `/v1/sic/retire` endpoints let a host seal a failed or indeterminate DSH
  session without replay. A new session may bind the same Agent only after the
  pending WB, tool tasks, DSH tool mappings and execution leases are resolved;
  the new session does not inherit the old native DSH transcript.

For a failed session, the authenticated host can inspect and seal the exact
unresolved turn. This never calls the model or executes a tool:

```powershell
$headers = @{ Authorization = "Bearer $env:SIC_RUST_BEARER" }
$status = Invoke-RestMethod -Method Post -Uri 'http://127.0.0.1:18930/v1/sic/status' -Headers $headers -ContentType 'application/json' -Body (@{session_id='<DSH session ID>'} | ConvertTo-Json)
$body = @{session_id='<DSH session ID>'; fingerprint=$status.active_fingerprint; reason='host inspected the failure'} | ConvertTo-Json
Invoke-RestMethod -Method Post -Uri 'http://127.0.0.1:18930/v1/sic/retire' -Headers $headers -ContentType 'application/json' -Body $body
```

After retirement, binding a fresh DSH session can still be rejected while
work is unresolved. Inspect Core `tools.tasks` and the Agent state; do not
infer that an externally claimed tool did or did not execute. This recovery
path does not recreate the previous native DSH transcript.

The protocol module has conservative lossless-subset tests for OpenAI Chat,
Responses and Anthropic messages, including static inline images. This does not
mean all three provider APIs are wired end to end: the current configured
provider path is OpenAI Chat text; Core explicitly rejects image ingress until
images can be retained throughout editing and Worker execution. Streaming, full multimodal processing,
provider billing reconciliation, quality scoring, production stability and
Langflow/Studio GUI acceptance remain unverified. Provider usage, costs and
physical cache hits must not be inferred from local counters or prefix
digests.

Run the isolated regression suite with
`cargo test --offline` from this directory. Real-provider
smoke, DSH headless/mock-provider checks and any DSH/real-model acceptance are
separate results; report the provider type and scope for each. See
`LEAN-ACCEPTANCE.md` for itemized evidence and remaining gaps.
