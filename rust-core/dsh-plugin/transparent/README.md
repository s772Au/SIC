# Transparent native Harness integration

These opt-in modules preserve an existing Harness Provider route. They do not modify the legacy Chat bridge or auto-install into an active profile.

The primary Cordis entry is `cordis-entry.mjs` (JSON/YAML); `plugin.mjs` is the lower-level JavaScript factory. Follow [INSTALL.md](INSTALL.md). `sdk-context.mjs` edits the Harness SDK context before its original Provider adapter. The adapter keeps its route, model, authentication, native API, stream, parser and choice policy. No new SIC Provider/model profile or admission configuration is required.

`host-hierarchy.mjs` executes persistent Rust scheduler plans through the original Harness Editor. Rust retains recursive grouping, validation, publication and automatic adoption. `goal-runtime.mjs`, `jobs-runtime.mjs` and `compaction-runtime.mjs` connect original drivers, executors, commands and UI to the selected authority. Attachment offload remains a Harness responsibility.

See [MULTIMODAL.md](MULTIMODAL.md) for native readonly media, editable text, source linkage and the distinction between protocol preservation and model media understanding.

The native browser view records actual SDK context, candidates, CB/cut publication, independent I receipts and higher Editor traces, with independent version/I/trace pagination. Reads recheck dependencies and Board access. Higher-level V is bound to a real same-level successor WB by the Core scheduler, with exact source dependencies and P/volatile provenance. See the individual runtime documents.

Sessions without SIC keep native compression, including local regions. Sessions started with SIC use its continual editing, recursive publication and automatic common adoption; native automatic and manual global compression entries hand off to that same authority. A positional region without a trusted complete-CB mapping is an explicit compatibility boundary and preserves the archive. Existing ordinary history requires a separate explicit migration; a local compression request must never initiate migration. This boundary does not indicate missing SIC core compression.

Run the `test/transparent-*.spec.mjs` suite and reviewed source-patch checks. Fixtures use real Harness runtime/services and private Core HTTP/SQLite with scripted model responses; they do not measure external Provider quality, cost or physical cache hits.

## Optional native-wire compatibility

The following modules also support an explicit serializer-local wire hook. Other Providers need no fetch hook for the default SDK integration.

* `transport.mjs`: Provider-owned native fetch decorator, Core begin/commit, final text-anchor guard, original Harness admission, required adopted-projection acknowledgement, original streaming Response returned untouched.
* `core-client.mjs`: existing `/api/v2/operations` envelope with injected host authentication/context; no credential file/environment reads.
* `projection.mjs`, `file-store.mjs`: immutable durable original→units→edits→receipt→adopted request records, required native-session correlation, authorized per-session browsing route `/api/sic-core/native-projections`.
* `prompt.mjs`: final rendered-system/schema/runtime activation guard. Invoke after assembly complete restoration and runtime refresh; a waterfall-only guard is insufficient.

The host provides `resolveInvocation` (authenticated actor/session/request/format), an authorized Editor driver, admission, projection persistence and session-event append. The existing Provider injects the decorated fetch at its native HTTP serializer. `deepseek-native-fetch.patch` implements this minimal optional seam for the real DSH direct Messages adapter; `prepare-deepseek-fixture.mjs` makes an isolated copy of the actual bundled adapter for integration tests, with a source hash manifest. `git apply --check --ignore-space-change` validates the source patch. The current general pi-ai adapter does not advertise a universal fetch injection: other Providers need their own transport seam. Do not mutate frozen `llm/stream` options, replace model routes, translate native formats, or fall back to the old fixed Chat runner.

The native Core transactions and compatibility ecosystem are separate dependencies. Candidate scopes must be authenticated and Editor access confined to assigned source units. These modules neither grant tools from prompt text nor supply a model router. A successful transaction receipt is not automatically a published CB; CB/PVI publication must be wired to the authoritative Core lifecycle and reported distinctly.

Run `node --test test/transparent-host.spec.mjs test/transparent-dsh-assembly.spec.mjs test/transparent-deepseek-native.spec.mjs`. The tests import the actual local DSH registry, LlmRuntime, direct Messages serializer/adapter and SSE parser. The direct adapter has exactly one injected fetch seam and offline HTTP response. This verifies the real Harness/provider code path, not a paid Provider or installed desktop end-to-end deployment. Override `DSH_SOURCE_ROOT`, `SIC_HOST_FIXTURE_ROOT` and `SIC_PROMPT_ARTIFACTS` only with explicitly selected roots.
