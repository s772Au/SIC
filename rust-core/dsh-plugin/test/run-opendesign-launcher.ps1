param(
    [Parameter(Mandatory = $true)][string]$CoreBinaryPath,
    [Parameter(Mandatory = $true)][string]$PluginDirectory,
    [string]$FixtureRoot = $env:TEMP,
    [string]$DshCredentialReference,
    [string]$DshCredentialHome
)

$ErrorActionPreference = 'Stop'
$providerKey = [Environment]::GetEnvironmentVariable('SIC_PROVIDER_KEY', 'Process')
if ([string]::IsNullOrWhiteSpace($DshCredentialReference) -and [string]::IsNullOrWhiteSpace($providerKey)) {
    throw 'The fixed Provider credential reference is missing; no model call was made.'
}
$coreRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$root = (Resolve-Path -LiteralPath $FixtureRoot -ErrorAction Stop).Path
$fixture = Join-Path $root ('sic-launcher-opendesign-' + [guid]::NewGuid().ToString('N'))
$null = New-Item -ItemType Directory -Path $fixture
$guard = $null
$savedUpstream = [Environment]::GetEnvironmentVariable('SIC_LIVE_UPSTREAM', 'Process')
try {
    # The loopback guard never inherits the Provider credential.
    Remove-Item Env:SIC_PROVIDER_KEY -ErrorAction SilentlyContinue
    $env:SIC_LIVE_UPSTREAM = 'https://amr-link.open-design.ai/v1/chat/completions'
    $guardOut = Join-Path $fixture 'guard.out'; $guardErr = Join-Path $fixture 'guard.err'
    $guardScript = Join-Path $PSScriptRoot 'deepseek-call-guard.mjs'
    $guard = Start-Process -FilePath node -ArgumentList @('"' + $guardScript + '"') -WindowStyle Hidden -PassThru -RedirectStandardOutput $guardOut -RedirectStandardError $guardErr
    $port = $null
    for ($i = 0; $i -lt 100; $i++) {
        if (Test-Path -LiteralPath $guardOut) { try { $port = (Get-Content -LiteralPath $guardOut -TotalCount 1 | ConvertFrom-Json).port } catch {} }
        if ($port) { break }
        Start-Sleep -Milliseconds 100
    }
    if (-not $port) { throw 'Fixed-model guard did not start; no model call was made.' }
    $profile = @{ model = 'openai/deepseek-v4.1-flash-eco'; expected_returned_models = @('deepseek-v4.1-flash-eco'); api_base = "http://127.0.0.1:$port/v1"; api_key_env = 'SIC_PROVIDER_KEY'; tokenizer_model = 'host-o200k-surrogate'; context_window = 1000000; max_input_tokens = 1000000; max_output_tokens = 393216; default_output_tokens = 4096; timeout_seconds = 180; retry_editor_http_5xx = $true }
    $modelPath = Join-Path $fixture 'model.json'
    @{ roles = @{ worker = $profile; editor = $profile } } | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $modelPath -Encoding utf8
    $launcher = Join-Path $coreRoot 'scripts\run-dsh-headless.ps1'
    $state = Join-Path $fixture 'state'
    $launchParams = @{ StateDirectory = $state; WorkingDirectory = $fixture
        Prompt = 'Use the sic_fixture_read synthetic tool exactly once, then answer with its returned marker.'
        CoreBinaryPath = $CoreBinaryPath; PluginDirectory = $PluginDirectory; ModelConfigPath = $modelPath
        AllowedTools = @('sic_fixture_read'); FixtureReadTool = $true }
    if ($DshCredentialReference) {
        $launchParams.DshCredentialMap = @{ SIC_PROVIDER_KEY = $DshCredentialReference }
        if ($DshCredentialHome) { $launchParams.DshCredentialHome = $DshCredentialHome }
    } else { $env:SIC_PROVIDER_KEY = $providerKey }
    $output = & $launcher @launchParams | Out-String
    Remove-Item Env:SIC_PROVIDER_KEY -ErrorAction SilentlyContinue
    $status = Invoke-RestMethod -Uri "http://127.0.0.1:$port/status" -NoProxy -TimeoutSec 3
    if (-not $output.Contains('SIC_RUST_SYNTHETIC_FIXTURE_ONLY') -or $status.editorCount -lt 1 -or $status.workerCount -lt 2 -or
        $status.editorCount -gt 3 -or $status.workerCount -gt 4 -or $status.invalidModel -or $status.invalidBudget -or
        @($status.calls | Where-Object { $_.status -ne 200 }).Count -gt 0) {
        throw 'The bounded launcher/DSH/Provider fixture did not pass.'
    }
    $reportPath = Join-Path $fixture 'redacted-report.json'
    @{ result = 'passed'; model = 'deepseek-v4.1-flash-eco'; actual_max_tokens = 4096; editor_calls = $status.editorCount; worker_calls = $status.workerCount; synthetic_tool_result_observed = $true; provider_calls = $status.calls; prompt_or_credential_in_report = $false } |
        ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $reportPath -Encoding utf8
    Write-Output "PASS: isolated launcher completed the fixed-model DSH tool roundtrip. Redacted report: $reportPath"
}
finally {
    if ($guard -and -not $guard.HasExited) { Stop-Process -Id $guard.Id -ErrorAction SilentlyContinue }
    if ($null -eq $providerKey) { Remove-Item Env:SIC_PROVIDER_KEY -ErrorAction SilentlyContinue }
    else { [Environment]::SetEnvironmentVariable('SIC_PROVIDER_KEY', $providerKey, 'Process') }
    if ($null -eq $savedUpstream) { Remove-Item Env:SIC_LIVE_UPSTREAM -ErrorAction SilentlyContinue }
    else { [Environment]::SetEnvironmentVariable('SIC_LIVE_UPSTREAM', $savedUpstream, 'Process') }
}
