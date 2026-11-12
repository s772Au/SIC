# Outstanding external evidence

See `EXTERNAL-ACCEPTANCE-BLOCKERS.md` for the exact items that cannot be produced from this checkout.

This file separates isolated Rust implementation/tests from the external
evidence that remains. It is not a new acceptance record and does not change
CONTRACT-STATUS. No item below is claimed as fully accepted. Some real evidence
now exists (2026-11-12 main/peer DSH runs); it is noted where relevant, but the
remaining gaps still apply.

## 1. Real DSH / real Provider sessions

- F03/F31-F33: real tool execution overlapping an Editor, catch-up/serial wait,
  actual tool artifacts and call counts; current overlap/wait tests use a
  host-simulated tool delay.
- F04/F13/F16/F70/F14/F71: real multi-Editor dispatch, real representation
  switch and private recursion, cancellation/recovery.
- F05/F06/F69: real adapter encode for the next turn and real DSH late tool
  results; Core/protocol tests exist.
- F22/F27/F72: real model search/level selection, frozen-stage adoption and
  anchors under real DSH.
- F38-F44/F41/F45/F46/F49: 2026-11-12 one real DSH parent + two sequential
  peers + two concurrently running DSH peers completed native `read`/`write`,
  plus a real child from the reused cut1 early checkpoint after the parent
  advanced to cut5; all replies delivered and adopted (see
  `research/real-evidence-20261112.md`). A real Socket task cancelled before
  child execution is also evidenced (unknown + rejected late advance, parent
  continues), and a real `glob` child lists two files and is adopted at cut8.
  A real prepared-task mid-run cancellation is evidenced (unknown revision 4,
  late advance rejected, parent cut9). A real `grep` child finds NEEDLE and is
  adopted at cut10. A real child provider failure (HTTP 500 after the first
  Editor call) ends in child exit1, task unknown revision4 and parent cut11.
  Four, six and then eight real DSH peers are concurrent from one checkpoint
  (4/4 at cut12, 6/6 at cut13, 8/8 at cut14). Still missing: batches larger
  than eight/history, business tools beyond grep/glob/read/write, Signal/Message
  under real load, and supervisor cancellation.
- F48/F50: a normal three-turn DSH session across Core restarts, native
  `read`/`write`, and recovery from a failed turn (failed active fingerprint
  reclaimed; stale pending sources filtered) are now evidenced; HTTP 500/unknown
  turn semantics and more business tools remain. A loopback HTTP 500 injection
  now shows the turn becomes `indeterminate`, direct retry stays 409, and a
  retirement plus `runtime.discard_unpublished` permits a new session to run
  `HTTP500-RECOVERED` for cut0 and `LATER-RECOVERED` after a published cut1
  (`BASE-CUT` retained). Killed-Core `bridge_restarted` turns are also recovered
  via retire/discard and a new session (`UNKNOWN-RECOVERED`; an `open` job needs
  explicit `allow_open=true` and then `UNKNOWN2-RECOVERED`). Arbitrary unknown
  turns and more business tools remain.
- F54/F55/F67: real DSH revocation/retry, cross-process crash recovery, physical
  erasure/key disposal (explicitly not claimed).
- F68/F69: real model conflict resolution and real DSH late material.

## 2. Independent quality experiments

- F29/F60/F61: frozen tasks, real model outputs, independent rubric/scorer,
  human or externally validated scores. Current checks are synthetic fact
  retention plus a not-evaluated rubric gate.
- F73: new task set and A/B/C/D comparison run; sources/adapter/plan exist,
  comparison not run.

## 3. Physical or cryptographic guarantees

- F54/F67: no physical erasure, key disposal, or cross-context/peer physical
  destruction is implemented. `security.revoke` is access revocation only and
  reports `physical_erasure=false`.

## 4. Official workbench and deployment

- F51-F53: Langflow/Studio binding, GUI interaction and same-Core deployment
  are not verified. A read-only ledger HTTP route is not a workbench UI.

## 5. Pricing and billing

- F56-F59: real usage/timing samples now exist (main and peer; see the evidence
  file), but there are no real unit prices, invoice reconciliation, provider
  cache-write categories or fleet ledger. C01-C10/T-COST-CONTROL are arithmetic
  fixtures.

## 6. Research evaluation

- F60-F62/F73: pack/verify and manifests are mechanisms; no real two-arm run,
  raw traces, independent cost report or quality report exists.

## What isolated tests do prove

Core state transitions, authorization/redaction, pack digests/verification,
bounded scheduling/budgets, W/E request composition, retry/idempotency and the
failure paths named in CONTRACT-STATUS. They do not prove model quality,
physical cache hits, complete real DSH behavior, production readiness or bills;
the 2026-11-12 samples cover one main session and one sequential peer.
