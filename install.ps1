# Yopes installer: fetches the latest GitHub release for Windows, verifies the
# SHA256SUMS checksum, installs yopes.exe into a user directory, and puts it on
# the user PATH. Works on Windows PowerShell 5.1 and PowerShell 7+.
#
# Remote one-liner (defaults, no parameters):
#   irm https://raw.githubusercontent.com/Jamicow6/YopesCLI/main/install.ps1 | iex
#
# Or download first to pass parameters:
#   .\install.ps1 [-Destination <dir>] [-NoPath]

[CmdletBinding()]
param(
    # Install directory. The updater keeps updating in place here.
    [string]$Destination = (Join-Path $env:LOCALAPPDATA 'Programs\yopes'),
    # Place the install directory on the user PATH (direct runs only).
    [switch]$NoPath
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$Repo = 'Jamicow6/YopesCLI'
$Asset = 'yopes-Windows-X64.zip'

# Windows PowerShell 5.1 defaults to TLS 1.0 for .NET clients; force 1.2+.
if ($PSVersionTable.PSVersion.Major -lt 6) {
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12 -bor [Net.SecurityProtocolType]::Tls13
}

if ($IsWindows -eq $false) {
    throw 'install.ps1 is for Windows. Download the matching archive from https://github.com/Jamicow6/YopesCLI/releases'
}

# An already-installed yopes updates itself in place; replacing a different
# install would leave two copies shadowing each other on PATH.
$Existing = Get-Command yopes -ErrorAction SilentlyContinue
if ($Existing -and ((Resolve-Path $Existing.Path).Path -notlike "$Destination\*")) {
    Write-Host 'Already installed:' $Existing.Path
    Write-Host 'Run `yopes update` to update that install.'
    return
}
if (Get-Process yopes -ErrorAction SilentlyContinue) {
    throw 'yopes is running. Close it, then run the installer again.'
}

Write-Host 'Looking up the latest release...' -NoNewline
$Headers = @{ Accept = 'application/vnd.github+json'; 'X-GitHub-Api-Version' = '2022-11-28' }
$TokenEnv = $env:GITHUB_TOKEN, $env:GH_TOKEN | Where-Object { $_ } | Select-Object -First 1
if ($TokenEnv) { $Headers.Authorization = "Bearer $TokenEnv" }
$Release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" -Headers $Headers
Write-Host " $($Release.tag_name)"

$AssetUrl = $Release.assets | Where-Object name -eq $Asset | Select-Object -ExpandProperty browser_download_url
$ChecksumsUrl = $Release.assets | Where-Object name -eq 'SHA256SUMS' | Select-Object -ExpandProperty browser_download_url
if (-not $AssetUrl -or -not $ChecksumsUrl) {
    throw "Release $($Release.tag_name) has no $Asset and/or SHA256SUMS."
}

$Temporary = New-Item -ItemType Directory -Path (Join-Path ([IO.Path]::GetTempPath()) "yopes-install-$(Get-Random)")
try {
    Write-Host "Downloading $Asset..." -NoNewline
    $Zip = Join-Path $Temporary $Asset
    Invoke-WebRequest -Uri $AssetUrl -OutFile $Zip
    $ChecksumsText = Invoke-WebRequest -Uri $ChecksumsUrl -UseBasicParsing | Select-Object -ExpandProperty Content
    Write-Host " done"

    # Keep this: an unverified binary is the one thing we must never install.
    $Expected = ($ChecksumsText -split "`r?`n" | Where-Object {
        $_ -match '^([0-9A-Fa-f]{64})\s+\*?' + [regex]::Escape($Asset) + '$'
    } | ForEach-Object { ($_ -split '\s+')[0] })
    if (-not $Expected) { throw "SHA256SUMS has no line for $Asset." }
    $Actual = (Get-FileHash -Algorithm SHA256 $Zip).Hash
    if ($Actual -ne $Expected) { throw "Checksum mismatch for $Asset ($Actual, expected $Expected)." }
    Write-Host 'Checksum verified.' -ForegroundColor Green

    Write-Host "Installing to $Destination..." -NoNewline
    $Extracted = Join-Path $Temporary 'extracted'
    Expand-Archive -Path $Zip -DestinationPath $Extracted
    $NewBinary = Join-Path $Extracted 'yopes.exe'
    if (-not (Test-Path $NewBinary)) { throw "The downloaded archive does not contain yopes.exe." }
    New-Item -ItemType Directory -Path $Destination -Force | Out-Null
    Copy-Item $NewBinary (Join-Path $Destination 'yopes.exe') -Force
    Write-Host " done"
}
finally {
    Remove-Item $Temporary -Recurse -Force -ErrorAction SilentlyContinue
}

if (-not $NoPath) {
    $UserPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if ($UserPath -notlike "*$Destination*") {
        [Environment]::SetEnvironmentVariable(
            'Path', ($UserPath.TrimEnd(';') + ';' + $Destination), 'User')
        Write-Host "Added $Destination to your user PATH." -ForegroundColor Cyan
        Write-Host 'Open a new terminal, then run `yopes`.'
    }
}
Write-Host "Installed yopes $($Release.tag_name)." -ForegroundColor Green
