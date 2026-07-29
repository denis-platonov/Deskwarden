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

# Organization (O=) values accepted as proof that a downloaded bw.exe really
# was signed by Bitwarden.
#
# Matched as whole, exact DN *components* (see Get-CertificateDnComponent
# below), never as substrings of the subject string. The check this replaced
# was `$sig.SignerCertificate.Subject -match 'Bitwarden'`: an unanchored,
# case-insensitive regex over the entire subject DN, which would have accepted
# any validly-signed binary whose subject merely contained the word somewhere
# -- `O=Bitwarden Solutions LLC`, `CN=Not Bitwarden`, `OU=bitwarden-integration`
# -- from an unrelated but legitimately-issued certificate.
#
# Several spellings are listed because Bitwarden Inc. was formerly 8bit
# Solutions LLC and DN punctuation ("Bitwarden Inc." vs "Bitwarden, Inc.")
# varies between issuances; each entry is still an exact whole-component
# match, so the list widens what is accepted only by these named
# organizations, not by anything that happens to contain the string.
#
# TODO (verify before shipping): confirm this list against a real
# Bitwarden-signed bw.exe -- download a current bw-windows-*.zip release,
# extract bw.exe, and run
#   (Get-AuthenticodeSignature bw.exe).SignerCertificate.SubjectName.Format($true)
# then make sure its O= value appears verbatim below (and drop the entries
# that don't apply). This is the same verify-against-reality step the CLI
# download URL and asset-naming pattern got on 2026-07-28 (see the .DESCRIPTION
# block above); it could not be repeated for the certificate here because no
# bw.exe was available on the machine this was written on and downloading one
# was out of scope. Failure mode if the list is wrong is fail-closed and
# recoverable: bootstrap exits 2, and the installer tells the user to install
# the CLI themselves.
#
# Deliberately not a thumbprint pin (unlike updater.rs's pin on deskwarden's
# own signer): that pin is appropriate for a binary verifying its own future
# builds, whereas this is a third party whose signing certificate may
# legitimately rotate without warning. Pinning the organization is the
# strongest check that survives certificate rotation.
$BitwardenSignerOrganizations = @(
    'Bitwarden Inc.',
    'Bitwarden, Inc.',
    'Bitwarden Inc',
    'Bitwarden',
    '8bit Solutions LLC'
)

<#
.SYNOPSIS
    Returns the values of one component (e.g. 'O', 'CN') of a certificate's
    subject DN.
.DESCRIPTION
    Uses X500DistinguishedName.Format($true), which emits one RDN per line
    with proper DN parsing, rather than splitting the subject string on
    commas -- a value like `O="Bitwarden, Inc."` contains a comma of its own
    and a naive split would tear it in half. Surrounding quotes (which Format
    keeps for values that need them) are stripped so callers compare against
    plain values.
#>
function Get-CertificateDnComponent {
    param(
        [Parameter(Mandatory = $true)]
        [System.Security.Cryptography.X509Certificates.X509Certificate2]$Certificate,
        [Parameter(Mandatory = $true)]
        [string]$Key
    )

    $values = @()
    foreach ($line in ($Certificate.SubjectName.Format($true) -split "`r?`n")) {
        $line = $line.Trim()
        if ($line.Length -eq 0) { continue }
        $separator = $line.IndexOf('=')
        if ($separator -lt 1) { continue }
        if ($line.Substring(0, $separator).Trim() -ieq $Key) {
            $values += $line.Substring($separator + 1).Trim().Trim('"')
        }
    }
    return $values
}

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
    # third-party binary instead of deskwarden's own installer/updater. Two
    # independent conditions, both required: the signature itself is valid
    # and chains to a trusted root ($sig.Status), and the signer's subject DN
    # names Bitwarden in its organization component (see
    # $BitwardenSignerOrganizations above for why that is an exact
    # whole-component match rather than a substring search, and why it is not
    # a thumbprint pin).
    $sig = Get-AuthenticodeSignature -FilePath $bwExe.FullName
    $signerOrgs = @()
    if ($sig.SignerCertificate) {
        $signerOrgs = Get-CertificateDnComponent -Certificate $sig.SignerCertificate -Key 'O'
    }
    $signerOk = @($signerOrgs | Where-Object { $BitwardenSignerOrganizations -contains $_ }).Count -gt 0
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
