# LongMemEval-V2 task-set preparation (F60/F61/F73)

Status: **preparation only**. No model quality, cost, or two-arm result is claimed.

## Source

- Dataset: `D:\Claude\SIC\benchmarks\data\longmemeval-v2`
- Official repository revision: `2cc8c540bdb87fe6761629b585e727e1c4704520`
- License: Apache-2.0 (see the dataset `LICENSE`)
- Scale: 451 questions, 1,870 trajectories, small/medium haystacks.
- `questions.jsonl` SHA256: `0a3ae5ebea938c24d7800e1e0b0828e08ae1646f939a53853b2b8cdc08e292b7`
- `haystacks/lme_v2_small.json` SHA256: `9b5301defb23a088a5f06e45ff8d5f35e569d78305a66d492046a9fff9b46593`
- Official adapter/source: `D:\Claude\SIC\.local\contract-rebuild\lme-official-adapter`
- Source lock: `D:\Claude\SIC\benchmarks\lme-official-source-lock.json`
- Qwen processor: `D:\Claude\SIC\.local\contract-rebuild\lme-qwen-processor`

## Prepared runtime

Command used (official adapter, isolated data root):

```powershell
python benchmarks/run_core_lme.py --official-root '.local/contract-rebuild/lme-official-adapter' prepare-runtime --data-root 'benchmarks/data/longmemeval-v2' --domain web --tier small --limit 1 --output-dir 'D:\SIC-real-evidence-20261112\lme-runtime-web1'
```

Result: 1 question, 100 trajectories; `research_evaluation=not_evaluated`.

- Runtime: `D:\SIC-real-evidence-20261112\lme-runtime-web1`
- Question id: `00aa905a` (webarena-reddit, dynamic-environment)
- Official evaluator: `mc_choice_match|require_non_empty=true`
- Runtime questions SHA256: `00314977958bbd057be9570da72a5d17610aa377f0f55e75ba30de7af187c89e`

## SIC memory smoke (Python reference, not Rust)

A reduced 2-trajectory haystack was derived from the prepared runtime. The official harness, with the SIC Python reference memory (`benchmarks/core_lme_memory.py`), successfully built and saved memory:

- questions: same `00aa905a`
- haystack: `D:\\SIC-real-evidence-20261112\\lme-runtime-web1\\haystack-smoke2.json` (2 trajectories)
- config: `D:\\SIC-real-evidence-20261112\\lme-smoke-config.json` (`fanout=2`, `memory_chars=32768`)
- output: `D:\\SIC-real-evidence-20261112\\lme-program-smoke3` (`memory_state/sic-core.sqlite`, 4,976,640 bytes)
- exit: 0; `skip-evaluation=true`

This proves the official runtime and SIC memory plumbing can ingest a tiny slice. It is **not** the Rust Core, not a baseline/SIC two-arm run, not a reader answer, and not an independent quality score.

## Two-arm pilot (single question, Python reference)

The same frozen model and official evaluator were used for one baseline and one SIC arm. This is a pilot, not a quality conclusion.

| arm | memory | response | score | prompt/completion tokens |
|---|---|---|---|---|
| baseline | `no_retrieval` | `\\boxed{False}` | 1.0 | 211 / 704 |
| SIC | 2-trajectory SIC memory | `\\boxed{UNKNOWN}` | 0.0 | 15,620 / 95 |

- Model: `deepseek-v4.1-flash-eco`
- Official evaluator: `mc_choice_match|require_non_empty=true`
- Baseline output: `D:\\SIC-real-evidence-20261112\\lme-base-arm1`
- SIC output: `D:\\SIC-real-evidence-20261112\\lme-sic-arm5`
- SIC memory context: 17,970 tokens, not truncated
- SIC memory implementation: Python `benchmarks/core_lme_memory.py`, not the Rust Core
- One question only; baseline is trivial for this question; the SIC arm answered UNKNOWN and lost the point.

### Additional two questions

| question | type | base score | SIC score | base response | SIC response |
|---|---|---|---|---|---|
| `00aa905a` | dynamic | 1.0 | 0.0 | `False` | `UNKNOWN` |
| `05cce9b3` | static | provider returned no choices (no score) | 0.0 | — | `UNKNOWN` |
| `06a5a25f` | dynamic | 0.0 | 0.0 | `You updated the products.` | `You updated the attributes.` |

- SIC arm answered `UNKNOWN` on all three; memory context ~17,966–18,072 tokens.
- Baseline run for `05cce9b3` failed in the reader provider (empty choices), retained as a failure, not a score.
- Outputs: `lme-multi/mem-*`, `lme-multi/sic-*`, `lme-multi/base-*`; failed base log in `lme-pair-outer3.log`/`pair.log`.

This is evidence that the two arms can be run and independently scored, not evidence that SIC improves quality. It must remain `research_evaluation=not_evaluated` for the full set/full task distribution.

## What this establishes

- An independently authored, licensed long-horizon task set and official rubric entry exist locally.
- The official runtime-preparation path works on the local dataset.

## What it does not establish

- No SIC memory build, no baseline arm, no SIC arm, no model call, no independent scoring.
- The full 100-trajectory Python reference build hit `hierarchy_limit`; only the 2-trajectory smoke succeeded. It is not a Rust Core result and is not counted as F60/F61 acceptance.
- A two-arm run and independent quality report remain required.

## Next run (not yet executed)

Prepare a controlled runtime with a small haystack for plumbing smoke, then run baseline/SIC arms with the frozen model, preserving all trajectories, configs, usage, and the official evaluator. Keep `research_evaluation=not_evaluated` until the independent rubric is actually scored.
