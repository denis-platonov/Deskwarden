# The daemon and the UI as separate processes

**Status:** designed, not started. Supersedes the "The shape" and "What this costs, honestly" sections of `2026-08-21-daemon-and-ui-split-design.md`. That document's *measurements* stand and are still the reason this work exists; several of the costs it predicted have since turned out not to be real, and this spec records why.

## The problem, measured

On the owner's machine, this build:

| state | private | GPU driver loaded |
| --- | --- | --- |
| `--autostart`, tray-resident | **11.3 MB** | no — only the `opengl32.dll` stub |
| after the vault window has been opened once | **128.6 MB** | `nvoglv64.dll`, `nvgpucomp64.dll` |
| the picker card alone, as a standalone process | ~2 MB | no |

Closing the vault window does not undo the second row. `2026-08-21`'s measurements established that the cost is the OpenGL driver's own committed arenas, that eframe already destroys the GL context, that explicit teardown changes nothing, and that the cost *ratchets* — roughly 4 MB per open/close cycle. **Only process exit returns it.**

So a single-process app has two steady states and no way back from the second. One accidental double-click on the tray icon at 9am and Deskwarden is a 128 MB process until the machine reboots.

## The shape

**One executable, two modes.** Not two binaries.

- **Daemon** — `deskwarden.exe --autostart`. Tray, global hotkey, match engine, `bw serve` ownership, and the bare-Win32 surfaces: the unlock prompt and the account picker. Never creates a GL context. Measured at 11.3 MB.
- **UI** — `deskwarden.exe --ui <surface>`, spawned per window. The vault window, Preferences, the save-login form, the generator, the sequence editor, rehearsal. Pays the ~90-115 MB while open and **exits when its window closes**, which is the only mechanism that returns the driver's memory.

**Why one binary and not two.** Two executables buy nothing the mode flag does not, and cost:

- **Version skew.** A half-applied update leaves an old daemon talking to a new UI. With one file there is nothing to mismatch — this is why the old spec listed "version-matched protocol, two-part atomic update" as a cost and why that cost disappears here.
- **Installer, updater, signing and uninstaller stay single-target.** The self-update replaces one binary today; making it two is where a partial failure leaves an unusable install.
- **Single-instance stays one mutex.** The old spec warned about "doubled single-instance logic"; the takeover protocol took a full session to get right for one process and should not be written twice.

## The UI needs nothing passed to it

This is the finding that makes the split cheap, and it is why this spec supersedes the old one's central objection.

`2026-08-21` said: *"Secrets cross a process boundary. Today the posture is 'one process; a password never leaves it'. Split, and the UI must ask for what it displays or types. A named pipe with a correct DACL can be done, but it is a new surface where none exists, and the kind that is hard to prove right."*

**That is no longer true, and on inspection was never necessary.** Everything the UI needs is already on disk or is a constant:

- **The session token** is DPAPI-wrapped per account at `accounts::session_path_for(config_dir, id)` (`session_store.rs`). DPAPI unwraps under the user's own credentials, so a second process running as that user loads it directly. No secret crosses any new boundary because no secret is transferred at all — each process independently unwraps the same file.
- **The active account** is `Settings::active_account` in `settings.json`.
- **The backend port** is `bw_serve::BW_SERVE_PORT`, a compile-time constant (8087) — and the UI is the same binary, so it already has the value.

So the UI is launched with a mode and a surface, and reads the rest itself. **There is no pipe, no protocol, no DACL, and no rendezvous file.** `bw serve` is an HTTP API on localhost; the UI is one of its clients, exactly as the single process is today.

**Nothing secret may ever go on a command line.** Command lines are readable by other processes on the machine. The mode and the surface name are not secrets; a session token or a password would be, and passing one this way would reintroduce a worse version of the boundary this design avoids.

## Lifetimes

**The daemon owns `bw serve`.** It is the orchestrator; the UI is a client of the backend it runs.

**The UI is *not* a kill-on-close child of the daemon's job object.** The two are loosely coupled: a daemon restart — an update, a crash, a manual quit and relaunch — must not take an open vault window with it. When the daemon comes back it brings `bw serve` up on the same constant port, and the UI's next request succeeds. Recovery is a retry, not a handshake.

**A UI with no daemon exits by itself.** Quitting from the tray leaves an open vault window whose backend is gone. Rather than teach that window to explain itself, it ends: **no daemon, no UI.**

The signal already exists and needs nothing new built. `app_mutex::APP_MUTEX_NAME` is a `Local\`-scoped named mutex the daemon holds for the life of the process, per logon session. The UI opens it to ask "is Deskwarden running?" — a question, not a claim, so it never contends for ownership.

**It exits on sustained absence, not on the first miss.** Read literally, "exit when the tray is gone" and "the daemon can reconnect if restarted" contradict each other: a UI that quits the instant the mutex disappears leaves nothing for a restarted daemon to reconnect to, and an update — which stops the old daemon before starting the new one — would close every window the user had open. So absence is timed: a brief gap is a restart or an update and the UI waits and reconnects; a sustained gap means Deskwarden was quit, and the UI exits.

**The exact grace period is not fixed here.** It must comfortably exceed a self-update's daemon-restart gap, because closing the user's windows during an update would be a worse bug than the one this rule fixes. Measure that gap and choose from it; do not guess a round number.

**First launch is exempt, and this is the easy thing to get wrong.** A plain double-click on `deskwarden.exe` finds no mutex because no daemon is running *yet* — and must start one, not exit immediately. The rule is "a UI whose daemon *went away* exits", not "a UI that finds no daemon exits". Whatever implements this must be able to tell the two apart, and a test must pin the difference, because getting it backwards makes the application impossible to start by double-clicking and the failure looks like the binary is broken.

**One UI per surface, not per request.** Asking for the vault window while one is open must focus the existing one, not spawn a second. The window-per-surface identity needs an owner; the existing single-instance machinery is the precedent to follow rather than a second scheme.

## The staleness question — open, deliberately

If the user adds an app-match in the vault window, the daemon's match engine does not know. The options, cheapest first:

1. **The daemon re-syncs when a UI process exits.** No IPC — a wait on the child handle. Cost: a match added stays inert until the window closes.
2. **A named event the UI sets on any vault write**, which the daemon waits on. Carries no data, so it is not the pipe the old spec feared, but it is a second thing that must agree.
3. **The daemon polls `bw serve`** on an interval. Simplest to reason about, wasteful, and picks up changes from other Bitwarden clients too, which 1 and 2 do not.

**This is not decided.** The owner's stated position — "daemon can just reconnect to the UI app if restarted" — settles *restart resilience*, not staleness; they are different questions and conflating them would be a design decided by accident. Option 1 is the default if nobody chooses, because it adds nothing that can disagree.

## What this costs, honestly

**The UI pays the full ~90-115 MB every time it opens**, and now also pays process startup — `bw serve` is already warm, but eframe, the fonts and the driver are not. The single-process app pays this once; the split pays it per window. **For someone who lives in the vault window this is strictly worse.** The win is real only if the steady state is genuinely tray-idle.

**Two processes are two things to observe.** A hung UI and a hung daemon look different, log to the same file, and will need to be told apart in a bug report.

**The daemon must not outlive an uninstall.** It already must not, but a second long-lived process makes the failure more visible.

**The 4 MB-per-cycle ratchet moves rather than disappears** for a user who opens and closes the vault window repeatedly *within one UI process* — but that process now exits, so the ratchet is bounded by one window's lifetime instead of the app's.

## Amended 2026-08-23, after the startup was actually mapped

`docs/superpowers/notes/2026-08-23-startup-role-map.md` traced the code this spec describes and overturned three of its claims. The corrections are here rather than edited invisibly into the text above, because the reasoning that produced the wrong version is worth keeping.

**1. The fill path's surfaces cannot become UI processes. They must be redrawn in Win32.**

The daemon opens **five** egui windows on its own autofill path today: the prompt overlay (2a), the locked card (3b), the save-login form (3c), the generator (3d), and the send preflight. `overlay_ui::show_prompt_overlay` and `show_locked_overlay` are the production presenter at `app.rs:2129`.

They cannot move to a UI process, because they are not merely *shown* during a fill — they are *part of* it. They anchor to the target window's HWND, they sit next to the injector, and `preflight_host::show_preflight` takes the password as a `Zeroizing<String>` argument. Moving them would put a secret across a process boundary, which this design forbids for the reason stated above.

So the only way the daemon stays free of a GL context is to **redraw them in Win32** — which the original text below lists as out of scope. That was wrong, and it is the largest single piece of work this split needs.

**2. The 11.3 MB steady state does not survive first use, as measured.**

That number was taken from a daemon that had never been asked to fill anything. On today's code the first `CTRL+ALT+B` on a matched app, or any fill against a locked vault, opens egui in the daemon and loads the OpenGL driver — permanently, since the arenas are never returned. **Any future measurement of the daemon must be taken after exercising the fill path**, or it measures nothing.

**3. "The UI needs nothing passed to it" holds only for input, not for results.**

It is true that a UI process can *start* from settings, the DPAPI session and the constant port. It is false that nothing needs to come back. The vault window's result carries six daemon-actionable outcomes — `locked`, `needs_reauth`, `edited_settings`, `switch_to`, `add_account`, `remove_account` — which drive teardown, re-auth and account re-pointing. Preferences returns a result and a process-global account status; "Add app…" needs `last_active_pid` and a live `VaultEra` from the shared cache.

**A result channel is therefore required.** It carries no secrets, so it is still nothing like the pipe the 2026-08-21 spec feared, but the claim that this split needs no channel at all was wrong.

**Also found, and not fixed:** the login flow needs the target account id passed explicitly during an account switch. Without it a UI process would overwrite the wrong account's `session.bin`. That is a latent bug in the current single-process code, not something this split introduces.

## What is explicitly out of scope

- **The overlay's rich states — superseded, see the amendment above.** Originally: "3c (save-login) and 3d (generator) stay egui and become UI-mode surfaces. Redrawing them in Win32 is a separate question and is not required by this split." The map showed this is exactly backwards.
- **The `wgpu` renderer.** A D3D-backed renderer measured ~40-59 MB against OpenGL's ~102 MB on this machine and would roughly halve the UI's cost — but it is blocked on a `windows-core` version conflict and is independent of this work. Doing both is better than either; neither depends on the other.
- **The REST backend.** `deskwarden/src/rest/` will let self-hosted users drop `bw serve` entirely. That changes *what* the daemon owns, not *whether* the daemon owns it, so it composes with this design rather than competing with it.

## The rule that keeps it safe

**Every surface lives in exactly one renderer.** If a card exists in both the daemon's Win32 and the UI's egui, that is the "two things that must agree" defect at the worst possible place — the surface that types passwords. A Win32 "no saved login" card and an egui save-login form are different cards, and *New login* is a handoff, not a duplicate. This rule was applied when the account picker replaced design 3a, and it is why that change deleted the egui card rather than adding a second one.
