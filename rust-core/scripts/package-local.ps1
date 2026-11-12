[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [string]$OutputDirectory,

    [string]$ReleaseExe,
    [string]$SupervisorExe,
    [string]$LibUnwindDll,
    [string]$LibUnwindLicense
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$coreRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$repositoryRoot = [System.IO.Path]::GetFullPath((Join-Path $coreRoot '..'))
$rootLicense = Join-Path $coreRoot 'LICENSE'
if (-not (Test-Path -LiteralPath $rootLicense -PathType Leaf)) {
    $rootLicense = Join-Path $repositoryRoot 'LICENSE'
}
$destination = [System.IO.Path]::GetFullPath($OutputDirectory)

if (Test-Path -LiteralPath $destination) {
    throw 'Output directory already exists; packaging never overwrites or deletes.'
}

$releaseArguments = @(
    @($ReleaseExe, $LibUnwindDll, $LibUnwindLicense) |
        Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
)
if ($releaseArguments.Count -ne 0 -and $releaseArguments.Count -ne 3) {
    throw 'Specify ReleaseExe, LibUnwindDll, and LibUnwindLicense together.'
}
if (-not [string]::IsNullOrWhiteSpace($SupervisorExe) -and $releaseArguments.Count -ne 3) {
    throw 'SupervisorExe requires the Core executable, DLL, and DLL license.'
}

function Resolve-RequiredFile([string]$Path, [string]$Label) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "Required $Label file is missing."
    }
    return (Resolve-Path -LiteralPath $Path).Path
}

$releaseFiles = @()
if ($releaseArguments.Count -eq 3) {
    $exe = Resolve-RequiredFile $ReleaseExe 'release executable'
    $dll = Resolve-RequiredFile $LibUnwindDll 'libunwind DLL'
    $dllLicense = Resolve-RequiredFile $LibUnwindLicense 'libunwind license'
    if ([System.IO.Path]::GetExtension($exe) -ine '.exe') {
        throw 'ReleaseExe must be an .exe file.'
    }
    if ([System.IO.Path]::GetExtension($dll) -ine '.dll') {
        throw 'LibUnwindDll must be a .dll file.'
    }
    $releaseFiles = @(
        @{ Source = $exe; Relative = "release\$([System.IO.Path]::GetFileName($exe))" },
        @{ Source = $dll; Relative = "release\$([System.IO.Path]::GetFileName($dll))" },
        @{ Source = $dllLicense; Relative = 'release\libunwind-LICENSE' }
    )
    if (-not [string]::IsNullOrWhiteSpace($SupervisorExe)) {
        $supervisor = Resolve-RequiredFile $SupervisorExe 'supervisor executable'
        if ([System.IO.Path]::GetExtension($supervisor) -ine '.exe') {
            throw 'SupervisorExe must be an .exe file.'
        }
        $releaseFiles += @{ Source = $supervisor; Relative = "release\$([System.IO.Path]::GetFileName($supervisor))" }
    }
}

$requiredFiles = @(
    @{ Source = (Join-Path $coreRoot 'Cargo.toml'); Relative = 'Cargo.toml' },
    @{ Source = (Join-Path $coreRoot 'Cargo.lock'); Relative = 'Cargo.lock' },
    @{ Source = (Join-Path $coreRoot 'README.md'); Relative = 'README.md' },
    @{ Source = (Join-Path $coreRoot '快速开始.md'); Relative = '快速开始.md' },
    @{ Source = (Join-Path $coreRoot 'LEAN-ACCEPTANCE.md'); Relative = 'LEAN-ACCEPTANCE.md' },
    @{ Source = (Join-Path $coreRoot 'CONTRACT-STATUS.md'); Relative = 'CONTRACT-STATUS.md' },
    @{ Source = (Join-Path $coreRoot 'DSH-CONTEXT-ANALYSIS.md'); Relative = 'DSH-CONTEXT-ANALYSIS.md' },
    @{ Source = (Join-Path $coreRoot 'PEER-SUPERVISOR.md'); Relative = 'PEER-SUPERVISOR.md' },
    @{ Source = $rootLicense; Relative = 'LICENSE' },
    @{ Source = (Join-Path $coreRoot 'scripts\package-local.ps1'); Relative = 'scripts\package-local.ps1' },
    @{ Source = (Join-Path $coreRoot 'scripts\setup-dsh-runtime.ps1'); Relative = 'scripts\setup-dsh-runtime.ps1' },
    @{ Source = (Join-Path $coreRoot 'scripts\run-dsh-headless.ps1'); Relative = 'scripts\run-dsh-headless.ps1' },
    @{ Source = (Join-Path $coreRoot 'scripts\read-dsh-credential.cjs'); Relative = 'scripts\read-dsh-credential.cjs' },
    @{ Source = (Join-Path $coreRoot 'dsh-plugin\index.mjs'); Relative = 'dsh-plugin\index.mjs' },
    @{ Source = (Join-Path $coreRoot 'dsh-plugin\native-events.mjs'); Relative = 'dsh-plugin\native-events.mjs' },
    @{ Source = (Join-Path $coreRoot 'dsh-plugin\view-host.mjs'); Relative = 'dsh-plugin\view-host.mjs' },
    @{ Source = (Join-Path $coreRoot 'dsh-plugin\client.js'); Relative = 'dsh-plugin\client.js' },
    @{ Source = (Join-Path $coreRoot 'dsh-plugin\package.json'); Relative = 'dsh-plugin\package.json' },
    @{ Source = (Join-Path $coreRoot 'dsh-plugin\pnpm-lock.yaml'); Relative = 'dsh-plugin\pnpm-lock.yaml' },
    @{ Source = (Join-Path $coreRoot 'dsh-plugin\cordis.patch.yml'); Relative = 'dsh-plugin\cordis.patch.yml' },
    @{ Source = (Join-Path $coreRoot 'dsh-plugin\README.md'); Relative = 'dsh-plugin\README.md' }
)

foreach ($entry in $requiredFiles) {
    if (-not (Test-Path -LiteralPath $entry.Source -PathType Leaf)) {
        throw "Required allowlisted file is missing: $($entry.Relative)"
    }
}
$cargoConfig = Join-Path $coreRoot '.cargo\config.toml'
if (Test-Path -LiteralPath $cargoConfig -PathType Leaf) {
    $requiredFiles += @{ Source = $cargoConfig; Relative = '.cargo\config.toml' }
}

function Copy-AllowlistedTree(
    [string]$SourceRoot,
    [string]$RelativeRoot,
    [string[]]$Extensions
) {
    if (-not (Test-Path -LiteralPath $SourceRoot -PathType Container)) {
        throw "Required source directory is missing: $RelativeRoot"
    }
    $sourcePrefix = [System.IO.Path]::GetFullPath($SourceRoot).TrimEnd('\') + '\'
    $files = Get-ChildItem -LiteralPath $SourceRoot -Recurse -File |
        Where-Object {
            $relative = $_.FullName.Substring($sourcePrefix.Length)
            $allowedExtension = (-not $Extensions) -or ($Extensions -contains $_.Extension.ToLowerInvariant())
            $temporaryPath = $relative -match '(?i)(^|\\)(target|node_modules|\.local|\.git|tmp|temp|test-output[^\\]*)(\\|$)'
            $forbiddenExtension = $_.Extension -match '(?i)^\.(db|sqlite|sqlite3|log|env|pem|key)$'
            $allowedLicenseFile = $RelativeRoot -ne 'third-party-licenses' -or
                $relative -eq 'THIRD-PARTY-LICENSES.md' -or
                $_.Name -eq 'UPSTREAM-SOURCE.txt' -or
                $_.Name -match '(?i)^(LICENSE|LICENCE|COPYING|NOTICE|COPYRIGHT)(?:[._-].*)?$'
            $allowedExtension -and $allowedLicenseFile -and -not $temporaryPath -and -not $forbiddenExtension
        }
    foreach ($file in $files) {
        $relative = $file.FullName.Substring($sourcePrefix.Length)
        $packageRelative = Join-Path $RelativeRoot $relative
        Copy-PackageFile $file.FullName $packageRelative
    }
}

function Copy-PackageFile([string]$Source, [string]$Relative) {
    $target = Join-Path $destination $Relative
    $targetParent = Split-Path -Parent $target
    if (-not (Test-Path -LiteralPath $targetParent -PathType Container)) {
        New-Item -ItemType Directory -Path $targetParent | Out-Null
    }
    Copy-Item -LiteralPath $Source -Destination $target
}

New-Item -ItemType Directory -Path $destination | Out-Null
foreach ($entry in $requiredFiles) {
    Copy-PackageFile $entry.Source $entry.Relative
}
Copy-AllowlistedTree (Join-Path $coreRoot 'src') 'src' @('.rs')
Copy-AllowlistedTree (Join-Path $coreRoot 'config') 'config' @('.json', '.toml', '.yaml', '.yml')
Copy-AllowlistedTree (Join-Path $coreRoot 'tests') 'tests' @('.rs', '.json', '.txt')
Copy-AllowlistedTree (Join-Path $coreRoot 'third-party-licenses') 'third-party-licenses' @()
$mockTestAllowlist = @(
    'dsh-plugin\test\run-isolated-headless.ps1',
    'dsh-plugin\test\bind-provider.spec.mjs',
    'dsh-plugin\test\native-events.spec.mjs',
    'dsh-plugin\test\view-host.spec.mjs',
    'dsh-plugin\test\run-web-overview-fixture.ps1',
    'dsh-plugin\test\inspect-web-fixture.mjs',
    'dsh-plugin\test\run-bind-headless.ps1',
    'dsh-plugin\test\run-rust-bind-headless.ps1',
    'dsh-plugin\test\run-peer-bootstrap-headless.ps1',
    'dsh-plugin\test\run-peer-task-bootstrap-headless.ps1',
    'dsh-plugin\test\run-peer-supervisor-headless.ps1',
    'dsh-plugin\test\run-peer-supervisor-watch-headless.ps1',
    'dsh-plugin\test\run-peer-native-read-watch-headless.ps1',
    'dsh-plugin\test\run-local-launcher-fixture.ps1',
    'dsh-plugin\test\run-idle-rotation-dsh-fixture.ps1',
    'dsh-plugin\test\idle-rotation-mock-provider.mjs',
    'dsh-plugin\test\native-read-mock-chat-provider.mjs',
    'dsh-plugin\test\verify-status-fixture.py',
    'dsh-plugin\test\peer-supervisor-watch-config.template.json',
    'dsh-plugin\test\mock-chat-provider.mjs',
    'dsh-plugin\test\mock-bind-bridge.mjs',
    'dsh-plugin\test\peer-mock-chat-provider.mjs',
    'dsh-plugin\test\bridge-observer.mjs',
    'dsh-plugin\test\receipt-summary.py',
    'dsh-plugin\test\rust-bind-db-summary.py',
    'dsh-plugin\test\peer-fixture-state-summary.py'
)
foreach ($relative in $mockTestAllowlist) {
    $source = Resolve-RequiredFile (Join-Path $coreRoot $relative) 'allowlisted local mock test asset'
    Copy-PackageFile $source $relative
}
foreach ($entry in $releaseFiles) {
    Copy-PackageFile $entry.Source $entry.Relative
}

# Fail closed if an unexpected database, log, env/credential file, cache, or
# live-test tree somehow appeared in the staged package.
$forbiddenFiles = @(
    Get-ChildItem -LiteralPath $destination -Recurse -File |
        Where-Object { $_.Extension -match '(?i)^\.(db|sqlite|sqlite3|log|env|pem|key)$' }
)
if ($forbiddenFiles.Count -gt 0) {
    throw 'Staged package contains a forbidden database, log, or credential file.'
}
$forbiddenDirectories = @(
    Get-ChildItem -LiteralPath $destination -Recurse -Directory |
        Where-Object { $_.Name -match '(?i)^(node_modules|target|\.local|tmp|temp|test-output.*)$' }
)
if ($forbiddenDirectories.Count -gt 0) {
    throw 'Staged package contains a forbidden generated or temporary directory.'
}

$manifestPath = Join-Path $destination 'SHA256SUMS.txt'
$manifestLines = Get-ChildItem -LiteralPath $destination -Recurse -File |
    Where-Object { $_.FullName -ne $manifestPath } |
    Sort-Object { $_.FullName.Substring($destination.Length).TrimStart('\').ToLowerInvariant() } |
    ForEach-Object {
        $relative = $_.FullName.Substring($destination.Length).TrimStart('\').Replace('\', '/')
        $hash = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        "$hash  $relative"
    }
[System.IO.File]::WriteAllLines($manifestPath, [string[]]$manifestLines, [System.Text.UTF8Encoding]::new($false))

$fileCount = @(Get-ChildItem -LiteralPath $destination -Recurse -File).Count
Write-Output "Package created: $destination"
Write-Output "Files including SHA256SUMS.txt: $fileCount"
