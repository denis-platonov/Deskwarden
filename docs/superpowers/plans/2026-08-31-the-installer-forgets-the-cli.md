# The Installer Forgets the CLI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A user runs Setup. It asks where to install, offers the autostart
checkbox, copies two files, and finishes. There is no "Installing Bitwarden
CLI..." page, no minute of waiting, no 37 MB fetched before a server has been
chosen, and no dialog telling anyone to go and install a command-line tool from
a help page. The word "CLI" does not appear in the installer at all.

**Architecture:** This implements
`docs/superpowers/specs/2026-08-31-the-installer-forgets-the-cli-design.md`. It
is **deletion only** — one file removed, one `[Files]` line, one procedure, one
wizard page, and one `CurStepChanged` branch. Nothing is added to the installer.
The only additions anywhere are three source-pin tests and the prose fixes that
the deletion makes necessary.

The rule that orders it, and it is not a preference:

> **`2026-08-31-the-cli-arrives-when-it-is-needed.md` ships first.**
> `backend_policy::choose(None, true)` is `BwServe`, so a fresh install with no
> `bw.exe` reaches `main`'s `fatal_startup_error` — which currently tells the
> user to reinstall the thing that just broke them. **Do not start this plan
> until acquisition is merged and green.** Verify it before Task 1: the branch
> is merged, `bw_acquire::acquire_if_needed` exists, and its tests pass.

**Tech Stack:** Inno Setup 6 (Pascal Script), and Rust for the three guards that
read the `.iss` — the idiom `the_installers_run_entry_passes_the_flag_the_app_reads`
and `the_installer_ships_the_wordlist_this_module_reads` established.

## Global Constraints

- `cfg(test)` seams are banned crate-wide; seams are `fn`-pointer structs in production code.
- Build with `RUSTFLAGS="-D warnings"`, on the build **and** on `cargo test --no-run`; zero warnings.
- `export CARGO_TARGET_DIR=/e/_dw_agent/run` — never create a second target directory.
- Tests must not pass vacuously: every negative assertion carries a positive control. The house defect is "a test that passes because it never reached the thing it names", and **a test asserting a file does not contain a string is the purest form of it** — a typo'd path reads as an empty string and every such assertion passes.
- Judge a failing test by reading it, never by its name prefix.

Additionally, and specific to this branch:

- **Do not touch `[Tasks]`, `[Run]`, `[Registry]`, `AppMutex`, or the wordlist `[Files]` line.** Each carries its own long-argued comment and none of them is about the CLI. If a task appears to need a change there, stop and report.
- **Do not delete `SuppressibleMsgBox` as an idiom.** Only the four boxes that name the CLI go. The distinction between `MsgBox` and `SuppressibleMsgBox` is load-bearing for `/VERYSILENT` self-update and must survive in the file's comments.
- **Do not touch `deskwarden/src/signature.rs`.** Its PowerShell-free property is asserted from the other side by Task 2 and must stay exactly as it is.
- Commit with explicit paths and `-F` a message file. Never `git add -A`, `--amend`, `reset`, `rebase`, or `git stash`.

## File Structure

| File | Responsibility |
| --- | --- |
| `deskwarden/installer/bootstrap-bw.ps1` (delete) | The 370-line acquisition script, its GitHub API query, its DN parser, its PATH write. |
| `deskwarden/installer/deskwarden.iss` (modify) | Remove the `[Files]` line, `InstallBwCliIfMissing`, `ProgressPage`, `InitializeWizard`, `CurStepChanged`. |
| `deskwarden/installer/README.md` (modify) | Remove the bootstrap sections. |
| `deskwarden/src/main.rs` (modify) | The three guards, and the one fatal message that names the installer. |
| `README.md`, `deskwarden/README.md`, `CHANGELOG.md` (modify) | The claims the deletion falsifies. |

## Interfaces

Nothing programmatic changes. The `.iss` keeps `AppMutex`, its `[Files]`,
`[Icons]`, `[Registry]`, `[Tasks]` and `[Run]` sections; it loses its entire
`[Code]` section.

---

### Task 1: The guards, written before the deletion

**Files:** `deskwarden/src/main.rs`

**Interfaces**

- *Consumes:* `include_str!("../installer/deskwarden.iss")`, the path helper the two existing installer-reading tests use.
- *Produces:* three source pins.

Written first, deliberately. Each must **fail now**, against the installer as it
stands, and pass after Task 2. A guard written afterwards is a guard nobody has
seen fail.

- [ ] **Step 1: Write the failing tests**

```rust
    /// **The installer names no CLI.** The owner's rule, held by the file
    /// rather than by review: "so it doesn't even ask about the bw cli".
    ///
    /// The positive control is not optional decoration. This test asserts a
    /// file does NOT contain some strings, which is exactly the assertion
    /// that passes when the file could not be read, when the path is wrong,
    /// and when `include_str!` picked up something empty. So it first proves
    /// it is looking at the real installer.
    #[test]
    fn the_installer_says_nothing_about_the_bitwarden_cli() {
        let iss = include_str!("../installer/deskwarden.iss");

        // Control, first: this IS the installer, and it still does the two
        // things the installer exists to do.
        assert!(iss.contains("deskwarden.exe"), "control: this is not the installer");
        assert!(iss.contains("wordlist.txt"), "control: this is not the installer");
        assert!(iss.contains("AppMutex"), "control: this is not the installer");

        let lower = iss.to_lowercase();
        for forbidden in ["bw.exe", "bootstrap-bw", "bitwarden cli", "bitwarden.com/help/cli", "\\bin\\bw"] {
            assert!(
                !lower.contains(forbidden),
                "the installer still names {forbidden}: the user is being told about machinery"
            );
        }
    }

    /// **The installer runs no PowerShell.**
    ///
    /// This is the pin that closes the crate's SECOND Authenticode
    /// mechanism. `signature.rs` verifies with `WinVerifyTrust` and
    /// `verification_needs_no_external_process` pins that it does, because
    /// `Get-AuthenticodeSignature` fails wherever
    /// `Microsoft.PowerShell.Security` cannot autoload. The installer used
    /// the PowerShell one, and hand-maintained a second X.500 DN parser to
    /// go with it. Together these two tests say, from both ends: **this
    /// crate verifies Authenticode in one way, in one place.**
    #[test]
    fn the_installer_shells_out_to_nothing() {
        let iss = include_str!("../installer/deskwarden.iss");
        assert!(iss.contains("deskwarden.exe"), "control: this is not the installer");
        let lower = iss.to_lowercase();
        for forbidden in ["powershell", "exec(", "extracttemporaryfile"] {
            assert!(!lower.contains(forbidden), "the installer still shells out: {forbidden}");
        }
    }

    /// The script is gone from the tree, not merely unreferenced.
    ///
    /// Control: its sibling still exists, read through the same join, so a
    /// wrong directory cannot make this pass.
    #[test]
    fn the_bootstrap_script_is_not_in_the_tree() {
        let installer = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("installer");
        assert!(installer.join("deskwarden.iss").exists(), "control: wrong directory");
        assert!(
            !installer.join("bootstrap-bw.ps1").exists(),
            "the bootstrap script is still shipped in the installer payload"
        );
    }
```

Run: **all three must fail**, and read the failures. Expect
`the_installer_says_nothing_about_the_bitwarden_cli` to name `bw.exe`,
`the_installer_shells_out_to_nothing` to name `powershell`, and
`the_bootstrap_script_is_not_in_the_tree` to report the file exists. **If any of
the three passes now, it is not reaching the installer — fix the test before
touching the installer, or Task 2 will appear to succeed against nothing.**

- [ ] **Step 2: Commit the failing guards**

Committed red, on purpose, so the diff shows what they caught.

---

### Task 2: The deletion

**Files:** `deskwarden/installer/deskwarden.iss`, `deskwarden/installer/bootstrap-bw.ps1`, `deskwarden/installer/README.md`

**Interfaces**

- *Removes:* `bootstrap-bw.ps1`, the `[Files]` `dontcopy` line, `InstallBwCliIfMissing`, `ProgressPage`, `InitializeWizard`, `CurStepChanged`.

- [ ] **Step 1: Delete, in this order**

1. The `[Files]` line and its four-line comment:
   `Source: "bootstrap-bw.ps1"; DestDir: "{tmp}"; Flags: dontcopy`.
2. `procedure InstallBwCliIfMissing()` entire, and the ~30-line `{ Runs
   bootstrap-bw.ps1 … }` rationale block above it. The three numbered reasons in
   that block — the monorepo tag filter, matching `signature.rs`'s choice, and
   avoiding `idp.iss` — are **findings, not commentary**, and the first two are
   preserved in the acquisition design. The third dies with the dependency.
3. `procedure CurStepChanged` — its `ssPostInstall` branch was the only caller,
   so the procedure is empty and goes.
4. `var ProgressPage`, `procedure InitializeWizard`, and the
   `CreateOutputProgressPage('Setting up Deskwarden', …)` call. The page's own
   comment says it existed because the compatibility check and the CLI
   bootstrap "used to run with SW_HIDE and zero visible feedback"; both are
   gone, so the page has nothing to show.
5. The whole `[Code]` section header, now empty.
6. `git rm deskwarden/installer/bootstrap-bw.ps1`.
7. `installer/README.md`'s bootstrap sections (:51–95) and the two references
   above them.

**Preserve the uninstall reasoning.** The comment explaining that uninstall
deliberately leaves `bw` and its PATH entry in place is still true — the app now
writes both — so move that paragraph into `bw_acquire.rs`'s module doc rather
than deleting it. A decision that survives its code should survive with it.

- [ ] **Step 2: Run the guards**

All three from Task 1 go green. **The two pre-existing installer tests —
`the_installers_run_entry_passes_the_flag_the_app_reads` and
`the_installer_ships_the_wordlist_this_module_reads` — must stay green.** If
either goes red, something load-bearing was deleted alongside the CLI: read what
it says and put that back, do not adjust the test.

Then compile the installer with Inno Setup and confirm it produces a setup
executable, since these guards read the source and cannot catch a Pascal syntax
error left by a partial deletion.

- [ ] **Step 3: Commit**

`deskwarden/installer/deskwarden.iss`, `deskwarden/installer/bootstrap-bw.ps1`,
`deskwarden/installer/README.md`, explicit paths.

---

### Task 3: The sentences that became false

**Files:** `deskwarden/src/main.rs`, `README.md`, `deskwarden/README.md`, `CHANGELOG.md`

Three shipped claims stop being true on the day Task 2 lands. They change in the
same branch, because a README that describes a bundled binary that is not
bundled is a defect with a longer half-life than any code.

- [ ] **Step 1: Fix each, and say what it now says**

- **`main.rs`**, the `bw_is_this_accounts_vault` fatal arm: *"Install the
  Bitwarden CLI, or reinstall Deskwarden (its installer downloads a signed copy
  for you)."* The installer does not. After acquisition this arm should be very
  nearly unreachable — it now means "an official account, at startup, with no
  CLI and no completed setup" — so the text should say that setup will finish
  the next time they sign in, and stop naming the installer.
- **`README.md:254`**: *"the Authenticode signature check on the bundled
  `bw.exe`"*. Not bundled. The check is unchanged and now happens in one place
  instead of two — which is worth saying, since that table is about tradeoffs.
- **`README.md:306`**: `| Full install (app + bundled bw CLI) | ~169 MB |`.
  A fresh install is the app alone until an official server is chosen. Give both
  numbers; two honest rows beat one convenient one.
- **`deskwarden/README.md:26`**: the CLI listed under Requirements. Delete the
  bullet — it has not been accurate since the installer started fetching it and
  is emphatically wrong now.
- **`CHANGELOG.md`**: a **new entry**. Do not edit the existing lines; they
  record what shipped and that did happen.

- [ ] **Step 2: Verify the whole suite, then commit**

`cargo test`, then `RUSTFLAGS="-D warnings" cargo test --no-run`, then commit
with explicit paths.

## Status

Plan. Not executed. **Blocked on
`2026-08-31-the-cli-arrives-when-it-is-needed.md` being merged and green.**
