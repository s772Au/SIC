[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$StateDirectory,
    [Parameter(Mandatory = $true)][string]$WorkingDirectory,
    [Parameter(Mandatory = $true)][string]$Prompt,
    [string]$SessionId,
    [string]$BindChainId,
    [string]$CoreBinaryPath,
    [string]$PluginDirectory,
    [string]$ModelConfigPath,
    [hashtable]$DshCredentialMap = @{},
    [string]$DshCredentialHome,
    [string[]]$AllowedTools = @('read'),
    [ValidateRange(1, 64)][int]$MaxWorkerCallsPerTurn = 8,
    [ValidateRange(0, 2)][int]$MaxInvalidChoiceRetries = 1,
    [switch]$FixtureReadTool,
    [switch]$JsonEvents,
    [decimal]$InputPricePerMillion = 0,
    [decimal]$OutputPricePerMillion = 0,
    [decimal]$CacheReadPricePerMillion = 0,
    [decimal]$CacheWritePricePerMillion = 0
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($PSVersionTable.PSVersion.Major -lt 7) { throw 'PowerShell 7 or newer is required.' }

$coreRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
if ([string]::IsNullOrWhiteSpace($CoreBinaryPath)) { $CoreBinaryPath = Join-Path $coreRoot 'release\sic-core-rust.exe' }
if ([string]::IsNullOrWhiteSpace($PluginDirectory)) { $PluginDirectory = Join-Path $coreRoot 'dsh-plugin' }
if ([string]::IsNullOrWhiteSpace($ModelConfigPath)) { $ModelConfigPath = Join-Path $coreRoot 'config\opendesign.example.json' }
foreach ($path in @($StateDirectory, $WorkingDirectory, $CoreBinaryPath, $PluginDirectory, $ModelConfigPath)) {
    if (-not [System.IO.Path]::IsPathFullyQualified($path)) { throw 'All directory and file paths must be absolute.' }
}
$state = [System.IO.Path]::GetFullPath($StateDirectory)
$working = (Resolve-Path -LiteralPath $WorkingDirectory -ErrorAction Stop).Path
$binary = (Resolve-Path -LiteralPath $CoreBinaryPath -ErrorAction Stop).Path
$plugin = (Resolve-Path -LiteralPath $PluginDirectory -ErrorAction Stop).Path
$modelPath = (Resolve-Path -LiteralPath $ModelConfigPath -ErrorAction Stop).Path
if (-not (Test-Path -LiteralPath $working -PathType Container) -or
    -not (Test-Path -LiteralPath $binary -PathType Leaf) -or
    -not (Test-Path -LiteralPath (Join-Path $plugin 'index.mjs') -PathType Leaf) -or
    -not (Test-Path -LiteralPath (Join-Path $plugin 'node_modules') -PathType Container) -or
    -not (Test-Path -LiteralPath $modelPath -PathType Leaf)) {
    throw 'Core binary, working directory, model configuration, or prepared DSH plugin is missing.'
}
if ([string]::IsNullOrWhiteSpace($Prompt)) { throw 'A nonempty headless prompt is required.' }
if ($SessionId -and $SessionId -notmatch '^[A-Za-z0-9_-]{1,128}$') { throw 'Invalid DSH session identity.' }
if (-not $AllowedTools -or $AllowedTools.Count -gt 128) { throw 'Provide 1..128 explicitly allowed DSH tool names.' }
foreach ($name in $AllowedTools) {
    if ($name -notmatch '^[A-Za-z_][A-Za-z0-9_-]{0,63}$') { throw 'A DSH tool name is invalid.' }
}
if ($InputPricePerMillion -lt 0 -or $OutputPricePerMillion -lt 0 -or
    $CacheReadPricePerMillion -lt 0 -or $CacheWritePricePerMillion -lt 0) { throw 'Price fields must be nonnegative.' }
if (-not (Get-Command dsh -ErrorAction SilentlyContinue)) { throw 'DSH CLI is not installed or is not on PATH.' }

function Read-DshCredential([string]$Reference) {
    if ($Reference -cnotmatch '^[A-Za-z_][A-Za-z0-9_]{0,127}$') { throw 'Invalid DSH credential reference.' }
    $homePath = $DshCredentialHome
    if ([string]::IsNullOrWhiteSpace($homePath)) {
        $homePath = if ($env:DSH_HOME) { $env:DSH_HOME } else { Join-Path ([Environment]::GetFolderPath('UserProfile')) '.dsh' }
    }
    if (-not [System.IO.Path]::IsPathFullyQualified($homePath)) { throw 'DSH credential home must be an absolute path.' }
    $credentialFile = Join-Path ([System.IO.Path]::GetFullPath($homePath)) '.credentials.yaml'
    if (-not (Test-Path -LiteralPath $credentialFile -PathType Leaf)) { throw 'The selected DSH credential store is unavailable.' }
    $dshCommand = Get-Command dsh -ErrorAction Stop
    $dshDirectory = Split-Path -Parent $dshCommand.Source
    $electron = [System.IO.Path]::GetFullPath((Join-Path $dshDirectory '..\..\..\..\DeepSeek Harness.exe'))
    $asar = [System.IO.Path]::GetFullPath((Join-Path $dshDirectory '..\..\..\app.asar'))
    $cli = Join-Path $asar 'dsh\node_modules\@deepseek-ai\dsh-desktop-host\lib\cli.js'
    $reader = Join-Path $PSScriptRoot 'read-dsh-credential.cjs'
    if (-not (Test-Path -LiteralPath $electron -PathType Leaf) -or
        -not (Test-Path -LiteralPath $asar -PathType Leaf) -or
        -not (Test-Path -LiteralPath $reader -PathType Leaf)) { throw 'The installed DSH credential reader is unavailable.' }
    $spec = [System.Diagnostics.ProcessStartInfo]::new()
    $spec.FileName = $electron
    $spec.UseShellExecute = $false
    $spec.CreateNoWindow = $true
    $spec.RedirectStandardOutput = $true
    $spec.RedirectStandardError = $true
    $spec.Environment.Clear()
    foreach ($name in @('PATH', 'SYSTEMROOT', 'WINDIR', 'TEMP', 'TMP')) {
        $value = [Environment]::GetEnvironmentVariable($name, 'Process')
        if ($null -ne $value) { $spec.Environment[$name] = $value }
    }
    $spec.Environment['ELECTRON_RUN_AS_NODE'] = '1'
    foreach ($arg in @($reader, $cli, $credentialFile, $Reference)) { $spec.ArgumentList.Add($arg) }
    $readerProcess = [System.Diagnostics.Process]::Start($spec)
    if (-not $readerProcess) { throw 'Could not start the DSH credential reader.' }
    try {
        $valueTask = $readerProcess.StandardOutput.ReadToEndAsync()
        $discardError = $readerProcess.StandardError.BaseStream.CopyToAsync([System.IO.Stream]::Null)
        if (-not $readerProcess.WaitForExit(10000)) { $readerProcess.Kill(); throw 'DSH credential lookup timed out.' }
        $value = $valueTask.GetAwaiter().GetResult()
        if ($readerProcess.ExitCode -ne 0 -or [string]::IsNullOrWhiteSpace($value) -or $value.Length -gt 4096 -or $value -match '[\r\n]') {
            throw 'The selected DSH credential reference is unavailable.'
        }
        return $value
    } finally { $readerProcess.Dispose() }
}

# Resolve only references used by a role or an explicitly configured Editor
# mapping. Values remain in memory and are sent only to the Core child.
$model = Get-Content -LiteralPath $modelPath -Raw | ConvertFrom-Json -AsHashtable
if (-not ($model -is [System.Collections.IDictionary]) -or
    -not ($model['roles'] -is [System.Collections.IDictionary])) { throw 'Model configuration has no roles object.' }
$profiles = if ($model['profiles'] -is [System.Collections.IDictionary]) { $model['profiles'] } else { @{} }
function Resolve-ModelProfile($value) {
    if ($value -is [string]) {
        if (-not $profiles.Contains($value)) { throw 'A model profile reference is not configured.' }
        return $profiles[$value]
    }
    if ($value -isnot [System.Collections.IDictionary]) { throw 'A model role has no profile.' }
    if ($value.Contains('profile')) {
        $name = $value['profile']
        if ($name -isnot [string] -or -not $profiles.Contains($name)) { throw 'A model profile reference is not configured.' }
        return $profiles[$name]
    }
    return $value
}
$workerProfile = Resolve-ModelProfile $model['roles']['worker']
$editorProfile = Resolve-ModelProfile $model['roles']['editor']
$usedProfiles = @($workerProfile, $editorProfile)
foreach ($mappingName in @('editor_profiles_by_level', 'editor_profiles')) {
    $mapping = $model[$mappingName]
    if ($null -ne $mapping) {
        if ($mapping -isnot [System.Collections.IDictionary]) { throw 'An Editor profile mapping is invalid.' }
        foreach ($name in $mapping.Values) { $usedProfiles += Resolve-ModelProfile $name }
    }
}
$maxTokens = if ($workerProfile.Contains('default_output_tokens')) { $workerProfile['default_output_tokens'] } else { $workerProfile['max_output_tokens'] }
if ($maxTokens -isnot [long] -and $maxTokens -isnot [int]) { throw 'Worker output budget must be an integer.' }
if ($maxTokens -lt 1 -or $maxTokens -gt 393216) { throw 'Worker output budget is outside the DSH plugin limit.' }
$secrets = @{}
$originalCredentialEnv = @{}
foreach ($profile in $usedProfiles) {
    if ($profile -isnot [System.Collections.IDictionary]) { throw 'A selected model profile is invalid.' }
    $name = $profile['api_key_env']
    if ($name -isnot [string] -or $name -cnotmatch '^[A-Za-z_][A-Za-z0-9_]{0,127}$' -or
        $name -in @('PATH', 'SYSTEMROOT', 'WINDIR', 'TEMP', 'TMP', 'DSH_HOME', 'SIC_RUST_BEARER')) {
        throw 'A selected profile has an invalid or reserved credential reference.'
    }
    if ($secrets.ContainsKey($name)) { continue }
    $originalCredentialEnv[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
    $value = if ($DshCredentialMap.ContainsKey($name)) {
        Read-DshCredential $DshCredentialMap[$name]
    } else {
        $originalCredentialEnv[$name]
    }
    if ([string]::IsNullOrWhiteSpace($value)) { throw "The configured credential environment reference $name is missing." }
    $secrets[$name] = $value
}
foreach ($name in $DshCredentialMap.Keys) {
    if (-not $secrets.ContainsKey($name)) { throw 'DSH credential mapping names an unused model reference.' }
}

$marker = Join-Path $state 'sic-launcher.json'
if (Test-Path -LiteralPath $state) {
    if (-not (Test-Path -LiteralPath $state -PathType Container)) { throw 'StateDirectory is not a directory.' }
    if (-not (Test-Path -LiteralPath $marker -PathType Leaf) -and @(Get-ChildItem -LiteralPath $state -Force).Count -ne 0) {
        throw 'StateDirectory is nonempty and was not initialized by this launcher.'
    }
} else { $null = New-Item -ItemType Directory -Path $state }

$savedDshHome = [Environment]::GetEnvironmentVariable('DSH_HOME', 'Process')
$savedBearer = [Environment]::GetEnvironmentVariable('SIC_RUST_BEARER', 'Process')
$lock = $null
$core = $null
$start = $null
try {
    $lockPath = Join-Path $state 'sic-launcher.lock'
    try { $lock = [System.IO.File]::Open($lockPath, [System.IO.FileMode]::OpenOrCreate, [System.IO.FileAccess]::ReadWrite, [System.IO.FileShare]::None) }
    catch { throw 'Another launcher already owns this state directory.' }
    if (Test-Path -LiteralPath $marker -PathType Leaf) {
        $saved = Get-Content -LiteralPath $marker -Raw | ConvertFrom-Json -AsHashtable
        if ($saved['format'] -ne 'sic-dsh-headless-v1' -or $saved['plugin_directory'] -ine $plugin) {
            throw 'StateDirectory belongs to a different launcher or plugin path.'
        }
    } else {
        @{ format = 'sic-dsh-headless-v1'; plugin_directory = $plugin } | ConvertTo-Json -Compress |
            Set-Content -LiteralPath $marker -Encoding utf8
    }

    # Remove provider credentials before DSH profile installation and launch.
    foreach ($name in $secrets.Keys) { [Environment]::SetEnvironmentVariable($name, $null, 'Process') }
    $env:DSH_HOME = Join-Path $state 'dsh-home'
    $random = [byte[]]::new(32)
    [System.Security.Cryptography.RandomNumberGenerator]::Fill($random)
    $env:SIC_RUST_BEARER = [Convert]::ToHexString($random)

    $profileReady = Join-Path $state 'dsh-profile-ready'
    if (-not (Test-Path -LiteralPath $profileReady -PathType Leaf)) {
        & dsh --profile sic-rust-local --from-default-profile headless --dump-config *> $null
        if ($LASTEXITCODE -ne 0) { throw 'Could not create the isolated DSH profile.' }
        & dsh plugin --profile sic-rust-local add $plugin *> $null
        if ($LASTEXITCODE -ne 0) { throw 'Could not install the SIC plugin into the isolated DSH profile.' }
        [System.IO.File]::WriteAllText($profileReady, 'sic-rust-local', [System.Text.UTF8Encoding]::new($false))
    }
    $database = Join-Path $state 'sic-rust.sqlite'
    if (-not (Test-Path -LiteralPath $database)) {
        & $binary init $database *> $null
        if ($LASTEXITCODE -ne 0) { throw 'Could not create the independent Rust database.' }
    }

    $toolYaml = ($AllowedTools | ForEach-Object { '      - ' + (ConvertTo-Json -InputObject $_ -Compress) }) -join "`n"
    $invariant = [System.Globalization.CultureInfo]::InvariantCulture
    $inputPrice = $InputPricePerMillion.ToString($invariant)
    $outputPrice = $OutputPricePerMillion.ToString($invariant)
    $cacheReadPrice = $CacheReadPricePerMillion.ToString($invariant)
    $cacheWritePrice = $CacheWritePricePerMillion.ToString($invariant)
    $fixture = if ($FixtureReadTool) { 'true' } else { 'false' }
    $bindLine = if ([string]::IsNullOrWhiteSpace($BindChainId)) { '' } else { "    bindChainId: $BindChainId" }
    $patchPath = Join-Path $state 'sic-dsh.patch.yml'
@"
- id: agent-default-model
  config:
    provider: sic-core-rust
    model: sic-worker
- id: sic-core-rust-dsh
  config:
    provider: sic-core-rust
    baseURL: http://127.0.0.1:__SIC_DYNAMIC_PORT__/v1
    apiKeyEnv: SIC_RUST_BEARER
    model: sic-worker
$bindLine
    maxTokens: $maxTokens
    inputPricePerMillion: $inputPrice
    outputPricePerMillion: $outputPrice
    cacheReadPricePerMillion: $cacheReadPrice
    cacheWritePricePerMillion: $cacheWritePrice
    allowedTools:
$toolYaml
    fixtureReadTool: $fixture
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
"@ | Set-Content -LiteralPath $patchPath -Encoding utf8

    $start = [System.Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $binary
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $start.WorkingDirectory = $state
    # Port 0 lets the Core bind an available port itself. Probing and releasing
    # a port first leaves a race with other applications on the user's machine.
    foreach ($arg in @('serve', $database, '127.0.0.1:0', 'local-owner', 'SIC_RUST_BEARER', '--allow-model', '--model-config', $modelPath, '--dsh-alias', 'sic-worker', '--dsh-max-worker-calls', "$MaxWorkerCallsPerTurn", '--dsh-invalid-choice-retries', "$MaxInvalidChoiceRetries")) {
        $start.ArgumentList.Add($arg)
    }
    $start.Environment.Clear()
    foreach ($name in @('PATH', 'SYSTEMROOT', 'WINDIR', 'TEMP', 'TMP')) {
        $value = [Environment]::GetEnvironmentVariable($name, 'Process')
        if ($null -ne $value) { $start.Environment[$name] = $value }
    }
    $start.Environment['SIC_RUST_BEARER'] = $env:SIC_RUST_BEARER
    foreach ($name in $secrets.Keys) { $start.Environment[$name] = $secrets[$name] }
    try { $core = [System.Diagnostics.Process]::Start($start) }
    finally { $start.Environment.Clear() }
    if (-not $core) { throw 'Could not start the Rust Core.' }
    # Read only the dedicated child's listening banner; reject an unexpected
    # address and never probe a port selected by a separate, released listener.
    $bannerTask = $core.StandardOutput.ReadLineAsync()
    $bannerReady = $false
    for ($i = 0; $i -lt 300; $i++) {
        if ($bannerTask.Wait(100)) { $bannerReady = $true; break }
        if ($core.HasExited) { throw 'Rust Core exited before reporting its bound port.' }
    }
    if (-not $bannerReady) { throw 'Rust Core did not report its bound port.' }
    $banner = $bannerTask.GetAwaiter().GetResult()
    if ($banner -notmatch '^SIC Rust core listening on 127\.0\.0\.1:(\d{1,5})$') {
        throw 'Rust Core returned an unexpected listening banner.'
    }
    $port = [int]$Matches[1]
    if ($port -lt 1 -or $port -gt 65535) { throw 'Rust Core returned an invalid bound port.' }
    $patch = [System.IO.File]::ReadAllText($patchPath)
    if (-not $patch.Contains('__SIC_DYNAMIC_PORT__')) { throw 'DSH patch port placeholder is missing.' }
    [System.IO.File]::WriteAllText($patchPath, $patch.Replace('__SIC_DYNAMIC_PORT__', "$port"), [System.Text.UTF8Encoding]::new($false))
    # Keep all child stdout out of DSH's machine-readable stdout.
    $outputDrain = $core.StandardOutput.BaseStream.CopyToAsync([System.IO.Stream]::Null)
    $ready = $false
    for ($i = 0; $i -lt 300; $i++) {
        if ($core.HasExited) { throw 'Rust Core exited before becoming ready.' }
        try {
            $null = Invoke-RestMethod -Uri "http://127.0.0.1:$port/health" -Headers @{ Authorization = "Bearer $env:SIC_RUST_BEARER" } -TimeoutSec 1 -NoProxy
            $ready = $true; break
        } catch { Start-Sleep -Milliseconds 100 }
    }
    if (-not $ready) { throw 'Rust Core did not become ready.' }
    $arguments = @('--profile', 'sic-rust-local', '--patch', $patchPath)
    if ($JsonEvents) { $arguments += '--json' }
    if ($SessionId) { $arguments += @('--session-id', $SessionId) }
    # DSH reads the prompt from stdin, keeping it out of its process arguments.
    $arguments += '-'
    Push-Location -LiteralPath $working
    try { $Prompt | & dsh @arguments; $dshExitCode = $LASTEXITCODE } finally { Pop-Location }
    if ($dshExitCode -ne 0) { throw 'DSH headless did not complete successfully; inspect the isolated state before resuming.' }
}
finally {
    if ($core) {
        try { if (-not $core.HasExited) { $core.Kill(); $null = $core.WaitForExit(5000) } }
        catch { Write-Warning 'Could not stop the dedicated Rust Core process; inspect its status before restarting.' }
        finally { $core.Dispose() }
    }
    foreach ($name in $secrets.Keys) {
        if ($null -eq $originalCredentialEnv[$name]) { Remove-Item -LiteralPath ("Env:" + $name) -ErrorAction SilentlyContinue }
        else { [Environment]::SetEnvironmentVariable($name, $originalCredentialEnv[$name], 'Process') }
    }
    $secrets.Clear()
    $originalCredentialEnv.Clear()
    if ($null -eq $savedDshHome) { Remove-Item Env:DSH_HOME -ErrorAction SilentlyContinue }
    else { [Environment]::SetEnvironmentVariable('DSH_HOME', $savedDshHome, 'Process') }
    if ($null -eq $savedBearer) { Remove-Item Env:SIC_RUST_BEARER -ErrorAction SilentlyContinue }
    else { [Environment]::SetEnvironmentVariable('SIC_RUST_BEARER', $savedBearer, 'Process') }
    if ($lock) { $lock.Dispose() }
}
