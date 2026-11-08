param([string]$TargetDirectory = "", [string]$PythonExecutable = "python", [switch]$ProbeOnly)
$ErrorActionPreference = "Stop"
$project = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
Set-Location $project
if (-not $TargetDirectory) { $TargetDirectory = Join-Path $project "target/privacy-release" }
$TargetDirectory = [IO.Path]::GetFullPath($TargetDirectory)
$rustc = (Get-Command rustc -ErrorAction Stop).Source
$sysroot = (& $rustc --print sysroot).Trim()
if ($LASTEXITCODE -ne 0) { throw "Cannot discover Rust toolchain." }
$candidates = @(@{ Root=$project; Public='/build/sic' },
    @{ Root=$TargetDirectory; Public='/build/target' }, @{ Root=$sysroot; Public='/build/toolchain' })
foreach ($name in @('USERPROFILE','HOME','CARGO_HOME','RUSTUP_HOME')) {
    $value = [Environment]::GetEnvironmentVariable($name)
    if ($value) { $candidates += @{ Root=[IO.Path]::GetFullPath($value); Public=('/build/' + $name.ToLowerInvariant()) } }
}
$profileRoot = [Environment]::GetFolderPath('UserProfile')
if ($profileRoot) {
    $candidates += @{ Root=(Split-Path -Parent $profileRoot); Public='/build/profiles' }
    $candidates += @{ Root=$profileRoot; Public='/build/profile' }
    $candidates += @{ Root=(Join-Path $profileRoot '.cargo'); Public='/build/cargo' }
}
# rustc resolves the last matching map; place specific roots after broad roots.
$maps = @{}
foreach ($candidate in ($candidates | Sort-Object @{Expression={$_.Root.Length}}, @{Expression={$_.Root}}, @{Expression={$_.Public}})) {
    $native = $candidate.Root.TrimEnd('\','/')
    foreach ($variant in @($native.Replace('\','/'), $native.Replace('/','\'))) { $maps[$variant] = $candidate.Public }
}
$orderedMaps = @($maps.GetEnumerator() | Sort-Object @{Expression={$_.Key.Length}}, @{Expression={$_.Key}})
$flags = @('-C','strip=symbols','-C','debuginfo=0')
$cflags = @()
foreach ($map in $orderedMaps) {
    $flags += "--remap-path-prefix=$($map.Key)=$($map.Value)"
    # clang's cc-rs flags use shell-style parsing; quote each complete option.
    $cflags += ('"-ffile-prefix-map=' + $map.Key + '=' + $map.Value + '"')
    $cflags += ('"-fdebug-prefix-map=' + $map.Key + '=' + $map.Value + '"')
}
$restore = @{}
foreach ($name in @('CARGO_ENCODED_RUSTFLAGS','RUSTFLAGS','CFLAGS','CXXFLAGS','CC_SHELL_ESCAPED_FLAGS')) { $restore[$name] = [Environment]::GetEnvironmentVariable($name) }
try {
    [Environment]::SetEnvironmentVariable('CARGO_ENCODED_RUSTFLAGS', ($flags -join [char]31))
    [Environment]::SetEnvironmentVariable('RUSTFLAGS', $null)
    [Environment]::SetEnvironmentVariable('CFLAGS', ($cflags -join ' '))
    [Environment]::SetEnvironmentVariable('CXXFLAGS', ($cflags -join ' '))
    [Environment]::SetEnvironmentVariable('CC_SHELL_ESCAPED_FLAGS', '1')
    if ($ProbeOnly) {
        $probe = Join-Path $TargetDirectory 'privacy-probe'
        New-Item -ItemType Directory -Force -Path $probe | Out-Null
        $source = Join-Path $probe 'probe.rs'
        'fn main() { println!("{}", file!()); }' | Set-Content -LiteralPath $source -Encoding utf8
        $binary = Join-Path $probe 'privacy-probe.exe'
        & $rustc $source -O @flags -o $binary
        if ($LASTEXITCODE -ne 0) { throw "Privacy compile probe failed." }
    } else {
        cargo build --release --target-dir $TargetDirectory
        if ($LASTEXITCODE -ne 0) { throw "Private release build failed." }
        $binary = Join-Path $TargetDirectory 'release/sic-proxy.exe'
    }
    $report = Join-Path (Split-Path -Parent $binary) 'privacy-build.json'
    $scanArgs = @('scripts/scan_release_privacy.py', $binary, '--require-stripped', '--out', $report)
    foreach ($map in $orderedMaps) { $scanArgs += @('--forbid-root', $map.Key) }
    & $PythonExecutable @scanArgs
    if ($LASTEXITCODE -ne 0) { throw "Privacy scan rejected the binary; matching values are withheld." }
    $evidence = Get-Content -LiteralPath $report -Raw | ConvertFrom-Json
    $evidence | Add-Member -NotePropertyName build_policy -NotePropertyValue @{
        kind = $(if ($ProbeOnly) { 'probe' } else { 'release' })
        remap_path_prefix = $true
        mapped_root_forms = $orderedMaps.Count
        c_and_cpp_prefix_maps = $true
        symbol_strip = 'symbols'
        debug_info = 0
    } -Force
    $evidence | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $report -Encoding utf8
    Write-Host 'Remapped, symbol-stripped binary passed privacy scanning.'
} finally {
    foreach ($name in $restore.Keys) { [Environment]::SetEnvironmentVariable($name, $restore[$name]) }
}
