$ErrorActionPreference = 'Stop'
$plugin = Split-Path $PSScriptRoot -Parent
$oldEnv = @{}
foreach ($key in @('DSH_HOME', 'SIC_CORE_TOKEN', 'DSH_TOOLS_MODE')) {
  $oldEnv[$key] = [Environment]::GetEnvironmentVariable($key, 'Process')
}
$tempRoot = [System.IO.Path]::GetFullPath($env:TEMP).TrimEnd([System.IO.Path]::DirectorySeparatorChar)
$temp = [System.IO.Path]::GetFullPath((Join-Path $tempRoot ('sic-bind-headless-' + [guid]::NewGuid().ToString('N'))))
if (-not $temp.StartsWith($tempRoot + [System.IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
  throw 'Fixture directory is outside the intended temporary directory'
}
$null = New-Item -ItemType Directory -Force -Path $temp
$mock = $null
$reportPath = Join-Path $env:TEMP ('sic-bind-headless-redacted-' + [guid]::NewGuid().ToString('N') + '.json')
$passed = $false
$status = $null
$safeFailure = $null
try {
  $env:DSH_HOME = Join-Path $temp 'dsh-home'
  $env:DSH_TOOLS_MODE = 'native'
  $env:SIC_CORE_TOKEN = [guid]::NewGuid().ToString('N')
  $mockOut = Join-Path $temp 'mock-bridge.out'
  $mockErr = Join-Path $temp 'mock-bridge.err'
  $mock = Start-Process -FilePath 'node' -ArgumentList @('"' + (Join-Path $plugin 'test\mock-bind-bridge.mjs') + '"') -WorkingDirectory $plugin -WindowStyle Hidden -PassThru -RedirectStandardOutput $mockOut -RedirectStandardError $mockErr
  $mockPort = $null
  for ($i = 0; $i -lt 100; $i++) {
    if (Test-Path -LiteralPath $mockOut) {
      $line = Get-Content -LiteralPath $mockOut -TotalCount 1
      if ($line) { try { $mockPort = (ConvertFrom-Json $line).port } catch {} }
    }
    if ($mockPort) { break }
    Start-Sleep -Milliseconds 100
  }
  if (-not $mockPort) { throw 'mock bridge failed to bind loopback' }

  $routePatch = Join-Path $temp 'bind-headless.patch.yml'
  @"
- id: agent-default-model
  config:
    provider: sic-core-rust
    model: sic-worker
- id: sic-core-rust-dsh
  config:
    provider: sic-core-rust
    baseURL: http://127.0.0.1:$mockPort/v1
    apiKeyEnv: SIC_CORE_TOKEN
    model: sic-worker
    bindChainId: synthetic-migrated-chain
    bindTimeoutMs: 3000
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

  & dsh --profile sic-bind-headless --from-default-profile headless --dump-config *> $null
  if ($LASTEXITCODE -ne 0) { throw 'isolated DSH profile creation failed' }
  & dsh plugin --profile sic-bind-headless add $plugin *> $null
  if ($LASTEXITCODE -ne 0) { throw 'isolated SIC plugin installation failed' }
  $runOutput = (& dsh --profile sic-bind-headless --patch $routePatch 'Use sic_fixture_read exactly once, then answer with its returned marker.' 2>&1 | Out-String)
  if ($LASTEXITCODE -ne 0 -or -not $runOutput.Contains('SIC_BIND_HEADLESS_ROUNDTRIP_OK')) {
    throw 'headless mock Agent did not complete the fixture roundtrip'
  }
  $status = Invoke-RestMethod -Uri "http://127.0.0.1:$mockPort/status" -TimeoutSec 3 -NoProxy
  $ordered = (@($status.order) -join ',') -eq 'bind,chat,chat'
  $sameSession = $status.bindings.Count -eq 1 -and $status.chats.Count -eq 2 -and
    @($status.chats | Where-Object { $_.sessionId -ne $status.bindings[0].sessionId }).Count -eq 0
  $fixtureObserved = @($status.chats | Where-Object { $_.fixtureResultObserved }).Count -eq 1
  $onlyFixtureTool = @($status.chats | Where-Object { $_.toolNames.Count -ne 1 -or $_.toolNames[0] -ne 'sic_fixture_read' }).Count -eq 0
  $passed = $status.authValid -and $status.protocolValid -and $ordered -and $sameSession -and $fixtureObserved -and $onlyFixtureTool
  if (-not $passed) { throw 'bind ordering, session identity, tool allowlist, or fixture result check failed' }
}
catch {
  $safeFailure = 'isolated_bind_headless_failed'
}
finally {
  if ($mock -and -not $mock.HasExited) { Stop-Process -Id $mock.Id -Force -ErrorAction SilentlyContinue }
  foreach ($key in @('DSH_HOME', 'SIC_CORE_TOKEN', 'DSH_TOOLS_MODE')) {
    if ($null -eq $oldEnv[$key]) { Remove-Item -Path ("Env:" + $key) -ErrorAction SilentlyContinue }
    else { [Environment]::SetEnvironmentVariable($key, $oldEnv[$key], 'Process') }
  }
}

$report = [ordered]@{
  result = if ($passed) { 'passed' } else { 'failed' }
  failure = $safeFailure
  dshVersion = (dsh --version | Out-String).Trim()
  bridge = 'local synthetic Rust Agent; no real model or external tools'
  chainBindingEnabled = $true
  bindBeforeChat = if ($status) { (@($status.order).IndexOf('bind') -ge 0 -and @($status.order).IndexOf('bind') -lt @($status.order).IndexOf('chat')) } else { $false }
  bindingCount = if ($status) { $status.bindings.Count } else { 0 }
  chatCount = if ($status) { $status.chats.Count } else { 0 }
  oneSessionAcrossBindingAndChat = if ($status -and $status.bindings.Count -eq 1) { @($status.chats | Where-Object { $_.sessionId -ne $status.bindings[0].sessionId }).Count -eq 0 } else { $false }
  onlyFixtureTool = if ($status) { @($status.chats | Where-Object { $_.toolNames.Count -ne 1 -or $_.toolNames[0] -ne 'sic_fixture_read' }).Count -eq 0 } else { $false }
  syntheticFixtureReturned = if ($status) { @($status.chats | Where-Object { $_.fixtureResultObserved }).Count -eq 1 } else { $false }
  bearerValid = if ($status) { $status.authValid } else { $false }
  protocolValid = if ($status) { $status.protocolValid } else { $false }
  order = if ($status) { @($status.order) } else { @() }
}
$report | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $reportPath -Encoding utf8
if (Test-Path -LiteralPath $temp) {
  $resolved = (Resolve-Path -LiteralPath $temp -ErrorAction Stop).Path
  $item = Get-Item -LiteralPath $resolved -Force -ErrorAction Stop
  if ($resolved -ne $temp -or -not $resolved.StartsWith($tempRoot + [System.IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase) -or
      -not $item.Name.StartsWith('sic-bind-headless-', [StringComparison]::Ordinal) -or
      ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint)) {
    throw 'Refusing to remove a fixture directory outside the verified target'
  }
  Remove-Item -LiteralPath $resolved -Recurse -Force -ErrorAction Stop
}
if ($passed) { Write-Output "PASS: DSH headless bind-before-chat mock; chain bound once, same generated session used for both chats, fixture executed. Report: $reportPath" }
else { Write-Output "STOP: $safeFailure. Report: $reportPath"; exit 1 }
