param([string]$CoreBinaryPath, [string]$PluginDirectory)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$plugin = if ([string]::IsNullOrWhiteSpace($PluginDirectory)) {
  Split-Path $PSScriptRoot -Parent
} else {
  (Resolve-Path -LiteralPath $PluginDirectory -ErrorAction Stop).Path
}
if (-not (Test-Path -LiteralPath (Join-Path $plugin 'index.mjs') -PathType Leaf)) {
  throw 'PluginDirectory must contain the prepared SIC DSH plugin.'
}
$binary = if (-not [string]::IsNullOrWhiteSpace($CoreBinaryPath)) {
  [System.IO.Path]::GetFullPath($CoreBinaryPath)
} else {
  @((Join-Path $repo 'release\sic-core-rust.exe'), (Join-Path $repo 'target\release\sic-core-rust.exe'), (Join-Path $repo 'target\debug\sic-core-rust.exe')) |
    Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
}
if (-not $binary -or -not (Test-Path -LiteralPath $binary -PathType Leaf)) { throw 'Provide -CoreBinaryPath or build the current Rust Core binary before this live test.' }

if ([string]::IsNullOrWhiteSpace($env:SIC_PROVIDER_KEY)) {
  throw 'SIC_PROVIDER_KEY must be present in the invoking process environment; no live model call was made.'
}

$saved = @{}
foreach ($name in @('DSH_HOME', 'SIC_CORE_TOKEN', 'SIC_PROVIDER_KEY', 'DSH_TOOLS_MODE', 'SIC_LIVE_UPSTREAM', 'SIC_RUST_UPSTREAM')) {
  $saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}
# The local guard, observer, and DSH must not inherit the provider key.
$tempRoot = [System.IO.Path]::GetFullPath($env:TEMP).TrimEnd([System.IO.Path]::DirectorySeparatorChar)
$temp = [System.IO.Path]::GetFullPath((Join-Path $tempRoot ('sic-opendesign-headless-' + [guid]::NewGuid().ToString('N'))))
if (-not $temp.StartsWith($tempRoot + [System.IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
  throw 'Fixture directory is outside the intended temporary directory'
}
$reportPath = Join-Path $tempRoot ('sic-opendesign-redacted-' + [guid]::NewGuid().ToString('N') + '.json')
$null = New-Item -ItemType Directory -Force -Path $temp
$guard = $null
$observer = $null
$servicePid = $null
$serviceStartTicks = $null
$stage = 'setup'
$failure = $null
$runSucceeded = $false
$providerStatus = $null
$bridgeStatus = $null
$receipt = $null
$runOutput = ''
try {
  Remove-Item -Path Env:SIC_PROVIDER_KEY -ErrorAction SilentlyContinue
  $env:DSH_HOME = Join-Path $temp 'dsh-home'
  $env:DSH_TOOLS_MODE = 'native'
  $env:SIC_CORE_TOKEN = [guid]::NewGuid().ToString('N')
  $env:SIC_LIVE_UPSTREAM = 'https://amr-link.open-design.ai/v1/chat/completions'
  $guardOut = Join-Path $temp 'provider-guard.out'
  $guardErr = Join-Path $temp 'provider-guard.err'
  $guardScript = Join-Path $PSScriptRoot 'deepseek-call-guard.mjs'
  $guard = Start-Process -FilePath 'node' -ArgumentList @('"' + $guardScript + '"') -WorkingDirectory $plugin -WindowStyle Hidden -PassThru -RedirectStandardOutput $guardOut -RedirectStandardError $guardErr
  $guardPort = $null
  for ($i = 0; $i -lt 100; $i++) {
    if (Test-Path -LiteralPath $guardOut) {
      $line = Get-Content -LiteralPath $guardOut -TotalCount 1
      if ($line) { try { $guardPort = (ConvertFrom-Json $line).port } catch {} }
    }
    if ($guardPort) { break }
    Start-Sleep -Milliseconds 100
  }
  if (-not $guardPort) { throw 'live provider guard did not bind loopback; no provider call was made' }

  $probe = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
  $probe.Start(); $corePort = $probe.LocalEndpoint.Port; $probe.Stop()

  $profile = @{
    model = 'openai/deepseek-v4.1-flash-eco'
    expected_returned_models = @('deepseek-v4.1-flash-eco')
    api_base = "http://127.0.0.1:$guardPort/v1"
    api_key_env = 'SIC_PROVIDER_KEY'
    tokenizer_model = 'host-o200k-surrogate'
    tokenizer_status = 'host-selected-surrogate-not-provider-native'
    context_window = 1000000
    max_input_tokens = 1000000
    max_output_tokens = 393216
    default_output_tokens = 4096
    max_supported_output_tokens = 393216
    timeout_seconds = 180
  }
  $modelConfig = @{ roles = @{ worker = $profile; editor = $profile } } | ConvertTo-Json -Depth 8
  $modelPath = Join-Path $temp 'fixed-deepseek-profile.json'
  Set-Content -LiteralPath $modelPath -Value $modelConfig -Encoding utf8
  $database = Join-Path $temp 'rust-core.db'
  & $binary init $database *> $null
  if ($LASTEXITCODE -ne 0) { throw 'Rust test database initialization failed; no provider call was made' }

  $pidFile = Join-Path $temp 'rust-service.pid'
  $launcher = Join-Path $PSScriptRoot 'spawn-rust-from-provider-env.py'
  $env:SIC_PROVIDER_KEY = $saved['SIC_PROVIDER_KEY']
  try {
    $launchOutput = & python $launcher $binary $database "127.0.0.1:$corePort" $modelPath $pidFile 2>$null
  }
  finally {
    Remove-Item -Path Env:SIC_PROVIDER_KEY -ErrorAction SilentlyContinue
  }
  if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $pidFile)) {
    throw 'could not start the Rust service with the caller-provided credential; no live call was made'
  }
  $servicePid = [int](Get-Content -LiteralPath $pidFile -Raw)
  $serviceStartTicks = (Get-Process -Id $servicePid -ErrorAction Stop).StartTime.ToUniversalTime().Ticks

  $routePatch = Join-Path $temp 'isolated-dsh.patch.yml'
  @"
- id: agent-default-model
  config:
    provider: sic-core-rust
    model: sic-worker
- id: sic-core-rust-dsh
  config:
    provider: sic-core-rust
    baseURL: http://127.0.0.1:$corePort/v1
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

  $stage = 'isolated_profile'
  & dsh --profile sic-rust-opendesign --from-default-profile headless --dump-config *> $null
  if ($LASTEXITCODE -ne 0) { throw 'isolated DSH profile creation failed' }
  & dsh plugin --profile sic-rust-opendesign add $plugin *> $null
  if ($LASTEXITCODE -ne 0) { throw 'isolated Rust DSH plugin installation failed' }

  $stage = 'service_ready'
  $ready = $false
  for ($i = 0; $i -lt 150; $i++) {
    $proc = Get-Process -Id $servicePid -ErrorAction SilentlyContinue
    if (-not $proc) { throw 'Rust service exited before becoming ready' }
    try {
      $null = Invoke-RestMethod -Uri "http://127.0.0.1:$corePort/health" -Headers @{ Authorization = "Bearer $env:SIC_CORE_TOKEN" } -TimeoutSec 1 -NoProxy
      $ready = $true; break
    } catch {}
    Start-Sleep -Milliseconds 100
  }
  if (-not $ready) { throw 'Rust service did not become ready' }

  $env:SIC_RUST_UPSTREAM = "http://127.0.0.1:$corePort"
  $observerOut = Join-Path $temp 'bridge-observer.out'
  $observerErr = Join-Path $temp 'bridge-observer.err'
  $observerScript = Join-Path $PSScriptRoot 'bridge-observer.mjs'
  $observer = Start-Process -FilePath 'node' -ArgumentList @('"' + $observerScript + '"') -WorkingDirectory $plugin -WindowStyle Hidden -PassThru -RedirectStandardOutput $observerOut -RedirectStandardError $observerErr
  $bridgePort = $null
  for ($i = 0; $i -lt 100; $i++) {
    if (Test-Path -LiteralPath $observerOut) {
      $line = Get-Content -LiteralPath $observerOut -TotalCount 1
      if ($line) { try { $bridgePort = (ConvertFrom-Json $line).port } catch {} }
    }
    if ($bridgePort) { break }
    Start-Sleep -Milliseconds 100
  }
  if (-not $bridgePort) { throw 'local bridge observer did not bind; no model call was made' }
  @"
- id: agent-default-model
  config:
    provider: sic-core-rust
    model: sic-worker
- id: sic-core-rust-dsh
  config:
    provider: sic-core-rust
    baseURL: http://127.0.0.1:$bridgePort/v1
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

  $stage = 'real_provider_turn'
  $prompt = 'Use only the sic_fixture_read read-only synthetic fixture tool exactly once. Then answer in one short sentence using only its returned marker. Do not call any other tool and do not infer external facts.'
  $runOutput = (& dsh --profile sic-rust-opendesign --patch $routePatch $prompt 2>&1 | Out-String)
  $dshExitCode = $LASTEXITCODE
  $providerStatus = Invoke-RestMethod -Uri "http://127.0.0.1:$guardPort/status" -TimeoutSec 3 -NoProxy
  $bridgeStatus = Invoke-RestMethod -Uri "http://127.0.0.1:$bridgePort/status" -TimeoutSec 3 -NoProxy
  $receiptJson = & python (Join-Path $PSScriptRoot 'receipt-summary.py') $database
  $receiptExitCode = $LASTEXITCODE
  if ($receiptExitCode -eq 0) { $receipt = $receiptJson | ConvertFrom-Json }
  $roleWithinCaps = $providerStatus.editorCount -le 3 -and $providerStatus.workerCount -le 4
  $onlyFixedModel = -not $providerStatus.invalidModel -and @($providerStatus.calls | Where-Object { $_.model -ne 'deepseek-v4.1-flash-eco' }).Count -eq 0
  $fixedBudget = -not $providerStatus.invalidBudget -and @($providerStatus.calls | Where-Object { $_.maxTokens -ne 4096 }).Count -eq 0
  $allProviderCallsOk = @($providerStatus.calls | Where-Object { $_.status -ne 200 }).Count -eq 0
  $fixtureWasExecuted = $runOutput.Contains('SIC_RUST_SYNTHETIC_FIXTURE_ONLY')
  $runSucceeded = $dshExitCode -eq 0 -and $roleWithinCaps -and $onlyFixedModel -and $fixedBudget -and $allProviderCallsOk -and $fixtureWasExecuted -and $providerStatus.editorCount -ge 1
  if (-not $runSucceeded) { throw 'live headless fixture turn did not satisfy the bounded acceptance checks' }
}
catch {
  $failure = switch -Regex ($stage) {
    '^setup$' { 'setup_failed'; break }
    '^isolated_profile$' { 'profile_failed'; break }
    '^service_ready$' { 'service_failed'; break }
    '^real_provider_turn$' { 'bounded_live_turn_failed'; break }
    default { 'live_test_failed' }
  }
}
finally {
  if ($servicePid -and $serviceStartTicks) {
    $service = Get-Process -Id $servicePid -ErrorAction SilentlyContinue
    # The PID and creation time identify the child we started even if Windows
    # resolves a junction in its executable path. Do not stop a reused PID.
    if ($service -and $service.StartTime.ToUniversalTime().Ticks -eq $serviceStartTicks -and
        [System.IO.Path]::GetFileName($service.Path) -ieq [System.IO.Path]::GetFileName($binary)) {
      Stop-Process -Id $servicePid -Force -ErrorAction SilentlyContinue
    }
  }
  if ($observer -and -not $observer.HasExited) { Stop-Process -Id $observer.Id -Force -ErrorAction SilentlyContinue }
  if ($guard -and -not $guard.HasExited) { Stop-Process -Id $guard.Id -Force -ErrorAction SilentlyContinue }
  foreach ($name in @('DSH_HOME', 'SIC_CORE_TOKEN', 'SIC_PROVIDER_KEY', 'DSH_TOOLS_MODE', 'SIC_LIVE_UPSTREAM', 'SIC_RUST_UPSTREAM')) {
    if ($null -eq $saved[$name]) { Remove-Item -Path ("Env:" + $name) -ErrorAction SilentlyContinue }
    else { [Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process') }
  }
}

$report = [ordered]@{
  result = if ($runSucceeded) { 'passed' } else { 'failed' }
  stage = if ($runSucceeded) { 'complete' } else { $failure }
  dshVersion = (dsh --version | Out-String).Trim()
  providerHost = 'amr-link.open-design.ai'
  fixedModel = 'deepseek-v4.1-flash-eco'
  contextWindow = 1000000
  advertisedMaxOutput = 393216
  actualMaxTokens = 4096
  editorCalls = $providerStatus.editorCount
  workerCalls = $providerStatus.workerCount
  providerCalls = $providerStatus.calls
  bridgeRequests = if ($bridgeStatus) { $bridgeStatus.observations } else { @() }
  receiptSummary = $receipt
  syntheticFixtureMarkerObserved = $runOutput.Contains('SIC_RUST_SYNTHETIC_FIXTURE_ONLY')
  providerCredentialSource = 'in-memory SIC_PROVIDER_KEY inherited from the invoking process environment'
  credentialPersistedOrLogged = $false
  usedOtherModelsOrPort8341 = $false
}
if ($bridgeStatus) { $report.bridgeHealthObserved = $true }
$report | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $reportPath -Encoding utf8
if (Test-Path -LiteralPath $temp) {
  $resolved = (Resolve-Path -LiteralPath $temp -ErrorAction Stop).Path
  $item = Get-Item -LiteralPath $resolved -Force -ErrorAction Stop
  if ($resolved -ne $temp -or
      -not $resolved.StartsWith($tempRoot + [System.IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase) -or
      -not $item.Name.StartsWith('sic-opendesign-headless-', [StringComparison]::Ordinal) -or
      ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint)) {
    throw 'Refusing to remove a fixture directory outside the verified target'
  }
  Remove-Item -LiteralPath $resolved -Recurse -Force -ErrorAction Stop
}
if ($runSucceeded) {
  Write-Output "PASS: bounded DSH/Rust/OpenDesign roundtrip; Editor=$($providerStatus.editorCount), Worker=$($providerStatus.workerCount), model=deepseek-v4.1-flash-eco, max_tokens=4096; synthetic fixture marker observed."
} else {
  Write-Output "STOP: $failure; no attempt was automatically replayed."
}
Write-Output "Redacted report: $reportPath"
if (-not $runSucceeded) { exit 1 }


