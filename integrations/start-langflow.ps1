param(
    [int]$Port = 17860,
    [switch]$CoreDemo,
    [string]$PythonPath = $env:SIC_LANGFLOW_PYTHON,
    [string]$RuntimeDir = $env:SIC_UI_RUNTIME_DIR,
    [string]$ExtraPythonPath = $env:SIC_LANGFLOW_EXTRA_PYTHONPATH,
    [string]$PrivateAuthPath,
    [switch]$NoProxy
)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$python = if ($PythonPath) { $PythonPath } else { Join-Path $root '.local\langflow-ui-venv\Scripts\python.exe' }
if (-not (Test-Path $python)) { throw 'Install the integration environment first; see integrations/README.md.' }
if ($CoreDemo) { . (Join-Path $PSScriptRoot 'use-core-demo-environment.ps1') }
if (-not $CoreDemo -and -not $env:SIC_CORE_TOKEN -and $env:SIC_CORE_TOKEN_FILE) {
    $env:SIC_CORE_TOKEN = [IO.File]::ReadAllText($env:SIC_CORE_TOKEN_FILE).Trim()
}
$env:PYTHONPATH = (@($ExtraPythonPath, $PSScriptRoot, $root) | Where-Object { $_ }) -join [IO.Path]::PathSeparator
$env:LANGFLOW_COMPONENTS_PATH = Join-Path $PSScriptRoot 'components'
$taskLocal = if ($RuntimeDir) { $RuntimeDir } else { Join-Path $root '.local' }
if ($Port -eq 17860) {
    $env:LANGFLOW_CONFIG_DIR = Join-Path $taskLocal 'langflow-config'
    $taskDatabase = Join-Path $taskLocal 'langflow.db'
} else {
    $taskInstance = Join-Path $taskLocal "langflow-$Port"
    New-Item -ItemType Directory -Force $taskInstance | Out-Null
    $env:LANGFLOW_CONFIG_DIR = Join-Path $taskInstance 'config'
    $taskDatabase = Join-Path $taskInstance 'langflow.db'
}
$env:LANGFLOW_DATABASE_URL = 'sqlite:///' + $taskDatabase.Replace('\','/')
if ($CoreDemo -or $PrivateAuthPath) {
    $taskAuthPath = if ($PrivateAuthPath) { $PrivateAuthPath } else { Join-Path (Split-Path $taskDatabase -Parent) 'admin-auth.json' }
    if (-not (Test-Path -LiteralPath $taskAuthPath)) {
        $taskAuth = @{username='local-sic-demo';password=([Guid]::NewGuid().ToString('N')+[Guid]::NewGuid().ToString('N'))}
        [IO.File]::WriteAllText($taskAuthPath, ($taskAuth | ConvertTo-Json))
    }
    $taskAuth = Get-Content -Raw -Encoding UTF8 -LiteralPath $taskAuthPath | ConvertFrom-Json
    $env:LANGFLOW_SUPERUSER = $taskAuth.username
    $env:LANGFLOW_SUPERUSER_PASSWORD = $taskAuth.password
    $env:LANGFLOW_AUTO_LOGIN = 'false'
    $env:LANGFLOW_CUSTOM_COMPONENT_ADMIN_ONLY = 'true'
}
$env:LANGFLOW_DO_NOT_TRACK = 'true'
$env:DO_NOT_TRACK = 'true'
$env:LANGCHAIN_TRACING_V2 = 'false'
$env:LANGSMITH_TRACING = 'false'
# Keep default authentication; credentials belong in the environment, never flows.
if ($NoProxy) {
    & $python (Join-Path $PSScriptRoot 'run_langflow_no_proxy.py') run --host 127.0.0.1 --port $Port --no-open-browser
} else {
    & $python -m langflow run --host 127.0.0.1 --port $Port --no-open-browser
}
