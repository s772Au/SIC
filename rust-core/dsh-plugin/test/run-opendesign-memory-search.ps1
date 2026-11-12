param(
    [Parameter(Mandatory = $true)][string]$CoreBinaryPath,
    [Parameter(Mandatory = $true)][string]$PluginDirectory,
    [string]$FixtureRoot = $env:TEMP,
    [string]$DshCredentialHome
)

$ErrorActionPreference = 'Stop'
$coreRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$root = (Resolve-Path -LiteralPath $FixtureRoot -ErrorAction Stop).Path
$fixture = Join-Path $root ('sic-memory-search-live-' + [guid]::NewGuid().ToString('N'))
$null = New-Item -ItemType Directory -Path $fixture
$savedUpstream = [Environment]::GetEnvironmentVariable('SIC_LIVE_UPSTREAM', 'Process')
$guard = $null
try {
    $env:SIC_LIVE_UPSTREAM = 'https://amr-link.open-design.ai/v1/chat/completions'
    $guardOut = Join-Path $fixture 'guard.out'
    $guardErr = Join-Path $fixture 'guard.err'
    $guardScript = Join-Path $PSScriptRoot 'deepseek-call-guard.mjs'
    $guard = Start-Process -FilePath node -ArgumentList @('"' + $guardScript + '"') -WindowStyle Hidden -PassThru -RedirectStandardOutput $guardOut -RedirectStandardError $guardErr
    $port = $null
    for ($i = 0; $i -lt 100; $i++) {
        if (Test-Path -LiteralPath $guardOut) { try { $port = (Get-Content -LiteralPath $guardOut -TotalCount 1 | ConvertFrom-Json).port } catch {} }
        if ($port) { break }
        Start-Sleep -Milliseconds 100
    }
    if (-not $port) { throw 'The fixed-model guard did not start; no model call was made.' }
    $profile = @{ model = 'openai/deepseek-v4.1-flash-eco'; expected_returned_models = @('deepseek-v4.1-flash-eco'); api_base = "http://127.0.0.1:$port/v1"; api_key_env = 'SIC_PROVIDER_KEY'; tokenizer_model = 'host-o200k-surrogate'; context_window = 1000000; max_input_tokens = 1000000; max_output_tokens = 393216; default_output_tokens = 4096; timeout_seconds = 180; retry_editor_http_5xx = $true }
    $modelPath = Join-Path $fixture 'model.json'
    @{ roles = @{ worker = $profile; editor = $profile } } | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $modelPath -Encoding utf8
    $state = Join-Path $fixture 'state'
    $launcher = Join-Path $coreRoot 'scripts\run-dsh-headless.ps1'
    $marker = 'SICSEARCHMARKER739241'
    $launchParams = @{
        StateDirectory = $state; WorkingDirectory = $fixture
        Prompt = "For this synthetic test remember the exact marker $marker. Reply STORED."
        CoreBinaryPath = $CoreBinaryPath; PluginDirectory = $PluginDirectory; ModelConfigPath = $modelPath
        AllowedTools = @('read'); DshCredentialMap = @{ SIC_PROVIDER_KEY = 'OPENDESIGN_API_KEY' }
    }
    if ($DshCredentialHome) { $launchParams.DshCredentialHome = $DshCredentialHome }
    $firstOutput = & $launcher @launchParams | Out-String
    $database = Join-Path $state 'sic-rust.sqlite'
    $sessionCode = 'import sqlite3,sys; c=sqlite3.connect(sys.argv[1]); r=c.execute("SELECT session_key FROM dsh_bridge_sessions").fetchall(); print(r[0][0] if len(r)==1 else "")'
    $session = (& python -c $sessionCode $database | Out-String).Trim()
    if (-not $session) { throw 'The first DSH session was not recorded.' }
    $launchParams.SessionId = $session
    $launchParams.Prompt = "Call sic_memory_search with query $marker to find its earlier source and then briefly report the CB and WB identifiers. This is a synthetic verification; do not invoke external tools."
    $secondOutput = & $launcher @launchParams | Out-String
    $verification = & python (Join-Path $PSScriptRoot 'verify-memory-search-fixture.py') $database $marker
    if ($LASTEXITCODE -ne 0) { throw 'Real DSH run did not produce a verified memory-search tool result.' }
    $status = Invoke-RestMethod -Uri "http://127.0.0.1:$port/status" -NoProxy -TimeoutSec 3
    if ($status.invalidModel -or $status.invalidBudget -or @($status.calls | Where-Object { $_.status -ne 200 }).Count -gt 0) {
        throw 'The fixed-model guard recorded a rejected request.'
    }
    $reportPath = Join-Path $fixture 'redacted-report.json'
    @{ result = 'passed'; model = 'deepseek-v4.1-flash-eco'; actual_max_tokens = 4096; editor_calls = $status.editorCount; worker_calls = $status.workerCount; provider_calls = $status.calls; verification = ($verification | ConvertFrom-Json); prompt_or_credential_in_report = $false } |
        ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $reportPath -Encoding utf8
    Write-Output "PASS: real DSH and the fixed Provider executed an internally verified memory-search roundtrip. Redacted report: $reportPath"
} finally {
    if ($guard -and -not $guard.HasExited) { Stop-Process -Id $guard.Id -ErrorAction SilentlyContinue }
    if ($null -eq $savedUpstream) { Remove-Item Env:SIC_LIVE_UPSTREAM -ErrorAction SilentlyContinue }
    else { [Environment]::SetEnvironmentVariable('SIC_LIVE_UPSTREAM', $savedUpstream, 'Process') }
}
