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

## What this does not establish

- The official Studio web UI is `https://smith.langchain.com/studio/?baseUrl=...`;
  this environment did not log in or perform browser GUI editing/screenshots.
- No typed-form save/reload, thread history, peer/tool/ledger UI, or F52
  independent Studio acceptance. F53 still has no nonce round-trip or
  one-click status command.
