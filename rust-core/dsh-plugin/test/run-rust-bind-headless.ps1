$ErrorActionPreference = 'Stop'
$coreRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$plugin = Split-Path $PSScriptRoot -Parent
$binary = @((Join-Path $coreRoot 'target\debug\sic-core-rust.exe'), (Join-Path $coreRoot 'target\release\sic-core-rust.exe'), (Join-Path $coreRoot 'release\sic-core-rust.exe')) |
    Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
if (-not $binary) { throw 'Provide release\sic-core-rust.exe or build the Rust Core binary in target.' }

$saved = @{}
foreach ($name in @('DSH_HOME', 'SIC_CORE_TOKEN', 'SIC_MOCK_PROVIDER_KEY', 'SIC_RUST_UPSTREAM', 'DSH_TOOLS_MODE')) {
  $saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}
$tempRoot = [System.IO.Path]::GetFullPath($env:TEMP).TrimEnd([System.IO.Path]::DirectorySeparatorChar)
$temp = [System.IO.Path]::GetFullPath((Join-Path $tempRoot ('sic-rust-bind-headless-' + [guid]::NewGuid().ToString('N'))))
if (-not $temp.StartsWith($tempRoot + [System.IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
  throw 'Fixture directory is outside the intended temporary directory'
}
$null = New-Item -ItemType Directory -Force -Path $temp
$reportPath = Join-Path $tempRoot ('sic-rust-bind-headless-redacted-' + [guid]::NewGuid().ToString('N') + '.json')
$mock = $null
$observer = $null
$core = $null
$status = $null
$dbSummary = $null
$providerStatus = $null
$passed = $false
$failure = 'setup_failed'
try {
  $env:DSH_HOME = Join-Path $temp 'dsh-home'
  $env:DSH_TOOLS_MODE = 'native'
  $env:SIC_CORE_TOKEN = [guid]::NewGuid().ToString('N')
  $env:SIC_MOCK_PROVIDER_KEY = [guid]::NewGuid().ToString('N')

  $mockOut = Join-Path $temp 'mock-provider.out'
  $mockErr = Join-Path $temp 'mock-provider.err'
  $mockScript = Join-Path $plugin 'test\mock-chat-provider.mjs'
  $mock = Start-Process -FilePath 'node' -ArgumentList @('"' + $mockScript + '"') -WorkingDirectory $plugin -WindowStyle Hidden -PassThru -RedirectStandardOutput $mockOut -RedirectStandardError $mockErr
  $mockPort = $null
  for ($i = 0; $i -lt 100; $i++) {
    if (Test-Path -LiteralPath $mockOut) {
      $line = Get-Content -LiteralPath $mockOut -TotalCount 1
      if ($line) { try { $mockPort = (ConvertFrom-Json $line).port } catch {} }
    }
    if ($mockPort) { break }
    Start-Sleep -Milliseconds 100
  }
  if (-not $mockPort) { throw 'local mock provider did not bind' }

  $profile = @{
    model = 'openai/mock-model'
    api_base = "http://127.0.0.1:$mockPort/v1"
    api_key_env = 'SIC_MOCK_PROVIDER_KEY'
    tokenizer_model = 'host-o200k-surrogate'
    tokenizer_status = 'local-synthetic-fixture'
    context_window = 1000000
    max_input_tokens = 1000000
    max_output_tokens = 393216
    default_output_tokens = 4096
    max_supported_output_tokens = 393216
    timeout_seconds = 20
  }
  $modelPath = Join-Path $temp 'mock-model.json'
  @{ roles = @{ worker = $profile; editor = $profile } } | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $modelPath -Encoding utf8
  $database = Join-Path $temp 'rust-core.db'
  & $binary init $database *> $null
  if ($LASTEXITCODE -ne 0) { throw 'isolated Rust database initialization failed' }

  $probe = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
  $probe.Start(); $corePort = $probe.LocalEndpoint.Port; $probe.Stop()
  $coreOut = Join-Path $temp 'rust-core.out'
  $coreErr = Join-Path $temp 'rust-core.err'
  $coreArgs = @('serve', $database, "127.0.0.1:$corePort", 'integration-actor', 'SIC_CORE_TOKEN', '--allow-model', '--model-config', $modelPath, '--dsh-alias', 'sic-worker')
  $core = Start-Process -FilePath $binary -ArgumentList $coreArgs -WorkingDirectory $temp -WindowStyle Hidden -PassThru -RedirectStandardOutput $coreOut -RedirectStandardError $coreErr
  $ready = $false
  for ($i = 0; $i -lt 150; $i++) {
    if ($core.HasExited) { throw 'isolated Rust Core exited during startup' }
    try {
      $null = Invoke-RestMethod -Uri "http://127.0.0.1:$corePort/health" -Headers @{ Authorization = "Bearer $env:SIC_CORE_TOKEN" } -TimeoutSec 1 -NoProxy
      $ready = $true; break
    } catch { Start-Sleep -Milliseconds 100 }
  }
  if (-not $ready) { throw 'isolated Rust Core did not become ready' }

  $createBody = @{
    operation = 'chain.create'
    context = @{ actor = 'integration-actor'; allow_write = $true; allow_model = $false }
    payload = @{}
  } | ConvertTo-Json -Depth 6 -Compress
  try {
    $created = Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$corePort/api/v2/operations" `
      -Headers @{ Authorization = "Bearer $env:SIC_CORE_TOKEN" } -ContentType 'application/json' -Body $createBody -TimeoutSec 10 -NoProxy
  } catch { throw 'Core API could not create the isolated test Agent' }
  $chainId = $created.result.chain_id
  if ([string]::IsNullOrWhiteSpace($chainId)) { throw 'Core API did not return a test Agent identity' }

  $env:SIC_RUST_UPSTREAM = "http://127.0.0.1:$corePort"
  $observerOut = Join-Path $temp 'bridge-observer.out'
  $observerErr = Join-Path $temp 'bridge-observer.err'
  $observerScript = Join-Path $plugin 'test\bridge-observer.mjs'
  $observer = Start-Process -FilePath 'node' -ArgumentList @('"' + $observerScript + '"') -WorkingDirectory $plugin -WindowStyle Hidden -PassThru -RedirectStandardOutput $observerOut -RedirectStandardError $observerErr
  $observerPort = $null
  for ($i = 0; $i -lt 100; $i++) {
    if (Test-Path -LiteralPath $observerOut) {
      $line = Get-Content -LiteralPath $observerOut -TotalCount 1
      if ($line) { try { $observerPort = (ConvertFrom-Json $line).port } catch {} }
    }
    if ($observerPort) { break }
    Start-Sleep -Milliseconds 100
  }
  if (-not $observerPort) { throw 'local bridge observer did not bind' }

  $routePatch = Join-Path $temp 'isolated-dsh.patch.yml'
  @"
- id: agent-default-model
  config:
    provider: sic-core-rust
    model: sic-worker
- id: sic-core-rust-dsh
  config:
    provider: sic-core-rust
    baseURL: http://127.0.0.1:$observerPort/v1
    apiKeyEnv: SIC_CORE_TOKEN
    model: sic-worker
    bindChainId: $chainId
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
"@ | Set-Content -LiteralPath $routePatch -Encoding utf8

  & dsh --profile sic-rust-bind-headless --from-default-profile headless --dump-config *> $null
  if ($LASTEXITCODE -ne 0) { throw 'isolated DSH headless profile creation failed' }
  & dsh plugin --profile sic-rust-bind-headless add $plugin *> $null
  if ($LASTEXITCODE -ne 0) { throw 'isolated DSH plugin installation failed' }
  $failure = 'headless_turn_failed'
  $runOutput = (& dsh --profile sic-rust-bind-headless --patch $routePatch 'Use sic_fixture_read exactly once, then answer with its returned marker.' 2>&1 | Out-String)
  if ($LASTEXITCODE -ne 0 -or -not $runOutput.Contains('SIC_RUST_DSH_MOCK_ROUNDTRIP_OK')) {
    throw 'isolated headless fixture turn did not complete'
  }

  $observerStatus = Invoke-RestMethod -Uri "http://127.0.0.1:$observerPort/status" -TimeoutSec 3 -NoProxy
  $providerStatus = Invoke-RestMethod -Uri "http://127.0.0.1:$mockPort/status" -TimeoutSec 3 -NoProxy
  $dbJson = & python (Join-Path $plugin 'test\rust-bind-db-summary.py') $database $chainId
  if ($LASTEXITCODE -ne 0) { throw 'could not read isolated Rust bridge receipt summary' }
  $dbSummary = $dbJson | ConvertFrom-Json

  $observations = @($observerStatus.observations)
  $bindObservations = @($observations | Where-Object { $_.route -eq '/v1/sic/bind' })
  $chatObservations = @($observations | Where-Object { $_.route -eq '/v1/chat/completions' })
  $firstChatIndex = -1
  $bindIndex = -1
  for ($i = 0; $i -lt $observations.Count; $i++) {
    if ($observations[$i].route -eq '/v1/sic/bind' -and $bindIndex -eq -1) { $bindIndex = $i }
    if ($observations[$i].route -eq '/v1/chat/completions' -and $firstChatIndex -eq -1) { $firstChatIndex = $i }
  }
  $boundSessions = @($dbSummary.sessions)
  $boundSessionDigest = if ($bindObservations.Count -eq 1) { $bindObservations[0].requestSessionDigest } else { $null }
  $allChatSameSession = $chatObservations.Count -gt 0 -and @($chatObservations | Where-Object { $_.bridgeSessionDigest -ne $boundSessionDigest }).Count -eq 0
  $dbHasSameSession = $boundSessions.Count -eq 1 -and $boundSessions[0].session_digest -eq $boundSessionDigest -and
    $boundSessions[0].chain_digest -eq $bindObservations[0].bindChainDigest
  $deliveredTurns = @($dbSummary.turns | Where-Object { $_.session_digest -eq $boundSessionDigest -and $_.state -eq 'delivered' }).Count -ge 1
  $fixtureObserved = @($providerStatus.calls | Where-Object { $_.responseKind -eq 'worker_final_text' -and $_.fixtureResultObserved }).Count -eq 1
  $oneFixtureHostTool = @($chatObservations | Where-Object { $_.toolNames.Count -ne 1 -or $_.toolNames[0] -ne 'sic_fixture_read' }).Count -eq 0
  $expectedProviderFlow = $providerStatus.requestCount -eq 3 -and
    @($providerStatus.calls | Where-Object { $_.responseKind -eq 'editor_edit_blocks' }).Count -eq 1 -and
    @($providerStatus.calls | Where-Object { $_.responseKind -eq 'worker_fixture_tool_call' }).Count -eq 1 -and
    @($providerStatus.calls | Where-Object { $_.responseKind -eq 'worker_final_text' }).Count -eq 1
  $bindChatOrderOk = $bindObservations.Count -eq 1 -and $chatObservations.Count -ge 2 -and $bindIndex -ge 0 -and $bindIndex -lt $firstChatIndex -and
    $bindObservations[0].responseStatus -eq 200 -and $chatObservations.Count -eq @($chatObservations | Where-Object { $_.responseStatus -eq 200 }).Count
  $passed = $observerStatus.observations.Count -eq $observations.Count -and
    $bindObservations[0].authorizationPresent -and $allChatSameSession -and $dbHasSameSession -and
    $deliveredTurns -and $fixtureObserved -and $oneFixtureHostTool -and $expectedProviderFlow -and $bindChatOrderOk -and
    @($dbSummary.task_counts | Where-Object { $_.state -eq 'completed' -and $_.count -eq 1 }).Count -eq 1
  if (-not $passed) { throw 'Rust bind/chat order, session identity, task completion, or fixture checks failed' }
  $failure = $null
}
catch {
  if ($failure -eq 'setup_failed' -or [string]::IsNullOrEmpty($failure)) { $failure = 'setup_failed' }
}
finally {
  if ($core -and -not $core.HasExited) { Stop-Process -Id $core.Id -Force -ErrorAction SilentlyContinue }
  if ($observer -and -not $observer.HasExited) { Stop-Process -Id $observer.Id -Force -ErrorAction SilentlyContinue }
  if ($mock -and -not $mock.HasExited) { Stop-Process -Id $mock.Id -Force -ErrorAction SilentlyContinue }
  foreach ($name in @('DSH_HOME', 'SIC_CORE_TOKEN', 'SIC_MOCK_PROVIDER_KEY', 'SIC_RUST_UPSTREAM', 'DSH_TOOLS_MODE')) {
    if ($null -eq $saved[$name]) { Remove-Item -Path ("Env:" + $name) -ErrorAction SilentlyContinue }
    else { [Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process') }
  }
}

$safeReport = [ordered]@{
  result = if ($passed) { 'passed' } else { 'failed' }
  failure = $failure
  dshVersion = (dsh --version | Out-String).Trim()
  topology = 'DSH headless -> plugin bindChainId -> real Rust /v1/sic/bind -> Core -> local mock Provider -> DSH fixture -> Rust'
  bindAndChatObservations = if ($observerStatus) { @($observerStatus.observations) } else { @() }
  providerCallSummary = if ($providerStatus) { $providerStatus.calls } else { @() }
  rustDbSummary = $dbSummary
  rawPromptsOrCredentialsPersisted = $false
  externalModelOrRealToolUsed = $false
}
$safeReport | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $reportPath -Encoding utf8
if (Test-Path -LiteralPath $temp) {
  $resolved = (Resolve-Path -LiteralPath $temp -ErrorAction Stop).Path
  $item = Get-Item -LiteralPath $resolved -Force -ErrorAction Stop
  if ($resolved -ne $temp -or
      -not $resolved.StartsWith($tempRoot + [System.IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase) -or
      -not $item.Name.StartsWith('sic-rust-bind-headless-', [StringComparison]::Ordinal) -or
      ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint)) {
    throw 'Refusing to remove a fixture directory outside the verified target'
  }
  Remove-Item -LiteralPath $resolved -Recurse -Force -ErrorAction Stop
}
if ($passed) {
  Write-Output "PASS: real Rust bind-before-chat with DSH headless and local mock Provider; Editor=1, Worker=2, fixture task completed. Report: $reportPath"
} else {
  Write-Output "STOP: $failure; no real model was called. Report: $reportPath"
  exit 1
}
