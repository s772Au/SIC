# F73 preparation pack

This directory is the F73 preparation package, not a completed comparison.

| File | Purpose |
|---|---|
| `sources.json` | Six public sources locked by repository commit, git blob SHA and license. |
| `adapter.json` | SIC adapter snapshot (Worker teaching, Editor contract, ops, rules) with local SHA256 values. |
| `ADAPTER.md` | Extraction/adaptation notes and explicit non-claims. |
| `PLAN.md` | Frozen A/B/C/D comparison plan; no run has been executed. |

Verify the local pack without network access:

```powershell
python rust-core/scripts/verify-f73-pack.py
```

Status: preparation only. Mechanical acceptance and research evaluation are
`not_run` / `not_evaluated`; no compression, cost or quality advantage is
claimed.
