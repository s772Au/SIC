$ErrorActionPreference = 'Stop'
$coreRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$plugin = Split-Path $PSScriptRoot -Parent
$binary = @((Join-Path $coreRoot 'target\debug\sic-core-rust.exe'), (Join-Path $coreRoot 'target\release\sic-core-rust.exe'), (Join-Path $coreRoot 'release\sic-core-rust.exe')) |
    Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
if (-not $binary) { throw 'Provide release\sic-core-rust.exe or build the Rust Core binary in target.' }

$actor = 'synthetic-peer-host'
$saved = @{}
foreach ($name in @('DSH_HOME', 'SIC_CORE_TOKEN', 'SIC_MOCK_PROVIDER_KEY', 'SIC_RUST_UPSTREAM', 'DSH_TOOLS_MODE')) {
  $saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}
$tempRoot = [System.IO.Path]::GetFullPath($env:TEMP).TrimEnd([System.IO.Path]::DirectorySeparatorChar)
$temp = [System.IO.Path]::GetFullPath((Join-Path $tempRoot ('sic-peer-task-bootstrap-headless-' + [guid]::NewGuid().ToString('N'))))
if (-not $temp.StartsWith($tempRoot + [System.IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw 'Fixture directory is outside the intended temporary directory' }
$null = New-Item -ItemType Directory -Force -Path $temp
$reportPath = Join-Path $tempRoot ('sic-peer-task-bootstrap-redacted-' + [guid]::NewGuid().ToString('N') + '.json')
$mock = $null; $observer = $null; $core = $null
$providerStatus = $null; $observerStatus = $null; $stateSummary = $null
$parentPublished = $false; $childInboxPublished = $false; $fixtureResultObserved = $false
$messageAccepted = $false; $messageAdopted = $false; $replyReplayDeduplicated = $false
$peerAttemptObserved = $false; $parentExit = $null; $childExit = $null
$peerTaskClaimed = $false; $peerTaskPrepared = $false; $peerPrepareEditorObserved = $false
$peerPrepareState = $null; $peerPrepareReady = $false; $peerPrepareProviderCalled = $false
$peerPrepareDeliveryVerified = $false; $peerPrepareContentStatus = $null
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
  $database = Join-Path $temp 'peer-task-bootstrap.sqlite'; & $binary init $database *> $null
  if ($LASTEXITCODE -ne 0) { throw 'could not initialize isolated Rust database' }
  $probe = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0); $probe.Start(); $corePort = $probe.LocalEndpoint.Port; $probe.Stop()
  $coreOut = Join-Path $temp 'core.out'; $coreErr = Join-Path $temp 'core.err'
  $coreArgs = @('serve', $database, "127.0.0.1:$corePort", $actor, 'SIC_CORE_TOKEN', '--allow-model', '--model-config', $modelPath, '--dsh-alias', 'sic-worker')
  $core = Start-Process -FilePath $binary -ArgumentList $coreArgs -WorkingDirectory $temp -WindowStyle Hidden -PassThru -RedirectStandardOutput $coreOut -RedirectStandardError $coreErr
  $headers = @{ Authorization = "Bearer $env:SIC_CORE_TOKEN" }; $ready = $false
  for ($i = 0; $i -lt 150; $i++) { if ($core.HasExited) { throw 'isolated Core exited during startup' }; try { $null = Invoke-RestMethod -Uri "http://127.0.0.1:$corePort/health" -Headers $headers -TimeoutSec 1 -NoProxy; $ready = $true; break } catch { Start-Sleep -Milliseconds 100 } }
  if (-not $ready) { throw 'isolated Core did not become ready' }

  $stage = 'parent_bootstrap'
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
  $parentOutput = (& dsh --profile sic-peer-parent-headless --patch $parentPatch 'SIC_PEER_PARENT_BOOTSTRAP_SYNTHETIC. Reply briefly that the synthetic parent is ready.' 2>&1 | Out-String)
  $parentExit = $LASTEXITCODE
  if ($parentExit -ne 0 -or -not $parentOutput.Contains('SIC_PARENT_BOOTSTRAP_READY')) { throw 'parent DSH bootstrap failed before publishing its protected prefix' }
  $parentState = Invoke-CoreOperation 'chain.get' $parentId @{}
  $parentPublished = [int]$parentState.cut_seq -ge 1
  if (-not $parentPublished) { throw 'parent DSH bootstrap did not publish a CB' }

  $stage = 'peer_task_claim_prepare'
  $checkpoint = Invoke-CoreOperation 'chain.checkpoint' $parentId @{}
  $socket = Invoke-CoreOperation 'socket.create' $parentId @{ checkpoint_id = $checkpoint.checkpoint_id; acl = @{ message = @(); signal = @(); spawn = @($actor); read = @() } }
  $spawn = Invoke-CoreOperation 'socket.spawn' $parentId @{ socket_id = $socket.socket_id; request_id = 'peer-bootstrap-request-1'; message_id = 'peer-bootstrap-task-1'; text = 'SIC_PEER_CHILD_TASK_SYNTHETIC. Complete the local fixture task, then report the result.'; lifecycle = 'P' }
  $childId = [string]$spawn.chain_id
  $taskId = [string]$spawn.task_id
  $null = Invoke-CoreOperation 'socket.acl.update' $parentId @{ socket_id = $socket.socket_id; acl = @{ message = @($childId); signal = @(); spawn = @($actor); read = @() } }
  $null = Invoke-CoreOperation 'config.update' $childId @{ editor_feedback = @{ max_corrections = 0; feedback_chars = 8192 } }
  $claimId = 'peer-fixture-host-claim-1'
  $claim = Invoke-CoreOperation 'peer.task.claim' $parentId @{ task_id = $taskId; claim_id = $claimId }
  $peerTaskClaimed = $claim.state -eq 'claimed' -and $claim.execution_permit -eq $true -and $claim.revision -eq 1 -and $claim.replayed -eq $false
  if (-not $peerTaskClaimed) { throw 'peer.task.claim did not issue a first-execution permit' }
  $policy = @{ context_id = 'main'; min_sources_per_wb = 1; max_sources_per_wb = 1; max_units_per_wb = 64; level_batch_sizes = @{}; max_level = 1; flush = $true; max_actions = 64; max_model_calls = 2; editor_mode = 'provider'; program_rules = @() }
  $preparePayload = @{ task_id = $taskId; claim_id = $claimId; expected_revision = 1; prepare_id = 'peer-fixture-prepare-1'; policy = $policy }
  $prepare = Invoke-CoreOperation 'peer.task.prepare' $parentId $preparePayload
  $prepareResult = $prepare
  $delivery = $prepareResult.evidence.spawn_message_delivery
  $peerPrepareState = $prepareResult.state
  $peerPrepareReady = $prepareResult.ready_for_dsh -eq $true
  $peerPrepareProviderCalled = $prepareResult.evidence.provider_called -eq $true
  $peerPrepareDeliveryVerified = $delivery.verified -eq $true
  $peerPrepareContentStatus = $delivery.content_retained.status
  $peerTaskPrepared = $prepareResult.state -eq 'prepared' -and $prepareResult.ready_for_dsh -eq $true -and $prepareResult.execution_started -eq $true -and
    $prepareResult.evidence.provider_called -eq $true -and $prepareResult.evidence.worker_run -eq $false -and
    $prepareResult.evidence.external_tools_executed -eq $false -and $prepareResult.evidence.requester_message_delivered -eq $false
  $childInboxPublished = $delivery.verified -eq $true -and $delivery.content_retained.status -eq 'verified'
  if (-not $peerTaskPrepared -or -not $childInboxPublished) { throw 'bounded peer.task.prepare did not prove Editor-only inbox publication' }
  $providerBeforeDsh = Invoke-RestMethod -Uri "http://127.0.0.1:$mockPort/status" -TimeoutSec 3 -NoProxy
  $peerPrepareEditorObserved = @($providerBeforeDsh.calls | Where-Object { $_.callIndex -eq 3 -and $_.responseKind -eq 'peer_editor' }).Count -eq 1
  if (-not $peerPrepareEditorObserved) { throw 'mock Provider did not observe the bounded peer Editor call' }
  $stateJson = & python (Join-Path $plugin 'test\peer-fixture-state-summary.py') $database $parentId $childId
  if ($LASTEXITCODE -ne 0) { throw 'could not compare inherited protected-prefix digests' }
  $stateSummary = $stateJson | ConvertFrom-Json
  $protectedPrefixInherited = $stateSummary.parent.protected_count -gt 0 -and $stateSummary.parent.protected_digest -eq $stateSummary.child.protected_digest
  if (-not $protectedPrefixInherited) { throw 'child did not inherit the parent protected prefix exactly' }

  $stage = 'child_dsh_fixture'
  Install-IsolatedProfile 'sic-peer-child-headless'
  $childPatch = Join-Path $temp 'child.patch.yml'; Write-DshPatch $childPatch $childId $observerPort
  $childOutput = (& dsh --profile sic-peer-child-headless --patch $childPatch 'SIC_PEER_CHILD_TASK_SYNTHETIC. Use sic_fixture_read exactly once, then report its returned marker.' 2>&1 | Out-String)
  $childExit = $LASTEXITCODE
  try { $observerStatus = Invoke-RestMethod -Uri "http://127.0.0.1:$observerPort/status" -TimeoutSec 3 -NoProxy } catch {}
  try { $providerStatus = Invoke-RestMethod -Uri "http://127.0.0.1:$mockPort/status" -TimeoutSec 3 -NoProxy } catch {}
  $fixtureResultObserved = $false
  if ($providerStatus) { $fixtureResultObserved = @($providerStatus.calls | Where-Object { $_.responseKind -eq 'peer_final_text_after_fixture' -and $_.fixtureResultObserved }).Count -eq 1 }
  $allChatsUseFixtureOnly = $false
  if ($observerStatus) {
    $allChats = @($observerStatus.observations | Where-Object { $_.route -eq '/v1/chat/completions' })
    $allChatsUseFixtureOnly = $allChats.Count -ge 2 -and @($allChats | Where-Object { $_.toolNames.Count -ne 1 -or $_.toolNames[0] -ne 'sic_fixture_read' }).Count -eq 0
  }
  $parentBind = @($observerStatus.observations | Where-Object { $_.route -eq '/v1/sic/bind' })[0]
  $childBind = @($observerStatus.observations | Where-Object { $_.route -eq '/v1/sic/bind' })[1]
  $parentChats = @($observerStatus.observations | Where-Object { $_.route -eq '/v1/chat/completions' -and $_.bridgeSessionDigest -eq $parentBind.requestSessionDigest })
  $childChats = @($observerStatus.observations | Where-Object { $_.route -eq '/v1/chat/completions' -and $_.bridgeSessionDigest -eq $childBind.requestSessionDigest })
  $bindsMatchRoutes = @($observerStatus.observations | Where-Object { $_.route -eq '/v1/sic/bind' }).Count -eq 2 -and
    $parentBind.bindChainDigest -eq (Get-IdentityDigest $parentId) -and $childBind.bindChainDigest -eq (Get-IdentityDigest $childId) -and
    $parentBind.requestSessionDigest -ne $childBind.requestSessionDigest -and $parentChats.Count -ge 1 -and $childChats.Count -ge 1
  if ($childExit -ne 0 -or -not $childOutput.Contains('SIC_RUST_DSH_MOCK_ROUNDTRIP_OK') -or -not $fixtureResultObserved -or -not $allChatsUseFixtureOnly -or -not $bindsMatchRoutes) { throw 'child DSH did not complete the bound fixture roundtrip' }

  $stage = 'peer_reply_adoption'
  $peerState = Invoke-CoreOperation 'chain.get' $childId @{}
  $workerAttempts = @($peerState.events | Where-Object { $_.type -eq 'worker_attempt' })
  if ($workerAttempts.Count -eq 0) { throw 'peer route has no real Core Worker attempt to reply from' }
  $latestAttemptId = [string]$workerAttempts[-1].record_id
  $peerAttemptObserved = -not [string]::IsNullOrWhiteSpace($latestAttemptId)
  if (-not $peerAttemptObserved) { throw 'peer latest Worker attempt has no stable ID' }
  $replyPayload = @{ task_id = $taskId; reply_id = 'peer-fixture-reply-1'; expected_attempt_id = $latestAttemptId; socket_id = $socket.socket_id }
  $reply = Invoke-CoreOperation 'peer.task.reply' $parentId $replyPayload
  $messageAccepted = $reply.state -eq 'reply_delivered' -and $reply.message_delivered -eq $true -and
    $reply.peer_chain_id -eq $childId -and $reply.requester_chain_id -eq $parentId -and $reply.attempt_id -eq $latestAttemptId
  if (-not $messageAccepted) { throw 'peer.task.reply did not deliver the latest verified Worker result' }
  $replyInbox = Invoke-CoreOperation 'socket.inbox' $parentId @{}
  $replyEntry = @($replyInbox.messages | Where-Object { $_.message.message_id -eq $reply.receipt.message_id -and $_.message.phase_id -eq 'peer-task-reply' })[0]
  if (-not $replyEntry) { throw 'peer.task.reply receipt is missing from requester inbox' }
  $returnedText = [string]$replyEntry.message.text
  if ([string]::IsNullOrWhiteSpace($returnedText)) { throw 'peer.task.reply has no actual Worker response text' }

  $replyReplay = Invoke-CoreOperation 'peer.task.reply' $parentId $replyPayload
  $messageAccepted = $messageAccepted -and $replyReplay.replayed -eq $true
  $replayInbox = Invoke-CoreOperation 'socket.inbox' $parentId @{}
  $replyReceiptCount = @($replayInbox.messages | Where-Object { $_.message.message_id -eq $reply.receipt.message_id -and $_.message.phase_id -eq 'peer-task-reply' }).Count
  $replyReplayDeduplicated = $replyReplay.replayed -eq $true -and $replyReceiptCount -eq 1
  if (-not $replyReplayDeduplicated) { throw 'peer.task.reply replay was not deduplicated to one Socket Message' }

  $null = Invoke-CoreOperation 'source.ingest' $parentId @{ request = @{ messages = @(@{ role = 'user'; content = 'Review the synthetic peer response.' }) } }
  $parentReturn = Invoke-ProgramFixture $parentId
  $parentCut = Invoke-CoreOperation 'cut.get' $parentId @{}
  $parentInbox = Invoke-CoreOperation 'socket.inbox' $parentId @{}
  $messageAdopted = [bool]$parentReturn.completed -and @($parentReturn.published).Count -ge 1 -and
    @($parentCut.view.cbs | Where-Object { $_.body.Contains($returnedText) }).Count -ge 1 -and
    @($parentInbox.messages | Where-Object { $_.message.message_id -eq $reply.receipt.message_id -and $_.consumed_by_cut }).Count -eq 1
  if (-not $messageAdopted) { throw 'requester CB did not adopt the peer Worker reply' }
  $stateJson = & python (Join-Path $plugin 'test\peer-fixture-state-summary.py') $database $parentId $childId
  if ($LASTEXITCODE -ne 0) { throw 'could not read final redacted peer state' }
  $stateSummary = $stateJson | ConvertFrom-Json
  if ($stateSummary.parent.protected_digest -ne $stateSummary.child.protected_digest) { throw 'child protected prefix changed after its DSH turn' }
  $passed = $true; $stage = 'complete'
}
catch {
  $failure = switch ($stage) {
    'parent_bootstrap' { 'parent_bootstrap_failed' }
    'peer_task_claim_prepare' { 'peer_task_claim_or_prepare_failed' }
    'child_dsh_fixture' { 'child_bind_or_fixture_failed' }
    'peer_reply_adoption' { 'peer_reply_adoption_failed' }
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
  topology = 'host-created requester route -> DSH bootstrap/published prefix -> Core checkpoint/Socket -> host peer.task.claim/prepare (bounded Provider Editor only) -> DSH peer route bind/fixture -> Core Worker attempt -> peer.task.reply over authorized Socket -> requester ProgramFixture adoption'
  orchestration = 'host-supervised peer-task journal and turn sequence; not an autonomous concurrent peer runner'
  requesterPublished = $parentPublished; peerInboxPublished = $childInboxPublished
  requesterDshExitCode = $parentExit; peerDshExitCode = $childExit
  peerTaskClaimed = $peerTaskClaimed; peerTaskPrepared = $peerTaskPrepared; peerPrepareEditorObserved = $peerPrepareEditorObserved
  peerPrepareState = $peerPrepareState; peerPrepareReadyForDsh = $peerPrepareReady; peerPrepareProviderCalled = $peerPrepareProviderCalled
  peerPrepareDeliveryVerified = $peerPrepareDeliveryVerified; peerPrepareContentStatus = $peerPrepareContentStatus
  protectedPrefixSummary = if ($stateSummary) { [ordered]@{ requesterCount = $stateSummary.parent.protected_count; peerCount = $stateSummary.child.protected_count; requesterDigest = $stateSummary.parent.protected_digest; peerDigest = $stateSummary.child.protected_digest; identical = $stateSummary.parent.protected_digest -eq $stateSummary.child.protected_digest } } else { $null }
  bindAndChat = @($observations | ForEach-Object { [ordered]@{ route = $_.route; sessionDigest = $_.requestSessionDigest; chainDigest = $_.bindChainDigest; chatSessionDigest = $_.bridgeSessionDigest; status = $_.responseStatus; errorCode = $_.errorCode; toolNames = $_.toolNames } })
  providerCallKinds = if ($providerStatus) { @($providerStatus.calls | ForEach-Object { $_.responseKind }) } else { @() }
  allDSHChatCallsAllowedOnlyFixture = $allChatsUseFixtureOnly
  fixtureResultObservedByProvider = $fixtureResultObserved
  peerWorkerAttemptObserved = $peerAttemptObserved
  peerReplyDeliveredUnderSocketAcl = $messageAccepted; peerReplyReplayDeduplicated = $replyReplayDeduplicated
  requesterAdoptedPeerReply = $messageAdopted
  rawPromptsOrMessagesInReport = $false; credentialsInReport = $false; externalModelOrApplicationToolUsed = $false
}
$safeReport | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $reportPath -Encoding utf8
if (Test-Path -LiteralPath $temp) {
  $resolved = (Resolve-Path -LiteralPath $temp -ErrorAction Stop).Path; $item = Get-Item -LiteralPath $resolved -Force -ErrorAction Stop
  if ($resolved -ne $temp -or -not $resolved.StartsWith($tempRoot + [System.IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase) -or -not $item.Name.StartsWith('sic-peer-task-bootstrap-headless-', [StringComparison]::Ordinal) -or ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint)) { throw 'Refusing to remove a fixture directory outside the verified target' }
  Remove-Item -LiteralPath $resolved -Recurse -Force -ErrorAction Stop
}
if ($passed) { Write-Output "PASS: local peer task claim/prepare plus DSH fixture; latest Core Worker result returned through peer.task.reply, replay deduplicated, and requester CB adopted it. No autonomous peer runner. Report: $reportPath" }
else { Write-Output "STOP: $failure at $stage; no external model or application tool was used. Report: $reportPath"; exit 1 }
