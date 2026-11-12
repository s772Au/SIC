param([int]$TimeoutSeconds = 600, [switch]$VerifyNativeRecovery)
# Isolated DSH web + headless + Rust + synthetic Provider, no production profile.
# Stop by creating <fixture-root>\done. SIC and Provider bearers are never
# written to disk or output. The isolated web service writes its own temporary
# access token to web.out, which remains private to the fixture directory.
$ErrorActionPreference = 'Stop'
if ($TimeoutSeconds -lt 30 -or $TimeoutSeconds -gt 1800) { throw 'Invalid timeout.' }
$plugin = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$coreRoot = (Resolve-Path (Join-Path $plugin '..')).Path
$binary = Join-Path $coreRoot 'release\sic-core-rust.exe'
if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) { throw 'Fixture needs the isolated runtime binary.' }
$tempRoot = [System.IO.Path]::GetFullPath($env:TEMP).TrimEnd('\')
$temp = [System.IO.Path]::GetFullPath((Join-Path $tempRoot ('sic-web-view-ui-' + [guid]::NewGuid().ToString('N'))))
if (-not $temp.StartsWith($tempRoot + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Invalid fixture root.' }
New-Item -ItemType Directory -Path $temp | Out-Null
$saved = @{}
foreach ($name in @('DSH_HOME','SIC_CORE_TOKEN','SIC_MOCK_PROVIDER_KEY','DSH_TOOLS_MODE','ELECTRON_RUN_AS_NODE')) {
  $saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}
$mock = $null; $core = $null; $web = $null
function FreePort {
  $probe = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
  $probe.Start(); $port = $probe.LocalEndpoint.Port; $probe.Stop(); return $port
}
try {
  $env:DSH_HOME = Join-Path $temp 'dsh-home'
  $env:DSH_TOOLS_MODE = 'native'
  $env:SIC_CORE_TOKEN = [guid]::NewGuid().ToString('N')
  $env:SIC_MOCK_PROVIDER_KEY = [guid]::NewGuid().ToString('N')
  $mockOut = Join-Path $temp 'mock.out'; $mockErr = Join-Path $temp 'mock.err'
  $mockScript = Join-Path $plugin 'test\native-read-mock-chat-provider.mjs'
  $mock = Start-Process -FilePath 'node' -ArgumentList @(('"' + $mockScript + '"')) -WorkingDirectory $plugin -WindowStyle Hidden -PassThru -RedirectStandardOutput $mockOut -RedirectStandardError $mockErr
  $mockPort = $null
  for ($i=0; $i -lt 100; $i++) { if (Test-Path $mockOut) { try { $mockPort = (Get-Content $mockOut -TotalCount 1 | ConvertFrom-Json).port } catch {} }; if ($mockPort) { break }; Start-Sleep -Milliseconds 100 }
  if (-not $mockPort) { throw 'Synthetic Provider startup failed.' }
  $profile = @{ model='openai/mock-model'; api_base="http://127.0.0.1:$mockPort/v1"; api_key_env='SIC_MOCK_PROVIDER_KEY'; tokenizer_model='host-o200k-surrogate'; tokenizer_status='local-fixture'; context_window=1000000; max_input_tokens=1000000; max_output_tokens=393216; default_output_tokens=4096; timeout_seconds=20 }
  $modelPath = Join-Path $temp 'model.json'; @{ roles=@{ worker=$profile; editor=$profile } } | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $modelPath -Encoding utf8
  $database = Join-Path $temp 'fixture.sqlite'; & $binary init $database *> $null
  if ($LASTEXITCODE -ne 0) { throw 'Synthetic database initialization failed.' }
  $corePort = FreePort
  $core = Start-Process -FilePath $binary -ArgumentList @('serve', ('"' + $database + '"'), "127.0.0.1:$corePort", 'web-fixture-actor', 'SIC_CORE_TOKEN', '--allow-model', '--model-config', ('"' + $modelPath + '"'), '--dsh-alias', 'sic-worker') -WorkingDirectory $temp -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $temp 'core.out') -RedirectStandardError (Join-Path $temp 'core.err')
  $ready=$false
  $headers=@{ Authorization="Bearer $env:SIC_CORE_TOKEN" }
  for ($i=0; $i -lt 100; $i++) { try { $null=Invoke-RestMethod -Uri "http://127.0.0.1:$corePort/health" -Headers $headers -TimeoutSec 1 -NoProxy; $ready=$true; break } catch { Start-Sleep -Milliseconds 100 } }
  if (-not $ready) { throw 'Synthetic Rust service did not become ready.' }
  foreach ($pair in @(@('sic-headless-fixture','headless'),@('sic-web-fixture','web'))) {
    & dsh --profile $pair[0] --from-default-profile $pair[1] --dump-config *> $null
    if ($LASTEXITCODE -ne 0) { throw 'Isolated DSH profile creation failed.' }
    & dsh plugin --profile $pair[0] add $plugin *> $null
    if ($LASTEXITCODE -ne 0) { throw 'Isolated DSH plugin installation failed.' }
  }
  $patch = Join-Path $temp 'fixture.patch.yml'
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
    allowedTools: [read]
    fixtureReadTool: false
- id: session-title-llm
  disabled: true
- id: llm-deepseek
  disabled: true
- id: llm-deepseek-account
  disabled: true
- id: web-search-deepseek
  disabled: true
- id: web-deepseek-api
  disabled: true
- id: session-telemetry-otel
  disabled: true
- id: otel
  disabled: true
"@ | Set-Content -LiteralPath $patch -Encoding utf8
  Push-Location -LiteralPath $temp
  try {
    [System.IO.File]::WriteAllText((Join-Path $temp 'SIC_TOOL_FIXTURE.txt'), 'SIC_RUST_SYNTHETIC_FIXTURE_ONLY')
    & dsh --profile sic-headless-fixture --patch $patch 'SIC synthetic UI session, reply briefly. SIC_PEER_CHILD_TASK_SYNTHETIC Read SIC_TOOL_FIXTURE.txt.' *> (Join-Path $temp 'headless.out')
  }
  finally { Pop-Location }
  if ($LASTEXITCODE -ne 0) { throw 'Synthetic DSH session failed.' }
  $session = & python -c "import sqlite3,sys; d=sqlite3.connect(sys.argv[1]); r=d.execute('select session_key from dsh_bridge_sessions').fetchall(); print(r[0][0] if len(r)==1 else '')" $database
  if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($session)) { throw 'Synthetic session identity missing.' }
  if ($VerifyNativeRecovery) {
    # Simulate an observer outage only in this freshly created synthetic DB.
    # The authoritative Core tool observation and DSH session log stay intact.
    $original = & python -c "import json,sqlite3,sys; d=sqlite3.connect(sys.argv[1]); r=d.execute('select call_id,event_seq,outcome from dsh_bridge_tool_events').fetchall(); assert len(r)==1; print(json.dumps(r[0]))" $database
    if ($LASTEXITCODE -ne 0) { throw 'Synthetic original event was unavailable.' }
    $original = $original | ConvertFrom-Json
    & python -c "import sqlite3,sys; d=sqlite3.connect(sys.argv[1]); n=d.execute('select count(*) from dsh_bridge_tool_events').fetchone()[0]; assert n==1; d.execute('delete from dsh_bridge_tool_events'); d.commit()" $database
    if ($LASTEXITCODE -ne 0) { throw 'Synthetic event-loss setup failed.' }
    Push-Location -LiteralPath $temp
    try { & dsh --profile sic-headless-fixture --patch $patch --session-id $session 'Continue the synthetic session briefly.' *> (Join-Path $temp 'resume.out') }
    finally { Pop-Location }
    if ($LASTEXITCODE -ne 0) { throw 'Synthetic DSH session resume failed.' }
    $restored = & python -c "import sqlite3,sys; d=sqlite3.connect(sys.argv[1]); print(d.execute('select count(*) from dsh_bridge_tool_events where call_id=? and event_seq=? and outcome=? and core_observed=1',(sys.argv[2],int(sys.argv[3]),sys.argv[4])).fetchone()[0])" $database ([string]$original[0]) ([string]$original[1]) ([string]$original[2])
    if ($LASTEXITCODE -ne 0 -or $restored.Trim() -ne '1') { throw 'DSH asynchronous native event recovery was not observed.' }
    $report = Join-Path $tempRoot ('sic-native-recovery-redacted-' + [guid]::NewGuid().ToString('N') + '.json')
    @{result='passed'; scenario='missing bridge event after first DSH process exited; resume same session';
      exactCallSequenceAndOutcomeRestored=$true; coreObservationReconciled=$true; remoteModelUsed=$false;
      syntheticDatabaseOnly=$true; credentialsInReport=$false} | ConvertTo-Json -Compress | Set-Content -LiteralPath $report -Encoding utf8
    Write-Output 'Verified: resumed DSH session recovered the missing native event through the query service.'
    Write-Output "Redacted recovery report: $report"
  }
  $webPort = FreePort
  $dshBin = Split-Path -Parent (Get-Command dsh -ErrorAction Stop).Source
  $dshExe = [System.IO.Path]::GetFullPath((Join-Path $dshBin '..\..\..\..\DeepSeek Harness.exe'))
  $dshAsar = [System.IO.Path]::GetFullPath((Join-Path $dshBin '..\..\..\app.asar'))
  $dshCli = Join-Path $dshAsar 'dsh\node_modules\@deepseek-ai\dsh-desktop-host\lib\cli.js'
  $env:ELECTRON_RUN_AS_NODE = '1'
  $web = Start-Process -FilePath $dshExe -ArgumentList @('--expose-internals',('"'+$dshCli+'"'),'--profile','sic-web-fixture','--patch',('"'+$patch+'"'),'--host','127.0.0.1','--port',"$webPort",'--no-open') -WorkingDirectory $temp -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $temp 'web.out') -RedirectStandardError (Join-Path $temp 'web.err')
  $ready=$false; $webUrl=$null; $webOut=Join-Path $temp 'web.out'
  for ($i=0; $i -lt 200; $i++) {
    try {
      if (Test-Path -LiteralPath $webOut) {
        $contents=Get-Content -LiteralPath $webOut -Raw
        if ($contents -match '(http://127\.0\.0\.1:[0-9]+/\?token=[A-Za-z0-9_-]+)') { $webUrl=$Matches[1] }
      }
      if ($webUrl) { $null=Invoke-WebRequest -Uri $webUrl -TimeoutSec 1 -NoProxy; $ready=$true; break }
    } catch {}
    if ($web.HasExited) { break }
    Start-Sleep -Milliseconds 100
  }
  if (-not $ready) {
    $webLog = @((Join-Path $temp 'web.out'), (Join-Path $temp 'web.err')) | ForEach-Object { if (Test-Path -LiteralPath $_) { Get-Content -LiteralPath $_ -Raw } }
    $safeLog = ($webLog -join "`n").Replace($env:SIC_CORE_TOKEN, '[fixture-token-redacted]').Replace($env:SIC_MOCK_PROVIDER_KEY, '[fixture-token-redacted]') -replace '(?i)(token=)[A-Za-z0-9_-]+', '$1[fixture-token-redacted]'
    if ($safeLog.Length -gt 8000) { $safeLog = $safeLog.Substring(0,8000) }
    Write-Output $safeLog
    throw 'Isolated DSH web service did not become ready.'
  }
  Write-Output (@{ root=$temp; url="http://127.0.0.1:$webPort/"; session=$session } | ConvertTo-Json -Compress)
  $deadline=[DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
  while ([DateTime]::UtcNow -lt $deadline -and -not (Test-Path -LiteralPath (Join-Path $temp 'done'))) { Start-Sleep -Milliseconds 250 }
} finally {
  foreach ($process in @($web,$core,$mock)) { if ($process -and -not $process.HasExited) { Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue } }
  foreach ($process in @($web,$core,$mock)) { if ($process) { try { $null = $process.WaitForExit(10000) } catch {} } }
  foreach ($name in @('DSH_HOME','SIC_CORE_TOKEN','SIC_MOCK_PROVIDER_KEY','DSH_TOOLS_MODE','ELECTRON_RUN_AS_NODE')) {
    if ($null -eq $saved[$name]) { Remove-Item -Path ("Env:" + $name) -ErrorAction SilentlyContinue } else { [Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process') }
  }
  if (Test-Path -LiteralPath $temp) {
    $resolved=(Resolve-Path -LiteralPath $temp).Path; $item=Get-Item -LiteralPath $resolved -Force
    if ($resolved -ne $temp -or -not $resolved.StartsWith($tempRoot + '\', [StringComparison]::OrdinalIgnoreCase) -or -not $item.Name.StartsWith('sic-web-view-ui-', [StringComparison]::Ordinal) -or ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint)) { throw 'Refusing to remove an unverified fixture path.' }
    $removed = $false
    for ($attempt=0; $attempt -lt 20 -and -not $removed; $attempt++) {
      try { Remove-Item -LiteralPath $resolved -Recurse -Force -ErrorAction Stop; $removed = $true }
      catch { if ($attempt -eq 19) { throw }; Start-Sleep -Milliseconds 250 }
    }
  }
}
