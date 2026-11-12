# Rust Core service probe

Status: **runtime probe only**, not F51–F53 acceptance.

- Built `sic-core-rust` release successfully (1m33s) from the worktree.
- Initialized an isolated DB: `D:\SIC-real-evidence-20261112\rust-core-service\core.db`
- Started:
  `sic-core-rust serve <db> 127.0.0.1:18888 host SIC_CORE_TOKEN`
- PID: 41968; log: `server.out`/`server.err`
- Authenticated `GET /api/v2/capabilities` returned `api_version=2`,
  `implementation=sic-rust-core-development`, and the implemented operation list.
- The bearer is a process-only test token; it is not written to the repo.

## Seeded GUI binding

A real chain/source was created through the Rust Core API, then the official
Langflow canvas (7868) ran `SICEditorTask-1` with `allow_write`; a new Core job
`job_d8a1577f1c4f4bdaabc512839cce865a` (status `open`, base cut 0) appeared.
The regenerated candidate flow then completed edit → validate → publish:
cut `cut_837849cdb363474eb8aaf48ad20ed6eb`, CB
`cb_cd0866d480074b15915959f13edd3327`, and body containing `Round 164 P fact`.
See `research/langflow-owned.md` and `langflow-published.png`.

A second level-1 CB (`cb_8596ca…`, cut 2) was published, then
`hierarchy.form` produced level-2 WB `wb_7479ba…`; its pending source was
program-edited and published as high-order CB `cb_3791bb…` (cut 3).
`hierarchy.select` set worker/editor role views to level 2. This is API/core
higher-order evidence, not yet a GUI higher-order acceptance.

## Limitation

The prebuilt Langflow SIC components use `integrations/sic_workflow/client.py`,
which targets the legacy `/api/v1` session/draft/commit API, not this Rust
`/api/v2` surface. Therefore this service probe does **not** connect the
Langflow canvas to the Rust Core and does not prove F51/F52/F53.
