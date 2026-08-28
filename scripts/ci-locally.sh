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
    ( cd deskwarden && cargo test -j 8 2>&1 ) | tee /tmp/ci-local-tests.txt | grep -E "^test result|^error" || true
    echo
    echo "  failing tests, if any:"
    # `^test result:` is the SUMMARY line, not a test. Matching it made an
    # earlier version of this script report a module called "result".
    grep -E "^test [a-z]" /tmp/ci-local-tests.txt | grep -v "^test result:" | grep " FAILED" | sed 's/^/    /' | head -30
    # Only a compile error is an outright failure here. Test failures are
    # printed for a human, because on this machine they are noise more often
    # than they are signal.
    ! grep -qE "^error(\[|:)" /tmp/ci-local-tests.txt
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
