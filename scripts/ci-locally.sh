#!/usr/bin/env bash
#
# The three CI jobs, run on this machine.
#
# WHY THIS EXISTS, and it is not "CI is slow". This repository is PUBLIC and
# its workflow triggers on `pull_request`, so a self-hosted runner here would
# execute a stranger's fork code beside a Bitwarden vault, `userkey.bin`, a
# DPAPI-wrapped session token and the SSH key that pushes to this repo. GitHub
# says not to do it and they are right. This runs the same checks without
# putting this machine in reach of anyone who can open a pull request.
#
# **It is not a replacement for CI's judgement.** It runs the same commands on
# one machine, in one configuration, against a working tree rather than a
# clean checkout. What it cannot tell you is whether the checks pass on a
# machine that is not this one -- which is the thing CI was actually for. Say
# "the local checks pass", never "CI is green".
#
#   scripts/ci-locally.sh              # the whole set
#   scripts/ci-locally.sh --quick      # skips screenshots and the commit walk
#
set -uo pipefail

cd "$(dirname "$0")/.."
: "${CARGO_TARGET_DIR:=/e/_dw_agent/run}"
export CARGO_TARGET_DIR

QUICK=0
[ "${1:-}" = "--quick" ] && QUICK=1

pass=0
fail=0
skipped=()

step() {
    local label="$1"; shift
    printf '\n=== %s ===\n' "$label"
    if "$@"; then
        printf '  PASS  %s\n' "$label"
        pass=$((pass + 1))
    else
        printf '  FAIL  %s\n' "$label"
        fail=$((fail + 1))
    fi
}

# ---- job 1: Tests and warnings ---------------------------------------------
#
# CI runs `cargo test -j 8` with no filter, so the bin target's tests run too.
# The local suite is NOT trustworthy here: a different set of `mockito`-using
# modules fails on every run, cause unknown, and the count has swung between
# 32 and 75 on an unchanged tree. So this reports the failures rather than
# judging them, and the caller has to look.
run_tests() {
    # **Both suites, reported separately.** `cargo test` runs the lib and the
    # bin target and prints a `test result:` line for each. An earlier version
    # of this printed only the first, so a bin-target failure was invisible --
    # and one got through to CI that way, which is the whole thing this script
    # exists to prevent.
    # **`--no-fail-fast`, and this flag is the whole point of the fix.**
    # `cargo test` stops after a target fails, and the library target ALWAYS
    # fails on this machine because of the mockito noise -- so without it the
    # bin target never runs here at all, and a bin failure is invisible
    # locally no matter how carefully the output is read. That is exactly how
    # a broken source guard reached CI. CI does not need the flag: its lib run
    # is clean, so it reaches the bin target on its own.
    ( cd deskwarden && cargo test -j 8 --no-fail-fast 2>&1 ) | tee /tmp/ci-local-tests.txt >/dev/null
    echo '  suite results (one line per target):'
    grep -E '^test result' /tmp/ci-local-tests.txt | sed 's/^/    /'
    echo
    echo '  failing tests:'
    grep -E '^test [a-z]' /tmp/ci-local-tests.txt | grep -v '^test result:' | grep ' FAILED' \
        | sed 's/^test /    /;s/ \.\.\..*//' | head -40
    local n
    n=$(grep -E '^test [a-z]' /tmp/ci-local-tests.txt | grep -v '^test result:' | grep -c ' FAILED')
    echo "    ($n failing)"
    echo
    # The local suite is NOT trustworthy: a different set of mockito-using
    # modules fails on every run, and the count has swung between 32 and 85 on
    # an unchanged tree. So the SIGNAL is a failure in something that does not
    # use mockito -- those are the ones worth chasing.
    echo '  failures in modules that do not use mockito (these are the real ones):'
    grep -E '^test [a-z]' /tmp/ci-local-tests.txt | grep -v '^test result:' | grep ' FAILED' \
        | sed 's/^test //;s/ \.\.\..*//' | cut -d: -f1 | sort -u | while read -r m; do
        f="deskwarden/src/${m//:://}.rs"
        [ -f "$f" ] || f="deskwarden/src/${m//:://}/mod.rs"
        # A name that is not a module at all (main.rs's own test mods) has no
        # file, and is reported rather than silently excused.
        if [ ! -f "$f" ]; then echo "    $m  (in main.rs or a test-only module)"
        elif ! grep -qi mockito "$f"; then echo "    $m"
        fi
    done
    ! grep -qE '^error(\[|:)' /tmp/ci-local-tests.txt
}

# The one that IS decisive, and the one CI actually gates on.
deny_warnings() {
    ( cd deskwarden && RUSTFLAGS="-D warnings" cargo build --all-targets -j 8 2>&1 ) \
        | grep -E "^(error|warning)" | head -20
    ( cd deskwarden && RUSTFLAGS="-D warnings" cargo build --all-targets -j 8 >/dev/null 2>&1 )
}

audit() {
    ( cd deskwarden && cargo deny check licenses advisories 2>&1 | tail -5 )
    ( cd deskwarden && cargo deny check licenses advisories >/dev/null 2>&1 )
}

# ---- job 2: Screenshots of every surface -----------------------------------
#
# CI installs Mesa3D llvmpipe for a software GL. This machine has a real GPU,
# so it renders on that instead -- which means a surface that only breaks
# under llvmpipe will pass here and fail there. Named, not hidden.
screenshots() {
    ( cd deskwarden && cargo run --example ui_preview -- --all 2>&1 | tail -3 )
    ls -1 "$CARGO_TARGET_DIR/ui_preview" 2>/dev/null | wc -l | sed 's/^/  surfaces rendered: /'
    [ -d "$CARGO_TARGET_DIR/ui_preview" ]
}

# ---- job 3: Every new commit compiles --------------------------------------
#
# The range CI walks is the pushed range; locally the useful equivalent is
# what this branch adds to main. Each commit is checked out into a detached
# worktree rather than by moving HEAD, so an interrupted run cannot leave the
# working tree on some ancestor commit.
each_commit_compiles() {
    local range="main..HEAD" bad=0 tmp
    tmp="$(mktemp -d)"
    for sha in $(git rev-list --reverse "$range"); do
        git worktree add --detach -q "$tmp/wt" "$sha" 2>/dev/null || { echo "  could not check out $sha"; bad=1; continue; }
        if ( cd "$tmp/wt/deskwarden" && cargo check --all-targets -j 8 >/dev/null 2>&1 ); then
            echo "  ok    $(git log -1 --format='%h %s' "$sha")"
        else
            echo "  BROKE $(git log -1 --format='%h %s' "$sha")"
            bad=1
        fi
        git worktree remove --force "$tmp/wt" 2>/dev/null
    done
    rm -rf "$tmp"
    [ "$bad" = 0 ]
}

step "Tests and warnings: the suite" run_tests
step "Tests and warnings: -D warnings" deny_warnings
step "Tests and warnings: licences and advisories" audit
if [ "$QUICK" = 1 ]; then
    skipped+=("Screenshots of every surface" "Every new commit compiles")
else
    step "Screenshots of every surface" screenshots
    step "Every new commit compiles" each_commit_compiles
fi

printf '\n========================================\n'
printf '  passed: %d   failed: %d\n' "$pass" "$fail"
for s in "${skipped[@]:-}"; do [ -n "$s" ] && printf '  skipped: %s\n' "$s"; done
printf '\n  These are the local checks, on one machine, against a working\n'
printf '  tree. They are not CI, and a green run here does not mean a green\n'
printf '  run there -- the screenshot job in particular renders on this\n'
printf '  GPU rather than the software llvmpipe CI installs.\n'
printf '========================================\n'
[ "$fail" = 0 ]
