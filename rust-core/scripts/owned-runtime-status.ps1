param(
  [int]$CorePort = 18888,
  [int]$LangflowPort = 7868,
  [int]$LangGraphPort = 12027
)
$ErrorActionPreference = 'Stop'
$token = $env:SIC_CORE_TOKEN
$result = [ordered]@{
  checked_at = [DateTime]::UtcNow.ToString('o')
  core = [ordered]@{ port = $CorePort; listening = $false; pid = $null; health = 'not-checked'; api_version = $null }
  langflow = [ordered]@{ port = $LangflowPort; listening = $false; pid = $null; health = 'not-checked' }
  langgraph = [ordered]@{ port = $LangGraphPort; listening = $false; pid = $null; health = 'not-checked' }
}
function Get-PortOwner([int]$Port) {
  $listener = Get-NetTCPConnection -State Listen -LocalPort $Port -ErrorAction SilentlyContinue | Select-Object -First 1
  if (-not $listener) { return $null }
  return [int]$listener.OwningProcess
}
$corePid = Get-PortOwner $CorePort
if ($corePid) {
  $result.core.listening = $true
  $result.core.pid = $corePid
  if (-not $token) {
    $result.core.health = 'missing-token'
  } else {
    try {
      $cap = Invoke-RestMethod -Uri "http://127.0.0.1:$CorePort/api/v2/capabilities" -Headers @{ Authorization = "Bearer $token" } -TimeoutSec 3 -NoProxy
      $result.core.health = if ($cap.api_version -eq '2') { 'ok' } else { 'unhealthy' }
      $result.core.api_version = $cap.api_version
    } catch { $result.core.health = 'health-failed' }
  }
}
$lfPid = Get-PortOwner $LangflowPort
if ($lfPid) {
  $result.langflow.listening = $true
  $result.langflow.pid = $lfPid
  try {
    $health = Invoke-RestMethod -Uri "http://127.0.0.1:$LangflowPort/health" -TimeoutSec 3 -NoProxy
    $result.langflow.health = if ($health.status -eq 'ok') { 'ok' } else { 'unhealthy' }
  } catch { $result.langflow.health = 'health-failed' }
}
$lgPid = Get-PortOwner $LangGraphPort
if ($lgPid) {
  $result.langgraph.listening = $true
  $result.langgraph.pid = $lgPid
  try {
    $info = Invoke-RestMethod -Uri "http://127.0.0.1:$LangGraphPort/info" -TimeoutSec 3 -NoProxy
    $result.langgraph.health = if ($info.version) { 'ok' } else { 'unhealthy' }
  } catch { $result.langgraph.health = 'health-failed' }
}
$result | ConvertTo-Json -Depth 6
$down = @($result.core, $result.langflow, $result.langgraph) | Where-Object { -not $_.listening -or $_.health -ne 'ok' }
if ($down.Count -gt 0) { exit 2 } else { exit 0 }
