<#
.SYNOPSIS
    Measures the deskwarden preflight mutations: applies each one to a
    throwaway copy of the tree, runs the suite, and reports how many tests go
    red and which ones.

.DESCRIPTION
    A developer tool, run by hand. It is deliberately NOT wired into
    `cargo test` -- it builds the crate once per mutant and takes minutes.

    Each case under `cases/` is an anchored source replacement, not a patch:
      target.txt   relative path of the file to mutate
      find.txt     the exact text to replace (must occur EXACTLY ONCE)
      replace.txt  what to put there
      about.md     the escape it stands for, and why it is spelled this way

    find/replace files are compared with newlines normalised to LF and one
    trailing newline stripped, so a CRLF checkout (which this repo pins for
    *.rs) and an LF one measure the same thing. The mutated file is written
    back with the line endings it was read with.

    Nothing is written to the working tree: every case gets a fresh detached
    `git worktree` under the system temp directory, and the whole run shares
    one fresh CARGO_TARGET_DIR, also outside the repository. Both are removed
    afterwards, including on failure.

.EXAMPLE
    pwsh -File mutations/run.ps1
    pwsh -File mutations/run.ps1 -Case 02-gate-neutralised
#>
[CmdletBinding()]
param(
    # Which cases to run. Default: all of them, in name order.
    [string[]] $Case,
    # The commit to measure. Default: whatever the repository's HEAD is.
    [string] $Commit
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$repo = (git -C $here rev-parse --show-toplevel).Trim()
if (-not $Commit) { $Commit = (git -C $repo rev-parse HEAD).Trim() }
$commitShort = (git -C $repo rev-parse --short $Commit).Trim()

$caseDirs = Get-ChildItem -Path (Join-Path $here 'cases') -Directory | Sort-Object Name
if ($Case) { $caseDirs = $caseDirs | Where-Object { $Case -contains $_.Name } }
if (-not $caseDirs) { throw "no cases matched" }

function Read-Snippet([string] $path) {
    $text = [IO.File]::ReadAllText($path) -replace "`r`n", "`n"
    if ($text.EndsWith("`n")) { $text = $text.Substring(0, $text.Length - 1) }
    return $text
}

# Applies one anchored replacement, and refuses to guess. A mutation that
# silently fails to apply would report 0 red and read as a catastrophically
# weakened gate, so an anchor that is missing -- or ambiguous -- is a hard
# error, not a warning.
function Invoke-Mutation([string] $file, [string] $find, [string] $replace) {
    $raw = [IO.File]::ReadAllText($file)
    $crlf = $raw.Contains("`r`n")
    $text = $raw -replace "`r`n", "`n"
    $count = ([regex]::Matches($text, [regex]::Escape($find))).Count
    if ($count -ne 1) {
        throw "anchor occurs $count times in $file (expected exactly 1); the case's find.txt has drifted from the source"
    }
    $text = $text.Replace($find, $replace)
    if ($crlf) { $text = $text -replace "`n", "`r`n" }
    [IO.File]::WriteAllText($file, $text, (New-Object Text.UTF8Encoding $false))
}

$stamp = [Guid]::NewGuid().ToString('N').Substring(0, 8)
$scratch = Join-Path ([IO.Path]::GetTempPath()) "deskwarden-mutations-$stamp"
$targetDir = Join-Path $scratch 'target'
New-Item -ItemType Directory -Path $targetDir -Force | Out-Null
$worktrees = New-Object Collections.Generic.List[string]
$results = New-Object Collections.Generic.List[object]

try {
    foreach ($c in $caseDirs) {
        Write-Host ""
        Write-Host "=== $($c.Name) ===" -ForegroundColor Cyan

        $wt = Join-Path $scratch $c.Name
        git -C $repo worktree add --detach --quiet $wt $Commit
        $worktrees.Add($wt)

        $rel = (Read-Snippet (Join-Path $c.FullName 'target.txt')).Trim()
        Invoke-Mutation (Join-Path $wt $rel) `
            (Read-Snippet (Join-Path $c.FullName 'find.txt')) `
            (Read-Snippet (Join-Path $c.FullName 'replace.txt'))
        Write-Host "applied to $rel"

        $env:CARGO_TARGET_DIR = $targetDir
        Push-Location $wt
        try {
            # `--no-fail-fast` is not optional here. Without it cargo stops
            # after the lib target goes red and never builds or runs the bin
            # target, so a mutant that also kills a bin test would be
            # under-counted -- silently, and only for the mutants that happen
            # to kill a lib test first.
            $out = & cargo test --no-fail-fast --manifest-path deskwarden/Cargo.toml 2>&1 |
                ForEach-Object { "$_" }
            $code = $LASTEXITCODE
        } finally { Pop-Location; Remove-Item Env:CARGO_TARGET_DIR }

        $joined = $out -join "`n"
        # A mutant that compiled always prints at least one `test result:`
        # line. Matching on `^error` would be wrong: `error: test failed` is
        # cargo reporting a RED SUITE, which is the measurement, not a
        # build failure.
        $compiled = [bool]($joined -match '(?m)^test result:')
        $failed = @($out |
            Where-Object { $_ -match '^test (\S+) \.\.\. FAILED' } |
            ForEach-Object { [regex]::Match($_, '^test (\S+) \.\.\. FAILED').Groups[1].Value } |
            Sort-Object -Unique)

        if (-not $compiled) {
            Write-Host "DID NOT COMPILE" -ForegroundColor Red
            $joined -split "`n" | Where-Object { $_ -match '^error' } | Select-Object -First 5 |
                ForEach-Object { Write-Host "  $_" }
            $results.Add([pscustomobject]@{ Case = $c.Name; Red = $null; Killers = @(); Status = 'BUILD ERROR' })
        } else {
            $status = if ($failed.Count -gt 0) { 'killed' } elseif ($code -eq 0) { 'SURVIVED' } else { 'no named failures, non-zero exit' }
            Write-Host "$($failed.Count) red -- $status"
            $failed | ForEach-Object { Write-Host "  $_" }
            $out | Where-Object { $_ -match '^test result:' } | ForEach-Object { Write-Host "  $_" }
            $results.Add([pscustomobject]@{ Case = $c.Name; Red = $failed.Count; Killers = $failed; Status = $status })
        }
    }
} finally {
    foreach ($wt in $worktrees) {
        git -C $repo worktree remove --force $wt 2>&1 | Out-Null
    }
    git -C $repo worktree prune 2>&1 | Out-Null
    if (Test-Path $scratch) { Remove-Item -Recurse -Force $scratch -ErrorAction SilentlyContinue }
}

Write-Host ""
Write-Host "deskwarden preflight mutations -- $commitShort -- $(Get-Date -Format 'yyyy-MM-dd')"
Write-Host ""
'{0,-26} {1,5}  {2}' -f 'case', 'red', 'status' | Write-Host
foreach ($r in $results) {
    $red = if ($null -eq $r.Red) { '--' } else { "$($r.Red)" }
    '{0,-26} {1,5}  {2}' -f $r.Case, $red, $r.Status | Write-Host
}
Write-Host ""
Write-Host "killing tests"
foreach ($r in $results) {
    Write-Host "  $($r.Case)"
    if ($r.Killers.Count -eq 0) { Write-Host "    (none)" }
    foreach ($k in $r.Killers) { Write-Host "    $k" }
}

if ($results | Where-Object { $_.Status -eq 'BUILD ERROR' -or $_.Status -eq 'SURVIVED' }) { exit 1 }
exit 0
