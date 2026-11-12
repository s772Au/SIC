param(
    [Parameter(Mandatory = $true)][string]$CoreBinaryPath,
    [Parameter(Mandatory = $true)][string]$PluginDirectory,
    [string]$FixtureRoot = $env:TEMP
)

$ErrorActionPreference = 'Stop'
$coreRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$fixtureRootPath = (Resolve-Path -LiteralPath $FixtureRoot -ErrorAction Stop).Path
$fixture = Join-Path $fixtureRootPath ('sic-launcher-fixture-' + [guid]::NewGuid().ToString('N'))
$null = New-Item -ItemType Directory -Path $fixture
$mock = $null
$saved = @{}
foreach ($name in @('SIC_MOCK_PROVIDER_KEY', 'SIC_PROVIDER_KEY', 'LAUNCHER_FAKE_KEY_FOR_CHECK')) {
    $saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}
try {
    $env:SIC_MOCK_PROVIDER_KEY = 'synthetic-launcher-key-' + [guid]::NewGuid().ToString('N')
    $env:SIC_PROVIDER_KEY = $env:SIC_MOCK_PROVIDER_KEY
    $mockScript = Join-Path $PSScriptRoot 'mock-chat-provider.mjs'
    $mockOut = Join-Path $fixture 'mock.out'; $mockErr = Join-Path $fixture 'mock.err'
    $mock = Start-Process -FilePath node -ArgumentList @('"' + $mockScript + '"') -WindowStyle Hidden -PassThru -RedirectStandardOutput $mockOut -RedirectStandardError $mockErr
    $port = $null
    for ($i = 0; $i -lt 100; $i++) {
        if (Test-Path -LiteralPath $mockOut) { try { $port = (Get-Content -LiteralPath $mockOut -TotalCount 1 | ConvertFrom-Json).port } catch {} }
        if ($port) { break }
        Start-Sleep -Milliseconds 100
    }
    if (-not $port) { throw 'Local mock did not start.' }
    $profile = @{ model = 'openai/mock-model'; api_base = "http://127.0.0.1:$port/v1"; api_key_env = 'SIC_PROVIDER_KEY'; tokenizer_model = 'host-o200k-surrogate'; context_window = 1000000; max_input_tokens = 1000000; max_output_tokens = 393216; default_output_tokens = 4096; timeout_seconds = 20 }
    $modelPath = Join-Path $fixture 'model.json'
    @{ roles = @{ worker = $profile; editor = $profile } } | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $modelPath -Encoding utf8
    $launcher = Join-Path $coreRoot 'scripts\run-dsh-headless.ps1'
    $state = Join-Path $fixture 'state'
    $launchParams = @{
        StateDirectory = $state; WorkingDirectory = $fixture; Prompt = 'Run the synthetic fixture tool and return its marker.'
        CoreBinaryPath = $CoreBinaryPath; PluginDirectory = $PluginDirectory; ModelConfigPath = $modelPath
        AllowedTools = @('sic_fixture_read'); FixtureReadTool = $true; JsonEvents = $true
    }
    $env:SIC_PROVIDER_KEY = $null
    $missingRejected = $false
    try { $null = & $launcher @launchParams } catch { $missingRejected = $_.Exception.Message -match 'credential environment reference' }
    if (-not $missingRejected -or (Test-Path -LiteralPath $state)) { throw 'Missing-key preflight changed state or did not fail closed.' }
    $credentialHome = Join-Path $fixture 'credential-home'
    $null = New-Item -ItemType Directory -Path $credentialHome
    @"
version: 1
refs:
  OPENDESIGN_API_KEY: '$($env:SIC_MOCK_PROVIDER_KEY)'
"@ | Set-Content -LiteralPath (Join-Path $credentialHome '.credentials.yaml') -Encoding utf8
    $launchParams.DshCredentialHome = $credentialHome
    $launchParams.DshCredentialMap = @{ SIC_PROVIDER_KEY = 'MISSING_TEST_REFERENCE' }
    $badReferenceRejected = $false
    try { $null = & $launcher @launchParams } catch { $badReferenceRejected = $_.Exception.Message -match 'credential reference is unavailable' }
    if (-not $badReferenceRejected -or (Test-Path -LiteralPath $state)) { throw 'Invalid DSH credential reference did not fail closed.' }
    $launchParams.DshCredentialMap = @{ SIC_PROVIDER_KEY = 'OPENDESIGN_API_KEY' }
    $foreignState = Join-Path $fixture 'unmanaged-state'
    $null = New-Item -ItemType Directory -Path $foreignState
    Set-Content -LiteralPath (Join-Path $foreignState 'untouched.txt') -Value 'fixture sentinel' -Encoding utf8
    $launchParams.StateDirectory = $foreignState
    $foreignRejected = $false
    try { $null = & $launcher @launchParams } catch { $foreignRejected = $_.Exception.Message -match 'nonempty' }
    if (-not $foreignRejected -or @(Get-ChildItem -LiteralPath $foreignState -Force).Count -ne 1 -or
        (Get-Content -LiteralPath (Join-Path $foreignState 'untouched.txt') -Raw).Trim() -ne 'fixture sentinel') {
        throw 'Unmanaged state directory was not preserved.'
    }
    $launchParams.StateDirectory = $state
    $first = & $launcher @launchParams | Out-String
    if (-not $first.Contains('SIC_RUST_DSH_MOCK_ROUNDTRIP_OK')) { throw 'First fixture output is missing.' }
    $database = Join-Path $state 'sic-rust.sqlite'
    $sessionCode = 'import sqlite3,sys; c=sqlite3.connect(sys.argv[1]); r=c.execute("SELECT session_key FROM dsh_bridge_sessions").fetchall(); print(r[0][0] if len(r)==1 else "")'
    $session = (& python -c $sessionCode $database | Out-String).Trim()
    if (-not $session) { throw 'Fixture session identity is missing.' }
    $events = @($first -split "`r?`n" | Where-Object { $_.Trim() } | ForEach-Object { $_ | ConvertFrom-Json -ErrorAction Stop })
    $sessionEvents = @($events | Where-Object { $_.type -eq 'session' })
    if ($sessionEvents.Count -ne 1 -or $sessionEvents[0].sessionId -ne $session) { throw 'DSH JSON session event is missing or differs from Core.' }
    $launchParams.SessionId = $session
    $launchParams.Prompt = 'Continue the same synthetic session.'
    $second = & $launcher @launchParams | Out-String
    if (-not $second.Contains('SIC_RUST_DSH_MOCK_ROUNDTRIP_OK')) { throw 'Resumed fixture output is missing.' }
    $held = [System.IO.File]::Open((Join-Path $state 'sic-launcher.lock'), [System.IO.FileMode]::Open, [System.IO.FileAccess]::ReadWrite, [System.IO.FileShare]::None)
    try {
        $lockedRejected = $false
        try { $null = & $launcher @launchParams } catch { $lockedRejected = $_.Exception.Message -match 'Another launcher' }
        if (-not $lockedRejected) { throw 'Concurrent launcher did not fail closed.' }
    } finally { $held.Dispose() }
    $status = Invoke-RestMethod -Uri "http://127.0.0.1:$port/status" -NoProxy -TimeoutSec 3
    if (-not $status.credentialPresent -or $status.invalidRequest) { throw 'Mock Provider verification failed.' }
    if ($null -ne [Environment]::GetEnvironmentVariable('SIC_PROVIDER_KEY', 'Process')) { throw 'A DSH-managed credential leaked into the caller environment.' }
    $env:LAUNCHER_FAKE_KEY_FOR_CHECK = $env:SIC_MOCK_PROVIDER_KEY
    $scanCode = 'import os,pathlib,sys; root=pathlib.Path(sys.argv[1]); key=os.environ["LAUNCHER_FAKE_KEY_FOR_CHECK"].encode(); files=[p for p in root.rglob("*") if p.is_file() and "node_modules" not in p.parts and "credential-home" not in p.parts and not p.is_symlink()]; print(sum(key in p.read_bytes() for p in files))'
    $leaks = (& python -c $scanCode $fixture | Out-String).Trim()
    if ($leaks -ne '0') { throw 'Synthetic credential was persisted.' }
    Write-Output "PASS: isolated launcher resolved a synthetic DSH credential, completed the mock tool roundtrip and resumed its session; invalid references, concurrent launch and unmanaged state were rejected, and no credential was copied to launcher state. Fixture: $fixture"
}
finally {
    if ($mock -and -not $mock.HasExited) { Stop-Process -Id $mock.Id -ErrorAction SilentlyContinue }
    foreach ($name in $saved.Keys) { [Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process') }
}
