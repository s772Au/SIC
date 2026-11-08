$taskProject = Split-Path $PSScriptRoot -Parent
$taskTokenFile = Join-Path $taskProject '.local/core-demo-token.txt'
if (-not (Test-Path -LiteralPath $taskTokenFile)) { throw 'Start the local core demo first; its private token file is missing.' }
$env:SIC_CORE_TOKEN = [IO.File]::ReadAllText($taskTokenFile).Trim()
$env:SIC_CORE_URL = 'http://127.0.0.1:18888'
$env:SIC_CORE_ACTOR = 'host'
$env:NO_PROXY = 'localhost,127.0.0.1,::1'
$env:LITELLM_LOCAL_MODEL_COST_MAP = 'true'
