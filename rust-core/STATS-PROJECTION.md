# Session statistics: what is measured, what is not

`accounting.get` now carries a `stats` block. It answers the same questions a
harness status bar does -- turns, steps, tokens, throughput, cache share, context
occupancy -- without inventing any of the numbers.

## Where each figure comes from

| Field | Source | Class |
|---|---|---|
| `turns` | Host bridge user-turn records | host-measured, `null` when the bridge tables are absent |
| `steps` / `steps_by_role` | One entry per recorded `model_attempt` | host-measured |
| `prompt_tokens`, `output_tokens` | `provider_reported_usage` per attempt | provider-reported |
| `cache_read_tokens` | `prompt_tokens_details.cached_tokens` or `prompt_cache_hit_tokens` | provider-reported |
| `tokens_per_second` | `output_tokens` / `model_seconds` | host-measured wall clock |
| `model_seconds`, `wall_seconds` | Editored/Worker `run.started` to attempt `finished` | host-measured |
| `context_tokens` | Newest completed attempt's `prompt_tokens` | provider-reported |
| `context_window`, `context_percent` | Host model profile `context_window` | host-declared |

## Three honest differences from a streaming harness

**1. No decode rate.** Every SIC model call is one non-streaming request. There is
no time-to-first-token to subtract, so speed is `whole_call_output_over_model_seconds_not_decode`.
It is a throughput figure, not a generation speed, and the field name says so.
A provider may still stream internally; SIC's transport requests `stream: false`.

**2. Cache reads are a subset, not a bucket.** A streaming harness usually sums
four disjoint buckets (uncached input, output, cache read, cache write). SIC
records the provider's `cached_tokens`, which is *already included* in
`prompt_tokens`. Summing them would double count. The share is therefore
`cache_read / prompt_tokens`, and the field `cache_semantics` states it.

**3. Occupancy is the last prompt, not the sum.** A status bar shows how full the
window is, which is the newest request's prompt size. `context_tokens` is the
newest completed attempt's prompt; it does not accumulate across the session.

## What stays unknown

`unknowns` lists every reason a figure is absent. Missing counters never become
zero:

- `prompt_tokens_incomplete` -- at least one attempt reported no usage
- `turn_count_owned_by_host_bridge` -- the bridge tables are not chain-scoped yet
- `context_window_not_bound` -- the host installed no model profile
- `no_completed_model_interval` -- nothing has finished, so speed is meaningless

`non_claims` states what this block will never assert: no price or bill, no
physical cache-hit proof, no task-quality score, no decode rate.

## Why these numbers are enough for the paper, and why they stay out of the core

The manuscript needs a cost argument, not a cost engine. `stats` gives it the
measured inputs -- per-role token counts, cache reads, real intervals -- and the
`sic-experiments` crate turns them into conditional arithmetic offline. Nothing
in the runtime prices a request, and no formula in the manuscript can change a
production code path.
