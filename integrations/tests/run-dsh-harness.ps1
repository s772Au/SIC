param(
    [Parameter(Mandatory=$true)][string]$DshCommand,
    [string]$DshHome,
    [string]$Profile='sic-core-headless',
    [ValidateRange(1,10)][int]$Runs=2,
    [string]$OutputDirectory,
    [switch]$Peer,
    [ValidateSet('P','V')][string]$PeerReturnLifecycle='V'
)
$ErrorActionPreference='Stop'
$taskRoot=Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
if(-not $DshHome){$DshHome=Join-Path $taskRoot '.local/dsh-harness-home'}
if(-not $OutputDirectory){$OutputDirectory=Join-Path $taskRoot ('.local/dsh-harness/'+[DateTime]::UtcNow.ToString('yyyyMMdd-HHmmss'))}
$taskHome=[IO.Path]::GetFullPath($DshHome)
$taskMarker=Join-Path $taskHome '.sic-integration-only'
if((Test-Path -LiteralPath $taskHome) -and -not(Test-Path -LiteralPath $taskMarker) -and (Get-ChildItem -LiteralPath $taskHome -Force | Select-Object -First 1)){
    throw 'Use a new empty DSH home. Existing unmarked homes are never modified.'
}
if($Profile -notmatch '^sic-[a-z0-9-]+$'){throw 'Use a dedicated sic-* integration profile'}
$taskDsh=(Resolve-Path -LiteralPath $DshCommand).Path
$taskPlugin=Join-Path $taskRoot 'sic_core/dsh_plugin'
if(-not(Test-Path -LiteralPath (Join-Path $taskPlugin 'node_modules/@deepseek-ai/dsh-llm-pi-ai'))){throw 'Install the pinned plugin dependencies first; see sic_core/dsh_plugin/README.md'}
$taskTokenPath=Join-Path $taskRoot '.local/core-demo-token.txt'
if(-not(Test-Path -LiteralPath $taskTokenPath)){throw 'Start the owned fixture Core/Bridge first'}
$taskOwners=@(foreach($taskPort in @(18888,18889,18890)){
    $taskListener=Get-NetTCPConnection -State Listen -LocalPort $taskPort -ErrorAction Stop | Select-Object -First 1
    $taskListener.OwningProcess
})
if(@($taskOwners | Select-Object -Unique).Count -ne 1){throw 'Core, model fixture and bridge must share the owned fixture process'}
$taskProcess=Get-CimInstance Win32_Process -Filter "ProcessId=$($taskOwners[0])"
if(-not $taskProcess.CommandLine.Contains((Join-Path $taskRoot 'integrations/run_core_demo.py'))){throw 'Only this checkout''s owned offline-fixture process may receive this test'}
New-Item -ItemType Directory -Force $taskHome,$OutputDirectory | Out-Null
[IO.File]::WriteAllText($taskMarker,'Dedicated synthetic SIC integration home; not a desktop profile.')
$taskOverlay=Join-Path $OutputDirectory 'fixture.overlay.yml'
$taskYaml=@'
- id: sic-core-dsh
  config:
    provider: sic-core
    baseURL: http://127.0.0.1:18890/v1
    apiKeyEnv: SIC_CORE_TOKEN
    model: sic-worker
    contextWindow: 128000
    maxTokens: 4096
    inputPricePerMillion: 0
    outputPricePerMillion: 0
    cacheReadPricePerMillion: 0
    cacheWritePricePerMillion: 0
    toolEvents: true
    fixtureWaitTool: true
    fixtureWaitMs: 600
- id: agent-default-model
  config:
    provider: sic-core
    model: sic-worker
'@
if($Peer){$taskYaml=$taskYaml.Replace('    fixtureWaitMs: 600',"    fixtureWaitMs: 600`n    peerTools: ['sic_fixture_wait']`n    fixturePeerTool: true`n    fixtureCorePort: 18888`n    fixturePeerReturnLifecycle: $PeerReturnLifecycle")}
foreach($taskId in @('compaction-basic','command-compact','tool-result-pruner','llm-pi-ai','llm-deepseek','llm-deepseek-account','session-title-llm','session-log-deepseek','session-telemetry-otel','web-search-deepseek','web-deepseek-api')){
    $taskYaml += "`n- id: $taskId`n  disabled: true`n"
}
[IO.File]::WriteAllText($taskOverlay,$taskYaml,[Text.UTF8Encoding]::new($false))
$taskSavedEnv=@{}
foreach($taskName in @('DSH_HOME','SIC_CORE_TOKEN','NO_PROXY','HTTP_PROXY','HTTPS_PROXY')){$taskSavedEnv[$taskName]=[Environment]::GetEnvironmentVariable($taskName,'Process')}
try{
    $env:DSH_HOME=$taskHome
    $env:SIC_CORE_TOKEN=[IO.File]::ReadAllText($taskTokenPath).Trim()
    $env:NO_PROXY='localhost,127.0.0.1,::1'
    $env:HTTP_PROXY=$null;$env:HTTPS_PROXY=$null
    $taskReady=Join-Path $taskHome '.sic-profile-ready'
    if(-not(Test-Path -LiteralPath $taskReady)){
        & $taskDsh --profile $Profile --from-default-profile headless --dump-config *> (Join-Path $OutputDirectory 'composed-before.yml')
        if($LASTEXITCODE -ne 0){throw 'Could not create the isolated headless profile'}
        & $taskDsh plugin --profile $Profile add $taskPlugin *> (Join-Path $OutputDirectory 'plugin-install.log')
        if($LASTEXITCODE -ne 0){throw 'Could not register the SIC plugin in the isolated profile'}
        [IO.File]::WriteAllText($taskReady,$Profile)
    }
    Push-Location $taskRoot
    try{
        foreach($taskRun in 1..$Runs){
            $taskLog=Join-Path $OutputDirectory ("roundtrip-$taskRun.jsonl")
            $taskPrompt=if($Peer){'SIC_DSH_PEER_TEST'}else{'SIC_DSH_TOOL_TEST'}
            & $taskDsh --profile $Profile --patch $taskOverlay --json $taskPrompt *> $taskLog
            if($LASTEXITCODE -ne 0){throw "Real DSH roundtrip $taskRun failed; inspect the private log"}
            Write-Output ("Real DSH roundtrip $taskRun completed; verify its tool/log/Core evidence next.")
        }
    }finally{Pop-Location}
}finally{
    foreach($taskName in $taskSavedEnv.Keys){[Environment]::SetEnvironmentVariable($taskName,$taskSavedEnv[$taskName],'Process')}
}
Write-Output 'This is a local transport/runtime fixture, not a model-quality experiment.'
