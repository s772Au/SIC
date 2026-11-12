# Peer timing scan (F31 gap)

Snapshot: 2026-11-12, isolated copy of `peer-real.sqlite` opened read-only-ish
by a separate Rust service on `127.0.0.1:18890`; no user DB/service touched.

- 20 recent chains were scanned via `accounting.get`.
- Native DSH tool intervals exist, e.g. `chain_60269d059d39446c8ff9a062145ad860`
  has a `read` tool interval `1794454580..1794454581` and Editor intervals
  `1794454562..1794454570` and `1794456340..1794456347`.
- Cross-scan found **zero** positive `tool ∩ editor` overlaps among those chains.
  The tool follows the Worker/Editor phases in this fixture rather than
  overlapping an in-flight Editor.
- This is useful negative evidence for F31: a real DSH native tool + real Editor
  store exists, but this dataset does not establish the required overlap.
- No model calls were made for this scan; only isolated accounting reads.
