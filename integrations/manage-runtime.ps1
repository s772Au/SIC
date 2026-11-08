param([ValidateSet('Start','Status')][string]$Action='Status', [ValidateSet('Fixture','Configured')][string]$Mode='Fixture', [string]$ConfigFile)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Net.Http
$taskRoot = Split-Path $PSScriptRoot -Parent
$taskLocal = Join-Path $taskRoot '.local'
$taskShell = (Get-Process -Id $PID).Path
New-Item -ItemType Directory -Force $taskLocal | Out-Null
if ($Mode -eq 'Configured' -and $Action -eq 'Start' -and -not $ConfigFile) { throw 'Configured mode requires an explicit private ConfigFile. It never falls back to the fixture.' }
$taskCoreScript = if ($Mode -eq 'Fixture') { 'start-core-demo.ps1' } else { 'start-core-host.ps1' }
$taskCoreArgs = if ($Mode -eq 'Fixture') { @('-Port','18888','-MockPort','18889') } elseif ($ConfigFile) { @('-ConfigFile',('"{0}"' -f (Resolve-Path -LiteralPath $ConfigFile).Path),'-Port','18888') } else { @() }
$taskServices = @(
    @{Name='core';Port=18888;Script=$taskCoreScript;Args=$taskCoreArgs;PidFile='core-demo-launcher.pid';Log='core-demo'},
    @{Name='langflow';Port=17861;Script='start-langflow.ps1';Args=@('-Port','17861','-CoreDemo');PidFile='langflow-v2-launcher.pid';Log='langflow-v2'},
    @{Name='langgraph';Port=12026;Script='start-langgraph.ps1';Args=@('-Port','12026','-CoreDemo');PidFile='langgraph-v2-launcher.pid';Log='langgraph-v2'}
)
function Test-TaskOwner($taskProcessId,$taskScript) {
    for ($taskDepth=0; $taskDepth -lt 6 -and $taskProcessId; $taskDepth++) {
        $taskProcess = Get-CimInstance Win32_Process -Filter "ProcessId=$taskProcessId" -ErrorAction SilentlyContinue
        if (-not $taskProcess) { return $false }
        $taskCommand = ($taskProcess.CommandLine -replace '\\','/').ToLowerInvariant()
        if ($taskCommand.Contains(($taskRoot -replace '\\','/').ToLowerInvariant()) -and $taskCommand.Contains($taskScript.ToLowerInvariant())) { return $true }
        $taskProcessId = $taskProcess.ParentProcessId
    }
    return $false
}
function Invoke-TaskLocalJson($taskUri, $taskHeaderValues=@{}) {
    $taskHandler = [System.Net.Http.HttpClientHandler]::new()
    $taskHandler.UseProxy = $false
    $taskClient = [System.Net.Http.HttpClient]::new($taskHandler)
    $taskClient.Timeout = [TimeSpan]::FromSeconds(5)
    foreach($taskKey in $taskHeaderValues.Keys) { $taskClient.DefaultRequestHeaders.Add($taskKey,$taskHeaderValues[$taskKey]) }
    try {
        $taskResponse = $taskClient.GetAsync($taskUri).GetAwaiter().GetResult()
        $taskResponse.EnsureSuccessStatusCode() | Out-Null
        return ($taskResponse.Content.ReadAsStringAsync().GetAwaiter().GetResult() | ConvertFrom-Json)
    } finally { $taskClient.Dispose() }
}
$taskStatus = @()
foreach ($taskService in $taskServices) {
    $taskListener = Get-NetTCPConnection -State Listen -LocalPort $taskService.Port -ErrorAction SilentlyContinue | Select-Object -First 1
    $taskPidPath = Join-Path $taskLocal $taskService.PidFile
    $taskOwned = $false
    if ($taskListener) {
        $taskOwned = Test-TaskOwner $taskListener.OwningProcess $taskService.Script
        if (-not $taskOwned -and $Action -eq 'Start') { throw ('Port {0} belongs to an unverified process; no service was stopped or replaced.' -f $taskService.Port) }
    } elseif ($Action -eq 'Start') {
        $taskAlreadyStarting = $false
        if (Test-Path -LiteralPath $taskPidPath) {
            $taskExistingPid = [int](Get-Content -Raw -LiteralPath $taskPidPath).Trim()
            $taskAlreadyStarting = Test-TaskOwner $taskExistingPid $taskService.Script
        }
        if (-not $taskAlreadyStarting) {
            $taskScriptPath = Join-Path $PSScriptRoot $taskService.Script
            $taskArguments = @('-NoProfile','-File',('"{0}"' -f $taskScriptPath)) + $taskService.Args
            $taskStarted = Start-Process -FilePath $taskShell -ArgumentList $taskArguments -WorkingDirectory $taskRoot -WindowStyle Hidden -RedirectStandardOutput (Join-Path $taskLocal ($taskService.Log+'.stdout.log')) -RedirectStandardError (Join-Path $taskLocal ($taskService.Log+'.stderr.log')) -PassThru
            $taskStarted.Id | Set-Content -LiteralPath $taskPidPath
        }
    }
    $taskHealth = 'not-listening'
    if ($taskListener) {
        try {
            if ($taskService.Name -eq 'core') {
                $taskTokenPath = Join-Path $taskLocal 'core-demo-token.txt'
                $taskHeaders = @{Authorization=('Bearer '+[IO.File]::ReadAllText($taskTokenPath).Trim())}
                $taskReply = Invoke-TaskLocalJson 'http://127.0.0.1:18888/api/v2/capabilities' $taskHeaders
                $taskHealth = if ($taskReply.api_version -eq '2') { 'v2-authenticated' } else { 'wrong-version' }
            } elseif ($taskService.Name -eq 'langflow') {
                $taskReply = Invoke-TaskLocalJson 'http://127.0.0.1:17861/health'
                $taskHealth = $taskReply.status
            } else {
                $taskReply = Invoke-TaskLocalJson 'http://127.0.0.1:12026/ok'
                $taskHealth = if ($taskReply.ok) { 'ok' } else { 'unexpected-reply' }
            }
        } catch { $taskHealth = 'health-failed' }
    }
    $taskStatus += @{service=$taskService.Name;port=$taskService.Port;owned=$taskOwned;pid=$(if($taskListener){$taskListener.OwningProcess}else{$null});health=$taskHealth}
}
$taskBridgeListener = Get-NetTCPConnection -State Listen -LocalPort 18890 -ErrorAction SilentlyContinue | Select-Object -First 1
$taskCoreStatus = $taskStatus | Where-Object service -eq 'core' | Select-Object -First 1
$taskBridgeHealth = 'not-listening'
$taskBridgeOwned = $false
if ($taskBridgeListener) {
    $taskBridgeOwned = $taskCoreStatus.owned -and $taskBridgeListener.OwningProcess -eq $taskCoreStatus.pid
    try {
        $taskBridgeHeaders = @{Authorization=('Bearer '+[IO.File]::ReadAllText((Join-Path $taskLocal 'core-demo-token.txt')).Trim())}
        $taskBridgeReply = Invoke-TaskLocalJson 'http://127.0.0.1:18890/api/v2/bridge/sessions' $taskBridgeHeaders
        $taskBridgeHealth = 'authenticated-same-core'
    } catch { $taskBridgeHealth = 'health-failed' }
}
$taskStatus += @{service='dsh-bridge';port=18890;owned=$taskBridgeOwned;pid=$(if($taskBridgeListener){$taskBridgeListener.OwningProcess}else{$null});health=$taskBridgeHealth}
@{action=$Action;requested_mode=$Mode;services=$taskStatus;old_services_untouched=$true;authentication='core Bearer; Langflow private admin; LangGraph loopback development';credential_file='.local/langflow-17861/admin-auth.json';logs='.local/*-v2.*.log';startup_changes_system_settings=$false} | ConvertTo-Json -Depth 5
