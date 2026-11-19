<#
.SYNOPSIS
  Builds the Microsoft Store package (.msix) for Deskwarden.

.DESCRIPTION
  Nothing here signs in anywhere, uploads anything or downloads anything. It
  needs, on this machine: Rust (cargo), Python 3, and the Windows SDK's
  makeappx.exe and makepri.exe (C:\Program Files (x86)\Windows Kits\10\bin\<ver>\x64).

  Steps: cargo build --release --features store -> generate the logo set
  (scripts/store/make-assets.py) -> stage Deskwarden.exe + wordlists + assets +
  AppxManifest.xml -> makepri (resources.pri, if present) -> makeappx pack.

  Identity (-PackageName, -Publisher, -PublisherDisplayName) comes from Partner
  Center -> Product identity, AFTER the owner reserves the name; see
  docs/store/CHECKLIST.md. Without them the script refuses -- unless -CiBuild,
  which stamps a placeholder identity and puts UNSUBMITTABLE in the file name,
  so the release workflow can prove the package builds without anyone able to
  upload it by accident.

  The Store signs the package itself on ingestion, so the file to upload is
  UNSIGNED. -SignPfx exists only to sideload-test the package on one's own PC
  (the publisher in the certificate must equal -Publisher).

.EXAMPLE
  scripts\store\build-msix.ps1 -PackageName 12345Owner.Deskwarden `
    -Publisher 'CN=ABCDEF01-2345-6789-ABCD-EF0123456789' `
    -PublisherDisplayName 'Owner Name'

.EXAMPLE
  scripts\store\build-msix.ps1 -CiBuild
#>
[CmdletBinding()]
param(
  [string]$PackageName,
  [string]$Publisher,
  [string]$PublisherDisplayName,
  # Four-part A.B.C.D. Default: Cargo.toml's version, `-dev` dropped, `.0` added.
  [string]$Version,
  [string]$OutDir,
  # Use an already-built Deskwarden.exe (it MUST have been built with
  # `--features store`; the script cannot tell).
  [string]$ExePath,
  [switch]$CiBuild,
  # Sideload testing only: a .pfx whose subject equals -Publisher.
  [string]$SignPfx,
  [string]$SignPfxPassword
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$crate = Join-Path $repo 'deskwarden'
$template = Join-Path $PSScriptRoot 'AppxManifest.xml'
if (-not $OutDir) { $OutDir = Join-Path $repo 'store-package' }

# ---- identity --------------------------------------------------------------

if ($CiBuild) {
  if (-not $PackageName) { $PackageName = 'Deskwarden.CiBuild' }
  if (-not $Publisher) { $Publisher = 'CN=Deskwarden CI Unsubmittable' }
  if (-not $PublisherDisplayName) { $PublisherDisplayName = 'Deskwarden CI (not for the Store)' }
} else {
  foreach ($pair in @(@('PackageName', $PackageName), @('Publisher', $Publisher), @('PublisherDisplayName', $PublisherDisplayName))) {
    if (-not $pair[1]) {
      throw "-$($pair[0]) is required. Reserve the app name in Partner Center and copy Product identity (docs/store/CHECKLIST.md, step M3). For a build that only proves the package assembles, pass -CiBuild."
    }
  }
  if ($Publisher -notmatch '^CN=') { throw "-Publisher must be the Partner Center value, starting 'CN='." }
}

# ---- version ---------------------------------------------------------------

if (-not $Version) {
  $cargoToml = Get-Content -LiteralPath (Join-Path $crate 'Cargo.toml') -Raw
  if ($cargoToml -notmatch '(?m)^version\s*=\s*"(\d+)\.(\d+)\.(\d+)') { throw 'could not read the version from Cargo.toml' }
  $Version = "$($Matches[1]).$($Matches[2]).$($Matches[3]).0"
}
if ($Version -notmatch '^\d+\.\d+\.\d+\.0$') {
  throw "Version '$Version' must be four-part with a 4th part of 0: the Store reserves it."
}

# ---- tools -----------------------------------------------------------------

function Find-SdkTool([string]$name) {
  $kits = 'C:\Program Files (x86)\Windows Kits\10\bin'
  $found = Get-ChildItem -LiteralPath $kits -Directory -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -match '^10\.' } |
    Sort-Object { [version]$_.Name } -Descending |
    ForEach-Object { Join-Path $_.FullName "x64\$name" } |
    Where-Object { Test-Path -LiteralPath $_ } |
    Select-Object -First 1
  if (-not $found) {
    $onPath = Get-Command $name -ErrorAction SilentlyContinue
    if ($onPath) { $found = $onPath.Source }
  }
  $found
}
$makeappx = Find-SdkTool 'makeappx.exe'
if (-not $makeappx) { throw 'makeappx.exe not found. Install the Windows 10/11 SDK ("Windows SDK Signing Tools for Desktop Apps" is enough); this script does not download it.' }
$makepri = Find-SdkTool 'makepri.exe'
$signtool = if ($SignPfx) { Find-SdkTool 'signtool.exe' } else { $null }
if ($SignPfx -and -not $signtool) { throw 'signtool.exe not found (Windows SDK).' }
Write-Host "makeappx: $makeappx"
Write-Host "makepri:  $(if ($makepri) { $makepri } else { '(not found -- the package will carry no resources.pri; qualified logo files still resolve)' })"

# ---- the store-flavoured binary --------------------------------------------

if (-not $ExePath) {
  Push-Location $crate
  try {
    cargo build --release --features store
    if ($LASTEXITCODE -ne 0) { throw 'cargo build --release --features store failed' }
  } finally { Pop-Location }
  $targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $crate 'target' }
  $ExePath = Join-Path $targetDir 'release\Deskwarden.exe'
}
if (-not (Test-Path -LiteralPath $ExePath)) { throw "no executable at $ExePath" }

# ---- stage -----------------------------------------------------------------

$stage = Join-Path $OutDir 'stage'
if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
New-Item -ItemType Directory -Force -Path $stage | Out-Null

Copy-Item -LiteralPath $ExePath -Destination (Join-Path $stage 'Deskwarden.exe')
foreach ($f in 'wordlist.txt', 'eff_large_wordlist.txt') {
  Copy-Item -LiteralPath (Join-Path $crate "assets\$f") -Destination (Join-Path $stage $f)
}

$python = (Get-Command python -ErrorAction SilentlyContinue)
if (-not $python) { $python = Get-Command py -ErrorAction SilentlyContinue }
if (-not $python) { throw 'Python 3 is needed to generate the logo set (standard library only).' }
$assetsOut = Join-Path $OutDir 'generated-assets'
& $python.Source -I (Join-Path $PSScriptRoot 'make-assets.py') $assetsOut
if ($LASTEXITCODE -ne 0) { throw 'make-assets.py failed' }
Copy-Item -LiteralPath (Join-Path $assetsOut 'Assets') -Destination (Join-Path $stage 'Assets') -Recurse

$manifest = Get-Content -LiteralPath $template -Raw -Encoding utf8
$manifest = $manifest.Replace('@PACKAGE_NAME@', $PackageName).
  Replace('@PUBLISHER@', [System.Security.SecurityElement]::Escape($Publisher)).
  Replace('@PUBLISHER_DISPLAY_NAME@', [System.Security.SecurityElement]::Escape($PublisherDisplayName)).
  Replace('@VERSION@', $Version)
# The comment block at the top of the template names the tokens; strip it
# before checking that none is left in the real manifest body.
$body = [regex]::Replace($manifest, '(?s)<!--.*?-->', '')
if ($body -match '@[A-Z_]+@') { throw "an unreplaced token is left in the manifest: $($Matches[0])" }
[System.IO.File]::WriteAllText((Join-Path $stage 'AppxManifest.xml'), $body.TrimStart(), (New-Object System.Text.UTF8Encoding($false)))

if ($makepri) {
  $priConfig = Join-Path $OutDir 'priconfig.xml'
  & $makepri createconfig /cf $priConfig /dq en-US /o | Out-Null
  if ($LASTEXITCODE -ne 0) { throw 'makepri createconfig failed' }
  & $makepri new /pr $stage /cf $priConfig /mn (Join-Path $stage 'AppxManifest.xml') /of (Join-Path $stage 'resources.pri') /o | Out-Null
  if ($LASTEXITCODE -ne 0) { throw 'makepri new failed' }
}

# ---- pack ------------------------------------------------------------------

$suffix = if ($CiBuild) { '-UNSUBMITTABLE' } else { '' }
$msix = Join-Path $OutDir "Deskwarden_${Version}_x64$suffix.msix"
if (Test-Path -LiteralPath $msix) { Remove-Item -LiteralPath $msix -Force }
& $makeappx pack /d $stage /p $msix /o
if ($LASTEXITCODE -ne 0) { throw 'makeappx pack failed' }

if ($SignPfx) {
  $signArgs = @('sign', '/fd', 'SHA256', '/f', $SignPfx)
  if ($SignPfxPassword) { $signArgs += @('/p', $SignPfxPassword) }
  & $signtool @signArgs $msix
  if ($LASTEXITCODE -ne 0) { throw 'signtool sign failed' }
  Write-Host 'Signed with the test certificate: for sideloading on this PC only. Upload the UNSIGNED file.'
}

Write-Host ''
Write-Host "Package: $msix"
Write-Host "Listing logos for Partner Center: $(Join-Path $assetsOut 'listing')"
if ($CiBuild) { Write-Host 'CI build: placeholder identity, not uploadable.' }
