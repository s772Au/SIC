# LangGraph Agent Server / Studio backend probe (F52–F53)

Status: **local backend probe**, not F52 GUI acceptance.

- Official LangGraph API 0.17.1, langgraph_py 1.2.20, local Agent Server:
  `127.0.0.1:12027`, PID 47884.
- Data dir: `D:\SIC-real-evidence-20261112\langgraph-core-owned`
- Environment: `SIC_CORE_URL=http://127.0.0.1:18888`,
  `SIC_CORE_TOKEN` (process-only), `SIC_CORE_ACTOR=host`.
- `GET /info` returned version 0.17.1 and self-hosted host info.
- Assistant `sic_core` graph `sic_core`:
  - `POST /runs/wait` with `{"actions":[{"operation":"capabilities"}]}`
    returned `api_version=2`, `implementation=sic-rust-core-development`
    and the Rust operation catalog.
  - With context `chain_id=chain_12dfed4877b24af689f11bd5b8720a14`,
    `chain.get` returned `cut_seq=1` and the same published job
    `job_6c8e473f10f34e849d507cf928e6efcf` that the Langflow canvas used.
- This proves two runtimes (Langflow UI and LangGraph Agent Server) point at the
  same Rust Core/chain.

## One-click owned runtime status

`scripts/owned-runtime-status.ps1` checks the three owned ports, listener PIDs,
and health without printing the bearer:

- Core 18888: listening, PID 41968, health ok, api_version 2
- Langflow 7868: listening, health ok
- LangGraph 12027: listening, health ok
- exit 0; JSON saved as `D:\SIC-real-evidence-20261112\runtime-status.json`

## Official Studio web UI

- Playwright loaded `https://smith.langchain.com/studio/?baseUrl=http://127.0.0.1:12027`.
- The page displayed the official Studio shell, graph selector, typed raw form,
  Thread, Interact/Trace tabs, and graph `sic_core`.
- A raw form submission with
  `{"actions":[{"operation":"chain.get"}],"context":{"chain_id":"chain_12dfed4877b24af689f11bd5b8720a14"}}`
  created thread `01a1cf7d-740b-72ea-91b0-3dbb023ed7b7`; the graph ran through
  `inspect_actual_capabilities → prepare_action → review_authorized_material →
  peer_chain_and_socket → finish` and reached status `completed`.
- Screenshot: `studio-chain-get.png`.
- A persistent LangSmith API key is absent (the page shows the warning), but the
  local `baseUrl` connection and graph execution worked.

This is now real F52 GUI/thread/form evidence for a read-only graph action, not
full F52 acceptance.

## Studio candidate read

- Selected graph `sic_candidate` in the same official Studio page.
- Submitted raw form:
  `{"chain_id":"chain_12dfed...","job_id":"job_6c8e...","form":{"action":"read"}}`.
- Studio created thread `01a1cf7f-c449-76dd-a9b2-fa23d584ed22` and completed
  with status `readonly`; trace ran
  `load_candidate_and_readonly_provenance → review_versioned_material →
  apply_authorized_candidate_action`.
- Screenshot: `studio-candidate-read.png`.
- A subsequent `revise` resume against the already-published job correctly
  returned HTTP 409 `candidate is not ready or revision changed`; no edit was
  claimed.

## Studio graph candidate edit/validate/publish

A fresh chain/job was seeded through Rust Core, then the official Studio
`sic_candidate` graph executed through its API surface:

- chain `chain_4e68b9da821d42d78b0cf7ec0e408768`
- job `job_5d6558f512084fb0872084750667f28d`
- `revise` (pvi_only, `Round 177 Studio P`, allow_write) resumed after the
  guardrail interrupt and produced candidate revision 1.
- `validate` returned material body
  `Round 177 Studio candidate source\nRound 177 Studio P`.
- `publish` produced cut `cut_359d55c6f9d74373b799ae558150daad` and CB
  `cb_7c8ebd98f7294462a73f542f8445fe09` (sequence 1).
- A separate official Studio UI raw-form `read` on this fresh job created
  thread `01a1cf82-9616-756e-accb-cdc2092e4f6d` and completed `readonly`;
  screenshot `studio-fresh-read.png`.

The edit/validate/publish portion was executed via the Studio graph API, not
mouse-driven form save/reload; the read-only portion is genuine GUI.

## Studio-published job read from Langflow

- Studio `sic_candidate` graph published fresh chain/job
  `job_5d6558f512084fb0872084750667f28d` as cut 1 / CB `cb_7c8ebd…`.
- The isolated Langflow canvas uploaded the candidate flow as
  `9feb9125-2395-4e8b-abfc-efd4717ff78b`, pointed at the same chain/job,
  and ran the candidate node in 66 ms; screenshot
  `langflow-read-studio-candidate.png`.
- This is a real cross-runtime read of the Studio-published candidate.

## Cross-runtime write back

- A new source/job was created on the Studio-published chain
  `chain_4e68...`: job `job_5c51e52b45cc41119955bd31488297fc`.
- The isolated **Langflow canvas** edited the candidate P/V/I in the official
  SIC candidate node; the Core read back revision 1 with
  `P = "Round 184 LF cross P"`.
- The same Langflow canvas then ran `发布此版本` with expected revision 1 /
  expected cut 1; Rust Core moved the chain to `cut_seq=2` and the job to
  `published`.
- This is a real Langflow write on a Studio-published chain, complementing the
  earlier Studio write read by Langflow.

The exact latest CB identity was not asserted from the cross-write because the
subsequent `cut.get` view still displayed the prior CB; this is a remaining
inspection gap, not a claim of a clean nonce round trip.

## Studio GUI direct candidate edit

- Added an optional `direct` field to the candidate form. When true, the graph
  applies the typed form in the same run instead of raising the Studio interrupt
  that the current Studio UI cannot resume.
- In the official Studio GUI, a raw form submitted with `direct=true`,
  `action=revise`, and `persistent=["Round 208 direct GUI P"]` after a Tab/blur
  created thread `01a1cfc9-6314-767b-adaa-a7bc120cf45b`; status was `applied`,
  and Rust Core read candidate revision 1 with exactly that P value.
- Screenshot: `studio-direct-tab.png`.
- The same direct GUI path then submitted `action=publish` with
  `expected_revision=1` / `expected_cut=0`; thread
  `01a1cfd4-7ae7-720c-83c6-08594785b471` reached `applied`. Rust Core produced
  cut `cut_cff7f25a3759426dbc295b546467e569` and CB
  `cb_6407fc935c934fae95d2abecba3023ca` with body
  `Round 207 direct GUI source\nRound 208 direct GUI P`.
- Screenshot: `studio-direct-publish.png`.
- This is a real GUI typed-form edit/save **and publish**, but it deliberately
  skips the pause/resume path; it does **not** satisfy F52's pause/resume
  requirement.

## True Studio GUI pause/resume

- With the candidate form correctly entered, the official Studio interrupt pane
  exposes a **Resume** button and a raw resume-value editor.
- The resume editor was filled with the CandidateForm fields (not the whole
  interrupt envelope); `Resume` then executed
  `review_versioned_material -> apply_authorized_candidate_action` and reached
  `applied`.
- Thread `01a1cfdb-57ec-70ca-a7aa-960895938a70`; Rust Core read candidate
  revision 1 with `P = "Round 214 pause P"`.
- Screenshot: `studio-pause-resumed3.png`.
- The same non-direct GUI path then submitted `action=publish` through the
  Resume editor; thread `01a1cfdd-3b31-7740-a19f-e4e9a244fd92` reached
  `applied`. Rust Core produced cut
  `cut_0e91f43909c3431a889f17ecdf30f16a` and CB
  `cb_6611c95a078d40c1a3805a83d1f0b012` with body
  `Round 214 pause test\nRound 214 pause P`.
- Screenshot: `studio-pause-publish.png`.
- This is the real GUI pause/resume edit **and publish** path, not the optional
  `direct` bypass.

## Studio hierarchy timeline

- Selected the official `sic_hierarchy` graph and submitted
  `action=cb_timeline`, `level=2`, chain
  `chain_12dfed4877b24af689f11bd5b8720a14`.
- Thread `01a1cfe1-c42c-7110-8a70-56121c10460b` reached `applied` and
  returned hierarchy `records`.
- Screenshot: `studio-hierarchy-timeline.png`.
- This is a GUI high-order timeline read, not the complete F52 B/peer path.

## What this does not establish

- The official Studio web UI is `https://smith.langchain.com/studio/?baseUrl=...`;
  this environment did not log in or perform browser GUI editing/screenshots.
- No typed-form save/reload, thread history, peer/tool/ledger UI, or F52
  independent Studio acceptance. F53 still has no nonce round-trip or
  one-click status command.
