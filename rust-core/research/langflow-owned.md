# Isolated Langflow owned-backend probe (F51–F53)

Status: **F53 partial only**. No GUI/F51/F52 acceptance is claimed.

- Langflow version: 1.13.1
- Venv: `D:\Claude\SIC\.local\langflow-venv`
- Python: 3.12.16
- Dependency repair: isolated `grpcio==1.74.0` overlay at
  `D:\SIC-real-evidence-20261112\langflow-overlay`; a no-proxy wrapper
  (`langflow_no_proxy.py`) removes the broken `NO_PROXY=[::1]` parsing path.
- Isolated DB: `D:\SIC-real-evidence-20261112\langflow-owned\langflow.db`
- Config dir: `D:\SIC-real-evidence-20261112\langflow-owned`
- Host/port: `127.0.0.1:7865`
- Backend-only mode; no user Langflow instance, DB, or proxy was touched.
- PID recorded in `langflow-owned\pid.txt`; startup log in `server.err`.
- Health: `GET http://127.0.0.1:7865/health` returned `{"status":"ok"}`.
- Application startup completed; Uvicorn listening.

## What this establishes

- An owned, isolated official Langflow service can be started with a private
  SQLite DB and health/ownership record. This is F53 preparation.

## Official frontend and SIC flow

- Separate isolated full Langflow instance: `127.0.0.1:7866` (PID 20708),
  private DB `D:\SIC-real-evidence-20261112\langflow-ui-owned\langflow.db`.
- Frontend `GET /` returned HTTP 200 and `<title>Langflow</title>`; screenshot:
  `D:\SIC-real-evidence-20261112\langflow-ui.png`.
- Auto-login API returned an access token; the prebuilt
  `D:\Claude\SIC\integrations\flows\workbench-candidate.json` was uploaded
  through the official API and persisted as flow
  `5fac4d59-0833-468c-ab21-08ce7b5db2de`.
- The official canvas renders the two SIC nodes and their typed fields:
  `D:\SIC-real-evidence-20261112\langflow-sic-candidate.png`.
- The flow was read back through the official API and persisted:
  `langflow-sic-flow-persisted.json`, SHA256
  `fc1f394f43ceadc30e490019d17d8f89e469b1c94177cc43a749e421cad47570`,
  2 nodes/0 edges, same flow id.
- These are still UI/preparation probes: no Core binding, no real SIC
  candidate publish, no model/tool request, no hierarchy navigation, and no
  typed-form save/reload acceptance.

## Component execution in the official canvas

- A second isolated UI instance was started at `127.0.0.1:7868` with the SIC
  components path and `SIC_CORE_URL=http://127.0.0.1:18888` /
  `SIC_CORE_TOKEN` in its process environment.
- The uploaded flow `3fefb72c-8a80-4ec6-9377-a438c3e93a29` rendered both SIC
  nodes and the official **Run component** action executed the first node
  (`SICEditorTask-1`) without a build error; elapsed badge showed 108 ms.
- Screenshots: `langflow-core-sic.png` (canvas) and
  `langflow-run-core.png` (after run).
- A real chain/source was seeded through the Rust Core API, then the GUI
  action was switched to "准备单Editor", `allow_write` was enabled, and
  **Run component** was executed again. The Rust Core `chain.get` afterwards
  showed a new job `job_d8a1577f1c4f4bdaabc512839cce865a` with status `open`
  and base cut 0.
- Screenshot: `langflow-prepare-job2.png`.
- This is a real GUI-to-Rust-Core write/read effect (job preparation). It still
  does not establish publish/higher-order navigation or typed-form persistence.

## Candidate edit, validation and publish from the canvas

After fixing two integration mismatches (`v2_client` serialized job-specific
keys into the narrow Rust context envelope; and the candidate component did not
unwrap Rust's `result.candidate`), the regenerated flow
`4ee1c726-1c2d-410a-ab59-bafea79700d4` was used in the official canvas:

1. Opened the existing job `job_6c8e473f10f34e849d507cf928e6efcf`.
2. Action **修改 P/V/I**, set `P = "Round 164 P fact"`, enabled write, clicked
   **Run component**.
3. The Rust Core `edit.candidate.get` then showed
   `revision = 1` and `pvi.P = ["Round 164 P fact"]`.
4. Action **校验候选** with `expected_revision = 1`.
5. Action **发布此版本** with `expected_revision = 1`,
   `expected_cut = 0`.
6. Rust Core `chain.get`/ `cut.get`:
   - `cut_seq = 1`
   - job status `published`
   - cut `cut_837849cdb363474eb8aaf48ad20ed6eb`
   - CB `cb_cd0866d480074b15915959f13edd3327`
   - CB body contains the seed text plus `Round 164 P fact`
   - ops: `ADD` (editor-P) and `KEEP` (implicit-program)
   - `candidate_revision = 1`
- Screenshots: `langflow-candidate-edited5.png`,
  `langflow-published.png`.

This is a genuine official-canvas → Rust-Core edit/validate/publish round trip.
It still does not establish a GUI-native source ingestion, peer/tool/billing
flow, higher-order CB navigation, Studio, or typed-form save/reload.

## Hierarchy selection through the official canvas

- Uploaded `workbench-hierarchy.json` as flow
  `5f01f537-5edd-42ba-a1cc-675fa5d5f3c0`.
- Selected action `select`, level 1, with write enabled; the Rust Core
  `chain.get` role views changed from level 2 to level 1 for both worker and
  editor.
- Restored level 2 through the Core API; role views returned to 2.
- Screenshot: `langflow-hierarchy-select2.png`.

This is a real GUI → Core hierarchy selection, but the high-order CB
(`cb_3791bb…`, cut 3) was formed through Core APIs, not through the canvas.

## Native source run through the canvas

- A copy of `workbench-source.json` with the visible `working_text` field
  prefilled as `Round 170 GUI source` was uploaded as flow
  `1f172555-a890-49fc-9714-1dc568f27c88`.
- The canvas showed that value and the official **Run component** created
  source `source_bb3cdfe298ae4eb5acee30ca21972457`.
- `source.read` returned unit `u1` with exactly
  `Round 170 GUI source`.
- Screenshot: `langflow-source-round170.png`.

The field was prefilled in the flow rather than typed character-by-character,
but the ingestion itself was executed through the official canvas. This closes
the GUI-source plumbing, not the full F51 interaction checklist.

## Accounting canvas read

- Uploaded `workbench-accounting.json` as flow
  `6e5149e3-2896-4c8b-86ec-abe90e2fcac6`.
- Ran the official **Run component** against the seeded Rust chain; the
  component completed in ~161 ms.
- Screenshot: `langflow-accounting.png`.
- This is a real GUI accounting read, but the chain has no peer/tool billing
  entries; it does not satisfy F51's peer/tool/billing checklist.

## What it does not establish

- No frontend GUI interaction (backend-only), no SIC chain/flow editing, no
  high-order navigation, no Core binding/nonce round-trip, no screenshots, no
  F51/F52 acceptance. The Langflow service is not yet coupled to the Rust Core.
- It does not stop or modify the user's existing Langflow/Studio services.
