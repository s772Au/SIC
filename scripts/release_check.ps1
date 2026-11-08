param([string]$OutputDirectory = "")
$ErrorActionPreference = "Stop"
Set-Location (Split-Path -Parent $PSScriptRoot)
if (-not $OutputDirectory) {
    $OutputDirectory = Join-Path $env:TEMP ("sic-release-check-" + [Guid]::NewGuid().ToString("N"))
}
function Check-Exit([string]$Step) {
    if ($LASTEXITCODE -ne 0) { throw "$Step failed (exit $LASTEXITCODE)" }
}
cargo fmt -- --check
Check-Exit "format"
cargo test
Check-Exit "Rust tests"
cargo clippy -- -D warnings
Check-Exit "Clippy"
cargo build
Check-Exit "build"
python tests/smoke_backend.py
Check-Exit "local smoke"
python tests/smoke_dsh_bridge.py
Check-Exit "DSH full-history bridge smoke"
python -m unittest discover -s tests -p "test_*.py"
Check-Exit "runner tests"
python scripts/run_experiment.py --out $OutputDirectory
Check-Exit "offline E2E"
python scripts/score_experiment.py $OutputDirectory
Check-Exit "artifact verification"
Write-Host "Release checks passed. Offline evidence: $OutputDirectory"
