param(
    [Parameter(Mandatory = $true)][string]$CoreBinaryPath,
    [string]$FixtureRoot = $env:TEMP
)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path -LiteralPath $FixtureRoot -ErrorAction Stop).Path
$fixture = Join-Path $root ('sic-port-fixture-' + [guid]::NewGuid().ToString('N'))
$null = New-Item -ItemType Directory -Path $fixture
$fakeBin = Join-Path $fixture 'bin'
$plugin = Join-Path $fixture 'plugin'
$null = New-Item -ItemType Directory -Path $fakeBin
$null = New-Item -ItemType Directory -Path $plugin
$null = New-Item -ItemType Directory -Path (Join-Path $plugin 'node_modules')
[System.IO.File]::WriteAllText((Join-Path $plugin 'index.mjs'), 'export default {}', [System.Text.UTF8Encoding]::new($false))
[System.IO.File]::WriteAllText((Join-Path $fakeBin 'dsh.cmd'), "@echo off`r`nexit /b 0`r`n", [System.Text.Encoding]::ASCII)
$savedPath = $env:PATH
$savedKey = [Environment]::GetEnvironmentVariable('SIC_PROVIDER_KEY', 'Process')
try {
    $env:PATH = $fakeBin + [System.IO.Path]::PathSeparator + $savedPath
    $env:SIC_PROVIDER_KEY = 'isolated-fixture-only'
    if ((Get-Command dsh).Source -ne (Join-Path $fakeBin 'dsh.cmd')) {
        throw 'Synthetic DSH executable was not selected.'
    }
    $profile = @{
        model='openai/mock'; api_base='http://127.0.0.1:1/v1'; api_key_env='SIC_PROVIDER_KEY'
        tokenizer_model='host-o200k-surrogate'; context_window=1000000; max_input_tokens=1000000
        max_output_tokens=4096; default_output_tokens=4096; timeout_seconds=5
    }
    $modelPath = Join-Path $fixture 'model.json'
    @{roles=@{worker=$profile;editor=$profile}} | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $modelPath -Encoding utf8
    $state = Join-Path $fixture 'state'
    $launcher = Join-Path (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path 'scripts\run-dsh-headless.ps1'
    & $launcher -CoreBinaryPath $CoreBinaryPath -PluginDirectory $plugin -ModelConfigPath $modelPath `
        -StateDirectory $state -WorkingDirectory $fixture -AllowedTools @('read') -Prompt 'isolated test'
    $patch = Get-Content -LiteralPath (Join-Path $state 'sic-dsh.patch.yml') -Raw
    if ($patch -notmatch 'baseURL: http://127\.0\.0\.1:([1-9][0-9]{0,4})/v1' -or
        [int]$Matches[1] -gt 65535 -or $patch.Contains('__SIC_DYNAMIC_PORT__')) {
        throw 'The independently bound port was not written to the isolated DSH patch.'
    }
    Write-Output 'PASS: the dedicated Core bound its own port; isolated launcher health and cleanup completed.'
}
finally {
    $env:PATH = $savedPath
    [Environment]::SetEnvironmentVariable('SIC_PROVIDER_KEY', $savedKey, 'Process')
}
