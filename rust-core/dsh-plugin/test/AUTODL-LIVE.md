# Explicit paid AutoDL checks

These opt-in scripts are separate from `transparent-*.spec.mjs`. They use only
the authorized AutoDL provider file, read its key inside the process, never put
the key in arguments/configuration/logs, and preserve redacted request/usage/
result evidence under `06-evidence/production-readiness-20261118/sdk-live`.
The shared ledger is not reset between runs. Before each call its admission
reserves the full request UTF8-byte input upper estimate and maximum output at
the supplied highest rates (¥2/M input, ¥8/M output). Completed calls with valid
nonzero usage use that usage at these rates, including cached input at full
input price; failed/unknown/invalid-usage attempts retain their reservation.
The cumulative ceiling stays ¥5. The default request cap is 20; the explicitly
extended authorized run uses `--max-requests=60` (never more than 4096 output tokens).

Run from the source workspace root, after freezing the intended Core binary:

```powershell
$env:DSH_SOURCE_ROOT = '<absolute path to the built DSH source checkout>'
$env:SIC_AUTODL_PROVIDER_FILE = '<absolute path to the authorized provider settings>'
$env:SIC_CORE_TEST_BIN = '<absolute path to the copied final release sic-core-rust.exe>'
$env:SIC_TRANSPARENT_LIVE_PACKAGE = '<absolute final release transparent package directory>'
node rust-core/dsh-plugin/test/run-autodl-sdk-live.mjs --live --mode=sdk --max-requests=60
```

The provider settings file is read only by the paid runners. Optional
`SIC_TEST_BUILD_ROOT` and `SIC_TEST_EVIDENCE_ROOT` select fixture and evidence
directories. The default directories are under the ignored `.local/` tree.

The optional `SIC_TRANSPARENT_LIVE_PACKAGE` loads the actual package modules
without changing their contents; omit it only for a source-worktree check.
`sdk` uses the original DSH LlmRuntime/SDK source port, original direct Messages
Adapter/serializer/SSE parser, default transparent Editor, real AutoDL Worker,
isolated native read schema/ToolRuntime executor, real low/high Core publication
and final selected cut. It intentionally loses one publication durability ack,
requires its retry to avoid another Editor, then checks a final answer based on
the actual read rather than a conflicting user-only marker. A final same-step
retry dispatches only Worker. It uses its own database/session/projections and
changes no application/profile/installed source. It currently takes about six
paid calls; actual model source paging may add calls.

The smaller original Messages probe is `--mode=probe`. The original DSH PiAi
OpenAI-compatible Adapter probe is:

```powershell
node rust-core/dsh-plugin/test/run-autodl-sdk-live.mjs --live --mode=openai-probe --max-requests=60
```

The OpenAI probe uses the provider's stated OpenAI endpoint and the original
pi-ai OpenAI serializer/parser, with only its native fetch option observing the
budget; it does not convert protocols. Neither probe changes SIC routing.
The original DSH interfaces tested here are streaming. An assembled final
answer is not a wire-level non-streaming check. No Responses endpoint is
inferred from the user-supplied provider file.

## Three original native API routes

Use `run-api-multimodal-live.mjs` for the complete Messages, Chat Completions
and Responses paths. Set `SIC_CORE_TEST_BIN` and
`SIC_TRANSPARENT_LIVE_PACKAGE` to the release you intend to verify. The runner
uses the original DSH DeepSeekAdapter for Messages and PiAiAdapter for the
other two APIs. Each flow exercises the real `read` tool, low and high CB
publication, common adoption, lost acknowledgement and same-step retry.

```powershell
foreach ($api in 'messages', 'chat', 'responses') {
  node rust-core/dsh-plugin/test/run-api-multimodal-live.mjs --local "--api=$api" --image --media-variants
  node rust-core/dsh-plugin/test/run-api-multimodal-live.mjs --local "--api=$api" --image-only-outer
}
```

Local mode uses its own protocol endpoint and checks original PNG bytes,
native offload placeholders, Core origin bindings and image-only requests.
Paid tests use the same script with `--live` in place of `--local`, an explicit
`--max-requests` cap and optional `--image`. Run paid cases serially against
the shared ledger. The ledger and output are under
`06-evidence/multimodal-api-acceptance-20261118/sdk-live`. The image witness
requires the model to identify the original user image despite receiving a
different image from the native tool. Preserve failures and validate the
exact tool marker, source and color in the actual model output.

To check an actual packaged declarative entry without rewriting imports or
installing a second copy of native peers:

```powershell
$env:SIC_TRANSPARENT_TEST_PACKAGE = '<absolute release package transparent directory>'
node --test rust-core/dsh-plugin/test/transparent-cordis-entry-http.spec.mjs
```

That check invokes the original DSH PluginPackages peer resolver and Loader,
verifies native Session/LlmAdapter/agentEvents/renderPrompt singleton identity,
uses JSON configuration and restarts the persisted binding. It is an isolated
local-model check and incurs no paid API requests. The source-patch handshakes,
reviewed complete prompt and real Session durability service remain required.
