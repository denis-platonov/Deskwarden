# The Installer Forgets the CLI

**Setup copies an app and finishes. It does not mention, fetch, verify, or make
a wizard page out of anybody else's binary.**

## Why

The owner:

> "fix the installer - so it doesn't even ask about the bw cli"

under the general rule:

> "users should not know about how it works underhood"

The installer today spends more of itself on a third-party dependency than on
Deskwarden. `installer/bootstrap-bw.ps1` is 370 lines: a GitHub releases-API
query, a monorepo tag filter, an archive extraction, a hand-written X.500 DN
parser, an Authenticode check, and an `HKCU\Environment` PATH write.
`deskwarden.iss` adds a `[Files]` entry, a custom `TOutputProgressWizardPage`, a
PowerShell-availability probe, an `Exec` with a four-way exit-code switch, and
four `SuppressibleMsgBox` dialogs. All of it exists so that a user who has not
yet chosen a server gets a binary that only one of the two choices needs.

**It is also the app's second, divergent copy of a security check it already
performs.** `signature.rs` verifies Authenticode in-process with
`WinVerifyTrust`, and `verification_needs_no_external_process` pins that it does
— because `Get-AuthenticodeSignature` fails wherever `Microsoft.PowerShell.Security`
cannot autoload. `bootstrap-bw.ps1` uses `Get-AuthenticodeSignature`, and
maintains its own `Get-CertificateDnComponent` whose doc admits it is "kept in
sync by hand with `dn_component` in `src/signature.rs`". Two parsers of the same
grammar, in two languages, held together by a comment. This change deletes one
of them.

## Ordering — this must not land first

`backend_policy::choose(None, true)` returns `BwServe`: a fresh install has no
account, `server_url` is `None`, and `None` is `bitwarden.com` by definition.
`main`'s startup gate makes a missing `bw.exe` a `fatal_startup_error` on exactly
that arm, with the text *"reinstall Deskwarden (its installer downloads a signed
copy for you)"*.

So an installer that stops fetching the CLI, shipped on its own, **bricks every
new install at first launch and tells the user to reinstall the thing that just
broke it.** `2026-08-31-the-cli-arrives-when-it-is-needed-design.md` lands
first, or the two land together. There is no arrangement in which this one goes
alone.

## What is deleted

Every item below is removal. Nothing is added to the installer.

| Where | What goes |
| --- | --- |
| `installer/bootstrap-bw.ps1` | The whole file. |
| `deskwarden.iss` `[Files]` | `Source: "bootstrap-bw.ps1"; DestDir: "{tmp}"; Flags: dontcopy` and its comment. |
| `deskwarden.iss` `[Code]` | `InstallBwCliIfMissing()` entire, its 30-line rationale comment, and the `ssPostInstall` branch in `CurStepChanged` that is its only caller. |
| `deskwarden.iss` `[Code]` | `ProgressPage`, `InitializeWizard`, and `CreateOutputProgressPage('Setting up Deskwarden', …)` — the page existed only to give the CLI bootstrap visible feedback, and its own comment says so. |
| `installer/README.md` | The bootstrap sections (:51–95) and the two mentions above them. |

`CurStepChanged` becomes empty and goes with it. The uninstall comment about
deliberately leaving `bw` and its PATH entry behind stays true and stays
relevant — the app now writes both — but it moves to the acquisition module's
doc, where the code that writes them lives.

## Exactly what it says today

For the record, and because these are the sentences that disappear.

The wizard page, `deskwarden.iss:253–254`:

> "Installing Bitwarden CLI..." / "Downloading and verifying the official CLI if
> it is not already present -- this can take a minute"

and before it, `:231`:

> "Checking compatibility..." / "Confirming PowerShell is available"

The four dialogs, `deskwarden.iss:242, 282, 290, 294`:

> "Deskwarden could not find PowerShell, which it needs to set up the Bitwarden
> CLI (bw). Please install it yourself from https://bitwarden.com/help/cli/ and
> ensure it's on your PATH."

> "Deskwarden could not launch PowerShell to set up the Bitwarden CLI (bw).
> Please install it yourself from https://bitwarden.com/help/cli/ and ensure
> it's on your PATH."

> "The Bitwarden CLI download did not pass signature verification, so it was not
> installed. Please install it yourself from https://bitwarden.com/help/cli/ and
> ensure it's on your PATH."

> "Deskwarden could not automatically set up the Bitwarden CLI (bw). Please
> install it yourself from https://bitwarden.com/help/cli/ and ensure it's on
> your PATH."

Note what these four have in common: **every failure path ends by telling the
user to go and install a command-line tool by hand.** That is the concrete thing
the owner's rule forbids, and it is unreachable from an app that acquires the
binary itself when and only when it needs it.

There is **no checkbox and no wizard question** about the CLI today — the
bootstrap runs unconditionally at `ssPostInstall`. So "doesn't even ask" is
partly satisfied already; what the installer does is *tell*, four ways, and take
a minute of the user's time and 37 MB of their connection before they have
chosen a server that needs it.

## What is not deleted

* `[Tasks] autostart` and its `/MERGETASKS=!autostart` pairing. Untouched, and
  the reasoning that binds those two together is unaffected.
* `[Run]`'s `postinstall nowait` relaunch. Load-bearing for self-update.
* `AppMutex` and the "please close Deskwarden" behaviour.
* The wordlist `[Files]` line, and `the_installer_ships_the_wordlist_this_module_reads`.
* The `HKCU\Run` registry line and `the_installers_run_entry_passes_the_flag_the_app_reads`.
* `SuppressibleMsgBox` as the idiom, for anything that still needs a box.

## Sentences elsewhere that become false

Deleting the bootstrap makes three claims in shipped text untrue on the day it
lands. They change in the same commit, not later.

1. **`main.rs`'s fatal startup text** — *"Install the Bitwarden CLI, or reinstall
   Deskwarden (its installer downloads a signed copy for you)"*. After
   acquisition exists, this arm should be nearly unreachable; the sentence
   becomes the acquisition failure's own wording.
2. **`README.md:254`** — *"the Authenticode signature check on the bundled
   `bw.exe`"*. The `bw.exe` is no longer bundled. The check moves; the word
   "bundled" goes.
3. **`README.md:306`** — *"Full install (app + bundled `bw` CLI) | ~169 MB"*.
   A fresh install is now the app alone until the user picks an official server.
   The table needs both numbers, which is more honest than the one it has.

`deskwarden/README.md:26` lists the CLI under Requirements — *"The Bitwarden CLI
(`bw`) installed and on your `PATH`"* — which has not been true since the
installer started fetching it and is emphatically not true after this. It goes.

`CHANGELOG.md` gets a new entry rather than an edit: its existing lines are
history and history did happen.

## How it will be known to work

The installer has no test runner, so the guards are source pins from the crate
that already reads this file — the idiom
`the_installers_run_entry_passes_the_flag_the_app_reads` and
`the_installer_ships_the_wordlist_this_module_reads` established.

1. **The installer names no CLI.** A test reading `installer/deskwarden.iss`
   asserting it contains none of `bw.exe`, `bootstrap-bw`, `Bitwarden CLI`, or
   `bitwarden.com/help/cli`. Positive control: the same read must find
   `deskwarden.exe` and `wordlist.txt`, so a test that silently read an empty
   string cannot pass.
2. **The installer runs no PowerShell.** The same read asserting no
   `powershell`, case-insensitively. This is the pin that closes the second
   verification mechanism, and it pairs with `signature.rs`'s existing
   `verification_needs_no_external_process` to say the same thing from both
   ends: **the crate verifies Authenticode in one way, in one place.**
3. **The script is gone from the tree.** `installer/bootstrap-bw.ps1` does not
   exist. Control: `installer/deskwarden.iss` does, read through the same
   helper, so the assertion is about that file's absence and not about a broken
   path.
4. **The pieces that stay, stay.** The two existing installer-reading tests are
   untouched and must remain green; if either goes red, something load-bearing
   was deleted alongside the CLI.

## Status

Design. Not implemented. Sequenced **after**
`2026-08-31-the-cli-arrives-when-it-is-needed-design.md`; see *Ordering* above
for why that is not a preference.
