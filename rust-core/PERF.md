# Performance and cost work (2026-11-14)

All numbers measured on this machine with `cargo test --release` against the
current tree. Two `#[ignore]` measurement tests keep them reproducible:
`rust-core/tests/prefix_perf.rs` and `rust-core/tests/scheduler_perf.rs`.

## 1. Scheduler pass: 7 Core operations -> 6

Before, one `runtime.schedule` pass issued:

```
chain.get, config.get, cut.get, chain.get, hierarchy.timeline, source.read, runtime.budget
```

The two redundant reads were:

- `cut.get` only supplied `expected_cut`. `chain.get` reports the same value as
  `cut_seq`, and the pass already held a full snapshot.
- the second `chain.get` only supplied `declared_waits`. Same snapshot, same
  state: a pass is the only writer while it runs, so re-reading bought nothing.

After:

```
chain.get, config.get, chain.get, hierarchy.timeline, source.read, runtime.budget
```

| measure | before | after |
|---|---|---|
| ops per pass | 7 | 6 |
| wall time per pass | 962 us | 654 us |
| chain.get calls | 3 | 2 |

Getting the `declared_waits` fold wrong once cost two test failures
(`left: Null, right: "w1"` and a wrong decision label) because `operate`
returns the whole envelope, not the `result` body. The test file caught it.

`SchedulerReport` now carries `operations: Vec<String>` listing every Core
operation the pass actually issued. That is evidence, not a convenience: a
claim that a pass reads five snapshots can be checked against the report
instead of inferred from the code.

## 2. Shared prefix assembly: registry construction cached

`PrefixPlan::validate` compares the frozen plan against `common_tool_registry()`
on *every* accessor call. The registry is 30 tool schemas built with `json!`
and nothing can change it at runtime, so rebuilding it per call dominated the
accessor cost.

| measure | before | after |
|---|---|---|
| `common_tool_registry()` | 65 us | 34 us |
| `plan.common_tools()` | 205 us | 168 us |
| `plan.common_messages()` | 168 us | 136 us |
| `plan.common_prefix_bytes()` | 222 us | 194 us |
| `PrefixPlan::freeze` | 189 us | 146 us |
| assembly loop (3 accessors) | 452 us | 364 us |

The remaining ~134 us in an accessor is `validate` itself: it re-serializes
the whole plan and re-hashes it to compare against the stored digest. That is
the integrity check the contract asks for, so it stays. Removing it would
trade a real tamper guarantee for latency.

## 3. Not done, and why

- **`validate` is not memoized.** Caching its result would defeat the point of
  re-checking a frozen plan on each read. Batching the three accessor calls in
  the request path into one would be correct and cheap, but it changes the
  call shape across `core.rs`, so it needs a deliberate refactor rather than a
  perf patch.
- **`chain.get` still runs twice.** The two reads happen at different points
  in the pass (`run()` entry and `schedule_first_order`), and between them the
  pass can publish a block, so the second read is not provably stale. It stays
  until the publish path can prove no mutation happened between them.
- **No token/cost optimization was attempted.** Provider usage is unknown
  without a real Provider run, and guessing from the token surrogate is exactly
  what the contract's `non_claims` forbid.

## 4. Reproduce

```
cargo test --release --test prefix_perf      -- --ignored --nocapture
cargo test --release --test scheduler_perf   -- --ignored --nocapture
```

## Correction: the aggregate test count was truncated

`cargo test --workspace` aborts at the first test *binary* that fails to run.
On this machine `signal_core_dispatch` is refused by the Windows image-load
policy (os error 740), so the aggregate run stops there and reports a total
that looks complete.

| measure | aggregate run | per-binary run |
|---|---|---|
| binaries | 94 | 115 (110 rust-core + 5 experiments) |
| tests passed | 391 | 460 |
| targets never run | 18 rust-core + 4 experiments | 0 |
| blocked | 1 (`signal_core_dispatch`) | 1 (verified 8/8 by copying) |

`scripts/run_tests_per_binary.py` now runs each target on its own and reports
green / blocked / errored separately, so a blocked binary is neither counted as
a pass nor quietly dropped. Both earlier totals in this record (380 and 386)
were produced the same truncated way and are superseded.
