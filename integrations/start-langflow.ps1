param([int]$Port = 17860, [switch]$CoreDemo)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$python = Join-Path $root '.local\langflow-ui-venv\Scripts\python.exe'
if (-not (Test-Path $python)) { throw 'Install the integration environment first; see integrations/README.md.' }
if ($CoreDemo) { . (Join-Path $PSScriptRoot 'use-core-demo-environment.ps1') }
$env:PYTHONPATH = $PSScriptRoot + [IO.Path]::PathSeparator + $root
$env:LANGFLOW_COMPONENTS_PATH = Join-Path $PSScriptRoot 'components'
if ($Port -eq 17860) {
    $env:LANGFLOW_CONFIG_DIR = Join-Path $root '.local\langflow-config'
    $taskDatabase = Join-Path $root '.local\langflow.db'
} else {
    $taskInstance = Join-Path $root ".local/langflow-$Port"
    New-Item -ItemType Directory -Force $taskInstance | Out-Null
    $env:LANGFLOW_CONFIG_DIR = Join-Path $taskInstance 'config'
    $taskDatabase = Join-Path $taskInstance 'langflow.db'
}
$env:LANGFLOW_DATABASE_URL = 'sqlite:///' + ($taskDatabase -replace '\\','/')
if ($CoreDemo) {
    $taskAuthPath = Join-Path (Split-Path $taskDatabase -Parent) 'admin-auth.json'
    if (-not (Test-Path -LiteralPath $taskAuthPath)) {
        $taskAuth = @{username='local-sic-demo';password=([Guid]::NewGuid().ToString('N')+[Guid]::NewGuid().ToString('N'))}
        [IO.File]::WriteAllText($taskAuthPath, ($taskAuth | ConvertTo-Json))
    }
    $taskAuth = Get-Content -Raw -Encoding UTF8 -LiteralPath $taskAuthPath | ConvertFrom-Json
    $env:LANGFLOW_SUPERUSER = $taskAuth.username
    $env:LANGFLOW_SUPERUSER_PASSWORD = $taskAuth.password
    $env:LANGFLOW_AUTO_LOGIN = 'false'
}
$env:LANGFLOW_DO_NOT_TRACK = 'true'
$env:DO_NOT_TRACK = 'true'
$env:LANGCHAIN_TRACING_V2 = 'false'
$env:LANGSMITH_TRACING = 'false'
# Keep default authentication; credentials belong in the environment, never flows.
& $python -m langflow run --host 127.0.0.1 --port $Port
