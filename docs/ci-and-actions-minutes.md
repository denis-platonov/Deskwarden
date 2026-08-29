# Where the 2,000 Actions minutes went, and what to change in CI

Written 2026-08-28, after an earlier answer in the same conversation blamed this
repository's four `windows-latest` jobs and was **wrong**. That answer is
retracted below, with the measurement that refutes it.

Throughout, **Measured** means a command was run and its output is reproduced or
summarised here. **Inferred** means a conclusion drawn from those measurements
that I could not confirm directly. **Unknown** means exactly that. Anything not
labelled `Measured` should not be repeated to anyone as fact.

---

## 1. What was measured

### 1.1 This repository is public, so its CI is free

```console
$ gh repo view denis-platonov/Deskwarden --json visibility,createdAt,isPrivate
{"createdAt":"2026-07-29T03:42:02Z","isPrivate":false,"visibility":"PUBLIC"}
```

**Measured.** `denis-platonov/Deskwarden` is public, and has 175 workflow runs
since 2026-08-01.

GitHub's documentation is unambiguous that this costs nothing:

> "GitHub Actions usage is free for self-hosted runners and for public
> repositories that use standard GitHub-hosted runners."
> — <https://docs.github.com/en/billing/concepts/product-billing/github-actions>

**So the earlier explanation — "four `windows-latest` jobs times the 2x Windows
multiplier" — cannot be the cause.** Those jobs are genuinely expensive in wall
time (about 18 job-minutes per run, all Windows), but on a public repository
GitHub bills none of it. The earlier answer applied a private-repository cost
model to a public repository.

### 1.2 Was Deskwarden ever private during August?

This was the one remaining way the earlier answer could have been salvaged, so
it was checked rather than assumed.

**Measured.** Making a repository public emits a `PublicEvent` on the owner's
event feed. `gh api users/denis-platonov/events --paginate` returns events back
to **2026-07-30** and contains **no `PublicEvent` for Deskwarden** (the three
`PublicEvent`s present are dated 2026-08-22, 2026-08-17 and 2026-07-30, none for
this repo).

**Inferred:** Deskwarden was public for the whole of August. **Caveat, and it is
a real one:** the event feed only retains ~90 days and its oldest entry
(2026-07-30) is *one day after* the repo was created (2026-07-29). A
public-ation on 2026-07-29 itself would fall outside the window. That gap is
before the August billing cycle either way, so it does not affect the
conclusion — but the evidence is "no event found", not "an event proves it".

### 1.3 The actual consumer: `denis-platonov/remux-toshiba` (private)

Every private repository was checked for runs in the current cycle:

| Repo | Visibility | Runs in Aug 2026 | Notes |
|---|---|---|---|
| `remux-toshiba` | private | **30** | all between 2026-08-22 and 2026-08-28 |
| `toshiba-remux` | private | 0 | no runs at all |
| `RegionConfiner` | private | 0 | 26 runs, all 2026-07-05/06 — previous cycle |
| `cursor-test` | private | 0 | — |
| `nos` | private | 0 | — |
| `qa-automation` | private | 0 | — |

**Measured.** `remux-toshiba` is the only private repository with any activity in
the cycle. It holds ten workflow files (`bench`, `build`, `lint`, `nightly`,
`pr`, `pr-docker`, `pr-title`, `release`, `test`, `toshiba-build`) and appears to
be a fork of an upstream multi-platform desktop/Docker project — its nightly
publishes to `ghcr.io/lostb1t/remux`.

Because `.../timing` returns `total_ms: 0` once the allowance is exhausted, the
minutes were reconstructed from the jobs API instead: for all 30 runs, every
job's `started_at`/`completed_at` and its runner label were pulled
(`repos/.../actions/runs/{id}/jobs`, 114 jobs total), each job's wall time
rounded **up** to the whole minute per GitHub's stated rule, and the OS
multiplier applied.

> "GitHub rounds the minutes and partial minutes each job uses up to the nearest
> whole minute."
> — <https://docs.github.com/en/billing/reference/actions-minute-multipliers>

| Runner label | Jobs | Wall minutes | Multiplier | **Billable minutes** |
|---|---:|---:|---:|---:|
| `ubuntu-latest` | 83 | 654 | 1x | 654 |
| `ubuntu-24.04-arm` | 12 | 52 | 1x | 52 |
| `windows-latest` | 12 | 163 | 2x | 326 |
| `macos-latest` | 6 | 101 | **10x** | **1,010** |
| **Total** | **113** | **970** | | **2,042** |

**2,042 billable minutes against a 2,000-minute allowance, from one private
repository, in seven days.**

The single largest line is six jobs. `Build / Build desktop
(aarch64-apple-darwin)` runs on `macos-latest` once per nightly; each took
10–22 wall minutes, and at 10x that is **1,010 minutes — half the entire monthly
allowance from six job executions.** The `Nightly` workflow is on
`cron: "0 2 * * *"`, so it fires whether or not anyone is working.

### 1.4 The multipliers and the allowance

**Measured (documentation).**

- Included minutes, GitHub Free personal account: **2,000/month**, reset at the
  start of each billing cycle.
  <https://docs.github.com/en/billing/concepts/product-billing/github-actions>
- Standard-runner rates: Linux `$0.008/min`, Windows `$0.016/min`, macOS
  `$0.08/min` — i.e. the familiar **1x / 2x / 10x** ratios, and the ratios
  (not the absolute prices) are what the included-minutes allowance is drawn
  down by.
  <https://docs.github.com/en/billing/reference/actions-minute-multipliers>
- Per-job round-up to the whole minute, as quoted above.

One honesty note on the multipliers: the current pricing page presents a *table
of per-minute dollar rates across many runner sizes* rather than a headline
"2x/10x" statement, and the standard-runner rates published there scale as
1 : ~1.67 : ~10.3 against Linux. The classic 1x/2x/10x figures are what the
minutes-multiplier reference gives for the standard runners, and are what I used
above. If you want the arithmetic to the cent rather than to the minute, use the
billing page in §3.

---

## 2. What was concluded

**High confidence (inference, but tightly constrained by measurement):** the
2,000 included minutes were consumed by **`denis-platonov/remux-toshiba`**, a
private fork whose scheduled `Nightly` and frequent `Toshiba Build` workflows ran
30 times between 22 and 28 August. Its reconstructed cost is 2,042 billable
minutes — enough on its own to exhaust the allowance, with nothing left over for
any other repository to have contributed. **Roughly half of that is six macOS
jobs**, billed at 10x.

**Verified:** Deskwarden's CI, despite being the heaviest workflow in wall-clock
terms of anything you own, contributed **zero** billable minutes, because the
repository is public and uses only standard GitHub-hosted runners.

**Retracted:** the earlier "four Windows jobs at 2x" explanation. It described a
real cost that GitHub does not charge you for.

The 2,042 figure is an inference, not a reading off the meter, for three reasons
worth stating plainly:

1. It is derived from `started_at`/`completed_at` timestamps. Those bound the job
   but are not the billing clock; queue time and runner teardown may be counted
   differently.
2. The cycle boundary is assumed to be the calendar month. Personal-account
   cycles usually are, but this was not confirmed for this account.
3. It cannot see anything the API does not list — runs deleted from the log, or
   usage against an organisation rather than the personal account.

That it lands within 2% of the allowance is suggestive, not proof.

---

## 3. What remains unknown, and how to settle it

**Unknown:** the authoritative per-repository minute breakdown, and the exact
billing-cycle dates.

`gh api users/denis-platonov/settings/billing/actions` returns nothing usable,
and `.../timing` on individual runs reports `total_ms: 0`, which is what GitHub
returns once the allowance is spent. There is no CLI path to the real numbers on
a personal account.

**To settle it, open the billing page in a browser:**

<https://github.com/settings/billing> → **Usage** tab (direct:
<https://github.com/settings/billing/usage>).

Set the date range to the current cycle and group by repository. That view
reports actual billed minutes per repository per runner OS, and the cycle's start
and end dates. Expect `remux-toshiba` to dominate it, with a macOS line roughly
half the total. **If it does not, this document's §2 is wrong and should be
corrected** — the measurement in §1.3 is sound but it is a reconstruction, and
the billing page is the meter.

The same page has **"Actions & Packages" spending limits**, which is where to set
a hard cap so the next surprise arrives as a stopped workflow rather than an
exhausted month.

**The cheapest single fix is not in this repository at all**: change
`remux-toshiba`'s nightly `Build desktop (aarch64-apple-darwin)` job to run
weekly, on `workflow_dispatch`, or only on release tags. That one edit is worth
about 1,000 minutes a month — half the allowance — and no change proposed below
comes close to it. Everything in §4 is about wall-clock time, review latency and
good hygiene in a repository that GitHub does not bill.

---

## 4. Proposed changes to this repository's CI

Framing, so these are not oversold: **Deskwarden is public and its Actions usage
is free.** None of the following saves money today. They are proposed because
they save *waiting*, because a doc-only commit should not take 18 minutes to go
green, and because the concurrency finding in §4.3 is a correctness bug in a
guard the file's own header describes as its reason for existing.

### 4.1 Path filters, so doc-only pushes do not run the matrix

**The problem, measured.** Between 21:19 and 22:10 on 2026-08-28, `main` received
a burst of commits that touch no Rust at all — `Drop the licence badge`,
`Screenshots in the README, from fixtures rather than a real vault`, `One
screenshot in the README, the rest on their own page`, `A logo at the top, and
the README follows the repo's new name`, `Say plainly that there is no warranty`,
`Record the tips of the branches that were deleted`. Each triggered all four
jobs: tests, a coverage run, a Mesa download plus an eleven-surface screenshot
walk, and a per-commit `cargo check`. None of it could have found anything.

```diff
--- a/.github/workflows/ci.yml
+++ b/.github/workflows/ci.yml
 on:
   push:
     branches: ["main"]
+    # A README, a screenshot or a spec cannot change whether the crate
+    # compiles, so they do not get the four-job matrix. Stated as
+    # paths-ignore rather than a paths allowlist on purpose: an allowlist
+    # silently stops running when somebody adds a new source directory,
+    # and a CI that quietly stops guarding is worse than one that runs too
+    # often.
+    #
+    # `.github/**` is deliberately NOT ignored -- a change to this file
+    # must run this file.
+    paths-ignore:
+      - "**.md"
+      - "docs/**"
+      - "LICENSE"
+      - ".gitignore"
   pull_request:
     branches: ["main"]
+    paths-ignore:
+      - "**.md"
+      - "docs/**"
+      - "LICENSE"
+      - ".gitignore"
```

**Caveat that must not be skipped.** If any CI job here is or becomes a
**required status check** for merging, `paths-ignore` will make pull requests
that touch only documentation hang forever on a check that will never report.
The documented remedy is a companion workflow of the same job names whose steps
are `run: 'true'`. Add path filters first; if merges start hanging, that is the
cause and that is the fix.

A second, smaller waste in the same area: `.github/workflows/ci.yml`'s
`compiles` job walks every commit in the pushed range with `cargo check`. A burst
of six doc commits pushed together makes it check six trees that differ only in
Markdown. The `paths-ignore` above stops the whole run; if you would rather keep
the run and skip only the walk, gate the per-commit loop with `git diff --quiet
$sha^ $sha -- ':!**.md' ':!docs/**'` and `continue` when it is clean.

### 4.2 The self-hosted runner, off by default

You asked for this weeks ago. Wire it as an *opt-in switch*, not a swap, so a
runner that is offline cannot wedge the project's CI:

```diff
--- a/.github/workflows/ci.yml
+++ b/.github/workflows/ci.yml
 on:
   push:
     branches: ["main"]
   pull_request:
     branches: ["main"]
+  # Manual runs can pick the runner. Default false, so nothing changes
+  # unless somebody deliberately asks for the local machine.
+  workflow_dispatch:
+    inputs:
+      self_hosted:
+        description: "Run on the self-hosted Windows runner"
+        type: boolean
+        default: false
+
+# One place that decides the runner, so the four jobs cannot drift apart.
+# `inputs.self_hosted` is unset for push and pull_request events, which is
+# falsey, so both keep using GitHub-hosted runners with no further
+# conditions anywhere.
+env:
+  RUNNER_LABELS: >-
+    ${{ inputs.self_hosted && '["self-hosted","windows"]' || '"windows-latest"' }}
```

and in each of the four jobs:

```diff
   test:
     name: Tests and warnings
-    runs-on: windows-latest
+    runs-on: ${{ fromJSON(env.RUNNER_LABELS) }}
```

(The `fromJSON` dance is what lets one expression yield either a single label or
the `[self-hosted, windows]` array that `runs-on` needs. Note that `env` is not
readable from a job-level `runs-on` in all contexts — if the expression above
does not resolve, hoist it into a tiny `ubuntu-latest` `setup` job that emits
`outputs.labels`, and have the four jobs read `needs.setup.outputs.labels`. That
costs one free Linux job-minute and is unambiguous.)

**Security caveat — this is the important part of this section, and it is not
optional.**

Deskwarden is a **public** repository, and this repository is a **credential
tool**. Attaching a self-hosted runner to a public repository is the single
riskiest thing in this document.

- Anyone on the internet can open a pull request. If a workflow runs on a
  self-hosted runner for fork pull requests, that stranger's code executes **on
  your machine**, as your user, with your filesystem, your network, your SSH
  keys and — on the machine you develop Deskwarden on — potentially your vault.
- GitHub says so directly: self-hosted runners "should only be used for private
  repositories", because forks "can potentially run dangerous code on your
  self-hosted runner machine".
  <https://docs.github.com/en/actions/how-tos/manage-runners/self-hosted-runners/manage-access>
- GitHub-hosted runners are safe from this because each job gets a clean VM that
  is destroyed afterwards. A self-hosted runner is *your* machine and keeps
  whatever a job leaves behind — including anything a job wrote to `~`, to the
  Cargo registry cache, or to a persistent `target/` directory.

Given that, only two configurations are defensible:

1. **Preferred: never on `pull_request`.** The `workflow_dispatch` gate above
   already achieves this — `inputs.self_hosted` is unset for `pull_request`, so a
   fork PR can never reach the runner no matter what its author writes. Keep it
   that way; do not "simplify" the condition later into something a PR can
   influence.
2. In the repository's **Settings → Actions → General**, set *Fork pull request
   workflows* to **Require approval for all outside collaborators** (and prefer
   "all external contributors"). This is belt-and-braces behind (1) and should be
   set even if you never enable the runner.

Additionally, register the runner **at repository scope, not account scope** — an
account-level runner is reachable by every repository you own, including any
public one you create later without thinking about it. And run it in a VM or a
throwaway Windows account, not on the workstation that holds a real Bitwarden
session.

If that is more care than the convenience is worth, the honest recommendation is
to not attach a self-hosted runner to this repository at all: the minutes it
would save are minutes GitHub is not charging you for.

### 4.3 The concurrency rule is not doing what its comment claims

The file says:

```yaml
concurrency:
  group: ci-${{ github.ref }}
  cancel-in-progress: ${{ github.ref != 'refs/heads/main' }}
```

with the comment: *"except on main, where each push's history is checked on its
own and cancelling would leave a range unverified."*

**Measured.** Four runs on `main` are marked `cancelled`:

| Run | Created | Head |
|---|---|---|
| 33212017060 | 2026-08-28T21:19:02Z | `6ca2122` |
| 33213431791 | 2026-08-28T21:38:28Z | `520dd0e` |
| 33214469055 | 2026-08-28T21:53:05Z | `71d0034` |
| 33040707278 | 2026-08-27T04:52:01Z | `efb005a` |

**Measured, and this is the tell:** `repos/.../actions/runs/{id}/jobs` returns
**zero jobs** for every one of them. They were cancelled before a single job was
created — they never started. They also each sit minutes ahead of another push
in the same doc-commit burst.

**Inferred, and I am fairly confident:** `cancel-in-progress` is behaving exactly
as written — no *running* job on `main` is being killed. But it only governs
*in-progress* runs. A concurrency group also serialises *queued* runs, and
GitHub's workflow-syntax reference states that "any existing `pending` job or
workflow in the same concurrency group will be canceled and the new queued job or
workflow will take its place." With `main`'s group holding one run for ~18
minutes and six pushes arriving inside an hour, each new push evicted the one
waiting behind it. Zero jobs and back-to-back timings are the signature of
eviction-while-pending, not of a cancelled run.

I could not confirm this from the API — GitHub does not expose *why* a run was
cancelled — and the documentation phrasing is not as crisp as one would like. It
is the only mechanism consistent with all four observations.

**Why this matters more than it looks.** The `compiles` job checks
`github.event.before..HEAD`. If push A's run is evicted while pending and push
B's run then executes, B's `github.event.before` is A's tip — so A's commits are
**never checked by anything**. The exact guarantee the comment claims the rule
buys is the guarantee it silently loses, and it loses it precisely during a rapid
burst, which is when a broken intermediate commit is most likely.

**The fix is to make the group per-commit on `main`**, so pushes to `main` do not
queue behind each other at all:

```diff
 concurrency:
-  group: ci-${{ github.ref }}
-  cancel-in-progress: ${{ github.ref != 'refs/heads/main' }}
+  # On a branch or a PR, one run per ref: a superseded run is wasted time
+  # and a misleading red.
+  #
+  # On main, one group PER COMMIT. `cancel-in-progress: false` was not
+  # enough, and the runs prove it: 33212017060, 33213431791 and
+  # 33214469055 are all `cancelled` on main with ZERO jobs -- evicted
+  # while pending, because a concurrency group replaces the run waiting in
+  # it when a newer one arrives. cancel-in-progress governs RUNNING jobs
+  # only. Since the `compiles` job walks `github.event.before..HEAD`, an
+  # evicted push's commits are then skipped by the next run too, which is
+  # exactly the range this file exists to check.
+  #
+  # Keying on the SHA gives each push its own group, so nothing queues and
+  # nothing is evicted. See docs/ci-and-actions-minutes.md.
+  group: >-
+    ci-${{ github.ref }}-${{
+      github.ref == 'refs/heads/main' && github.sha || 'ref'
+    }}
+  cancel-in-progress: ${{ github.ref != 'refs/heads/main' }}
```

This does mean several `main` runs may execute in parallel during a burst. On a
public repository that is free, and it is the price of the guarantee the file
already promises. (The alternative — keep the queue and make `compiles` resilient
by deriving its range from the last *successful* run rather than from
`github.event.before` — is more correct but considerably more machinery. Take it
if parallel `main` runs ever become a problem.)

### 4.4 `cargo install cargo-deny` is compiled from source on every run

In the `test` job:

```yaml
- name: Audit licences and advisories
  run: |
    cargo install cargo-deny --locked --version ^0.20
    cargo deny check licenses advisories
```

`cargo install` builds `cargo-deny` and its dependency tree from source, on
Windows, on every single run. `Swatinem/rust-cache` caches the *workspace*
target directory, not `~/.cargo/bin`, so nothing here is reused. This is several
minutes per run for a tool that ships prebuilt binaries — and this workflow
already uses the right mechanism one job over, for `cargo-llvm-cov`.

```diff
-      - name: Audit licences and advisories
-        working-directory: deskwarden
-        run: |
-          cargo install cargo-deny --locked --version ^0.20
-          cargo deny check licenses advisories
+      # A prebuilt binary rather than `cargo install`, which compiled
+      # cargo-deny and its whole dependency tree from source on every run --
+      # rust-cache caches the workspace target dir, not ~/.cargo/bin, so
+      # none of it was ever reused. `taiki-e/install-action` is already how
+      # the coverage job gets cargo-llvm-cov.
+      #
+      # The ^0.20 pin is preserved, and it is load-bearing: the original
+      # pin of ^0.16 failed on the first CI run because
+      # `unmaintained = "workspace"` is a key 0.16.4 removed and 0.20
+      # reintroduced. Pin the version the config was tested against.
+      - name: Install cargo-deny
+        uses: taiki-e/install-action@v2
+        with:
+          tool: cargo-deny@^0.20
+
+      - name: Audit licences and advisories
+        working-directory: deskwarden
+        run: cargo deny check licenses advisories
```

### 4.5 Smaller items, noted but not proposed as diffs

- **`test` and `coverage` build the same crate twice**, on two Windows runners,
  with separate caches. Merging them would roughly halve Windows wall time, but
  it would also let a coverage flake red the project's actual bar — which the
  file's comments say was a deliberate design choice. Left alone; recording it so
  the next person does not think it went unnoticed.
- **The `screenshots` job downloads a ~100 MB Mesa archive from GitHub releases
  on every run.** Correctly pinned and hash-verified — do not weaken that — but
  it is a candidate for `actions/cache` keyed on the URL and the expected hash.
- **`compiles` has a 25-commit backstop but no `timeout-minutes`.** 25 Windows
  `cargo check` runs is an unbounded-ish tail. A `timeout-minutes: 45` on that
  job is cheap insurance.
- **`release.yml` needs no change.** It runs once per `v*.*.*` tag on a single
  `windows-latest` job. It is not a contributor to anything discussed here.
