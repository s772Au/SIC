param([string]$OutputDirectory = $env:SIC_PACKAGE_OUTPUT, [string]$RuntimeDirectory = $env:SIC_TOOLCHAIN_ROOT,
    [string]$ReleaseVersion = "v0.1", [string]$BinaryPath = $env:SIC_RELEASE_BINARY,
    [string]$PythonExecutable = "python")
$ErrorActionPreference = "Stop"
$project = Split-Path -Parent $PSScriptRoot
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $project 'target/packages' }
if (-not $BinaryPath) { $BinaryPath = Join-Path $project 'target/privacy-release/release/sic-proxy.exe' }
if (-not $RuntimeDirectory) {
    $candidates = @()
    $compiler = Get-Command rustc -ErrorAction SilentlyContinue
    if ($compiler) {
        $candidates += (& $compiler.Source --print sysroot).Trim()
        $candidates += Split-Path -Parent (Split-Path -Parent $compiler.Source)
    }
    $RuntimeDirectory = $candidates | Where-Object { Test-Path -LiteralPath (Join-Path $_ 'bin/libunwind.dll') } | Select-Object -First 1
}
if (-not $RuntimeDirectory) { throw 'Pass -RuntimeDirectory or SIC_TOOLCHAIN_ROOT for the discovered ARM64 runtime.' }
if ($ReleaseVersion -ne 'v0.1') { throw 'Demo releases currently overwrite v0.1; use that release name.' }
$name = "sic-$ReleaseVersion-demo-windows-arm64"
$stage = Join-Path $OutputDirectory $name
$zip = Join-Path $OutputDirectory ($name + ".zip")
if ((Test-Path -LiteralPath $stage) -or (Test-Path -LiteralPath $zip)) { throw "Package output already exists; use another OutputDirectory." }
$exe = $BinaryPath
if (-not (Test-Path -LiteralPath $exe)) { throw 'Run scripts/build_private_release.ps1 first.' }
$buildReport = Join-Path (Split-Path -Parent $exe) 'privacy-build.json'
if (-not (Test-Path -LiteralPath $buildReport)) { throw 'Missing privacy build evidence; use build_private_release.ps1.' }
$identity = Get-Content -LiteralPath $buildReport -Raw | ConvertFrom-Json
if (-not $identity.passed -or $identity.files.Count -ne 1 -or
    $identity.files[0].sha256 -ne (Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash.ToLowerInvariant() -or
    -not $identity.build_policy.remap_path_prefix -or $identity.build_policy.kind -ne 'release') {
    throw 'Privacy build evidence does not match this remapped release binary.'
}
New-Item -ItemType Directory -Path (Join-Path $stage "bin"), (Join-Path $stage "scripts"), (Join-Path $stage "benchmarks"), (Join-Path $stage "docs"), (Join-Path $stage "licenses/libunwind") -Force | Out-Null
Copy-Item -LiteralPath $exe -Destination (Join-Path $stage "bin")
Copy-Item -LiteralPath $buildReport -Destination (Join-Path $stage 'bin')
Copy-Item -LiteralPath (Join-Path $RuntimeDirectory "bin/libunwind.dll") -Destination (Join-Path $stage "bin")
$strip = Get-Command llvm-strip -ErrorAction SilentlyContinue
if ($strip) {
    & $strip.Source --strip-all (Join-Path $stage 'bin/libunwind.dll')
    if ($LASTEXITCODE -ne 0) { throw 'Runtime symbol stripping failed.' }
}
& $PythonExecutable (Join-Path $PSScriptRoot 'scan_release_privacy.py') (Join-Path $stage 'bin/sic-proxy.exe') (Join-Path $stage 'bin/libunwind.dll') `
    --forbid-root $RuntimeDirectory --require-stripped --out (Join-Path $stage 'privacy-scan.json')
if ($LASTEXITCODE -ne 0) { throw 'Package binary privacy check failed.' }
Copy-Item -Path (Join-Path $RuntimeDirectory "share/licenses/libunwind/*") -Destination (Join-Path $stage "licenses/libunwind")
foreach ($script in @("sic_demo.py", "dsh_chat_bridge.py", "run_experiment.py", "score_experiment.py", "start_dsh_demo.ps1", "dataset_adapters.py", "dataset_runner.py", "benchmark_scoring.py", "verify_dataset_run.py", "audit_pi_bridge_roundtrip.mjs", "scan_release_privacy.py")) {
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot $script) -Destination (Join-Path $stage "scripts")
}
Copy-Item -LiteralPath (Join-Path $project "benchmarks/demo-cases.json") -Destination (Join-Path $stage "benchmarks")
Copy-Item -LiteralPath (Join-Path $project "benchmarks/requirements.txt"), (Join-Path $project "benchmarks/sources-lock.json") -Destination (Join-Path $stage "benchmarks")
Copy-Item -LiteralPath (Join-Path $project "benchmarks/test-catalog.json") -Destination (Join-Path $stage "benchmarks")
Copy-Item -LiteralPath (Join-Path $project "benchmarks/source_paths.py"), (Join-Path $project "benchmarks/source-overrides.example.json"), (Join-Path $project "benchmarks/SOURCE_PATHS.md") -Destination (Join-Path $stage "benchmarks")
New-Item -ItemType Directory -Force -Path (Join-Path $stage "benchmarks/vendor") | Out-Null
Get-ChildItem -LiteralPath (Join-Path $project "benchmarks/vendor") -File | ForEach-Object {
    Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $stage "benchmarks/vendor")
}
$integrationRoot = Join-Path $project "integrations"
$integrationTop = @('README.md','pyproject.toml','requirements.lock','generate_flows.py','langgraph.json','start-langflow.ps1','start-langgraph.ps1','dsh-setup.md','dsh-example.patch.yml')
Get-ChildItem -LiteralPath $integrationRoot -Recurse -File | Where-Object {
    $itemRelative = [IO.Path]::GetRelativePath($integrationRoot, $_.FullName).Replace('\','/')
    $parts = $itemRelative.Split('/')
    ($parts.Count -eq 1 -and $integrationTop -contains $_.Name) -or
    ($parts.Count -gt 1 -and $parts -notcontains '__pycache__' -and
        (($parts[0] -in @('components','sic_workflow','tests') -and $_.Extension -eq '.py') -or ($parts[0] -eq 'flows' -and $_.Extension -eq '.json')))
} | ForEach-Object {
    $relative = [IO.Path]::GetRelativePath($project, $_.FullName)
    $destination = Join-Path $stage $relative
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $destination) | Out-Null
    Copy-Item -LiteralPath $_.FullName -Destination $destination
}
Copy-Item -LiteralPath (Join-Path $project "docs/backend-release-readiness.md") -Destination (Join-Path $stage "docs")
Copy-Item -LiteralPath (Join-Path $project "docs/real-text-experiments.md") -Destination (Join-Path $stage "docs")
Copy-Item -LiteralPath (Join-Path $project "docs/dsh-bridge-replay-compatibility.md") -Destination (Join-Path $stage "docs")
New-Item -ItemType Directory -Force -Path (Join-Path $stage "tests/fixtures") | Out-Null
Copy-Item -LiteralPath (Join-Path $project "tests/test_bridge.py") -Destination (Join-Path $stage "tests")
Copy-Item -LiteralPath (Join-Path $project "tests/fixtures/pi-ai-1.1.0-replay.json") -Destination (Join-Path $stage "tests/fixtures")
$readme = @'
SIC __RELEASE_VERSION__ local demonstration - Windows ARM64 only
Mechanism version: 9.0.0-alpha.3. Not a stable release or quality result.

Requires Windows on ARM64, Python 3.10+ and PowerShell. Rust/MSYS2 is not required.
bin/libunwind.dll is bundled; see licenses/libunwind. Windows UCRT/system DLLs
are supplied by Windows. This package is not for x64 Windows, Linux or macOS.

Offline pipeline check (no real provider):
  python scripts/run_experiment.py --out ./outputs/my-new-offline-run
  python scripts/score_experiment.py ./outputs/my-new-offline-run
The destination must not already exist. Mock scores are plumbing checks only.

Visual editor: install Langflow separately using integrations/README.md and
integrations/requirements.lock. No Langflow installation or old WebUI is bundled.
The included official flow JSON and SIC components require the separate install.
Full real-dataset plans/runs require Python dependencies in benchmarks/requirements.txt;
large source datasets are not bundled. See docs/real-text-experiments.md.
Public dataset paths are relative. Private mount configuration remains local;
see benchmarks/SOURCE_PATHS.md and source-overrides.example.json.

DSH Chat full-history demonstration:
  Set SIC_COMPRESSER_URL to the full dedicated /v1/chat/completions endpoint.
  Set SIC_COMPRESSER_MODEL to a fixed model.
  Set SIC_EXPERIMENT_API_KEY to the private provider key.
  Run ./scripts/start_dsh_demo.ps1
DSH base URL: http://127.0.0.1:8788/v1 . Keep sending full history, use model
above, and supply a unique x-sic-bridge-session header for each conversation.
If DSH cannot set custom headers, a host adapter is still required.
Port 8341 is a non-fixed test route, excluded from stable-provider runs. The launcher rejects it.

The bridge automatically seals complete tool groups above a byte threshold,
runs one editing response, validates/commits/publishes, and forwards the live tail.
SSE is buffered; session cursors are in memory; failures require inspection and
a new session key. Actual DSH UI/provider compatibility is not verified.
No model-token admission, automatic N-order promotion, official full BEAM/CorpusQA
scoring reproduction or long-task quality evidence. Local versioned text-dataset
adapters/runner/scorer are included. Read docs/backend-release-readiness.md.

This patch permits only known empty metadata and lossless JSON object argument
reformatting on the last bridge-owned assistant reply. Prior history remains strict.
See docs/dsh-bridge-replay-compatibility.md for actual installed pi-ai fixtures,
precision-loss rejection, remaining limits, and the offline audit command.

This privacy revision remaps build roots, strips binary symbols and includes
privacy-build.json plus an ASCII/UTF16 path/credential scan. No local mount
configuration, papers, private review notes, runtime database or session logs
are included. Release identity and per-file checksums accompany this package.
'@
$readme.Replace('__RELEASE_VERSION__', $ReleaseVersion) | Set-Content -LiteralPath (Join-Path $stage "README.txt") -Encoding utf8
$hashes = @{}
Get-ChildItem -LiteralPath $stage -Recurse -File | ForEach-Object {
    $relative = [IO.Path]::GetRelativePath($stage, $_.FullName).Replace('\','/')
    $hashes[$relative] = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
}
$hashes | ConvertTo-Json -Depth 3 | Set-Content -LiteralPath (Join-Path $stage "checksums.json") -Encoding utf8
Compress-Archive -LiteralPath $stage -DestinationPath $zip -CompressionLevel Optimal
[pscustomobject]@{ Release=$ReleaseVersion; Asset=[IO.Path]::GetFileName($zip); Bytes=(Get-Item -LiteralPath $zip).Length;
    SHA256=(Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant() }
