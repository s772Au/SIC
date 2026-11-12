param(
    [Parameter(Mandatory = $true)][string]$CoreBinaryPath,
    [Parameter(Mandatory = $true)][string]$PluginDirectory,
    [string]$FixtureRoot = $env:TEMP,
    [switch]$InjectInvalidChoice
)

# Two real DSH headless launches against a local synthetic Provider. Only the
# fixture tool is registered; the model requests its schema but never runs it.
$ErrorActionPreference = 'Stop'
$coreRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$root = (Resolve-Path -LiteralPath $FixtureRoot -ErrorAction Stop).Path
$fixture = Join-Path $root ('sic-idle-rotation-' + [guid]::NewGuid().ToString('N'))
$null = New-Item -ItemType Directory -Path $fixture
$saved = @{}
foreach ($name in @('SIC_PROVIDER_KEY', 'SIC_MOCK_PROVIDER_KEY', 'SIC_ROTATION_CYCLE', 'SIC_INJECT_INVALID_CHOICE')) {
    $saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}
$mock = $null
try {
    $env:SIC_MOCK_PROVIDER_KEY = 'synthetic-rotation-' + [guid]::NewGuid().ToString('N')
    $env:SIC_PROVIDER_KEY = $env:SIC_MOCK_PROVIDER_KEY
    $env:SIC_ROTATION_CYCLE = '12'
    $env:SIC_INJECT_INVALID_CHOICE = if ($InjectInvalidChoice) { '1' } else { '0' }
    $mockOut = Join-Path $fixture 'mock.out'; $mockErr = Join-Path $fixture 'mock.err'
    $mockScript = Join-Path $PSScriptRoot 'idle-rotation-mock-provider.mjs'
    $mock = Start-Process -FilePath node -ArgumentList @('"' + $mockScript + '"') -WindowStyle Hidden -PassThru -RedirectStandardOutput $mockOut -RedirectStandardError $mockErr
    $port = $null
    for ($i = 0; $i -lt 100; $i++) {
        if (Test-Path -LiteralPath $mockOut) { try { $port = (Get-Content -LiteralPath $mockOut -TotalCount 1 | ConvertFrom-Json).port } catch {} }
        if ($port) { break }
        Start-Sleep -Milliseconds 100
    }
    if (-not $port) { throw 'Local rotation Provider did not start.' }
    $profile = @{ model = 'openai/mock-model'; api_base = "http://127.0.0.1:$port/v1"; api_key_env = 'SIC_PROVIDER_KEY'; tokenizer_model = 'host-o200k-surrogate'; context_window = 1000000; max_input_tokens = 1000000; max_output_tokens = 393216; default_output_tokens = 4096; timeout_seconds = 20 }
    $modelPath = Join-Path $fixture 'model.json'
    @{ roles = @{ worker = $profile; editor = $profile } } | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $modelPath -Encoding utf8
    $launcher = Join-Path $coreRoot 'scripts\run-dsh-headless.ps1'
    $launchParams = @{
        StateDirectory = (Join-Path $fixture 'state'); WorkingDirectory = $fixture; Prompt = 'Run the isolated dynamic-tool rotation fixture.'
        CoreBinaryPath = $CoreBinaryPath; PluginDirectory = $PluginDirectory; ModelConfigPath = $modelPath
        AllowedTools = @('sic_fixture_read'); FixtureReadTool = $true; JsonEvents = $true; MaxWorkerCallsPerTurn = 16; MaxInvalidChoiceRetries = 1
    }
    for ($round = 1; $round -le 2; $round++) {
        $output = & $launcher @launchParams | Out-String
        if (-not $output.Contains('SIC_ROTATION_ROUND_OK')) { throw 'DSH did not complete the synthetic Worker sequence.' }
        $events = @($output -split "`r?`n" | Where-Object { $_.Trim() } | ForEach-Object { $_ | ConvertFrom-Json -ErrorAction Stop })
        $session = @($events | Where-Object { $_.type -eq 'session' })
        if ($session.Count -ne 1 -or -not $session[0].sessionId) { throw 'DSH session identity is missing.' }
        if ($round -eq 1) { $launchParams.SessionId = $session[0].sessionId }
        elseif ($launchParams.SessionId -ne $session[0].sessionId) { throw 'DSH did not resume the same session.' }
        $launchParams.Prompt = 'Continue the isolated rotation fixture.'
    }
    $status = Invoke-RestMethod -Uri "http://127.0.0.1:$port/status" -NoProxy -TimeoutSec 3
    $calls = @($status.calls)
    if ($status.invalid -or $calls.Count -ne 24 -or $calls[0].fixtureExpanded -or
        -not $calls[1].fixtureExpanded -or -not $calls[15].fixtureExpanded -or $calls[16].fixtureExpanded) {
        throw 'DSH did not preserve the schema through fourteen idle responses and remove it after fifteen.'
    }
    if ($InjectInvalidChoice) {
        if ($status.invalidChoiceCount -ne 1) { throw 'Expected exactly one injected invalid choice set.' }
        $database = Join-Path $launchParams.StateDirectory 'sic-rust.sqlite'
        $auditCode = 'import json,sqlite3,sys; c=sqlite3.connect(sys.argv[1]); a=[json.loads(r[0]) for r in c.execute("select data from objects where kind=\x27worker_attempt\x27")]; print(json.dumps({"invalid":sum(x.get("error")=="provider response must contain exactly one choice" and x.get("source_id") is None for x in a),"tasks":c.execute("select count(*) from host_tool_tasks").fetchone()[0]}))'
        $audit = (& python -c $auditCode $database | Out-String) | ConvertFrom-Json
        if ($LASTEXITCODE -ne 0 -or $audit.invalid -ne 1 -or $audit.tasks -ne 0) { throw 'Rejected output was not audited, or an ambiguous tool was dispatched.' }
    }
    Write-Output "PASS: real DSH resumed one isolated session across two launches; each turn completed twelve Worker calls with the configured limit of sixteen, a dynamic schema disappeared after fifteen unused responses, and any injected invalid choice was not dispatched. Fixture: $fixture"
}
finally {
    if ($mock -and -not $mock.HasExited) { Stop-Process -Id $mock.Id -ErrorAction SilentlyContinue }
    foreach ($name in $saved.Keys) {
        if ($null -eq $saved[$name]) { Remove-Item -Path ("Env:" + $name) -ErrorAction SilentlyContinue }
        else { [Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process') }
    }
}
