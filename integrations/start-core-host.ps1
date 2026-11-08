param([Parameter(Mandatory=$true)][string]$ConfigFile, [int]$Port=18888, [int]$BridgePort=18890)
$ErrorActionPreference = 'Stop'
$taskRoot = Split-Path $PSScriptRoot -Parent
$taskConfigFile = (Resolve-Path -LiteralPath $ConfigFile).Path
$taskConfig = Get-Content -Raw -Encoding UTF8 -LiteralPath $taskConfigFile | ConvertFrom-Json
if ($taskConfig.tokens.SIC_CORE_TOKEN -ne 'host') { throw 'This control-plane profile requires SIC_CORE_TOKEN bound to host; configure the private file explicitly.' }
$taskTokenPath = Join-Path $taskRoot '.local/core-demo-token.txt'
New-Item -ItemType Directory -Force (Split-Path $taskTokenPath -Parent) | Out-Null
if (-not (Test-Path -LiteralPath $taskTokenPath)) { [IO.File]::WriteAllText($taskTokenPath,[Guid]::NewGuid().ToString('N')+[Guid]::NewGuid().ToString('N')) }
$env:SIC_CORE_TOKEN = [IO.File]::ReadAllText($taskTokenPath).Trim()
$env:SIC_CORE_ACTOR = 'host'
$env:SIC_CORE_URL = "http://127.0.0.1:$Port"
$env:PYTHONUTF8 = '1'
$taskPython = Join-Path $taskRoot '.local/langflow-ui-venv/Scripts/python.exe'
Push-Location $taskRoot
try { & $taskPython (Join-Path $PSScriptRoot 'run_core_host.py') --config $taskConfigFile --port $Port --bridge-port $BridgePort }
finally { Pop-Location }
