# SIC external acceptance blockers

Snapshot: 2026-11-13. Current local evidence: `cargo test` 417 passed / 0 failed;
audit 73 ids, 42 live rows, 91 cited files; status buckets implemented 22,
partial 50, other F63. These blockers are not implementation excuses; they are
the evidence that cannot be produced from this isolated checkout alone.

## User decisions (2026-11-12)

- Langflow/Studio isolated use: allowed.
- Real prices/bills (F56–F59): skipped.
- Long task set: use the previously prepared local set. Located:
  `D:\Claude\SIC\benchmarks\data\longmemeval-v2` (451 questions/1,870
  trajectories, Apache-2.0; official adapter and processor found). One-question
  runtime prepared at `D:\SIC-real-evidence-20261112\lme-runtime-web1`;
  preparation is documented in `research/f73/longmemeval-v2.md`.
- Physical/cryptographic erasure (F54/F67): skipped.
- Real DSH/Editor: use DSH Headless.
- Complex tools/long-run: use CTFy tool inventory/tasks.
- F60/F61/F73: proceed from the above.


## Status after user decisions

- F56–F59: skipped by user; remain not_evaluated.
- F54/F67: skipped; implementation remains access-revoke only.
- F51–F53: allowed; isolated Langflow 1.13.1 backend/UI report health and the
  official canvas completed a seeded candidate edit → validation → publish
  against Rust Core (CB `cb_cd08…`, cut 1, revision 1). A local official
  LangGraph Agent Server 0.17.1 also reads the same chain through `sic_core`.
  A level-2 CB was formed through Core APIs and GUI `select` changed/restored
  role-view levels and a prefilled source flow created a real Core source
  through the canvas, and an accounting flow read the seeded chain. Live-typed
  source ingress, actual peer/tool/billing entries, full higher-order
  navigation, typed forms, and the official Studio web UI/login remain. A one-click status script reports Core/Langflow/LangGraph
  listening/PID/health without printing the bearer.
- F60/F61/F73: LongMemEval-V2 located; one-question runtime prepared;
  2-trajectory Python-reference SIC memory smoke and a 3-question baseline/SIC
  pilot succeeded (SIC 0/3, all UNKNOWN; base 1/1 and 0/1, one provider
  failure). Full task distribution, Rust Core arm, and independent human rubric
  remain.
- F31–F33: DSH Headless scripts available; an isolated 20-chain peer-DB scan
  found native `read` intervals but zero tool∩editor overlap, so a run with the
  required overlap is still needed.
- F45/F49: CTFy tools/task inventory found; installed task payloads and a
  long-run watcher are not yet confirmed.

## 1. Real DSH native tool + real Editor overlap (F31–F33)

Local evidence exists for three orderings with a real external `pwsh` process
and a mock Editor Provider. It does not prove the DSH native tool interval and
the real Editor Provider interval overlap. Needed: an isolated DSH run whose
native tool call is slow enough to observe, a real fixed-model Editor call, and
both intervals plus the frozen WB input in the same trace.

## 2. Watcher long run / more business tools (F45/F49)

A one-cycle real DSH supervisor watcher fixture passes (`research/watch-native-20261112.json`),
using a local mock model. Needed: a multi-cycle watcher run and business tools
beyond `read`/`write`/`glob`/`grep`, with adoption and failure handling across
cycles. Eight concurrent peers are covered; larger batches are not required by
the contract, but long-run watcher behavior is.

## 3. More arbitrary unknown-turn variants (F48/F50)

Real `bridge_restarted` recovery is proven twice: a `running` Editor-stage
job and an `open` current-cut job, both followed by retire/discard/new session
(`UNKNOWN-RECOVERED`, `UNKNOWN2-RECOVERED`). Needed: interruption at additional
phases (tool result adoption, cut publish, reply delivery) without inventing a
generic "all unknown turns pass" claim.

## 4. Physical/cryptographic erasure and key disposal (F54/F67)

The implementation deliberately reports access revocation, not physical or
cryptographic erasure. Needed: a storage/key-management design with a real
erasure or key-destruction proof, plus evidence that revoked bytes cannot be
recovered. Current rows must remain "access revoke only".

## 5. Official Langflow/Studio workbench (F51–F53)

The official workbench is not installed in this checkpoint. Needed: an isolated
official install, a reproducible workflow, and screenshots/logs tying the SIC
Core operations to the workbench. A mock substitute would not satisfy the
contract.

## 6. Real prices and bills (F56–F59)

No versioned real provider price list or bill is available. Needed: a captured
price version, provider usage records, and an independent recalculation that
ties the SIC cost algebra to the bill, including missing-usage cases.

## 7. Independent tasks, rubric, human quality, two-arm run (F60/F61/F73)

Local tests can check mechanics, but not independent task quality, rubric
scoring, or a two-arm human comparison. Needed: a task set not authored by this
implementation, a frozen rubric, independent human scoring, and a two-arm run
with the same model/price version.

## Safety constraints

No user running instance, user database, or published candidate may be used as
a substitute. Real Provider calls must remain isolated and fixed-model. Physical
erasure and production-readiness are not claimed.
