$ErrorActionPreference = 'Stop'
$coreRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$plugin = Split-Path $PSScriptRoot -Parent
$binary = @((Join-Path $coreRoot 'target\debug\sic-core-rust.exe'), (Join-Path $coreRoot 'target\release\sic-core-rust.exe'), (Join-Path $coreRoot 'release\sic-core-rust.exe')) |
    Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
if (-not $binary) { throw 'Provide release\sic-core-rust.exe or build the Rust Core binary in target.' }

$oldEnv = @{}
foreach ($key in @('DSH_HOME', 'SIC_CORE_TOKEN', 'SIC_MOCK_PROVIDER_KEY', 'SIC_RUST_UPSTREAM', 'DSH_TOOLS_MODE')) {
  $oldEnv[$key] = [Environment]::GetEnvironmentVariable($key, 'Process')
}
$tempRoot = [System.IO.Path]::GetFullPath($env:TEMP).TrimEnd([System.IO.Path]::DirectorySeparatorChar)
$temp = [System.IO.Path]::GetFullPath((Join-Path $tempRoot ('sic-rust-dsh-integration-' + [guid]::NewGuid().ToString('N'))))
if (-not $temp.StartsWith($tempRoot + [System.IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
  throw 'Fixture directory is outside the intended temporary directory'
}
$null = New-Item -ItemType Directory -Force -Path $temp
$mock = $null
$proxy = $null
$core = $null
$keepArtifacts = $env:SIC_KEEP_TEST_ARTIFACTS -eq '1'
$reportSaved = $false
try {
  $env:DSH_HOME = Join-Path $temp 'dsh-home'
  $env:DSH_TOOLS_MODE = 'native'
  $env:SIC_CORE_TOKEN = [guid]::NewGuid().ToString('N')
  $env:SIC_MOCK_PROVIDER_KEY = [guid]::NewGuid().ToString('N')
  $mockOut = Join-Path $temp 'mock.out'
  $mockErr = Join-Path $temp 'mock.err'
  $mock = Start-Process -FilePath 'node' -ArgumentList @('"' + (Join-Path $plugin 'test\mock-chat-provider.mjs') + '"') -WorkingDirectory $plugin -WindowStyle Hidden -PassThru -RedirectStandardOutput $mockOut -RedirectStandardError $mockErr
  $mockPort = $null
  for ($i = 0; $i -lt 100; $i++) {
    if (Test-Path -LiteralPath $mockOut) {
      $line = Get-Content -LiteralPath $mockOut -TotalCount 1
      if ($line) { try { $mockPort = (ConvertFrom-Json $line).port } catch {} }
    }
    if ($mockPort) { break }
    Start-Sleep -Milliseconds 100
  }
  if (-not $mockPort) { throw 'mock provider failed to bind loopback' }

  $model = @{
    model = 'openai/mock-model'; api_base = "http://127.0.0.1:$mockPort/v1"; api_key_env = 'SIC_MOCK_PROVIDER_KEY'
    tokenizer_model = 'host-o200k-surrogate'; tokenizer_status = 'local-synthetic-fixture'
    context_window = 1000000; max_input_tokens = 1000000; max_output_tokens = 393216
    default_output_tokens = 4096; max_supported_output_tokens = 393216; timeout_seconds = 10
  }
  $modelConfig = @{ roles = @{ worker = $model; editor = $model } } | ConvertTo-Json -Depth 8
  $modelPath = Join-Path $temp 'mock-model.json'
  Set-Content -LiteralPath $modelPath -Value $modelConfig -Encoding utf8
  $probe = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
  $probe.Start(); $corePort = $probe.LocalEndpoint.Port; $probe.Stop()
  $overlay = Join-Path $temp 'isolated-route.patch.yml'
  @"
- id: agent-default-model
  config:
    provider: sic-core-rust
    model: sic-worker
- id: sic-core-rust-dsh
  config:
    provider: sic-core-rust
    baseURL: http://127.0.0.1:$proxyPort/v1
    apiKeyEnv: SIC_CORE_TOKEN
    model: sic-worker
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
- id: session-telemetry-otel
  name: '@deepseek-ai/dsh-session-telemetry-otel'
  disabled: true
- id: otel
  name: '@deepseek-ai/dsh-otel'
  disabled: true
"@ | Set-Content -LiteralPath $overlay -Encoding utf8
  $database = Join-Path $temp 'core.db'
  & $binary init $database *> $null
  if ($LASTEXITCODE -ne 0) { throw 'Rust Core fixture database initialization failed' }

  $coreOut = Join-Path $temp 'core.out'
  $coreErr = Join-Path $temp 'core.err'
  $coreArgs = @('serve', $database, "127.0.0.1:$corePort", 'integration-actor', 'SIC_CORE_TOKEN', '--allow-model', '--model-config', $modelPath, '--dsh-alias', 'sic-worker')
  $core = Start-Process -FilePath $binary -ArgumentList $coreArgs -WorkingDirectory $temp -WindowStyle Hidden -PassThru -RedirectStandardOutput $coreOut -RedirectStandardError $coreErr
  $ready = $false
  $healthError = 'none'
  for ($i = 0; $i -lt 150; $i++) {
    if ($core.HasExited) {
      $diagnostic = if (Test-Path -LiteralPath $coreErr) { (Get-Content -LiteralPath $coreErr -Raw).Trim() } else { 'no stderr captured' }
      throw "Rust Core fixture server exited during startup: $diagnostic"
    }
    try { $null = Invoke-RestMethod -Uri "http://127.0.0.1:$corePort/health" -Headers @{ Authorization = "Bearer $env:SIC_CORE_TOKEN" } -TimeoutSec 1 -NoProxy; $ready = $true; break } catch { $healthError = $_.Exception.Message }
    Start-Sleep -Milliseconds 100
  }
  if (-not $ready) { throw "Rust Core fixture server did not become ready: $healthError" }

  $env:SIC_RUST_UPSTREAM = "http://127.0.0.1:$corePort"
  $proxyOut = Join-Path $temp 'proxy.out'
  $proxyErr = Join-Path $temp 'proxy.err'
  $proxy = Start-Process -FilePath 'node' -ArgumentList @('"' + (Join-Path $plugin 'test\bridge-observer.mjs') + '"') -WorkingDirectory $plugin -WindowStyle Hidden -PassThru -RedirectStandardOutput $proxyOut -RedirectStandardError $proxyErr
  $proxyPort = $null
  for ($i = 0; $i -lt 100; $i++) {
    if (Test-Path -LiteralPath $proxyOut) {
      $line = Get-Content -LiteralPath $proxyOut -TotalCount 1
      if ($line) { try { $proxyPort = (ConvertFrom-Json $line).port } catch {} }
    }
    if ($proxyPort) { break }
    Start-Sleep -Milliseconds 100
  }
  if (-not $proxyPort) { throw 'local Rust bridge observer failed to bind loopback' }
  @"
- id: agent-default-model
  config:
    provider: sic-core-rust
    model: sic-worker
- id: sic-core-rust-dsh
  config:
    provider: sic-core-rust
    baseURL: http://127.0.0.1:$proxyPort/v1
    apiKeyEnv: SIC_CORE_TOKEN
    model: sic-worker
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
- id: session-telemetry-otel
  name: '@deepseek-ai/dsh-session-telemetry-otel'
  disabled: true
- id: otel
  name: '@deepseek-ai/dsh-otel'
  disabled: true
"@ | Set-Content -LiteralPath $overlay -Encoding utf8

  & dsh --profile sic-rust-headless --from-default-profile headless --dump-config *> $null
  if ($LASTEXITCODE -ne 0) { throw 'isolated DSH headless profile creation failed' }
  & dsh plugin --profile sic-rust-headless add $plugin *> $null
  if ($LASTEXITCODE -ne 0) { throw 'isolated DSH plugin installation failed' }
  $runOutput = (& dsh --profile sic-rust-headless --patch $overlay 'Reply with a short synthetic fixture acknowledgement. Do not call tools.' 2>&1 | Out-String)
  if ($LASTEXITCODE -ne 0 -or -not $runOutput.Contains('SIC_RUST_DSH_MOCK_ROUNDTRIP_OK')) {
    $failedStatus = Invoke-RestMethod -Uri "http://127.0.0.1:$mockPort/status" -TimeoutSec 3 -NoProxy
    $bridgeStatus = Invoke-RestMethod -Uri "http://127.0.0.1:$proxyPort/status" -TimeoutSec 3 -NoProxy
    $toolEvidence = @($bridgeStatus.observations | ForEach-Object { "auth=$($_.authorizationPresent),session=$($_.bridgeSessionDigest),model=$($_.model),max=$($_.maxTokens),messages=$($_.messageCount),tools=[$($_.toolNames -join ',')]#$($_.toolSchemaHash):$($_.responseStatus)/$($_.errorCode)" }) -join ' -> '
    $providerEvidence = @($failedStatus.calls | ForEach-Object { "model=$($_.model),max=$($_.maxTokens),tools=[$($_.toolNames -join ',')],response=$($_.responseKind)" }) -join ' -> '
    $receiptJson = & python (Join-Path $plugin 'test\receipt-summary.py') $database
    $receipt = if ($LASTEXITCODE -eq 0) { $receiptJson | ConvertFrom-Json | ConvertTo-Json -Compress -Depth 5 } else { 'unavailable' }
    $safeOutput = $runOutput.Replace($env:SIC_CORE_TOKEN, '[REDACTED]').Replace($env:SIC_MOCK_PROVIDER_KEY, '[REDACTED]')
    if ($safeOutput.Length -gt 1200) { $safeOutput = $safeOutput.Substring($safeOutput.Length - 1200) }
    throw "DSH headless did not complete the mock Rust bridge roundtrip; mock requests=$($failedStatus.requestCount); observed filtered tools=[$($observedTools -join ',')]; provider calls: $providerEvidence; bridge requests: $toolEvidence; redacted receipt summary: $receipt; sanitized output: $safeOutput"
  }
  $status = Invoke-RestMethod -Uri "http://127.0.0.1:$mockPort/status" -TimeoutSec 3 -NoProxy
  $bridgeStatus = Invoke-RestMethod -Uri "http://127.0.0.1:$proxyPort/status" -TimeoutSec 3 -NoProxy
  $observedTools = @($bridgeStatus.observations | ForEach-Object { $_.toolNames } | ForEach-Object { $_ } | Sort-Object -Unique)
  $workerKinds = @($status.calls | ForEach-Object { $_.responseKind })
  $fixtureObserved = @($status.calls | Where-Object { $_.responseKind -eq 'worker_final_text' -and $_.fixtureResultObserved }).Count -gt 0
  $mainSessionDigests = @($bridgeStatus.observations | ForEach-Object { $_.bridgeSessionDigest } | Select-Object -Unique)
  if (-not $status.credentialPresent -or $status.invalidRequest -or -not ($workerKinds -contains 'worker_fixture_tool_call') -or -not $fixtureObserved -or $mainSessionDigests.Count -ne 1 -or $observedTools.Count -ne 1 -or $observedTools[0] -ne 'sic_fixture_read') {
    throw 'mock provider did not observe the expected authenticated Core calls'
  }

  $failOutput = (& dsh --profile sic-rust-headless --patch $overlay 'SIC_DSH_NO_RETRY_PROBE: return a short answer.' 2>&1 | Out-String)
  $afterFailure = Invoke-RestMethod -Uri "http://127.0.0.1:$mockPort/status" -TimeoutSec 3 -NoProxy
  if ($afterFailure.failureCount -ne 1) { throw 'expected exactly one provider failure attempt; inspect the isolated run' }
  $finalBridgeStatus = Invoke-RestMethod -Uri "http://127.0.0.1:$proxyPort/status" -TimeoutSec 3 -NoProxy
  $receiptJson = & python (Join-Path $plugin 'test\receipt-summary.py') $database
  if ($LASTEXITCODE -ne 0) { throw 'could not produce the sanitized receipt report' }
  $report = [ordered]@{
    dshVersion = (dsh --version | Out-String).Trim()
    result = 'passed'
    mode = 'native tools; isolated DSH_HOME; temporary Rust DB; local synthetic Chat provider'
    maxTokens = 4096
    providerCalls = $afterFailure.calls
    bridgeRequests = $finalBridgeStatus.observations
    receiptSummary = ($receiptJson | ConvertFrom-Json)
    failureProbeAttempts = $afterFailure.failureCount
  }
  $reportPath = Join-Path $temp 'redacted-report.json'
  $report | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $reportPath -Encoding utf8
  $reportSaved = $true
  Write-Output "PASS: DSH $($report.dshVersion) -> Rust /v1 -> local mock Chat; authenticated provider calls=$($status.requestCount); host fixture executed and observed=$fixtureObserved; failure probe attempts=$($afterFailure.failureCount)."
  Write-Output 'PASS: request cap matched; bearer/session were present; model retry probe made one provider attempt; no real model/network used.'
}
finally {
  if ($core -and -not $core.HasExited) { Stop-Process -Id $core.Id -Force -ErrorAction SilentlyContinue }
  if ($proxy -and -not $proxy.HasExited) { Stop-Process -Id $proxy.Id -Force -ErrorAction SilentlyContinue }
  if ($mock -and -not $mock.HasExited) { Stop-Process -Id $mock.Id -Force -ErrorAction SilentlyContinue }
  foreach ($key in @('DSH_HOME', 'SIC_CORE_TOKEN', 'SIC_MOCK_PROVIDER_KEY', 'SIC_RUST_UPSTREAM', 'DSH_TOOLS_MODE')) {
    if ($null -eq $oldEnv[$key]) { Remove-Item -Path ("Env:" + $key) -ErrorAction SilentlyContinue }
    else { [Environment]::SetEnvironmentVariable($key, $oldEnv[$key], 'Process') }
  }
  if ($keepArtifacts -and $reportSaved) { Write-Output "Redacted synthetic report and isolated DB retained under: $temp" }
  elseif (Test-Path -LiteralPath $temp) {
    $resolved = (Resolve-Path -LiteralPath $temp -ErrorAction Stop).Path
    $item = Get-Item -LiteralPath $resolved -Force -ErrorAction Stop
    if ($resolved -ne $temp -or -not $resolved.StartsWith($tempRoot + [System.IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase) -or
        -not $item.Name.StartsWith('sic-rust-dsh-integration-', [StringComparison]::Ordinal) -or
        ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint)) {
      throw 'Refusing to remove a fixture directory outside the verified target'
    }
    Remove-Item -LiteralPath $resolved -Recurse -Force -ErrorAction Stop
  }
}

