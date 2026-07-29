<#
.SYNOPSIS
    Installer-time helper: ensures the Bitwarden CLI (bw.exe) is available for
    deskwarden, downloading and installing it from Bitwarden's own official
    GitHub releases if it isn't already present.

.DESCRIPTION
    Invoked by deskwarden.iss (Inno Setup) during ssPostInstall via
    `Exec('powershell.exe', ...)`. Kept as a standalone .ps1 (extracted at
    install time via Inno's `Flags: dontcopy` + `ExtractTemporaryFile`)
    rather than embedded as an escaped Pascal string, so the logic here is
    ordinary, reviewable PowerShell instead of string-escaped Pascal.

    bitwarden/clients is a monorepo that publishes releases for several
    products (cli, desktop, browser, web) interleaved by date. GitHub's
    generic "latest release" for the repo is therefore NOT reliably the
    CLI's latest -- it can be whichever product last shipped. This script
    filters explicitly for the "cli-v*" tag prefix, excludes prereleases and
    drafts (so an RC tagged "cli-v*" ahead of its stable promotion is never
    picked up), and takes the newest remaining match, then picks the
    "bw-windows-<version>.zip" asset (the official
    standalone Windows CLI build; NOT the "bw-oss-windows-*" build, which is
    open-source-only and lacks paid-tier features some vault users rely on).

    Verified against the real bitwarden/clients releases API on 2026-07-28:
    latest CLI tag at that time was `cli-v2026.7.0`, with an asset literally
    named `bw-windows-2026.7.0.zip` at
    https://github.com/bitwarden/clients/releases/download/cli-v2026.7.0/bw-windows-2026.7.0.zip
    -- confirming both the repo and the "bw-windows-<version>.zip" naming
    pattern this script depends on.

.PARAMETER InstallDir
    deskwarden's own install directory (Inno Setup's {app}). bw.exe is
    placed in "<InstallDir>\bin\bw.exe", and "<InstallDir>\bin" is added to
    the current user's PATH so deskwarden's `Command::new("bw")` calls
    (see src/bw_serve.rs, src/login_ui.rs -- both invoke it as a bare `bw`,
    relying entirely on PATH) can find it.

.EXITCODE 0
    bw is available (either it was already installed, or it was just
    downloaded, verified, and installed successfully).
.EXITCODE 1
    Could not determine the latest CLI release or download its asset
    (network failure, unexpected API/release shape, etc).
.EXITCODE 2
    The downloaded file failed Authenticode signature verification. Refused
    to install or run it.
.EXITCODE 3
    Unexpected error not covered above.
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$InstallDir
)

$ErrorActionPreference = 'Stop'
# Invoke-WebRequest renders a progress bar per byte by default, which is
# dramatically slower over a remote/non-interactive host; this script never
# shows it to the user anyway (Inno runs it with SW_HIDE).
$ProgressPreference = 'SilentlyContinue'

# Defensive: some Windows Powershell 5.1 configurations still default
# ServicePointManager to pre-TLS1.2 protocols, which api.github.com and
# github.com's asset CDN both reject.
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

$UserAgent = 'deskwarden-installer'

function Test-BwAlreadyAvailable {
    param([string]$BinDir)
    if (Get-Command bw.exe -ErrorAction SilentlyContinue) { return $true }
    if (Test-Path (Join-Path $BinDir 'bw.exe')) { return $true }
    return $false
}

$binDir = Join-Path $InstallDir 'bin'

try {
    if (Test-BwAlreadyAvailable -BinDir $binDir) {
        Write-Output 'bw.exe already available (PATH or existing deskwarden install); skipping download.'
        exit 0
    }

    New-Item -ItemType Directory -Path $binDir -Force | Out-Null

    $releases = Invoke-RestMethod -Uri 'https://api.github.com/repos/bitwarden/clients/releases?per_page=50' -Headers @{ 'User-Agent' = $UserAgent }
    $cliRelease = $releases | Where-Object { $_.tag_name -like 'cli-v*' -and -not $_.prerelease -and -not $_.draft } | Select-Object -First 1
    if (-not $cliRelease) {
        Write-Error 'Could not find a stable CLI release (tag matching cli-v*, not a prerelease/draft) among bitwarden/clients releases.'
        exit 1
    }

    $asset = $cliRelease.assets | Where-Object { $_.name -like 'bw-windows-*.zip' } | Select-Object -First 1
    if (-not $asset) {
        Write-Error "Release $($cliRelease.tag_name) has no bw-windows-*.zip asset."
        exit 1
    }

    $zipPath = Join-Path $env:TEMP 'deskwarden-bw-cli-download.zip'
    Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $zipPath -Headers @{ 'User-Agent' = $UserAgent }

    $extractDir = Join-Path $env:TEMP 'deskwarden-bw-cli-extract'
    if (Test-Path $extractDir) { Remove-Item $extractDir -Recurse -Force }
    Expand-Archive -Path $zipPath -DestinationPath $extractDir -Force

    $bwExe = Get-ChildItem -Path $extractDir -Filter 'bw.exe' -Recurse | Select-Object -First 1
    if (-not $bwExe) {
        Write-Error "bw.exe not found inside the downloaded archive ($($asset.name))."
        exit 1
    }

    # Verify Bitwarden's own Authenticode signature before installing/running
    # anything extracted from the archive -- mirrors this project's own
    # signature.rs (Get-AuthenticodeSignature), applied here to a
    # third-party binary instead of deskwarden's own installer/updater. We
    # check validity plus that the signer is actually Bitwarden, but
    # deliberately don't pin one exact certificate thumbprint the way
    # updater.rs pins deskwarden's own self-update signer -- that pin is
    # appropriate for a binary verifying *itself* build-over-build; here
    # we're trusting a third party whose signing certificate may
    # legitimately rotate, and the design spec's Installer section only
    # requires a valid, genuinely-Bitwarden-signed binary, not thumbprint
    # pinning.
    $sig = Get-AuthenticodeSignature -FilePath $bwExe.FullName
    $signerOk = $sig.SignerCertificate -and ($sig.SignerCertificate.Subject -match 'Bitwarden')
    if ($sig.Status -ne 'Valid' -or -not $signerOk) {
        $subject = if ($sig.SignerCertificate) { $sig.SignerCertificate.Subject } else { '<none>' }
        Write-Error "Downloaded bw.exe failed signature verification (Status=$($sig.Status), Subject=$subject). Refusing to install it."
        Remove-Item $bwExe.FullName -ErrorAction SilentlyContinue
        exit 2
    }

    Copy-Item -Path $bwExe.FullName -Destination (Join-Path $binDir 'bw.exe') -Force

    # Add <InstallDir>\bin to the current user's PATH (HKCU, no admin
    # needed) so `bw` resolves for deskwarden's bare `Command::new("bw")`
    # calls. [Environment]::SetEnvironmentVariable(..., 'User') both writes
    # HKCU\Environment and broadcasts WM_SETTINGCHANGE, so already-running
    # Explorer picks up the change without a logoff/logon.
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if ($null -eq $userPath) { $userPath = '' }
    $pathEntries = $userPath -split ';' | Where-Object { $_.Length -gt 0 }
    $alreadyOnPath = $pathEntries -contains $binDir
    if (-not $alreadyOnPath) {
        $newPath = if ($userPath.Trim().Length -eq 0) { $binDir } else { "$userPath;$binDir" }
        [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
    }

    Write-Output "bw.exe installed to $binDir and added to PATH."
    exit 0
} catch {
    Write-Error $_.Exception.Message
    exit 3
}
