# SIC Rust core — development build

This is an independent Rust implementation with its own SQLite store. It does
not open a Python Core database for writing. Current evidence includes local
Core/SDK tests and fixed-model live smoke runs. A real DSH headless process also
completed a read-only fixture tool roundtrip through the Rust Core and the
fixed OpenDesign model. These are limited acceptance results; this build is not
a production replacement for the Python service.

Two delivery lines share this source tree. **Lean** is the CTFy-specific
delivery line (single Editor, fixed DSH alias, restricted protocol/media and
bounded scheduling); its trade-offs and evidence live in `LEAN-ACCEPTANCE.md`.
**Full** is the F01-F73 contract line in `CONTRACT-STATUS.md` and is not bound by
those lean trade-offs: F04 multi-Editor, F14/F71 Editor-private recursion and the
complete protocol/media path remain full-line work. Evidence from one line never
substitutes for the other.

## Local control service

For an isolated DSH headless session, the prepared runtime also provides
`scripts/run-dsh-headless.ps1`; see [快速开始.md](快速开始.md). It starts a
dedicated Rust Core and DSH profile, passes Provider credentials only to Core,
and can emit DSH JSON events containing the session ID for a later resume. This
helper can explicitly resolve credential references through the installed DSH
credential parser without exporting the secret to the caller environment. It
does not import an existing DSH profile or provide a file sandbox.

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
A read-only `GET /api/v2/chains/{chain_id}/ledger` returns `accounting.get` for
the authenticated owner; it is not a workbench UI and it does not expose a
bearer-derived chain identity.
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

The example explicitly enables `retry_editor_http_5xx`. If an Editor receives
HTTP 500, 502, 503 or 504, it can send the same request again after a bounded
delay, using the same finite attempt budget as invalid-output corrections.
Every attempt is recorded separately; an error response without usage remains
unknown. The option defaults to false when omitted. It does not retry 429,
timeouts, lost responses, Worker calls or external tools. Exhausting the budget
still leaves the job failed for explicit host handling.

The separate `retry_worker_http_5xx` profile setting defaults to false and is
not enabled by the example. When explicitly enabled, the DSH bridge may retry
a Worker call after an explicit HTTP 500, 502, 503 or 504, provided Core has
recorded the failed attempt without accepting a response or dispatching its
tools and the turn's Worker-call budget permits it. Two optional keys shape that
retry: `worker_retry_max_attempts` (total attempts, 1-5; omission keeps the
historical one retry) and `worker_retry_backoff_ms` (first retry delay, 0-5000,
doubling per retry, capped at 5 s; omission keeps 250 ms). Malformed values are
configuration errors. Each attempt is counted; unknown usage remains unknown. This does not retry 429, timeouts, lost
responses or external tools. The policy has isolated mock coverage and has not
been validated against a real Provider. Before either this retry or an
invalid-choice retry, Core checks the specific prior attempt, the rebuilt
request, the profile and the input/control state; a change rejects the retry
before making another Provider call.

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
is not provider-native usage or an exact billing record. `accounting.get`
additionally exposes host-measured `resources` (storage, blocks, intervals)
and `measurements` (block length, edit noise, cost counts) with explicit
`host_surrogate` / `provider_billed=false` / `non_claims`; none of these is a
price or a provider bill. The Core also exposes `experiment.pack` (ids,
stored-byte digests and host measurements only), `experiment.verify`
(recomputes object/manifest digests and reports stale cuts separately), and
`runtime.budget` (host-set per-WB permits with idempotent receipts). None of
these is a provider bill or a physical cache-hit proof.

For complete non-streaming Chat responses, optional tool-call indexes are
normalized to array order while call IDs, arguments and the original response
remain intact. `reasoning_content`, when returned as assistant text, is kept
as opaque protocol metadata across tool continuations. Other provider-specific
reasoning fields (including OpenDesign's observed `reasoning_details`) are
retained in the raw response but are not replayed; direct-provider compatibility
for those fields has not been established.

The host may configure `reasoning_effort` as `none`, `minimal`, `low`, `medium`,
`high`, `xhigh`, `ultra`, or `max`; the value is sent unchanged. These accepted
wire values and aliases do not imply eight distinct model effort levels or
universal Provider support. A fixed OpenDesign-model DSH fixture with `none`
completed once, while another request returned two choices despite `n=1` and
was rejected without adopting either. The default profile leaves effort unset.
The headless launcher now permits one bounded retry of a fully received Worker
response with an invalid choice count after Core rejects all choices and dispatches
no tool; operators can choose zero to two retries. Each model call is counted.
HTTP errors, timeouts, 429s and external tool execution are not retried by this
policy. A real DSH process passed a synthetic malformed-response fixture with
both a mock Provider and the fixed OpenDesign model; the malformed response
was injected, and the same upstream anomaly has not naturally recurred after
this change.

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
  logical request structure, not a physical provider cache hit. Plan scope
  fingerprints also distinguish configured routes and request controls. A
  profile may optionally declare a non-secret `cache_namespace` label; it is
  used only to compare host-side plans, is not sent to the Provider, and does
  not attest a cache hit. Omission stays unknown rather than assuming that
  Worker and Editor share a namespace.
- Core supports ordered WB/CB publication, provenance, text editing and replay,
  P/V/I routing, decision views, bounded higher-order formation and scheduling,
  Socket checkpoints/read ACLs, Message P/V, Signal and Board operations.
  Mock-provider tests exercise real Core calls. A single live hierarchy smoke
  reached a third-order CB; it does not establish every hierarchy/role path or
  sustained automatic scheduling.
- V output is made available immediately as pending source material. Its WB
  is formed at the next editing boundary, so subsequent Worker and user input
  can be included in the same frozen WB. The DSH bridge carries the V source
  into that next grouping. A narrowly checked compatibility path retains an
  older unedited `volatile-continuation` WB and marks it superseded when a new
  complete WB must be formed; it does not rewrite the old object. Local mock
  tests cover two-turn continuation, a claimed external-tool roundtrip across
  a database restart without replaying that tool, and a synthetic old Rust
  state. Existing user databases were not modified or validated.
- `signal.wait` lets an authorized host wait up to 30 seconds for a receipt
  on its own Agent route, either by exact ID or by `after_sequence`. Cursor
  waits return the earliest later Signal and `next_after_sequence`; they do not
  consume receipts, so the caller must persist the cursor to avoid replay. The
  Worker can use `sic_signal_wait` with a nonzero timeout as its sole tool call.
  Waiting releases the database
  transaction, rechecks authority, and stops when a Worker claim changes;
  each Core instance permits at most 32 simultaneous waits. Mixed tool calls
  receive an explicit per-tool error rather than being reordered. Waiting does
  not change the Agent's run state and does not wait for arbitrary incoming
  Messages or Board updates. A real DSH peer completed this explicitly
  prompted wait, a native `read`, and a Socket reply with the fixed OpenDesign
  model; requester bootstrap was mocked. Complete OS-style IPC and long-running
  concurrency are not established.
- Host-only `message.wait` waits for the next delivered inbox Message after a
  supplied sequence. It does not consume the Message or expose it to a Worker
  before CB publication; the caller saves `next_after_sequence` to avoid
  rereading. Nonzero Signal and Message waits share a 32-wait instance limit.
  The existing peer watcher still polls rather than using this endpoint, and
  each watcher cycle acts on a bounded set of tasks instead of blocking on waiters.
- Worker `sic_message_receive` pages already-published Message receipts and
  previews up to 512 codepoints of each text. `sic_message_read` pages the full
  text of a specified received Message on the same Agent route. An unpublished
  or another route's Message is rejected. This preserves complete archived
  text without repeating an unbounded inbox in one model tool result. Local
  tests cover multiple messages, Unicode and route/publication boundaries.
- Board state is persisted separately with ACL checks, revision CAS, task
  claims/progress and atomic latest-I refresh at CB publication. Model tools can
  claim and report progress but cannot attest completion. A bound Agent's Board
  access is rechecked on requests; a fork gets no parent-chain Board grant by
  inheritance.
- Pending sources, published source text, and CB bodies at a selected level are
  searchable by the host through `memory.search` and by the Worker through
  `sic_memory_search`. It is a case-sensitive exact substring search. Source
  hits return the first match per unit with its anchor, source, CB and WB;
  higher-level source units can themselves be derived from lower-level CBs.
  CB-body hits identify the CB and corresponding WB without inventing a source
  unit for CB-only additions. Pending hits distinguish an unformed WB from an
  already formed WB and correctly report that no CB exists yet. Published
  source hits also contain the materialized unit ID and up to 32 CB operation
  indexes with a total and truncation flag, and identify source fragments added
  after WB formation. It scans at most eight CBs or pending roots, sixteen
  source objects and 64 units per page; an empty page can still have a
  continuation cursor. A cursor is bound to the query and selected state;
  publication, scope, level or Worker-context changes require restarting the
  search. It searches only the selected Worker context; pending cursors also
  become stale when the pending group or its WB-formation state changes.
  Normal dependency and revocation checks still apply and may inspect more
  records than the search page itself. This is not semantic retrieval. One
  synthetic two-turn real DSH and fixed OpenDesign model run on the preceding
  registry revision verified first-level source search against the persisted
  CB, WB, source and operation index. The subsequent pending, higher-level,
  CB-body and Message-tool changes have not been rerun against the real
  Provider. This does not establish long-session or semantic search quality.
  Adding this tool changes the fixed shared tool registry and may invalidate
  an existing provider-side prefix cache.
- Worker memory-page, replay and hierarchy tools are scoped to its selected
  internal context. The authenticated host can inspect other contexts owned by
  the same Agent; switching the Worker view is an explicit host operation.
  Tests cover cross-context rejection and the Worker's default timeline. Shared
  internal-context DAGs and complete Editor-private recursion remain unfinished.
- Model-side hierarchy expansion returns pages of up to eight source-unit
  previews of 512 Unicode codepoints each, plus bounded source/member lists;
  `sic_memory_page` reads complete archived objects in pages. Model-side
  operation replay returns the reconstructed body in pages of at most 4096
  codepoints and a digest of the complete body. Host-side expansion and replay
  still return their full results. A synthetic 240,000-character WB is covered
  by `tests/worker_memory_core.rs`; other tools and external DSH tool results
  do not yet share a universal output-size budget.
- Before a Worker call, plain-text native tool results are limited to 32,768
  Unicode codepoints each and 131,072 preview codepoints in total, allocating
  the total to the newest results first. Reduced results are clearly marked
  previews with their source and unit IDs, original length and SHA-256.
  `sic_source_text_page` (or host
  `source.text`) retrieves the exact stored tool-message text in pages. The
  native source, DSH receipt and later Editor input remain unchanged. A host
  may set `tool_result_preview_chars` through `config.update` to 0
  (disabling projection) or 1,024–131,072, and set
  `tool_result_total_preview_chars` to 1,024–1,000,000. These are character
  limits, not a complete request byte limit; markers and other inputs can still
  exceed a gateway limit. Tests cover a large synthetic Unicode result, several
  results with newest-first allocation, an isolated DSH bridge continuation and
  disabling the projection; real model
  paging behavior and task quality remain unverified.
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
  counts both fixed SIC tools and expanded dynamic tools; the current 25 fixed
  tools leave 103 dynamic slots. The discoverable catalog can be larger;
  Worker I shows at most 20 names and `sic_tool_search` provides authorized pages.
- A real DSH headless process completed a synthetic read-only tool roundtrip
  using OpenDesign `deepseek-v4.1-flash-eco`, including after the multi-turn
  bridge fix. A separately tested, explicitly host-started Rust supervisor
  and its opt-in watcher discovered a peer task, ran a DSH fixture
  against a local mock Provider and returned its actual Worker response over
  an authorized Socket. The watcher defaults to one task per cycle and one
  running supervisor; a host can raise `max_tasks_per_cycle` (1-8) and
  `max_concurrent_runs` (1-4, never above the per-cycle bound) to run bounded
  concurrent supervisors. Terminal failures cool down and then quarantine until
  the host runs the `ack` subcommand; admitted or running children are only
  re-inspected. Concurrency and governance tests use a synthetic runner only:
  several real DSH children sharing one profile are not verified. The watcher also passed once with the fixed OpenDesign
  model driving the peer Editor/Worker and DSH's built-in `read` on an isolated
  synthetic file; requester bootstrap used a mock and requester CB adoption a
  program fixture. A test-only path guard restricted that read, but the runtime
  tool name allowlist is not a file sandbox. A one-shot supervisor separately
  completed a synthetic tool roundtrip with the fixed model and real DSH. See
  `PEER-SUPERVISOR.md`. This does not establish arbitrary application tools,
  unattended recovery or production readiness.
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

The authenticated status endpoint also exposes a bounded last-request
projection and a conditional `if_revision` fingerprint for a host-side view.
The DSH plugin additionally provides a session tab for native tool events,
role/model usage, on-demand main-context WB/CB tracing, and adding limited
decision items. It keeps the host bearer out of the browser. The status
projection itself does not expose message bodies or constitute a complete timeline; see
[DSH-CONTEXT-ANALYSIS.md](DSH-CONTEXT-ANALYSIS.md) for the API, evidence,
and boundaries when comparing SIC with DSH's own session projections.

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
