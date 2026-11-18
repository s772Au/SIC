param(
    [ValidateSet('Start','Status')][string]$Action = 'Start',
    [Parameter(Mandatory=$true)][string]$RuntimeDir,
    [string]$RustBinary,
    [string]$LangflowPython,
    [string]$LanggraphCli,
    [string]$LangflowExtraPythonPath,
    [string]$ModelConfig,
    [switch]$AllowModel,
    [switch]$AllowRevoke,
    [int]$CorePort = 18898,
    [int]$LangflowPort = 17862,
    [int]$LanggraphPort = 12028
)
$ErrorActionPreference = 'Stop'
$taskRoot = Split-Path $PSScriptRoot -Parent
$taskRuntime = [IO.Path]::GetFullPath($RuntimeDir)
$taskPrivate = Join-Path $taskRuntime 'private'
$taskTokenPath = Join-Path $taskPrivate 'core-token.txt'
$taskAuthPath = Join-Path $taskPrivate 'langflow-admin.json'
$taskDatabase = Join-Path $taskRuntime 'rust-core.db'
$taskPorts = @($CorePort,$LangflowPort,$LanggraphPort)
if (($taskPorts | Select-Object -Unique).Count -ne 3 -or
    @($taskPorts | Where-Object { $_ -lt 1 -or $_ -gt 65535 }).Count) {
    throw 'Choose three distinct valid TCP ports.'
}

function Get-TaskListener([int]$taskPort) {
    Get-NetTCPConnection -State Listen -LocalPort $taskPort -ErrorAction SilentlyContinue | Select-Object -First 1
}

function Get-TaskJson([string]$taskUri, [bool]$taskAuthenticated = $false) {
    $taskHandler = [System.Net.Http.HttpClientHandler]::new()
    $taskHandler.UseProxy = $false
    $taskClient = [System.Net.Http.HttpClient]::new($taskHandler)
    $taskClient.Timeout = [TimeSpan]::FromSeconds(3)
    try {
        if ($taskAuthenticated) {
            $taskToken = [IO.File]::ReadAllText($taskTokenPath).Trim()
            $taskClient.DefaultRequestHeaders.Authorization =
                [System.Net.Http.Headers.AuthenticationHeaderValue]::new('Bearer', $taskToken)
        }
        $taskResponse = $taskClient.GetAsync($taskUri).GetAwaiter().GetResult()
        $taskResponse.EnsureSuccessStatusCode() | Out-Null
        $taskResponse.Content.ReadAsStringAsync().GetAwaiter().GetResult() | ConvertFrom-Json
    } finally {
        $taskClient.Dispose()
        $taskHandler.Dispose()
    }
}

function Get-TaskStatus {
    $taskServices = @(
        @{name='rust-core';port=$CorePort;uri="http://127.0.0.1:$CorePort/api/v2/capabilities";auth=$true},
        @{name='langflow';port=$LangflowPort;uri="http://127.0.0.1:$LangflowPort/health";auth=$false},
        @{name='langgraph';port=$LanggraphPort;uri="http://127.0.0.1:$LanggraphPort/ok";auth=$false}
    )
    foreach ($taskService in $taskServices) {
        $taskListener = Get-TaskListener $taskService.port
        $taskHealth = 'not-listening'
        $taskImplementation = $null
        if ($taskListener) {
            try {
                $taskReply = Get-TaskJson $taskService.uri $taskService.auth
                $taskHealth = if ($taskService.name -eq 'rust-core') {
                    $taskImplementation = $taskReply.implementation
                    if ($taskReply.api_version -eq '2' -and $taskReply.implementation -like 'sic-rust-core*') { 'ok' } else { 'wrong-backend' }
                } elseif ($taskService.name -eq 'langflow') {
                    if ($taskReply.status -eq 'ok') { 'ok' } else { 'unexpected-reply' }
                } else {
                    if ($taskReply.ok) { 'ok' } else { 'unexpected-reply' }
                }
            } catch { $taskHealth = 'health-failed' }
        }
        [PSCustomObject]@{service=$taskService.name; port=$taskService.port; health=$taskHealth;
                         pid=$(if($taskListener){$taskListener.OwningProcess}else{$null}); implementation=$taskImplementation}
    }
}

if ($Action -eq 'Status') {
    Get-TaskStatus | ConvertTo-Json -Depth 5
    return
}
foreach ($taskPath in @($RustBinary,$LangflowPython,$LanggraphCli)) {
    if (-not $taskPath -or -not (Test-Path -LiteralPath $taskPath -PathType Leaf)) {
        throw 'Start requires existing -RustBinary, -LangflowPython and -LanggraphCli paths.'
    }
}
if ($LangflowExtraPythonPath -and -not (Test-Path -LiteralPath $LangflowExtraPythonPath -PathType Container)) {
    throw 'LangflowExtraPythonPath does not exist.'
}
if ($ModelConfig -and -not (Test-Path -LiteralPath $ModelConfig -PathType Leaf)) {
    throw 'ModelConfig does not exist.'
}
if ($AllowModel -and -not $ModelConfig) {
    throw 'AllowModel requires an explicit private Rust model configuration.'
}
foreach ($taskPort in $taskPorts) {
    if (Get-TaskListener $taskPort) { throw "Port $taskPort is already occupied; no service was replaced." }
}
New-Item -ItemType Directory -Force -Path $taskRuntime,$taskPrivate | Out-Null
# Limit the bearer and UI login files to this Windows user and SYSTEM before
# either file is created. The surrounding runtime directory is not re-ACL'd.
$taskSid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
& icacls.exe $taskPrivate /inheritance:r /grant:r ("*{0}:(OI)(CI)F" -f $taskSid) '*S-1-5-18:(OI)(CI)F' | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'Could not restrict the private credential directory.' }
if (-not (Test-Path -LiteralPath $taskTokenPath)) {
    $taskBytes = New-Object byte[] 32
    $taskRandom = [System.Security.Cryptography.RandomNumberGenerator]::Create()
    try { $taskRandom.GetBytes($taskBytes) } finally { $taskRandom.Dispose() }
    [IO.File]::WriteAllText($taskTokenPath, [Convert]::ToBase64String($taskBytes))
}
foreach ($taskSecret in @($taskTokenPath,$taskAuthPath)) {
    if (Test-Path -LiteralPath $taskSecret) {
        & icacls.exe $taskSecret /inheritance:r /grant:r ("*{0}:F" -f $taskSid) '*S-1-5-18:F' | Out-Null
        if ($LASTEXITCODE -ne 0) { throw 'Could not restrict a private credential file.' }
    }
}
if (-not (Test-Path -LiteralPath $taskDatabase)) {
    & $RustBinary init $taskDatabase | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'Rust Core could not initialize its independent database.' }
}
$env:SIC_CORE_TOKEN = [IO.File]::ReadAllText($taskTokenPath).Trim()
$env:SIC_CORE_TOKEN_FILE = $taskTokenPath
$env:SIC_CORE_URL = "http://127.0.0.1:$CorePort"
$env:SIC_CORE_ACTOR = 'host'
$env:SIC_UI_RUNTIME_DIR = $taskRuntime
$env:SIC_LANGFLOW_PYTHON = [IO.Path]::GetFullPath($LangflowPython)
$env:SIC_LANGGRAPH_CLI = [IO.Path]::GetFullPath($LanggraphCli)
$env:SIC_LANGFLOW_EXTRA_PYTHONPATH = $LangflowExtraPythonPath
$env:PYTHONUTF8 = '1'
$taskShell = (Get-Process -Id $PID).Path
$taskCoreArgs = @('serve',('"{0}"' -f $taskDatabase),"127.0.0.1:$CorePort",'host','SIC_CORE_TOKEN')
if ($ModelConfig) { $taskCoreArgs += @('--model-config',('"{0}"' -f [IO.Path]::GetFullPath($ModelConfig))) }
if ($AllowModel) { $taskCoreArgs += '--allow-model' }
if ($AllowRevoke) { $taskCoreArgs += '--allow-revoke' }
$taskCore = Start-Process -FilePath $RustBinary -ArgumentList $taskCoreArgs -WorkingDirectory $taskRoot -WindowStyle Hidden -RedirectStandardOutput (Join-Path $taskRuntime 'rust-core.stdout.log') -RedirectStandardError (Join-Path $taskRuntime 'rust-core.stderr.log') -PassThru
$taskCore.Id | Set-Content -LiteralPath (Join-Path $taskRuntime 'rust-core.pid')
for ($taskAttempt=0; $taskAttempt -lt 100; $taskAttempt++) {
    Start-Sleep -Milliseconds 200
    try {
        $taskCaps = Get-TaskJson "http://127.0.0.1:$CorePort/api/v2/capabilities" $true
        if ($taskCaps.api_version -eq '2' -and $taskCaps.implementation -like 'sic-rust-core*') { break }
    } catch {}
    if ($taskCore.HasExited) { throw 'Rust Core exited; inspect rust-core.stderr.log.' }
}
if (-not $taskCaps -or $taskCaps.implementation -notlike 'sic-rust-core*') {
    throw 'Rust Core did not become ready; inspect rust-core.stderr.log.'
}
$taskLfArgs = @('-NoProfile','-File',('"{0}"' -f (Join-Path $PSScriptRoot 'start-langflow.ps1')),
                '-Port',"$LangflowPort",'-PrivateAuthPath',('"{0}"' -f $taskAuthPath),'-NoProxy')
$taskLf = Start-Process -FilePath $taskShell -ArgumentList $taskLfArgs -WorkingDirectory $taskRoot -WindowStyle Hidden -RedirectStandardOutput (Join-Path $taskRuntime 'langflow.stdout.log') -RedirectStandardError (Join-Path $taskRuntime 'langflow.stderr.log') -PassThru
$taskLf.Id | Set-Content -LiteralPath (Join-Path $taskRuntime 'langflow-launcher.pid')
$taskLgArgs = @('-NoProfile','-File',('"{0}"' -f (Join-Path $PSScriptRoot 'start-langgraph.ps1')),'-Port',"$LanggraphPort")
$taskLg = Start-Process -FilePath $taskShell -ArgumentList $taskLgArgs -WorkingDirectory $taskRoot -WindowStyle Hidden -RedirectStandardOutput (Join-Path $taskRuntime 'langgraph.stdout.log') -RedirectStandardError (Join-Path $taskRuntime 'langgraph.stderr.log') -PassThru
$taskLg.Id | Set-Content -LiteralPath (Join-Path $taskRuntime 'langgraph-launcher.pid')
for ($taskAttempt=0; $taskAttempt -lt 240; $taskAttempt++) {
    Start-Sleep -Milliseconds 250
    try {
        $taskLfHealth = Get-TaskJson "http://127.0.0.1:$LangflowPort/health"
        $taskLgHealth = Get-TaskJson "http://127.0.0.1:$LanggraphPort/ok"
        if ($taskLfHealth.status -eq 'ok' -and $taskLgHealth.ok) { break }
    } catch {}
    if ($taskLf.HasExited -or $taskLg.HasExited) { break }
}
$taskStatus = @(Get-TaskStatus)
$taskStatus | ConvertTo-Json -Depth 5
if (@($taskStatus | Where-Object health -ne 'ok').Count) {
    throw 'A service did not become ready; inspect the logs in RuntimeDir.'
}
& $LangflowPython (Join-Path $PSScriptRoot 'install_rust_langflow.py') --url "http://127.0.0.1:$LangflowPort" --auth-file $taskAuthPath --runtime-dir $taskRuntime
if ($LASTEXITCODE -ne 0) { throw 'Langflow is running, but the Rust workbench could not be installed.' }
Write-Output "Langflow login file: $taskAuthPath"
Write-Output "Rust database: $taskDatabase"
