# SIC

**Streaming incremental context editing for continuing agent work.** Worker models, semantic Editor models and deterministic programs maintain one Agent's working context. Compression is one available edit.

The current Rust release is **v0.1.3**. This repository also contains the historical Python v0.1 prerelease. Pin a source commit and artifact SHA-256 for reproducibility. The Chinese [preprint v1](https://doi.org/10.5281/zenodo.24051713) describes the mechanism and cost conditions ([source](paper/sic_paper_zh_v1.md)).

The [Zenodo v0.1 archive](https://doi.org/10.5281/zenodo.24052123) preserves the original source and experimental materials for that version.

## Rust v0.1.3

The Rust Core implements recursive CB publication and automatic adoption of a common Worker/Editor cut. Its opt-in DSH SDK integration uses the original provider adapters and supports native media, four-stage image editing and source recall, and durable goal and peer lifecycle state.

The downloadable runtime targets Windows ARM64. The reviewed DSH integration uses the 0.2.1-rc.2 source with the supplied native patches and a fresh, explicitly bound session. See the [v0.1.3 download](https://github.com/s772Au/SIC/releases/tag/v0.1.3), [release notes](RELEASE-v0.1.3.md), [deployment and recovery guide](docs/rust-v2-production-operations.md), and [DSH integration instructions](rust-core/dsh-plugin/transparent/INSTALL.md).

The [v0.1.2 tag](https://github.com/s772Au/SIC/tree/v0.1.2) preserves the lean preview and its [acceptance scope](rust-core/LEAN-ACCEPTANCE.md).

## Video demo

[YouTube](https://youtu.be/s0I9LJ9-By8) · [Bilibili](https://www.bilibili.com/video/BV1TTWn6aERh/)

The video illustrates the mechanism; the acceptance reports below define the verified implementation scope.

## Python v0.1 research results

| Area | Included and verified scope |
|---|---|
| Python Core | Immutable CB/op history, independent WB/CB chronologies and same-level pairing, recursive N-order editing, P/V/I, Editor jobs, scheduling, peer routes, Socket, board, permissions and accounting |
| Shared-prefix B | Optional `prefix_layout=shared_i` production request wiring, frozen plans, trusted later I mode, exact assignment aliases and explicit complete fallback; the default remains `separate` |
| Regression | Recorded publication build: 273 Core tests, zero failures/errors/skips; 38 integration unit tests. Counts are not certification of all 73 feature conditions |
| Official workspaces | Langflow and Studio representative editing/navigation paths were visually verified on their recorded builds. New B configuration, plan/request pages and alias views passed source/SDK tests; standing B GUI acceptance is not complete |
| Real DSH exploration | Program-KEEP control g: 3 mechanically valid cases, 2 exact answers. Separate-layout SIC h: 2 valid/exact cases and one execution failure, unscored. B run i: 8 attempts, 3 CBs; independent wire audit completed, task failed and quality unscored |
| Research preparation | Seven executable compression/editing controls, actual local LLMLingua weights, official LME media/Memory adapter and independent scoring tools; no established quality/cost advantage |

The [functional acceptance ledger](docs/functional-acceptance.md), [runtime evidence](docs/runtime-acceptance-current.md) and [real DSH exploration](docs/dsh-live-results-20261108.md) preserve build boundaries and failed runs. Small synthetic tasks and owned loopback SDK tests do not establish a fixed physical model, a physical cache hit, cost parity or SIC superiority.

## Python v0.1 Core and APIs

[`sic_core/`](sic_core/) can be embedded in a Harness/provider or exposed through its authenticated HTTP API. Native API bodies are decoded into editable text plus protected protocol metadata. Outbound calls use LiteLLM with explicit profiles. Credentials stay in host environment variables and never belong in flow exports.

Published CBs and their ordered operations are immutable. Corrections use new P/V/I. Ordered lower CB members form a higher WB; editing appends a higher CB and preserves the lower objects. Worker and Editor can adopt authorized shared or private recursive contexts. Peer forks are real work routes, not copies of visual graphs.

B uses the same Worker SIC teaching and rendered CB prefix, followed by host-authenticated mode/scope and new WB material. Incompatible identity, scope, budget or media preserves the complete separate layout with an explicit reason. Logical equality and declared cache namespaces do not prove physical provider cache reuse. Late tool results may be retained by a program KEEP supplement after the Editor snapshot; this does not prove that the same first-order Editor saw or semantically edited every tool result.

The tested ingress/re-encoding profiles cover text/tools in OpenAI Chat, Responses and Anthropic Messages, plus verified static inline-image carriers. Worker images retain immutable source/protocol relationships; Editor editing stays textual with read-only media descriptors. Admission requires a known explicit model counting policy. Unknown counting, auto detail, remote image fetching, audio and video are rejected.

Provider execution is canonical Chat through LiteLLM. The full-history Chat bridge supports buffered SSE; native Responses/Anthropic HTTP proxying and incremental token forwarding are not implemented. The external gateway may have its own routing/retries; SIC-side attempt counts do not measure those hidden attempts.

See [Core setup](docs/core-v2.md), [Python installation](docs/python-core-package.md), [DSH plugin](sic_core/dsh_plugin/README.md) and [official integrations](integrations/README.md).

## Software, paper and research

- Original software is [MIT licensed](LICENSE). Third-party dependencies, datasets and weights retain their own terms.
- The preprint is separately licensed **CC BY 4.0**; that paper license does not relicense external code, corpora or model weights.
- [Prompt/engine controls](research/compression/README.md) freeze inputs, final post-edit Worker consumption, independent scores and noise annotations. [Official LME preparation](benchmarks/README-core-lme.md) preserves screenshot carriers and the upstream Memory/processor workflow.
- Source releases exclude credentials, private overrides, runtime databases, raw sessions/responses, private working manuscripts, videos and large datasets/weights. Reproducibility uses source/artifact manifests and separately prepared public assets.

## Legacy

[`src/`](src/) contains the older Rust `9.0.0-alpha.3` prototype. Its binaries, historical tests, custom WebUI and older bridge are **legacy**, not builds or acceptance evidence of the current Python Core. Retained legacy guides are explicitly scoped to that implementation.
