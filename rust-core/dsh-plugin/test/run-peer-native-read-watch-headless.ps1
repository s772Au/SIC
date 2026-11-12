param(
  [Parameter(Mandatory = $true)][string]$WatchBinaryPath,
  [string]$CoreBinaryPath,
  [string]$PluginDirectory,
  [string]$WatchConfigTemplatePath,
  [int]$WatchTimeoutSeconds = 180,
  [switch]$VerifySignalWait,
  [switch]$VerifySignalCursor,
  [switch]$VerifyMessageWait
)

# Requires PowerShell 7 (`pwsh`): Invoke-RestMethod -NoProxy is used for loopback calls.
# Independent local end-to-end acceptance. The bundled template follows the
# watch CLI's top-level WatchFile and nests the existing SupervisorConfig.
$ErrorActionPreference = 'Stop'
if ($VerifySignalCursor) { $VerifySignalWait = $true }
$coreRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$plugin = if ($PluginDirectory) { (Resolve-Path -LiteralPath $PluginDirectory -ErrorAction Stop).Path } else { Split-Path $PSScriptRoot -Parent }
$templateDefault = Join-Path $PSScriptRoot 'peer-supervisor-watch-config.template.json'
if ([string]::IsNullOrWhiteSpace($WatchConfigTemplatePath)) { $WatchConfigTemplatePath = $templateDefault }
$coreBinary = if ($CoreBinaryPath) { [System.IO.Path]::GetFullPath($CoreBinaryPath) } else {
  @((Join-Path $coreRoot 'target\debug\sic-core-rust.exe'), (Join-Path $coreRoot 'target\release\sic-core-rust.exe'), (Join-Path $coreRoot 'release\sic-core-rust.exe')) |
    Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
}
if ($coreBinary -and -not (Test-Path -LiteralPath $coreBinary -PathType Leaf)) { throw 'Core binary path does not exist.' }
if (-not $coreBinary) { throw 'Provide release\sic-core-rust.exe or build the Rust Core binary in target.' }
$watchBinary = [System.IO.Path]::GetFullPath($WatchBinaryPath)
$watchTemplatePath = [System.IO.Path]::GetFullPath($WatchConfigTemplatePath)
if (-not (Test-Path -LiteralPath $watchBinary -PathType Leaf)) { throw 'Watch binary path does not exist.' }
if (-not (Test-Path -LiteralPath $watchTemplatePath -PathType Leaf)) { throw 'Watch JSON template path does not exist.' }
if ($WatchTimeoutSeconds -lt 10 -or $WatchTimeoutSeconds -gt 3600) { throw 'Watch timeout must be 10..3600 seconds.' }

$actor = 'synthetic-peer-host'
$saved = @{}
foreach ($name in @('DSH_HOME', 'SIC_CORE_TOKEN', 'SIC_MOCK_PROVIDER_KEY', 'SIC_RUST_UPSTREAM', 'DSH_TOOLS_MODE', 'SIC_FIXTURE_SIGNAL_WAIT', 'SIC_FIXTURE_SIGNAL_CURSOR')) {
  $saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}
$tempRoot = [System.IO.Path]::GetFullPath($env:TEMP).TrimEnd([System.IO.Path]::DirectorySeparatorChar)
$tempName = 'sic-peer-supervisor-watch-' + [guid]::NewGuid().ToString('N')
$temp = [System.IO.Path]::GetFullPath((Join-Path $tempRoot $tempName))
if (-not $temp.StartsWith($tempRoot + [System.IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw 'Fixture directory is outside the intended temporary directory.' }
$null = New-Item -ItemType Directory -Force -Path $temp
$reportPath = Join-Path ([System.IO.Path]::GetTempPath()) ('sic-peer-native-read-watch-redacted-' + [guid]::NewGuid().ToString('N') + '.json')
$mock = $null; $observer = $null; $core = $null; $watch = $null
$mockPort = $null; $observerPort = $null; $corePort = $null
$mockStatus = $null; $observerStatus = $null; $stateSummary = $null
$parentPublished = $false; $spawnRecorded = $false; $peerInboxPublished = $false; $fixtureResultObserved = $false
$peerReplyDelivered = $false; $requesterAdoptedReply = $false; $watchExit = $null
$sicStatusVerified = $false
$nativeEventVerified = $false; $roleAccountingVerified = $false
$signalSent = $false; $signalWaitVerified = $false; $messageWaitVerified = $false; $messageCursor = [UInt64]0; $peerSocket = $null
$stage = 'setup'; $failure = $null; $passed = $false

function Invoke-CoreOperation([string]$Operation, [string]$Chain, $Payload) {
  $context = @{ actor = $actor; allow_write = $true; allow_model = $true; allow_revoke = $false }
  if (-not [string]::IsNullOrWhiteSpace($Chain)) { $context.chain_id = $Chain }
  $body = @{ operation = $Operation; context = $context; payload = $Payload } | ConvertTo-Json -Depth 16 -Compress
  try {
    return (Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$corePort/api/v2/operations" -Headers $headers -ContentType 'application/json' -Body $body -TimeoutSec 30 -NoProxy).result
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
      - read
    fixtureReadTool: false
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

function ConvertTo-JsonScalar($Value) { return ConvertTo-Json -InputObject $Value -Compress -Depth 16 }

function Write-WatchConfig([string]$TemplatePath, [string]$Destination, [hashtable]$Values) {
  $text = Get-Content -LiteralPath $TemplatePath -Raw
  foreach ($entry in $Values.GetEnumerator()) {
    # Template tokens must occupy a JSON value position without surrounding
    # quotes, e.g. { "db_path": @@DB_PATH@@ }. Objects/numbers remain JSON.
    $text = $text.Replace($entry.Key, (ConvertTo-JsonScalar $entry.Value))
  }
  if ($text -match '@@[A-Z0-9_]+@@') { throw 'Watch JSON template contains an unresolved fixture token.' }
  $parsed = $text | ConvertFrom-Json -ErrorAction Stop
  $expectedTop = @('enabled', 'namespace', 'poll_interval_ms', 'max_tasks_per_cycle', 'page_size', 'supervisor') | Sort-Object
  $actualTop = @($parsed.PSObject.Properties.Name) | Sort-Object
  if (Compare-Object -ReferenceObject $expectedTop -DifferenceObject $actualTop -CaseSensitive) { throw 'Watch JSON top-level keys do not match the CLI contract.' }
  if ($parsed.enabled -ne $true -or $parsed.max_tasks_per_cycle -ne 1 -or $parsed.poll_interval_ms -lt 500 -or $parsed.page_size -lt 1 -or $parsed.page_size -gt 100) { throw 'Watch settings do not satisfy the local one-task fixture bounds.' }
  $expectedSupervisor = @('db_path','core_base_url','bridge_base_url','bearer_env','actor','requester_chain_id','peer_chain_id','task_id','reply_socket_id','claim_id','prepare_id','launch_id','reply_id','policy','dsh_executable','dsh_cli_js','dsh_home','dsh_profile','working_directory','state_directory','model_alias','max_tokens','timeout_seconds','api_timeout_seconds','prices','allowed_tools','allow_all_tools','fixture_read_tool','prompt') | Sort-Object
  $actualSupervisor = @($parsed.supervisor.PSObject.Properties.Name)
  if (Compare-Object -ReferenceObject $expectedSupervisor -DifferenceObject ($actualSupervisor | Sort-Object) -CaseSensitive) { throw 'Supervisor JSON fields do not match the run configuration contract.' }
  [System.IO.File]::WriteAllText($Destination, $text, [System.Text.UTF8Encoding]::new($false))
}

try {
  $stage = 'isolated_setup'
  $env:DSH_HOME = Join-Path $temp 'dsh-home'; $env:DSH_TOOLS_MODE = 'native'
  $env:SIC_CORE_TOKEN = [guid]::NewGuid().ToString('N'); $env:SIC_MOCK_PROVIDER_KEY = [guid]::NewGuid().ToString('N')
  $env:SIC_FIXTURE_SIGNAL_WAIT = if ($VerifySignalWait) { '1' } else { '0' }
  $env:SIC_FIXTURE_SIGNAL_CURSOR = if ($VerifySignalCursor) { '1' } else { '0' }
  $stage = 'local_mock_startup'
  $mockOut = Join-Path $temp 'mock.out'; $mockErr = Join-Path $temp 'mock.err'
  $mockScript = Join-Path $PSScriptRoot 'native-read-mock-chat-provider.mjs'
  $mock = Start-Process -FilePath 'node' -ArgumentList @(('"' + $mockScript + '"')) -WorkingDirectory $plugin -WindowStyle Hidden -PassThru -RedirectStandardOutput $mockOut -RedirectStandardError $mockErr
  for ($i = 0; $i -lt 100; $i++) { if (Test-Path $mockOut) { try { $mockPort = (Get-Content $mockOut -TotalCount 1 | ConvertFrom-Json).port } catch {} }; if ($mockPort) { break }; Start-Sleep -Milliseconds 100 }
  if (-not $mockPort) { throw 'local synthetic Provider did not bind' }
  $profile = @{ model = 'openai/mock-model'; api_base = "http://127.0.0.1:$mockPort/v1"; api_key_env = 'SIC_MOCK_PROVIDER_KEY'; tokenizer_model = 'host-o200k-surrogate'; tokenizer_status = 'local-synthetic-fixture'; context_window = 1000000; max_input_tokens = 1000000; max_output_tokens = 393216; default_output_tokens = 4096; max_supported_output_tokens = 393216; timeout_seconds = 20 }
  $modelPath = Join-Path $temp 'mock-model.json'; @{ roles = @{ worker = $profile; editor = $profile } } | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $modelPath -Encoding utf8
  $stage = 'isolated_database_init'
  $database = Join-Path $temp 'peer-supervisor-watch.sqlite'; & $coreBinary init $database *> $null
  if ($LASTEXITCODE -ne 0) { throw 'could not initialize isolated Rust database' }
  $stage = 'isolated_core_startup'
  $probe = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0); $probe.Start(); $corePort = $probe.LocalEndpoint.Port; $probe.Stop()
  $coreOut = Join-Path $temp 'core.out'; $coreErr = Join-Path $temp 'core.err'
  $coreArgs = @('serve', $database, "127.0.0.1:$corePort", $actor, 'SIC_CORE_TOKEN', '--allow-model', '--model-config', $modelPath, '--dsh-alias', 'sic-worker')
  $core = Start-Process -FilePath $coreBinary -ArgumentList $coreArgs -WorkingDirectory $temp -WindowStyle Hidden -PassThru -RedirectStandardOutput $coreOut -RedirectStandardError $coreErr
  $headers = @{ Authorization = "Bearer $env:SIC_CORE_TOKEN" }; $ready = $false
  for ($i = 0; $i -lt 150; $i++) { if ($core.HasExited) { throw 'isolated Core exited during startup' }; try { $null = Invoke-RestMethod -Uri "http://127.0.0.1:$corePort/health" -Headers $headers -TimeoutSec 1 -NoProxy; $ready = $true; break } catch { Start-Sleep -Milliseconds 100 } }
  if (-not $ready) { throw 'isolated Core did not become ready' }

  $stage = 'requester_dsh_bootstrap'
  $requesterId = (Invoke-CoreOperation 'chain.create' '' @{}).chain_id
  $env:SIC_RUST_UPSTREAM = "http://127.0.0.1:$corePort"
  $observerOut = Join-Path $temp 'observer.out'; $observerErr = Join-Path $temp 'observer.err'
  $observerScript = Join-Path $plugin 'test\bridge-observer.mjs'
  $observer = Start-Process -FilePath 'node' -ArgumentList @(('"' + $observerScript + '"')) -WorkingDirectory $plugin -WindowStyle Hidden -PassThru -RedirectStandardOutput $observerOut -RedirectStandardError $observerErr
  for ($i = 0; $i -lt 100; $i++) { if (Test-Path $observerOut) { try { $observerPort = (Get-Content $observerOut -TotalCount 1 | ConvertFrom-Json).port } catch {} }; if ($observerPort) { break }; Start-Sleep -Milliseconds 100 }
  if (-not $observerPort) { throw 'local Rust bridge observer did not bind' }
  Install-IsolatedProfile 'sic-peer-watch-requester'
  $requesterPatch = Join-Path $temp 'requester.patch.yml'; Write-DshPatch $requesterPatch $requesterId $observerPort
  Push-Location -LiteralPath $temp
  try { $requesterOutput = (& dsh --profile sic-peer-watch-requester --patch $requesterPatch 'SIC_PEER_PARENT_BOOTSTRAP_SYNTHETIC. Reply briefly that the synthetic requester is ready.' 2>&1 | Out-String) }
  finally { Pop-Location }
  $requesterExit = $LASTEXITCODE
  if ($requesterExit -ne 0 -or -not $requesterOutput.Contains('SIC_PARENT_BOOTSTRAP_READY')) { throw 'requester DSH bootstrap failed before publishing its prefix' }
  $parentState = Invoke-CoreOperation 'chain.get' $requesterId @{}
  $parentPublished = [int]$parentState.cut_seq -ge 1
  if (-not $parentPublished) { throw 'requester bootstrap did not publish a CB' }

  $stage = 'checkpoint_and_spawn'
  $checkpoint = Invoke-CoreOperation 'chain.checkpoint' $requesterId @{}
  $socket = Invoke-CoreOperation 'socket.create' $requesterId @{ checkpoint_id = $checkpoint.checkpoint_id; acl = @{ message = @(); signal = @(); spawn = @($actor); read = @() } }
  $spawn = Invoke-CoreOperation 'socket.spawn' $requesterId @{ socket_id = $socket.socket_id; request_id = 'peer-watch-request-1'; message_id = 'peer-watch-task-1'; text = 'SIC_PEER_CHILD_TASK_SYNTHETIC. Complete the local fixture task, then report the result.'; lifecycle = 'P' }
  $peerId = [string]$spawn.chain_id; $taskId = [string]$spawn.task_id
  $null = Invoke-CoreOperation 'socket.acl.update' $requesterId @{ socket_id = $socket.socket_id; acl = @{ message = @($peerId); signal = @(); spawn = @($actor); read = @() } }
  $spawnRecorded = -not [string]::IsNullOrWhiteSpace($taskId) -and $spawn.task_queued -eq $true
  if (-not $spawnRecorded) { throw 'Core did not persist the synthetic peer task' }
  if ($VerifySignalWait) {
    $peerCheckpoint = Invoke-CoreOperation 'chain.checkpoint' $peerId @{}
    $peerSocket = (Invoke-CoreOperation 'socket.create' $peerId @{ checkpoint_id = $peerCheckpoint.checkpoint_id; acl = @{ message = @(); signal = @($requesterId); spawn = @(); read = @() } }).socket_id
  }

  $stage = 'watch_configuration'
  $dshCommand = Get-Command dsh -ErrorAction Stop
  $dshBinDirectory = Split-Path -Parent $dshCommand.Source
  $dshExecutable = [System.IO.Path]::GetFullPath((Join-Path $dshBinDirectory '..\..\..\..\DeepSeek Harness.exe'))
  $dshAsar = [System.IO.Path]::GetFullPath((Join-Path $dshBinDirectory '..\..\..\app.asar'))
  $dshCliJs = Join-Path $dshAsar 'dsh\node_modules\@deepseek-ai\dsh-desktop-host\lib\cli.js'
  if (-not (Test-Path -LiteralPath $dshExecutable -PathType Leaf) -or -not (Test-Path -LiteralPath $dshAsar -PathType Leaf)) { throw 'could not resolve the installed DSH headless runtime' }
  $dshHome = $env:DSH_HOME; $dshProfile = 'sic-peer-watch-child'
  Install-IsolatedProfile $dshProfile
  $workingDirectory = $temp; [System.IO.File]::WriteAllText((Join-Path $workingDirectory 'SIC_TOOL_FIXTURE.txt'), 'SIC_RUST_SYNTHETIC_FIXTURE_ONLY', [System.Text.UTF8Encoding]::new($false)); $stateDirectory = Join-Path $temp 'watch-state'; New-Item -ItemType Directory -Path $stateDirectory | Out-Null
  $watchConfigPath = Join-Path $temp 'peer-watch-config.json'
  $watchNamespace = 'sic-peer-watch-fixture'; $pollIntervalMs = 500; $maxTasksPerCycle = 1; $pageSize = 32
  $policy = @{ context_id = 'main'; min_sources_per_wb = 1; max_sources_per_wb = 1; max_units_per_wb = 64; level_batch_sizes = @{}; max_level = 1; flush = $true; max_actions = 64; max_model_calls = 4; editor_mode = 'provider'; program_rules = @() }
  $prices = @{ input = 0; output = 0; cache_read = 0; cache_write = 0 }
  $peerPrompt = 'SIC_PEER_CHILD_TASK_SYNTHETIC. Use the DSH built-in read tool exactly once on SIC_TOOL_FIXTURE.txt, then report its returned marker.'
  $watchValues = @{
    '@@DB_PATH@@' = $database; '@@CORE_BASE_URL@@' = "http://127.0.0.1:$corePort"; '@@BRIDGE_BASE_URL@@' = "http://127.0.0.1:$observerPort/v1"
    '@@BEARER_ENV@@' = 'SIC_CORE_TOKEN'; '@@ACTOR@@' = $actor; '@@REQUESTER_CHAIN_ID@@' = $requesterId; '@@PEER_CHAIN_ID@@' = 'watch-peer-placeholder'
    '@@TASK_ID@@' = 'watch-task-placeholder'; '@@SOCKET_ID@@' = $socket.socket_id; '@@REPLY_SOCKET_ID@@' = $socket.socket_id
    '@@CLAIM_ID@@' = 'watch-claim-placeholder'; '@@PREPARE_ID@@' = 'watch-prepare-placeholder'; '@@LAUNCH_ID@@' = 'watch-launch-placeholder'; '@@REPLY_ID@@' = 'watch-reply-placeholder'
    '@@DSH_EXECUTABLE@@' = $dshExecutable; '@@DSH_CLI_JS@@' = $dshCliJs
    '@@DSH_HOME@@' = $dshHome; '@@DSH_PROFILE@@' = $dshProfile; '@@WORKING_DIRECTORY@@' = $workingDirectory; '@@STATE_DIRECTORY@@' = $stateDirectory
    '@@MODEL_ALIAS@@' = 'sic-worker'; '@@MAX_TOKENS@@' = 4096; '@@TIMEOUT_SECONDS@@' = 120; '@@API_TIMEOUT_SECONDS@@' = 120
    '@@POLICY@@' = $policy; '@@PRICES@@' = $prices; '@@ALLOWED_TOOLS@@' = @('read')
    '@@ALLOW_ALL_TOOLS@@' = $false; '@@FIXTURE_READ_TOOL@@' = $false; '@@PROMPT@@' = $peerPrompt
    '@@WATCH_ENABLED@@' = $true; '@@WATCH_NAMESPACE@@' = $watchNamespace; '@@POLL_INTERVAL_MS@@' = $pollIntervalMs
    '@@MAX_TASKS_PER_CYCLE@@' = $maxTasksPerCycle; '@@PAGE_SIZE@@' = $pageSize
  }
  Write-WatchConfig $watchTemplatePath $watchConfigPath $watchValues

  $stage = 'watch_pending_task'
  $watchOut = Join-Path $temp 'watch.out'; $watchErr = Join-Path $temp 'watch.err'
  $watch = Start-Process -FilePath $watchBinary -ArgumentList @('watch', '--config', ('"' + $watchConfigPath + '"')) -WorkingDirectory $temp -WindowStyle Hidden -PassThru -RedirectStandardOutput $watchOut -RedirectStandardError $watchErr
  $deadline = [DateTime]::UtcNow.AddSeconds($WatchTimeoutSeconds)
  $replyEntry = $null
  while ([DateTime]::UtcNow -lt $deadline) {
    if ($watch.HasExited) { $watchExit = $watch.ExitCode; throw 'peer supervisor watcher exited before Socket reply delivery' }
    try {
      if ($VerifySignalWait -and -not $signalSent) {
        $providerProgress = Invoke-RestMethod -Uri "http://127.0.0.1:$mockPort/status" -TimeoutSec 3 -NoProxy
        if (@($providerProgress.calls | Where-Object { $_.responseKind -eq 'peer_signal_wait' }).Count -gt 0) {
          $null = Invoke-CoreOperation 'socket.signal' $requesterId @{ socket_id = $peerSocket; signal_id = 'fixture-peer-wake'; action = 'wake' }
          $signalSent = $true
        }
      }
      if ($VerifyMessageWait -and (-not $VerifySignalWait -or $signalSent)) {
        $notification = Invoke-CoreOperation 'message.wait' $requesterId @{ after_sequence = $messageCursor; timeout_ms = 1000 }
        $messageCursor = [UInt64]$notification.next_after_sequence
        $candidate = $notification.message
        if ($candidate -and $candidate.message.phase_id -eq 'peer-task-reply' -and
            ($candidate.message.reply_to -eq 'peer-watch-task-1' -or $candidate.message.task_id -eq $taskId)) {
          $messageWaitVerified = $null -eq $candidate.consumed_by_cut
          $replyEntry = $candidate
        }
      } elseif (-not $VerifyMessageWait) {
        $inbox = Invoke-CoreOperation 'socket.inbox' $requesterId @{}
        $replyEntry = @($inbox.messages | Where-Object { $_.message.phase_id -eq 'peer-task-reply' -and $_.message.reply_to -eq 'peer-watch-task-1' })[0]
        if (-not $replyEntry) { $replyEntry = @($inbox.messages | Where-Object { $_.message.phase_id -eq 'peer-task-reply' -and $_.message.task_id -eq $taskId })[0] }
      }
      if ($replyEntry) { break }
    } catch {}
    Start-Sleep -Milliseconds 500
  }
  if (-not $replyEntry) { throw 'watcher did not deliver a peer-task-reply Message before timeout' }
  if ($VerifyMessageWait -and -not $messageWaitVerified) { throw 'Host did not observe the unconsumed peer Message through message.wait.' }
  $replyMessageId = [string]$replyEntry.message.message_id
  $replyText = [string]$replyEntry.message.text
  $peerReplyDelivered = -not [string]::IsNullOrWhiteSpace($replyMessageId) -and -not [string]::IsNullOrWhiteSpace($replyText)
  if (-not $peerReplyDelivered) { throw 'Socket reply has no stable message ID or text' }

  $stage = 'verify_peer_and_adopt_reply'
  $peerInbox = Invoke-CoreOperation 'socket.inbox' $peerId @{}
  $peerInboxPublished = @($peerInbox.messages | Where-Object { $_.message.message_id -eq 'peer-watch-task-1' -and $_.consumed_by_cut }).Count -eq 1
  if (-not $peerInboxPublished) { throw 'watcher did not consume the spawn Message into the peer CB' }
  $requesterStatusJson = & python (Join-Path $plugin 'test\verify-status-fixture.py') $database $actor $requesterId $corePort 'requester'
  if ($LASTEXITCODE -ne 0) { throw 'requester SIC status snapshot fixture failed' }
  $requesterStatusReport = $requesterStatusJson | ConvertFrom-Json
  $statusJson = & python (Join-Path $plugin 'test\verify-status-fixture.py') $database $actor $peerId $corePort 'peer'
  if ($LASTEXITCODE -ne 0) { throw 'SIC status snapshot fixture failed' }
  $statusReport = $statusJson | ConvertFrom-Json
  $sicStatusVerified = [bool]$statusReport.status_verified -and [bool]$statusReport.condition_verified -and [bool]$requesterStatusReport.status_verified -and [bool]$requesterStatusReport.condition_verified
  $nativeEventVerified = [bool]$statusReport.native_event_verified
  $roleAccountingVerified = [bool]$statusReport.accounting_verified -and [bool]$requesterStatusReport.accounting_verified
  if (-not $sicStatusVerified) { throw 'SIC status snapshot evidence is incomplete' }
  if (-not $nativeEventVerified -or -not $roleAccountingVerified) { throw 'Native event or role accounting evidence is incomplete' }
  $null = Invoke-CoreOperation 'source.ingest' $requesterId @{ request = @{ messages = @(@{ role = 'user'; content = 'Review the synthetic peer response.' }) } }
  $parentReturn = Invoke-ProgramFixture $requesterId
  $parentCut = Invoke-CoreOperation 'cut.get' $requesterId @{}
  $parentInbox = Invoke-CoreOperation 'socket.inbox' $requesterId @{}
  $requesterAdoptedReply = [bool]$parentReturn.completed -and @($parentReturn.published).Count -ge 1 -and
    @($parentCut.view.cbs | Where-Object { $_.body.Contains($replyText) }).Count -ge 1 -and
    @($parentInbox.messages | Where-Object { $_.message.message_id -eq $replyMessageId -and $_.consumed_by_cut }).Count -eq 1
  if (-not $requesterAdoptedReply) { throw 'requester ProgramFixture CB did not consume the peer reply' }

  $observerStatus = Invoke-RestMethod -Uri "http://127.0.0.1:$observerPort/status" -TimeoutSec 3 -NoProxy
  $mockStatus = Invoke-RestMethod -Uri "http://127.0.0.1:$mockPort/status" -TimeoutSec 3 -NoProxy
  if ($VerifySignalWait) {
    $signalReceipt = Invoke-CoreOperation 'signal.wait' $peerId @{ signal_id = 'fixture-peer-wake'; timeout_ms = 0 }
    $signalWaitVerified = $signalSent -and [bool]$mockStatus.signalReceiptObserved -and $signalReceipt.receipt.signal_id -eq 'fixture-peer-wake'
    if (-not $signalWaitVerified) { throw 'Peer Signal wait did not observe the authorized wake receipt.' }
  }
  $fixtureResultObserved = @($mockStatus.calls | Where-Object { $_.responseKind -eq 'peer_final_text_after_fixture' -and $_.fixtureResultObserved }).Count -eq 1
  if (-not $fixtureResultObserved) { throw 'synthetic DSH fixture tool result was not observed by the mock Provider' }
  $allChats = @($observerStatus.observations | Where-Object { $_.route -eq '/v1/chat/completions' })
  if ($allChats.Count -lt 2 -or @($allChats | Where-Object { $_.toolNames.Count -ne 1 -or $_.toolNames[0] -ne 'read' }).Count -gt 0) { throw 'watch DSH turn exceeded the isolated read-only tool allowlist' }
  $stateJson = & python (Join-Path $plugin 'test\peer-fixture-state-summary.py') $database $requesterId $peerId
  if ($LASTEXITCODE -ne 0) { throw 'could not read final redacted peer state' }
  $stateSummary = $stateJson | ConvertFrom-Json
  if ($stateSummary.parent.protected_digest -ne $stateSummary.child.protected_digest) { throw 'peer protected prefix changed during watch processing' }
  $passed = $true; $stage = 'complete'
}
catch {
  $failure = switch ($stage) {
    'requester_dsh_bootstrap' { 'requester_bootstrap_failed' }
    'checkpoint_and_spawn' { 'checkpoint_or_spawn_failed' }
    'watch_configuration' { 'watch_config_failed' }
    'watch_pending_task' { 'watch_failed_or_timed_out' }
    'verify_peer_and_adopt_reply' { 'peer_reply_adoption_failed' }
    default { 'setup_failed' }
  }
  if ($observerPort) { try { $observerStatus = Invoke-RestMethod -Uri "http://127.0.0.1:$observerPort/status" -TimeoutSec 3 -NoProxy } catch {} }
  if ($mockPort) { try { $mockStatus = Invoke-RestMethod -Uri "http://127.0.0.1:$mockPort/status" -TimeoutSec 3 -NoProxy } catch {} }
  if ($database -and $requesterId -and $peerId -and (Test-Path -LiteralPath $database)) {
    try { $stateJson = & python (Join-Path $plugin 'test\peer-fixture-state-summary.py') $database $requesterId $peerId; if ($LASTEXITCODE -eq 0) { $stateSummary = $stateJson | ConvertFrom-Json } } catch {}
  }
}
finally {
  if ($watch -and -not $watch.HasExited) { Stop-Process -Id $watch.Id -Force -ErrorAction SilentlyContinue }
  if ($core -and -not $core.HasExited) { Stop-Process -Id $core.Id -Force -ErrorAction SilentlyContinue }
  if ($observer -and -not $observer.HasExited) { Stop-Process -Id $observer.Id -Force -ErrorAction SilentlyContinue }
  if ($mock -and -not $mock.HasExited) { Stop-Process -Id $mock.Id -Force -ErrorAction SilentlyContinue }
  foreach ($name in @('DSH_HOME', 'SIC_CORE_TOKEN', 'SIC_MOCK_PROVIDER_KEY', 'SIC_RUST_UPSTREAM', 'DSH_TOOLS_MODE', 'SIC_FIXTURE_SIGNAL_WAIT', 'SIC_FIXTURE_SIGNAL_CURSOR')) {
    if ($null -eq $saved[$name]) { Remove-Item -Path ("Env:" + $name) -ErrorAction SilentlyContinue } else { [Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process') }
  }
}

$safeReport = [ordered]@{
  result = if ($passed) { 'passed' } else { 'stopped' }; stage = $stage; failure = $failure
  topology = 'requester DSH bootstrap -> checkpoint/Socket/spawn -> supervisor watch -> DSH built-in read of synthetic file -> Socket reply -> requester ProgramFixture CB adoption'
  watcherMode = 'one local watch process; no production DSH profile or remote Provider'
  requesterPublished = $parentPublished; spawnRecorded = $spawnRecorded; peerInboxPublished = $peerInboxPublished
  peerReplyDelivered = $peerReplyDelivered; requesterAdoptedReply = $requesterAdoptedReply; watcherExitCode = $watchExit
  sicStatusSnapshotVerified = $sicStatusVerified
  nativeToolEventVerified = $nativeEventVerified; roleModelAccountingVerified = $roleAccountingVerified
  signalWaitRequested = [bool]$VerifySignalWait; signalWaitVerified = $signalWaitVerified; signalCursorRequested = [bool]$VerifySignalCursor
  messageWaitRequested = [bool]$VerifyMessageWait; messageWaitVerified = $messageWaitVerified
  fixtureResultObservedByMock = $fixtureResultObserved
  providerCallKinds = if ($mockStatus) { @($mockStatus.calls | ForEach-Object { $_.responseKind }) } else { @() }
  protectedPrefixSummary = if ($stateSummary) { [ordered]@{ requesterCount = $stateSummary.parent.protected_count; peerCount = $stateSummary.child.protected_count; requesterDigest = $stateSummary.parent.protected_digest; peerDigest = $stateSummary.child.protected_digest; identical = $stateSummary.parent.protected_digest -eq $stateSummary.child.protected_digest } } else { $null }
  rawPromptsOrMessagesInReport = $false; credentialsInReport = $false; remoteModelUsed = $false
  nativeReadResultObserved = $fixtureResultObserved
}
$safeReport | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $reportPath -Encoding utf8
if (Test-Path -LiteralPath $temp) {
  $resolved = (Resolve-Path -LiteralPath $temp -ErrorAction Stop).Path; $item = Get-Item -LiteralPath $resolved -Force -ErrorAction Stop
  if ($resolved -ne $temp -or -not $resolved.StartsWith($tempRoot + [System.IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase) -or -not $item.Name.StartsWith('sic-peer-supervisor-watch-', [StringComparison]::Ordinal) -or ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint)) { throw 'Refusing to remove a fixture directory outside the verified target.' }
  Remove-Item -LiteralPath $resolved -Recurse -Force -ErrorAction Stop
}
if ($passed) { Write-Output "PASS: local supervisor watch fixture adopted the Socket reply into the requester CB. Report: $reportPath" }
else { Write-Output "STOP: $failure at $stage; no production Provider was used. Report: $reportPath"; exit 1 }
