param(
    [string]$Endpoint = $env:SIC_COMPRESSER_URL,
    [string]$Model = $env:SIC_COMPRESSER_MODEL,
    [string]$ApiKeyEnvironment = "SIC_EXPERIMENT_API_KEY",
    [int]$SicPort = 8787,
    [int]$BridgePort = 8788,
    [int]$SealBytes = 65536
)
$ErrorActionPreference = "Stop"
$project = Split-Path -Parent $PSScriptRoot
Set-Location $project
if (-not $Endpoint -or -not $Model) { throw "Set SIC_COMPRESSER_URL and SIC_COMPRESSER_MODEL, or pass -Endpoint and -Model." }
$providerUri = [Uri]$Endpoint
if ($providerUri.Scheme -notin @("http", "https") -or $providerUri.Port -eq 8341 -or
    $providerUri.UserInfo -or $providerUri.Query -or $providerUri.Fragment -or
    -not $Endpoint.EndsWith("/chat/completions")) {
    throw "Use a stable dedicated Chat Completions endpoint; non-fixed test route 8341 and URL credentials are forbidden."
}
$providerKey = [Environment]::GetEnvironmentVariable($ApiKeyEnvironment)
if (-not $providerKey) { throw "Set the API key in environment variable $ApiKeyEnvironment." }
$executable = @("bin/sic-proxy.exe", "target/release/sic-proxy.exe", "target/debug/sic-proxy.exe") |
    ForEach-Object { Join-Path $project $_ } | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
if (-not $executable) { throw "Build first: cargo build --release" }
foreach ($port in @($SicPort, $BridgePort)) {
    if (Get-NetTCPConnection -State Listen -LocalPort $port -ErrorAction SilentlyContinue) {
        throw "Port $port is occupied. Select other -SicPort and -BridgePort values."
    }
}
if (-not $env:SIC_ADMIN_TOKEN) { $env:SIC_ADMIN_TOKEN = [Guid]::NewGuid().ToString("N") + [Guid]::NewGuid().ToString("N") }
$runtime = Join-Path $project "runtime"
New-Item -ItemType Directory -Force -Path $runtime | Out-Null
$env:SIC_BIND = "127.0.0.1:$SicPort"
$env:SIC_DB = "sqlite:///" + ((Join-Path $runtime "dsh-demo.db").Replace('\','/')) + "?mode=rwc"
$env:SIC_COMPRESSER_URL = $Endpoint
$env:SIC_COMPRESSER_MODEL = $Model
$env:SIC_COMPRESSER_API_KEY = $providerKey
$env:SIC_WORKER_UPSTREAM_BASE_URL = $Endpoint.Substring(0, $Endpoint.Length - "/chat/completions".Length)
$env:SIC_WORKER_UPSTREAM_API_KEY = $providerKey
$service = Start-Process -FilePath $executable -WorkingDirectory $project -WindowStyle Hidden -PassThru `
    -RedirectStandardOutput (Join-Path $runtime "sic-stdout.log") -RedirectStandardError (Join-Path $runtime "sic-stderr.log")
try {
    $ready = $false
    for ($attempt = 0; $attempt -lt 50; $attempt++) {
        if ($service.HasExited) { throw "SIC exited; inspect runtime/sic-stderr.log." }
        try {
            $null = Invoke-RestMethod -Uri "http://127.0.0.1:$SicPort/api/v1/ui/bootstrap" -TimeoutSec 1
            $ready = $true
            break
        } catch { Start-Sleep -Milliseconds 100 }
    }
    if (-not $ready) { throw "SIC startup timed out." }
    Write-Host "DSH provider base URL: http://127.0.0.1:$BridgePort/v1"
    Write-Host "DSH model: $Model ; custom header: x-sic-bridge-session = a unique conversation key"
    Write-Host "Keep sending full history. Ctrl+C stops this launcher. Responses are buffered before SSE delivery."
    python scripts/dsh_chat_bridge.py --sic-url "http://127.0.0.1:$SicPort" --port $BridgePort --seal-bytes $SealBytes
    if ($LASTEXITCODE -ne 0) { throw "Chat bridge exited (exit $LASTEXITCODE)." }
} finally {
    if (-not $service.HasExited) { Stop-Process -Id $service.Id -ErrorAction SilentlyContinue }
}
