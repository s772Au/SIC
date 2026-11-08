param([int]$Port = 12025, [switch]$CoreDemo)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$cli = Join-Path $root '.local\langgraph-venv\Scripts\langgraph.exe'
if (-not (Test-Path $cli)) { throw 'Install the separate Studio environment first; see integrations/README.md.' }
if ($CoreDemo) { . (Join-Path $PSScriptRoot 'use-core-demo-environment.ps1') }
$env:PYTHONPATH = $PSScriptRoot + [IO.Path]::PathSeparator + $root
$env:PYTHONUTF8 = '1'
$env:LANGSMITH_TRACING = 'false'
$env:LANGCHAIN_TRACING_V2 = 'false'
$taskInstance = Join-Path $root ".local/langgraph-$Port"
New-Item -ItemType Directory -Force $taskInstance | Out-Null
$taskConfig = Join-Path $taskInstance 'langgraph.json'
$taskGraphs = @{sic_core='sic_workflow.v2_graph:graph';sic_candidate='sic_workflow.workbench_graph:candidate_graph';sic_memory='sic_workflow.workbench_graph:memory_graph';sic_legacy_control='sic_workflow.graph:graph'}
foreach($taskDomain in @('decision','routes','runtime','configuration','editor','source')) { $taskGraphs['sic_'+$taskDomain] = 'sic_workflow.domain_graphs:'+$taskDomain+'_graph' }
$taskGraphs['sic_accounting'] = 'sic_workflow.accounting_graph:graph'
$taskGraphs['sic_hierarchy'] = 'sic_workflow.hierarchy_graph:graph'
$taskPublicConfig = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $PSScriptRoot 'langgraph.json') | ConvertFrom-Json
$taskConfigJson = @{dependencies=@($PSScriptRoot);graphs=$taskGraphs;http=$taskPublicConfig.http} | ConvertTo-Json -Depth 8
[IO.File]::WriteAllText($taskConfig, $taskConfigJson)
Push-Location $taskInstance
try { & $cli dev --config $taskConfig --host 127.0.0.1 --port $Port --no-browser --no-reload }
finally { Pop-Location }
