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

## What this does not establish

- The official Studio web UI is `https://smith.langchain.com/studio/?baseUrl=...`;
  this environment did not log in or perform browser GUI editing/screenshots.
- No typed-form save/reload, thread history, peer/tool/ledger UI, or F52
  independent Studio acceptance. F53 still has no nonce round-trip or
  one-click status command.
