# Rust SIC DSH provider

This isolated DSH bundle registers one PiAi OpenAI Chat Completions provider for the Rust Core `/v1/chat/completions` bridge. It sets `x-sic-bridge-session` from DSH's real session ID, obtains the bearer only from the named process environment variable, and disables provider retries. It does not implement Python bridge capabilities, evidence projection, tool-event reporting, Editor integration, or peer execution.

The model advertises a 1,000,000-token context window and a 393,216-token output ceiling. `maxTokens` is the configured actual fixed request cap and must match the Rust server's Worker profile (`--model-config`); defaults to 4,096. Configure the DSH value to the same actual cap used by the Rust service. DSH requires numeric price fields, so they must be explicitly configured. Zero in the isolated fixture is a placeholder, not evidence of free usage or a bill.

`bindChainId` is optional and defaults to disabled. Set it only when attaching one already-owned, idle Rust Agent to a DSH Session. On that process's first provider stream, the plugin uses the host bearer to call the loopback `/v1/sic/bind` endpoint with the actual `options.sessionId`, waits for the matching acknowledgement, and only then opens the Chat stream. Concurrent calls for that Session share one bind request. A configured chain is restricted to the first Session seen in that plugin process; another Session is rejected before model access, and the Core remains authoritative for conflicts and exact replay. `bindTimeoutMs` bounds this local setup wait (default 10 seconds, capped at 60 seconds and at `timeoutMs`). A failed or cancelled bind emits a provider error and does not send the model request. The local bind operation is not retried; after an uncertain transport failure, a later call for the same Session/chain can safely ask the Core again because exact binding is idempotent. The existing model provider remains no-retry.

DSH headless `--session-id` resumes an existing persisted Session; it does not create a blank Session ahead of time. The plugin therefore binds at the first provider stream, where DSH supplies the generated or resumed identity. Binding does not import DSH conversation history into Rust or make multiple DSH Sessions share one Rust Agent.

`fixtureReadTool` is disabled by default. When enabled it exposes only a fixed marker and performs no filesystem/network action; a returned marker demonstrates DSH ran that local fixture, never that an application tool executed.

`allowedTools` can further limit the native DSH tool schemas passed into SIC. An empty list preserves the host tool table; a nonempty list passes only matching names. The live fixture below allows only `sic_fixture_read`.

## Prepare the DSH runtime and profile

The verified bundle intentionally omits `node_modules`. Before installing this
provider into DSH, use the setup helper included in the package to
create a new runtime copy. The helper checks every file against
`SHA256SUMS.txt`, refuses an existing destination, copies the whole bundle, and
installs only the pinned production dependencies under the copied
`dsh-plugin` directory. It tries the local pnpm cache first, then the public
npm registry with a frozen lockfile and lifecycle scripts disabled. It clears
credential-bearing process variables and uses blank temporary npm config files;
it does not inspect DSH profiles or change the source package. The helper is
included at `scripts/setup-dsh-runtime.ps1` in the extracted package.

Run it from the verified package, passing an absolute verified package
path and a fresh absolute destination path that does not exist:

```powershell
$VerifiedPackage = '<absolute path to the extracted verified SIC package>'
$RuntimeDirectory = '<absolute path to a new directory that does not exist>'
& (Join-Path $VerifiedPackage 'scripts\setup-dsh-runtime.ps1') `
  -SourceDirectory $VerifiedPackage -RuntimeDirectory $RuntimeDirectory
$PluginDirectory = Join-Path $RuntimeDirectory 'dsh-plugin'
```

Use `$PluginDirectory` for DSH plugin installation. Do not install dependencies
inside the verified package itself.

After setup, use a fresh `DSH_HOME` directory. These commands never target the
normal profile:

```powershell
$env:DSH_HOME = '<new isolated directory>'
dsh --profile sic-rust-headless --from-default-profile headless --dump-config
dsh plugin --profile sic-rust-headless add $PluginDirectory
```

Set the bearer in the process environment as `SIC_CORE_TOKEN`; do not place it in the profile, patch, transcript, or logs. Start Rust Core with a loopback listener, same database/Core as any operator API, model grant, model config, and `--dsh-alias sic-worker`. Run DSH headless using a mock OpenAI Chat Completions provider or an approved local fixture. Never enable automatic model retries for this route.

## Local mock test operations

The local package includes only the allowlisted mock assets used below:
`bind-provider.spec.mjs`, `run-bind-headless.ps1`,
`run-isolated-headless.ps1`, `run-rust-bind-headless.ps1`,
`run-peer-bootstrap-headless.ps1`, `run-peer-task-bootstrap-headless.ps1`,
their local mock providers/observer, and redacted state summaries. These use
loopback-only services and synthetic fixtures. The remaining `dsh-plugin/test` files are development-workspace-only;
in particular, live OpenDesign and migration-database scripts are not packaged.
The scripts find `release\\sic-core-rust.exe` in a package or fall back to a
local Cargo release/debug build in the source workspace.

From either the package root or source workspace root, select the Rust Core
directory and its allowlisted test assets once:

```powershell
$CoreRoot = if (Test-Path .\dsh-plugin\test) { (Resolve-Path .).Path } else { (Resolve-Path .\rust-core).Path }
$PluginTest = Join-Path $CoreRoot 'dsh-plugin\test'
```

The mock roundtrip uses a temporary DSH home and Rust database, a local mock
Chat provider, a static synthetic read tool, and checks the actual DSH → Rust
`/v1` → Core → mock Provider flow. The bundle's `release` executable needs no
Rust build. In a source checkout without a built executable, run `cargo build
--locked --manifest-path (Join-Path $CoreRoot 'Cargo.toml') --bin sic-core-rust`
first; `--offline` works only when all Cargo dependencies are cached.

```powershell
& (Join-Path $PluginTest 'run-isolated-headless.ps1')
```

The isolated DSH headless mock Agent test uses loopback synthetic endpoints
only. It checks bind-before-chat ordering, session identity matching and the
fixture allowlist:

```powershell
& (Join-Path $PluginTest 'run-bind-headless.ps1')
```

The standalone `bind-provider.spec.mjs` additionally tests concurrent
deduplication, stream completion, cancellation and failure isolation. Run it
from the prepared runtime copy, where pinned production dependencies are
already installed:

```powershell
Push-Location $PluginDirectory
try { node --test test/bind-provider.spec.mjs } finally { Pop-Location }
```

With the latest Rust debug binary built, this second headless integration uses a fresh temporary Rust database, creates a new Agent through the authenticated Core API, and runs the actual Rust `/v1/sic/bind` and Chat bridge against the local mock Provider. DSH executes only the fixed `sic_fixture_read` tool; no real model or application tool is used. The sanitized report records bind/chat order, session digests, Core receipts and completion state:

```powershell
& (Join-Path $PluginTest 'run-rust-bind-headless.ps1')
```

`run-peer-bootstrap-headless.ps1` is the manual host-orchestration baseline:
the host bootstraps a requester route DSH session, creates a checkpoint and
Socket, spawns a peer route, publishes the peer inbox with `program_fixture`,
binds a second DSH session to that route, and returns the fixture result
through an ACL-authorized Socket Message for requester publication. SIC routes
are peer identities; the requester/peer wording describes the request
direction, not a route ownership hierarchy.

`run-peer-task-bootstrap-headless.ps1` adds the durable `peer.task.claim` and
`peer.task.prepare` host operations. A local mock Provider verifies a bounded
Editor-only prepare and the spawn-message publication receipt; DSH then binds a
second session to the prepared peer route and executes only `sic_fixture_read`.
The final answer is produced by a real Core Worker attempt through the mock
Provider, returned by `peer.task.reply`, replayed idempotently, and adopted by
the requester route's published CB. Both flows prove ordered host-supervised
operations under synthetic inputs.
Neither is an autonomous concurrent peer runner, real application tool
execution, nor production peer-execution acceptance. The earlier direct-bind
variant in the development workspace, `run-peer-fixture-headless.ps1`,
intentionally stops when a published peer route cannot accept a different initial
protected prefix; it is a negative fixture for the no-silent-prefix-rewrite
boundary and is not packaged.

```powershell
& (Join-Path $PluginTest 'run-peer-bootstrap-headless.ps1')
& (Join-Path $PluginTest 'run-peer-task-bootstrap-headless.ps1')
```

The following live OpenDesign command is available only in the development workspace; its script is not included in the local package. It sends real requests only to `amr-link.open-design.ai` with model `deepseek-v4.1-flash-eco`. It requires `SIC_PROVIDER_KEY` already present in the invoking PowerShell process environment, passes that inherited value only to the Rust child process in memory, and never scans another process or writes the credential to config, arguments, or logs. The loopback guard, bridge observer, and DSH process do not inherit this provider key. Missing credentials cause a safe exit before any model call. A loopback guard rejects other hosts, models or token budgets, and caps Editor and Worker calls. It uses a new DSH home/database, exposes only `sic_fixture_read`, disables DSH title-model/provider integrations and telemetry, retains a redacted summary, and removes the temporary database/profile afterward. The configured endpoint must stay `https://amr-link.open-design.ai/v1`; the guard intentionally rejects other hosts:

```powershell
.\rust-core\dsh-plugin\test\run-opendesign-headless.ps1
```

This live command makes billable provider calls. Its report records role counts, model/budget, sanitized response shapes, and whether DSH returned the local fixture marker; it never writes provider prompts, outputs, or credentials to the report.
