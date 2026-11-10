$ErrorActionPreference = 'Stop'
$coreRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$plugin = Split-Path $PSScriptRoot -Parent
$binary = @((Join-Path $coreRoot 'target\debug\sic-core-rust.exe'), (Join-Path $coreRoot 'target\release\sic-core-rust.exe'), (Join-Path $coreRoot 'release\sic-core-rust.exe')) |
    Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
if (-not $binary) { throw 'Provide release\sic-core-rust.exe or build the Rust Core binary in target.' }
$supervisorBinary = @((Join-Path $coreRoot 'target\debug\sic-peer-supervisor.exe'), (Join-Path $coreRoot 'target\release\sic-peer-supervisor.exe'), (Join-Path $coreRoot 'release\sic-peer-supervisor.exe')) |
    Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1

$actor = 'synthetic-peer-host'
$saved = @{}
foreach ($name in @('DSH_HOME', 'SIC_CORE_TOKEN', 'SIC_MOCK_PROVIDER_KEY', 'SIC_RUST_UPSTREAM', 'DSH_TOOLS_MODE')) {
  $saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}
$tempRoot = [System.IO.Path]::GetFullPath($env:TEMP).TrimEnd([System.IO.Path]::DirectorySeparatorChar)
$temp = [System.IO.Path]::GetFullPath((Join-Path $tempRoot ('sic-peer-supervisor-headless-' + [guid]::NewGuid().ToString('N'))))
if (-not $temp.StartsWith($tempRoot + [System.IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw 'Fixture directory is outside the intended temporary directory' }
$null = New-Item -ItemType Directory -Force -Path $temp
$reportPath = Join-Path $tempRoot ('sic-peer-supervisor-redacted-' + [guid]::NewGuid().ToString('N') + '.json')
$mock = $null; $observer = $null; $core = $null
$providerStatus = $null; $observerStatus = $null; $stateSummary = $null
$parentPublished = $false; $peerInboxPublished = $false; $fixtureResultObserved = $false
$supervisorState = $null; $supervisorExit = $null; $supervisorReplayed = $null
$supervisorErrorCode = $null
$peerAttemptObserved = $false; $replyDelivered = $false; $supervisorReplaySafe = $false; $messageAdopted = $false
$parentExit = $null; $allChatsUseFixtureOnly = $false; $bindsMatchRoutes = $false
$stage = 'setup'; $failure = $null; $passed = $false

function Invoke-CoreOperation([string]$Operation, [string]$Chain, $Payload) {
  $context = @{ actor = $actor; allow_write = $true; allow_model = $true; allow_revoke = $false }
  if (-not [string]::IsNullOrWhiteSpace($Chain)) { $context.chain_id = $Chain }
  $body = @{ operation = $Operation; context = $context; payload = $Payload } | ConvertTo-Json -Depth 16 -Compress
  try {
    $reply = Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$corePort/api/v2/operations" -Headers $headers -ContentType 'application/json' -Body $body -TimeoutSec 30 -NoProxy
    return $reply.result
  } catch { throw "Core operation failed: $Operation" }
}
function Invoke-ProgramFixture([string]$Chain) {
  $policy = @{ context_id = 'main'; min_sources_per_wb = 1; max_sources_per_wb = 8; max_units_per_wb = 64; level_batch_sizes = @{}; max_level = 1; flush = $false; max_actions = 16; max_model_calls = 4; editor_mode = 'program_fixture'; program_rules = @() }
  return Invoke-CoreOperation 'runtime.schedule' $Chain $policy
}
function Write-DshPatch([string]$Path, [string]$ChainId, [int]$ObserverPort) {
@"
- id: agent-default-model
  config:
    provider: sic-core-rust
    model: sic-worker
- id: sic-core-rust-dsh
  config:
    provider: sic-core-rust
    baseURL: http://127.0.0.1:$ObserverPort/v1
    apiKeyEnv: SIC_CORE_TOKEN
    model: sic-worker
    bindChainId: $ChainId
    bindTimeoutMs: 5000
    maxTokens: 4096
    inputPricePerMillion: 0
    outputPricePerMillion: 0
    cacheReadPricePerMillion: 0
    cacheWritePricePerMillion: 0
    allowedTools:
      - sic_fixture_read
    fixtureReadTool: true
- id: session-title-llm
  name: '@deepseek-ai/dsh-session-title-first-prompt-llm'
  disabled: true
- id: llm-deepseek
  name: '@deepseek-ai/dsh-llm-deepseek-api-key'
  disabled: true
- id: llm-deepseek-account
  name: '@deepseek-ai/dsh-llm-deepseek-account'
  disabled: true
- id: web-search-deepseek
  name: '@deepseek-ai/dsh-web-search-deepseek'
  disabled: true
- id: web-deepseek-api
  name: '@deepseek-ai/dsh-web-deepseek-api'
  disabled: true
- id: session-telemetry-otel
  name: '@deepseek-ai/dsh-session-telemetry-otel'
  disabled: true
- id: otel
  name: '@deepseek-ai/dsh-otel'
  disabled: true
"@ | Set-Content -LiteralPath $Path -Encoding utf8
}
function Install-IsolatedProfile([string]$Name) {
  & dsh --profile $Name --from-default-profile headless --dump-config *> $null
  if ($LASTEXITCODE -ne 0) { throw 'isolated DSH profile creation failed' }
  & dsh plugin --profile $Name add $plugin *> $null
  if ($LASTEXITCODE -ne 0) { throw 'isolated DSH plugin installation failed' }
}
function Get-IdentityDigest([string]$Value) {
  $sha = [System.Security.Cryptography.SHA256]::Create()
  try { return ([BitConverter]::ToString($sha.ComputeHash([System.Text.Encoding]::UTF8.GetBytes($Value))).Replace('-', '').ToLowerInvariant().Substring(0, 12)) }
  finally { $sha.Dispose() }
}
function Get-SafeSupervisorErrorCode([string]$Output) {
  try {
    $value = $Output | ConvertFrom-Json
    $candidate = if ($value.error_code) { [string]$value.error_code } elseif ($value.code) { [string]$value.code } elseif ($value.error.code) { [string]$value.error.code } else { '' }
    if ($candidate -match '^[A-Za-z0-9_-]{1,64}$') { return $candidate.ToLowerInvariant() }
  } catch {}
  return 'other'
}

try {
  $env:DSH_HOME = Join-Path $temp 'dsh-home'; $env:DSH_TOOLS_MODE = 'native'
  $env:SIC_CORE_TOKEN = [guid]::NewGuid().ToString('N'); $env:SIC_MOCK_PROVIDER_KEY = [guid]::NewGuid().ToString('N')
  $mockOut = Join-Path $temp 'mock.out'; $mockErr = Join-Path $temp 'mock.err'
  $mockScript = Join-Path $plugin 'test\peer-mock-chat-provider.mjs'
  $mock = Start-Process -FilePath 'node' -ArgumentList @('"' + $mockScript + '"') -WorkingDirectory $plugin -WindowStyle Hidden -PassThru -RedirectStandardOutput $mockOut -RedirectStandardError $mockErr
  $mockPort = $null
  for ($i = 0; $i -lt 100; $i++) { if (Test-Path $mockOut) { try { $mockPort = (Get-Content -LiteralPath $mockOut -TotalCount 1 | ConvertFrom-Json).port } catch {} }; if ($mockPort) { break }; Start-Sleep -Milliseconds 100 }
  if (-not $mockPort) { throw 'local mock Provider did not bind' }
  $profile = @{ model = 'openai/mock-model'; api_base = "http://127.0.0.1:$mockPort/v1"; api_key_env = 'SIC_MOCK_PROVIDER_KEY'; tokenizer_model = 'host-o200k-surrogate'; tokenizer_status = 'local-synthetic-fixture'; context_window = 1000000; max_input_tokens = 1000000; max_output_tokens = 393216; default_output_tokens = 4096; max_supported_output_tokens = 393216; timeout_seconds = 20 }
  $modelPath = Join-Path $temp 'mock-model.json'; @{ roles = @{ worker = $profile; editor = $profile } } | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $modelPath -Encoding utf8
  $database = Join-Path $temp 'peer-supervisor-bootstrap.sqlite'; & $binary init $database *> $null
  if ($LASTEXITCODE -ne 0) { throw 'could not initialize isolated Rust database' }
  $probe = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0); $probe.Start(); $corePort = $probe.LocalEndpoint.Port; $probe.Stop()
  $coreOut = Join-Path $temp 'core.out'; $coreErr = Join-Path $temp 'core.err'
  $coreArgs = @('serve', $database, "127.0.0.1:$corePort", $actor, 'SIC_CORE_TOKEN', '--allow-model', '--model-config', $modelPath, '--dsh-alias', 'sic-worker')
  $core = Start-Process -FilePath $binary -ArgumentList $coreArgs -WorkingDirectory $temp -WindowStyle Hidden -PassThru -RedirectStandardOutput $coreOut -RedirectStandardError $coreErr
  $headers = @{ Authorization = "Bearer $env:SIC_CORE_TOKEN" }; $ready = $false
  for ($i = 0; $i -lt 150; $i++) { if ($core.HasExited) { throw 'isolated Core exited during startup' }; try { $null = Invoke-RestMethod -Uri "http://127.0.0.1:$corePort/health" -Headers $headers -TimeoutSec 1 -NoProxy; $ready = $true; break } catch { Start-Sleep -Milliseconds 100 } }
  if (-not $ready) { throw 'isolated Core did not become ready' }

  $stage = 'requester_bootstrap'
  $parentId = (Invoke-CoreOperation 'chain.create' '' @{}).chain_id
  $env:SIC_RUST_UPSTREAM = "http://127.0.0.1:$corePort"
  $observerOut = Join-Path $temp 'observer.out'; $observerErr = Join-Path $temp 'observer.err'
  $observerScript = Join-Path $plugin 'test\bridge-observer.mjs'
  $observer = Start-Process -FilePath 'node' -ArgumentList @('"' + $observerScript + '"') -WorkingDirectory $plugin -WindowStyle Hidden -PassThru -RedirectStandardOutput $observerOut -RedirectStandardError $observerErr
  $observerPort = $null
  for ($i = 0; $i -lt 100; $i++) { if (Test-Path $observerOut) { try { $observerPort = (Get-Content -LiteralPath $observerOut -TotalCount 1 | ConvertFrom-Json).port } catch {} }; if ($observerPort) { break }; Start-Sleep -Milliseconds 100 }
  if (-not $observerPort) { throw 'local Rust bridge observer did not bind' }
  Install-IsolatedProfile 'sic-peer-parent-headless'
  $parentPatch = Join-Path $temp 'parent.patch.yml'; Write-DshPatch $parentPatch $parentId $observerPort
  Push-Location -LiteralPath $temp
  try { $parentOutput = (& dsh --profile sic-peer-parent-headless --patch $parentPatch 'SIC_PEER_PARENT_BOOTSTRAP_SYNTHETIC. Reply briefly that the synthetic parent is ready.' 2>&1 | Out-String) }
  finally { Pop-Location }
  $parentExit = $LASTEXITCODE
  if ($parentExit -ne 0 -or -not $parentOutput.Contains('SIC_PARENT_BOOTSTRAP_READY')) { throw 'parent DSH bootstrap failed before publishing its protected prefix' }
  $parentState = Invoke-CoreOperation 'chain.get' $parentId @{}
  $parentPublished = [int]$parentState.cut_seq -ge 1
  if (-not $parentPublished) { throw 'parent DSH bootstrap did not publish a CB' }

  $stage = 'checkpoint_spawn'
  $checkpoint = Invoke-CoreOperation 'chain.checkpoint' $parentId @{}
  $socket = Invoke-CoreOperation 'socket.create' $parentId @{ checkpoint_id = $checkpoint.checkpoint_id; acl = @{ message = @(); signal = @(); spawn = @($actor); read = @() } }
  $spawn = Invoke-CoreOperation 'socket.spawn' $parentId @{ socket_id = $socket.socket_id; request_id = 'peer-supervisor-request-1'; message_id = 'peer-supervisor-task-1'; text = 'SIC_PEER_CHILD_TASK_SYNTHETIC. Complete the local fixture task, then report the result.'; lifecycle = 'P' }
  $childId = [string]$spawn.chain_id
  $taskId = [string]$spawn.task_id
  $null = Invoke-CoreOperation 'socket.acl.update' $parentId @{ socket_id = $socket.socket_id; acl = @{ message = @($childId); signal = @(); spawn = @($actor); read = @() } }
  $stage = 'peer_supervisor'
  if (-not $supervisorBinary) { throw 'peer supervisor binary is not available yet' }
  $claimId = 'peer-supervisor-host-claim-1'
  $prepareId = 'peer-supervisor-prepare-1'
  $launchId = 'peer-supervisor-launch-1'
  $replyId = 'peer-supervisor-reply-1'
  $policy = @{ context_id = 'main'; min_sources_per_wb = 1; max_sources_per_wb = 1; max_units_per_wb = 64; level_batch_sizes = @{}; max_level = 1; flush = $true; max_actions = 64; max_model_calls = 4; editor_mode = 'provider'; program_rules = @() }
  $dshCommand = Get-Command dsh -ErrorAction Stop
  $dshBinDirectory = Split-Path -Parent $dshCommand.Source
  $dshExecutable = [System.IO.Path]::GetFullPath((Join-Path $dshBinDirectory '..\..\..\..\DeepSeek Harness.exe'))
  $dshAsar = [System.IO.Path]::GetFullPath((Join-Path $dshBinDirectory '..\..\..\app.asar'))
  $dshCliJs = Join-Path $dshAsar 'dsh\node_modules\@deepseek-ai\dsh-desktop-host\lib\cli.js'
  if (-not (Test-Path -LiteralPath $dshExecutable -PathType Leaf) -or -not (Test-Path -LiteralPath $dshAsar -PathType Leaf)) { throw 'could not resolve the installed DSH headless runtime' }
  $dshHome = $env:DSH_HOME
  $dshProfile = 'sic-peer-supervisor-headless'
  Install-IsolatedProfile $dshProfile
  $stateDirectory = Join-Path $temp 'supervisor-state'; New-Item -ItemType Directory -Path $stateDirectory | Out-Null
  $supervisorConfigPath = Join-Path $temp 'peer-supervisor-config.json'
  $supervisorConfig = [ordered]@{
    db_path = $database
    core_base_url = "http://127.0.0.1:$corePort"
    bridge_base_url = "http://127.0.0.1:$observerPort/v1"
    bearer_env = 'SIC_CORE_TOKEN'
    actor = $actor
    requester_chain_id = $parentId
    peer_chain_id = $childId
    task_id = $taskId
    reply_socket_id = $socket.socket_id
    claim_id = $claimId
    prepare_id = $prepareId
    launch_id = $launchId
    reply_id = $replyId
    policy = $policy
    dsh_executable = $dshExecutable
    dsh_cli_js = $dshCliJs
    dsh_home = $dshHome
    dsh_profile = $dshProfile
    working_directory = $temp
    state_directory = $stateDirectory
    model_alias = 'sic-worker'
    max_tokens = 4096
    timeout_seconds = 120
    api_timeout_seconds = 120
    prices = @{ input = 0; output = 0; cache_read = 0; cache_write = 0 }
    allowed_tools = @('sic_fixture_read')
    fixture_read_tool = $true
    prompt = 'SIC_PEER_CHILD_TASK_SYNTHETIC. Use sic_fixture_read exactly once, then report its returned marker.'
  }
  [System.IO.File]::WriteAllText($supervisorConfigPath, ($supervisorConfig | ConvertTo-Json -Depth 16), [System.Text.UTF8Encoding]::new($false))
  $supervisorOutput = & $supervisorBinary run --config $supervisorConfigPath 2>$null | Out-String
  $supervisorExit = $LASTEXITCODE
  if ($supervisorExit -ne 0) { $supervisorErrorCode = Get-SafeSupervisorErrorCode $supervisorOutput; throw 'peer supervisor exited unsuccessfully' }
  try { $supervisorResult = $supervisorOutput | ConvertFrom-Json } catch { throw 'peer supervisor did not return a JSON summary' }
  $supervisorState = $supervisorResult.state
  $supervisorReplayed = $supervisorResult.replayed
  $peerAttemptObserved = -not [string]::IsNullOrWhiteSpace([string]$supervisorResult.attempt_id)
  $replyDelivered = $supervisorState -eq 'reply_delivered' -and
    $supervisorResult.task_id -eq $taskId -and $supervisorResult.peer_chain_id -eq $childId -and
    $peerAttemptObserved -and -not [string]::IsNullOrWhiteSpace([string]$supervisorResult.reply_message_id) -and
    $supervisorReplayed -eq $false
  if (-not $replyDelivered) { throw 'peer supervisor summary did not prove a first successful peer reply' }

  try { $observerStatus = Invoke-RestMethod -Uri "http://127.0.0.1:$observerPort/status" -TimeoutSec 3 -NoProxy } catch {}
  try { $providerStatus = Invoke-RestMethod -Uri "http://127.0.0.1:$mockPort/status" -TimeoutSec 3 -NoProxy } catch {}
  if (-not $observerStatus -or -not $providerStatus) { throw 'could not read mock/bridge counts before supervisor replay' }
  $providerCountBeforeReplay = @($providerStatus.calls).Count
  $fixtureCallsBeforeReplay = @($providerStatus.calls | Where-Object { $_.responseKind -eq 'peer_fixture_tool_call' }).Count
  $bridgeChatCountBeforeReplay = @($observerStatus.observations | Where-Object { $_.route -eq '/v1/chat/completions' }).Count
  $supervisorReplayOutput = & $supervisorBinary run --config $supervisorConfigPath 2>$null | Out-String
  $supervisorReplayExit = $LASTEXITCODE
  if ($supervisorReplayExit -ne 0) { $supervisorErrorCode = Get-SafeSupervisorErrorCode $supervisorReplayOutput; throw 'peer supervisor replay exited unsuccessfully' }
  try { $supervisorReplayResult = $supervisorReplayOutput | ConvertFrom-Json } catch { throw 'peer supervisor replay did not return a JSON summary' }
  try { $observerAfterReplay = Invoke-RestMethod -Uri "http://127.0.0.1:$observerPort/status" -TimeoutSec 3 -NoProxy } catch {}
  try { $providerAfterReplay = Invoke-RestMethod -Uri "http://127.0.0.1:$mockPort/status" -TimeoutSec 3 -NoProxy } catch {}
  $providerCountAfterReplay = if ($providerAfterReplay) { @($providerAfterReplay.calls).Count } else { -1 }
  $fixtureCallsAfterReplay = if ($providerAfterReplay) { @($providerAfterReplay.calls | Where-Object { $_.responseKind -eq 'peer_fixture_tool_call' }).Count } else { -1 }
  $bridgeChatCountAfterReplay = if ($observerAfterReplay) { @($observerAfterReplay.observations | Where-Object { $_.route -eq '/v1/chat/completions' }).Count } else { -1 }
  $supervisorReplaySafe = $supervisorReplayResult.state -eq 'reply_delivered' -and $supervisorReplayResult.replayed -eq $true -and
    $supervisorReplayResult.task_id -eq $taskId -and $supervisorReplayResult.peer_chain_id -eq $childId -and
    $supervisorReplayResult.attempt_id -eq $supervisorResult.attempt_id -and
    $supervisorReplayResult.reply_message_id -eq $supervisorResult.reply_message_id -and
    $providerCountAfterReplay -eq $providerCountBeforeReplay -and $fixtureCallsAfterReplay -eq $fixtureCallsBeforeReplay -and
    $bridgeChatCountAfterReplay -eq $bridgeChatCountBeforeReplay
  if (-not $supervisorReplaySafe) { throw 'supervisor replay was not idempotent or caused additional Provider/bridge calls' }

  $fixtureResultObserved = $false
  if ($providerStatus) { $fixtureResultObserved = @($providerStatus.calls | Where-Object { $_.responseKind -eq 'peer_final_text_after_fixture' -and $_.fixtureResultObserved }).Count -eq 1 }
  if ($observerStatus) {
    $allChats = @($observerStatus.observations | Where-Object { $_.route -eq '/v1/chat/completions' })
    $allChatsUseFixtureOnly = $allChats.Count -ge 2 -and @($allChats | Where-Object { $_.toolNames.Count -ne 1 -or $_.toolNames[0] -ne 'sic_fixture_read' }).Count -eq 0
  }
  $parentBind = @($observerStatus.observations | Where-Object { $_.route -eq '/v1/sic/bind' })[0]
  $peerBind = @($observerStatus.observations | Where-Object { $_.route -eq '/v1/sic/bind' } | Where-Object { $_.bindChainDigest -eq (Get-IdentityDigest $childId) })[0]
  $parentChats = @($observerStatus.observations | Where-Object { $_.route -eq '/v1/chat/completions' -and $_.bridgeSessionDigest -eq $parentBind.requestSessionDigest })
  $peerChats = @($observerStatus.observations | Where-Object { $_.route -eq '/v1/chat/completions' -and $_.bridgeSessionDigest -eq $peerBind.requestSessionDigest })
  $bindsMatchRoutes = @($observerStatus.observations | Where-Object { $_.route -eq '/v1/sic/bind' }).Count -eq 2 -and
    $parentBind.bindChainDigest -eq (Get-IdentityDigest $parentId) -and $peerBind.bindChainDigest -eq (Get-IdentityDigest $childId) -and
    $parentBind.requestSessionDigest -ne $peerBind.requestSessionDigest -and $parentChats.Count -ge 1 -and $peerChats.Count -ge 1
  if (-not $fixtureResultObserved -or -not $allChatsUseFixtureOnly -or -not $bindsMatchRoutes) { throw 'peer supervisor DSH fixture roundtrip was not observed or exceeded its tool allowlist' }

  $peerInbox = Invoke-CoreOperation 'socket.inbox' $childId @{}
  $peerInboxPublished = @($peerInbox.messages | Where-Object { $_.message.message_id -eq 'peer-supervisor-task-1' -and $_.consumed_by_cut }).Count -eq 1
  if (-not $peerInboxPublished) { throw 'supervisor did not publish the spawn task Message into a peer CB' }
  $replyInbox = Invoke-CoreOperation 'socket.inbox' $parentId @{}
  $replyEntry = @($replyInbox.messages | Where-Object { $_.message.message_id -eq $supervisorResult.reply_message_id -and $_.message.phase_id -eq 'peer-task-reply' })[0]
  if (-not $replyEntry) { throw 'peer.task.reply receipt is missing from requester inbox' }
  $returnedText = [string]$replyEntry.message.text
  if ([string]::IsNullOrWhiteSpace($returnedText)) { throw 'peer.task.reply has no actual Worker response text' }
  $replyDelivered = $replyDelivered -and @($replyInbox.messages | Where-Object { $_.message.message_id -eq $supervisorResult.reply_message_id }).Count -eq 1
  if (-not $replyDelivered) { throw 'supervisor reply is not present exactly once in requester inbox' }

  $stage = 'requester_cb_adoption'
  $null = Invoke-CoreOperation 'source.ingest' $parentId @{ request = @{ messages = @(@{ role = 'user'; content = 'Review the synthetic peer response.' }) } }
  $parentReturn = Invoke-ProgramFixture $parentId
  $parentCut = Invoke-CoreOperation 'cut.get' $parentId @{}
  $parentInbox = Invoke-CoreOperation 'socket.inbox' $parentId @{}
  $messageAdopted = [bool]$parentReturn.completed -and @($parentReturn.published).Count -ge 1 -and
    @($parentCut.view.cbs | Where-Object { $_.body.Contains($returnedText) }).Count -ge 1 -and
    @($parentInbox.messages | Where-Object { $_.message.message_id -eq $supervisorResult.reply_message_id -and $_.consumed_by_cut }).Count -eq 1
  if (-not $messageAdopted) { throw 'requester CB did not adopt the peer Worker reply' }
  $stateJson = & python (Join-Path $plugin 'test\peer-fixture-state-summary.py') $database $parentId $childId
  if ($LASTEXITCODE -ne 0) { throw 'could not read final redacted peer state' }
  $stateSummary = $stateJson | ConvertFrom-Json
  if ($stateSummary.parent.protected_digest -ne $stateSummary.child.protected_digest) { throw 'child protected prefix changed after its DSH turn' }
  $passed = $true; $stage = 'complete'
}
catch {
  $failure = switch ($stage) {
    'requester_bootstrap' { 'requester_bootstrap_failed' }
    'checkpoint_spawn' { 'checkpoint_or_spawn_failed' }
    'peer_supervisor' { 'peer_supervisor_failed' }
    'requester_cb_adoption' { 'requester_cb_adoption_failed' }
    default { 'setup_failed' }
  }
  if ($database -and $parentId -and $childId -and (Test-Path -LiteralPath $database)) {
    try { $stateJson = & python (Join-Path $plugin 'test\peer-fixture-state-summary.py') $database $parentId $childId; if ($LASTEXITCODE -eq 0) { $stateSummary = $stateJson | ConvertFrom-Json } } catch {}
  }
  if ($observerPort) { try { $observerStatus = Invoke-RestMethod -Uri "http://127.0.0.1:$observerPort/status" -TimeoutSec 3 -NoProxy } catch {} }
  if ($mockPort) { try { $providerStatus = Invoke-RestMethod -Uri "http://127.0.0.1:$mockPort/status" -TimeoutSec 3 -NoProxy } catch {} }
}
finally {
  if ($core -and -not $core.HasExited) { Stop-Process -Id $core.Id -Force -ErrorAction SilentlyContinue }
  if ($observer -and -not $observer.HasExited) { Stop-Process -Id $observer.Id -Force -ErrorAction SilentlyContinue }
  if ($mock -and -not $mock.HasExited) { Stop-Process -Id $mock.Id -Force -ErrorAction SilentlyContinue }
  foreach ($name in @('DSH_HOME', 'SIC_CORE_TOKEN', 'SIC_MOCK_PROVIDER_KEY', 'SIC_RUST_UPSTREAM', 'DSH_TOOLS_MODE')) {
    if ($null -eq $saved[$name]) { Remove-Item -Path ("Env:" + $name) -ErrorAction SilentlyContinue } else { [Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process') }
  }
}

$observations = if ($observerStatus) { @($observerStatus.observations) } else { @() }
$safeReport = [ordered]@{
  result = if ($passed) { 'passed' } else { 'stopped' }
  stage = $stage; failure = $failure; dshVersion = (dsh --version | Out-String).Trim()
  topology = 'requester DSH bootstrap -> Core checkpoint/Socket/spawn + ACL -> sic-peer-supervisor run -> DSH peer bind/fixture -> verified Core Worker attempt and peer.task.reply -> requester ProgramFixture CB adoption'
  orchestration = 'one host-started supervisor process over a local mock Provider; not production concurrency acceptance'
  requesterPublished = $parentPublished; peerInboxPublished = $peerInboxPublished
  requesterDshExitCode = $parentExit; supervisorExitCode = $supervisorExit
  supervisorState = $supervisorState; supervisorReplay = $supervisorReplayed
  supervisorErrorCode = $supervisorErrorCode
  supervisorReplayVerified = $supervisorReplaySafe
  peerWorkerAttemptObserved = $peerAttemptObserved
  protectedPrefixSummary = if ($stateSummary) { [ordered]@{ requesterCount = $stateSummary.parent.protected_count; peerCount = $stateSummary.child.protected_count; requesterDigest = $stateSummary.parent.protected_digest; peerDigest = $stateSummary.child.protected_digest; identical = $stateSummary.parent.protected_digest -eq $stateSummary.child.protected_digest } } else { $null }
  bindAndChat = @($observations | ForEach-Object { [ordered]@{ route = $_.route; sessionDigest = $_.requestSessionDigest; chainDigest = $_.bindChainDigest; chatSessionDigest = $_.bridgeSessionDigest; status = $_.responseStatus; errorCode = $_.errorCode; toolNames = $_.toolNames } })
  providerCallKinds = if ($providerStatus) { @($providerStatus.calls | ForEach-Object { $_.responseKind }) } else { @() }
  allDSHChatCallsAllowedOnlyFixture = $allChatsUseFixtureOnly
  fixtureResultObservedByProvider = $fixtureResultObserved
  peerReplyDeliveredUnderSocketAcl = $replyDelivered
  requesterAdoptedPeerReply = $messageAdopted
  rawPromptsOrMessagesInReport = $false; credentialsInReport = $false; externalModelOrApplicationToolUsed = $false
}
$safeReport | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $reportPath -Encoding utf8
if (Test-Path -LiteralPath $temp) {
  $resolved = (Resolve-Path -LiteralPath $temp -ErrorAction Stop).Path; $item = Get-Item -LiteralPath $resolved -Force -ErrorAction Stop
  if ($resolved -ne $temp -or -not $resolved.StartsWith($tempRoot + [System.IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase) -or -not $item.Name.StartsWith('sic-peer-supervisor-headless-', [StringComparison]::Ordinal) -or ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint)) { throw 'Refusing to remove a fixture directory outside the verified target' }
  Remove-Item -LiteralPath $resolved -Recurse -Force -ErrorAction Stop
}
if ($passed) { Write-Output "PASS: local sic-peer-supervisor DSH/mock run delivered a verified peer reply and requester ProgramFixture adopted it. Report: $reportPath" }
else { Write-Output "STOP: $failure at $stage; no external model or application tool was used. Report: $reportPath"; exit 1 }
