param([int]$Port = 18888, [int]$MockPort = 18889, [int]$BridgePort = 18890)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$python = Join-Path $root '.local/langflow-ui-venv/Scripts/python.exe'
if (-not (Test-Path $python)) { throw 'Install the official UI environment; see integrations/README.md.' }
$local = Join-Path $root '.local'
New-Item -ItemType Directory -Force $local | Out-Null
$tokenPath = Join-Path $local 'core-demo-token.txt'
if (-not (Test-Path $tokenPath)) {
    [IO.File]::WriteAllText($tokenPath, [Guid]::NewGuid().ToString('N') + [Guid]::NewGuid().ToString('N'))
}
$env:SIC_CORE_TOKEN = [IO.File]::ReadAllText($tokenPath).Trim()
$env:SIC_CORE_ACTOR = 'host'
$env:SIC_CORE_URL = "http://127.0.0.1:$Port"
$env:PYTHONUTF8 = '1'
Push-Location $root
try { & $python (Join-Path $PSScriptRoot 'run_core_demo.py') --port $Port --mock-port $MockPort --bridge-port $BridgePort }
finally { Pop-Location }
