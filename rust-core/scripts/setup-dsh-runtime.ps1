[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$SourceDirectory,

    [Parameter(Mandatory = $true)]
    [string]$RuntimeDirectory
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Get-ResolvedDirectory([string]$Path, [string]$Label) {
    if (-not [System.IO.Path]::IsPathRooted($Path)) {
        throw "$Label must be an absolute path."
    }
    if (-not (Test-Path -LiteralPath $Path -PathType Container)) {
        throw "$Label directory does not exist."
    }
    $resolved = (Resolve-Path -LiteralPath $Path -ErrorAction Stop).Path
    $item = Get-Item -LiteralPath $resolved -Force
    if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "$Label cannot be a reparse point."
    }
    return [System.IO.Path]::GetFullPath($resolved).TrimEnd('\')
}

function Test-PathWithin([string]$Path, [string]$Root) {
    return $Path.StartsWith($Root.TrimEnd('\') + '\', [StringComparison]::OrdinalIgnoreCase)
}

function Get-PackageInventory([string]$Root, [switch]$AllowInstalledDependencies) {
    $files = @(Get-ChildItem -LiteralPath $Root -Force -File -Recurse)
    $directories = @(Get-ChildItem -LiteralPath $Root -Force -Directory -Recurse)
    foreach ($item in @($files) + @($directories)) {
        if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            $relativeItem = $item.FullName.Substring($Root.TrimEnd('\').Length + 1).Replace('\', '/')
            if ($AllowInstalledDependencies -and $relativeItem.StartsWith('dsh-plugin/node_modules/', [StringComparison]::OrdinalIgnoreCase)) {
                continue
            }
            throw 'Package contains a reparse point; verification stopped.'
        }
    }

    $relativeFiles = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    $rootPrefix = $Root.TrimEnd('\') + '\'
    foreach ($file in $files) {
        $relative = $file.FullName.Substring($rootPrefix.Length).Replace('\', '/')
        if ($relative -ieq 'SHA256SUMS.txt') { continue }
        if ($AllowInstalledDependencies -and $relative.StartsWith('dsh-plugin/node_modules/', [StringComparison]::OrdinalIgnoreCase)) {
            continue
        }
        if ($relative -match '(?i)(^|/)(node_modules|target|\.git|\.local|tmp|temp)(/|$)') {
            throw 'Package contains an unexpected generated or cache directory.'
        }
        if ($file.Extension -match '(?i)^\.(db|sqlite|sqlite3|log|env|pem|key|pfx|p12)$' -or $file.Name -ieq '.npmrc') {
            throw 'Package contains a database, log, credential, or package-manager config file.'
        }
        [void]$relativeFiles.Add($relative)
    }

    $manifest = Join-Path $Root 'SHA256SUMS.txt'
    if (-not (Test-Path -LiteralPath $manifest -PathType Leaf)) {
        throw 'Package SHA256SUMS.txt is missing.'
    }
    $manifestHash = (Get-FileHash -LiteralPath $manifest -Algorithm SHA256).Hash.ToLowerInvariant()
    $listed = [System.Collections.Generic.Dictionary[string,string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach ($line in [System.IO.File]::ReadAllLines($manifest)) {
        if ([string]::IsNullOrWhiteSpace($line)) { continue }
        if ($line -notmatch '^([0-9a-fA-F]{64})  (.+)$') {
            throw 'Package SHA256SUMS.txt has an invalid line.'
        }
        $expected = $Matches[1].ToLowerInvariant()
        $relative = $Matches[2].Replace('\', '/')
        if ([System.IO.Path]::IsPathRooted($relative) -or $relative -match '(^|/)\.\.?(/|$)' -or $relative -ieq 'SHA256SUMS.txt') {
            throw 'Package SHA256SUMS.txt contains an unsafe path.'
        }
        if ($listed.ContainsKey($relative)) { throw 'Package SHA256SUMS.txt contains duplicate paths.' }
        $listed.Add($relative, $expected)
    }
    if ($listed.Count -eq 0) { throw 'Package SHA256SUMS.txt contains no file entries.' }

    foreach ($relative in $relativeFiles) {
        if (-not $listed.ContainsKey($relative)) {
            throw 'Package contains a file not covered by SHA256SUMS.txt.'
        }
    }
    foreach ($relative in $listed.Keys) {
        if (-not $relativeFiles.Contains($relative)) {
            throw 'Package is missing a file listed in SHA256SUMS.txt.'
        }
        $path = Join-Path $Root ($relative.Replace('/', '\'))
        $actual = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($actual -ne $listed[$relative]) { throw 'Package checksum verification failed.' }
    }

    # Scan only allowlisted text documents/source, never print the matched value.
    $credentialPattern = '(?i)\bsk-[A-Za-z0-9_-]{24,}\b|\bAKIA[A-Z0-9]{16}\b|\bBearer\s+[A-Za-z0-9._-]{32,}\b|(?:api[_-]?key|secret|password|token)\s*[:=]\s*["'']?[A-Za-z0-9/+_.=-]{32,}'
    foreach ($file in $files) {
        $relative = $file.FullName.Substring($rootPrefix.Length).Replace('\', '/')
        if ($AllowInstalledDependencies -and $relative.StartsWith('dsh-plugin/node_modules/', [StringComparison]::OrdinalIgnoreCase)) { continue }
        if ($file.Extension -notmatch '(?i)^\.(md|txt|json|ya?ml|toml|mjs|js|ps1|py|rs)$') { continue }
        $content = [System.IO.File]::ReadAllText($file.FullName)
        if ([System.Text.RegularExpressions.Regex]::IsMatch($content, $credentialPattern)) {
            throw 'Package source appears to contain a credential value.'
        }
    }
    return [pscustomobject]@{ ManifestHash = $manifestHash; FileCount = $listed.Count }
}

$source = Get-ResolvedDirectory $SourceDirectory 'Source'
$destinationInput = [System.IO.Path]::GetFullPath($RuntimeDirectory)
if (-not [System.IO.Path]::IsPathRooted($RuntimeDirectory)) {
    throw 'RuntimeDirectory must be an absolute path.'
}
if (Test-Path -LiteralPath $destinationInput) {
    throw 'RuntimeDirectory already exists; setup never overwrites or removes a directory.'
}
$destinationParentInput = Split-Path -Parent $destinationInput
if (-not (Test-Path -LiteralPath $destinationParentInput -PathType Container)) {
    throw 'RuntimeDirectory parent must already exist.'
}
$destinationParent = (Resolve-Path -LiteralPath $destinationParentInput).Path.TrimEnd('\')
$destination = Join-Path $destinationParent (Split-Path -Leaf $destinationInput)
$destinationParentItem = Get-Item -LiteralPath $destinationParent -Force
if (($destinationParentItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
    throw 'RuntimeDirectory parent cannot be a reparse point.'
}
if ((Test-PathWithin $destination $source) -or (Test-PathWithin $source $destination)) {
    throw 'Source and runtime directory cannot contain one another.'
}

$sourceInventory = Get-PackageInventory $source
$pnpm = Get-Command pnpm -ErrorAction Stop
if ($pnpm.CommandType -notin @('Application', 'ExternalScript')) {
    throw 'A pnpm executable is required.'
}

New-Item -ItemType Directory -Path $destination | Out-Null
try {
    $sourcePrefix = $source.TrimEnd('\') + '\'
    foreach ($directory in Get-ChildItem -LiteralPath $source -Force -Directory -Recurse) {
        $relative = $directory.FullName.Substring($sourcePrefix.Length)
        $targetDirectory = Join-Path $destination $relative
        if (-not (Test-Path -LiteralPath $targetDirectory -PathType Container)) {
            New-Item -ItemType Directory -Path $targetDirectory | Out-Null
        }
    }
    foreach ($file in Get-ChildItem -LiteralPath $source -Force -File -Recurse) {
        $relative = $file.FullName.Substring($sourcePrefix.Length)
        $target = Join-Path $destination $relative
        Copy-Item -LiteralPath $file.FullName -Destination $target
    }

    $copiedInventory = Get-PackageInventory $destination
    if ($copiedInventory.ManifestHash -ne $sourceInventory.ManifestHash -or $copiedInventory.FileCount -ne $sourceInventory.FileCount) {
        throw 'Copied package verification does not match the verified source.'
    }

    $pluginDirectory = Join-Path $destination 'dsh-plugin'
    if (-not (Test-Path -LiteralPath (Join-Path $pluginDirectory 'package.json') -PathType Leaf) -or
        -not (Test-Path -LiteralPath (Join-Path $pluginDirectory 'pnpm-lock.yaml') -PathType Leaf)) {
        throw 'The copied package does not contain the DSH dependency manifest and lockfile.'
    }

    $tempRoot = [System.IO.Path]::GetFullPath($env:TEMP).TrimEnd('\')
    $configId = [guid]::NewGuid().ToString('N')
    $userConfig = Join-Path $tempRoot ("sic-pnpm-user-$configId.npmrc")
    $globalConfig = Join-Path $tempRoot ("sic-pnpm-global-$configId.npmrc")
    if ((Test-Path -LiteralPath $userConfig) -or (Test-Path -LiteralPath $globalConfig)) {
        throw 'Temporary package-manager config path already exists.'
    }
    [System.IO.File]::WriteAllText($userConfig, '', [System.Text.UTF8Encoding]::new($false))
    [System.IO.File]::WriteAllText($globalConfig, '', [System.Text.UTF8Encoding]::new($false))

    $savedEnvironment = @{}
    try {
        foreach ($entry in [Environment]::GetEnvironmentVariables('Process').GetEnumerator()) {
            $name = [string]$entry.Key
            if ($name -match '(?i)(KEY|TOKEN|SECRET|PASSWORD|CREDENTIAL|AUTH|PROXY|REGISTRY|NPM_CONFIG|PNPM_CONFIG)') {
                $savedEnvironment[$name] = [string]$entry.Value
                [Environment]::SetEnvironmentVariable($name, $null, 'Process')
            }
        }
        $env:NPM_CONFIG_USERCONFIG = $userConfig
        $env:NPM_CONFIG_GLOBALCONFIG = $globalConfig
        $env:NPM_CONFIG_REGISTRY = 'https://registry.npmjs.org/'
        $env:PNPM_CONFIG_USERCONFIG = $userConfig
        $env:PNPM_CONFIG_GLOBALCONFIG = $globalConfig
        $env:PNPM_CONFIG_REGISTRY = 'https://registry.npmjs.org/'

        Push-Location -LiteralPath $pluginDirectory
        try {
            & $pnpm.Source install --frozen-lockfile --prod --ignore-scripts --offline *> $null
            $installExit = $LASTEXITCODE
            $installMode = 'offline'
            if ($installExit -ne 0) {
                & $pnpm.Source install --frozen-lockfile --prod --ignore-scripts --registry 'https://registry.npmjs.org/' *> $null
                $installExit = $LASTEXITCODE
                $installMode = 'public-registry'
            }
            if ($installExit -ne 0) { throw "Pinned DSH dependencies could not be installed ($installMode attempt failed)." }
        }
        finally {
            Pop-Location
        }
    }
    finally {
        foreach ($name in @($savedEnvironment.Keys)) {
            [Environment]::SetEnvironmentVariable($name, $savedEnvironment[$name], 'Process')
        }
        foreach ($name in @('NPM_CONFIG_USERCONFIG', 'NPM_CONFIG_GLOBALCONFIG', 'NPM_CONFIG_REGISTRY', 'PNPM_CONFIG_USERCONFIG', 'PNPM_CONFIG_GLOBALCONFIG', 'PNPM_CONFIG_REGISTRY')) {
            if (-not $savedEnvironment.ContainsKey($name)) {
                [Environment]::SetEnvironmentVariable($name, $null, 'Process')
            }
        }
        foreach ($configPath in @($userConfig, $globalConfig)) {
            if (Test-Path -LiteralPath $configPath -PathType Leaf) {
                $resolvedConfig = (Resolve-Path -LiteralPath $configPath).Path
                if ((Test-PathWithin $resolvedConfig $tempRoot) -and (Split-Path -Leaf $resolvedConfig).StartsWith('sic-pnpm-', [StringComparison]::Ordinal)) {
                    Remove-Item -LiteralPath $resolvedConfig -Force
                }
            }
        }
    }

    $requiredModules = @(
        '@deepseek-ai/schemastery',
        '@deepseek-ai/dsh-llm',
        '@deepseek-ai/dsh-llm-pi-ai',
        '@deepseek-ai/dsh-session',
        '@earendil-works/pi-ai'
    )
    foreach ($moduleName in $requiredModules) {
        if (-not (Test-Path -LiteralPath (Join-Path $pluginDirectory ("node_modules\$moduleName\package.json")) -PathType Leaf)) {
            throw 'A required DSH plugin runtime dependency is missing after installation.'
        }
    }

    $runtimeInventory = Get-PackageInventory $destination -AllowInstalledDependencies
    $finalSourceInventory = Get-PackageInventory $source
    if ($finalSourceInventory.ManifestHash -ne $sourceInventory.ManifestHash -or $finalSourceInventory.FileCount -ne $sourceInventory.FileCount) {
        throw 'Source package changed during setup; the verified source was not kept immutable.'
    }
    if ($runtimeInventory.ManifestHash -ne $sourceInventory.ManifestHash -or $runtimeInventory.FileCount -ne $sourceInventory.FileCount) {
        throw 'Runtime package files differ from the verified package manifest.'
    }

    Write-Output "Runtime ready: $destination"
    Write-Output "Verified package files: $($sourceInventory.FileCount); DSH dependencies installed: $($requiredModules.Count); install mode: $installMode."
    Write-Output 'The source package was reverified unchanged. No process credential variables were passed to pnpm.'
}
catch {
    Write-Error $_.Exception.Message
    throw
}
