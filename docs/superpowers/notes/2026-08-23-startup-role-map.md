# Startup role map — Task 0

**Read-only mapping exercise. No production code changed.**

Sources: `deskwarden/src/main.rs` (26,273 lines; `fn main` is 171–3102),
`app_window.rs`, `bw_serve.rs`, `job_object.rs`, `app_mutex.rs`,
`single_instance.rs`, plus `app.rs`, `overlay_ui.rs`, `preflight_host.rs`,
`picker_ui.rs`, `login_ui.rs`, `prefs_ui.rs`, `vault_window/`.

Labels: **D** = must stay in the daemon, **U** = belongs to a UI process,
**B** = both processes need it.

---

## Step 1 — `main` from entry to the tray event loop

`fn main()` spans **171–3102**. The tray event loop is the `loop {` at
**2117–3101**. Everything below is inside `main` unless noted.

### Phase A — process identity and prerequisites (171–391)

| line | step | role |
| --- | --- | --- |
| 189 | `app_mutex::acquire()` — the installer-visible named mutex | **D** (a UI must *open*, never acquire — see Task 4) |
| 191–216 | `ProjectDirs`, `config_dir`, `update_download_dir`, `icon_cache_dir`, `app::set_icon_cache_dir` | **B** |
| 220 | `logging::init(&config_dir)` | **B** (both processes log to the same file — the spec already flags this as an observability cost) |
| 250–300 | `single_instance::resolve(..)` — takeover / stand-down / `exit(1)` | **D** |
| 309 | `single_instance::listen_for_takeover()` (spawns a thread) | **D** |
| 315 | panic hook → log | **B** |
| 325–348 | `bw_path::resolve_bw_exe`, existence check, `check_bw_signature`, `remember_verified_bw_exe` | **D** primarily; **B** only if a UI ever spawns `bw` (it should not) |
| 356 | `updater::cleanup_stale_downloads` | **D** |
| 380–381 | `settings_path`, `Settings::load` | **B** |
| 391 | `clipboard::configure(settings.clipboard_clearing())` | **B** (the vault window copies) |

### Phase B — account resolution (393–571)

| line | step | role |
| --- | --- | --- |
| 422 | `bw_path::multi_account_availability()` | **B** |
| 431–444 | `accounts::resolve_startup(..)` (pure; mints an account if none) | **B** |
| 451–514 | destructure `active_account`, `accounts_state`, `first_run_account` | **B** |
| 534–549 | `session_path`, `active_dir`, `bw_path::set_active_data_dir` | **B** — a UI process must repeat this before it touches the token store |
| 553 | `session_store::SessionStore::new(session_path)` | **B** |
| 564 | `login_context(..)` | **B** |
| 570–571 | `fill_stats::FillStats::new(config_dir/fill-stats.json)` | **B** (the vault window writes it) |

### Phase C — backend, cache, engine (586–970)

| line | step | role |
| --- | --- | --- |
| 586 | `job_object::KillOnCloseJob::new()` | **D** |
| 610 | `store.load()` → `cached_session` (DPAPI unwrap) | **B** |
| 627 | `VaultBridge::new(BW_SERVE_URL)` | **B** |
| 642–679 | `VaultCache::with_disk_cache(..)`, `load_from_disk()` | **B** |
| 749–826 | version, `update_panel::install_env(..)` | **D** |
| 834 | `cache.epoch()` → `startup_epoch` | **B** |
| 838 | `MatchEngine::new()` | **D** |
| 844 | `readiness_schedule(READINESS_DEADLINE)` | **B** |
| 849 | `Injector { .. }` | **D** |
| 860–970 | `startup_entries`, `startup_vault`, `cache_first`, `bw_serve_child`, `session_token`, `backend_op_tx/rx`, `startup_tray_effects` | **D** except the cache/token, which are **B** |

### Phase D — the launch decision (1036–1706)

| line | step | role |
| --- | --- | --- |
| **1036** | `launch_intent(std::env::args().skip(1))` | **B** — this is the fork Task 1 extends |
| 1037 | logs intent + `first_surface(launch)` | **B** |
| 1039 | `if let Some(token) = cached_session` — the *warm* branch | **B** |
| 1049 | `let surface = first_surface(launch)` | **B** |
| 1068 | `cache_first = surface == StayInTheTray && disk cache present` | **D** |
| **1085** | `start_backend(&session_token, job_ref(&job))` — spawns `bw serve` into the job | **D** |
| 1090–1143 | cache-first arm: seed `startup_entries` from the disk cache | **D** |
| 1144–1192 | `StayInTheTray` arm: `settle_a_tray_launch(..)`, `arm_autofill_and_seed_cache(..)` | **D** |
| 1193–1338 | `ShowTheWindow` arm: `app_window::run_from_working(..)` at **1223** | **U** (blocking egui window) |
| 1243 | worker `thread::spawn` inside the warm launch | **B** |
| 1341–1352 | build `SessionEstate` | **D** |
| **1353–1706** | the *cold* branch: `park_and_work(..)` at **1417** hosting `app_window::run(..)` at **1452** (login card → spinner → vault, one event loop); then `store.save(&token)` at 1618, `session_token`, `startup_vault`, `outcome.prepared` | **U** for the window at 1452; **D** for the estate it builds |

### Phase E — daemon-only wiring (1729–2115)

| line | step | role |
| --- | --- | --- |
| 1774–1783 | cache-first reconciliation | **D** |
| 1795 | `stop_backend_if_idle(..)` | **D** |
| **1832** | `hotkey::register_fill_hotkey()` | **D**, thread-bound (see Step 3) |
| **1836** | `tray::build_tray()` | **D**, thread-bound |
| 1850 | `away_lock::register_on_this_process()` (session notifications) | **D** |
| 1856–1864 | drain `startup_tray_effects`, `rebuild_accounts_menu` | **D** |
| 1872–1908 | `updater::build_api_agent()`, update channel, background check thread (**1900**) | **D** |
| 1923 | `app::FillProof::default()` | **D** |
| 1944–1958 | account-status prefetch, `prefs_ui::publish_account_status`, thread at **1955** | **D** |
| **1970–1977** | `window_watch` foreground event channel + watcher thread | **D** |
| 1982–2061 | autofill loop state: `pending_hotkey_fill`, `last_dispatched_hwnd`, `field_probe_memo`, `pending_vault_search`, `last_active_pid`, `own_pid`, `pending_menu_events` | **D** |
| 2027–2053 | seed pass: `window_watch::current_foreground_event()` → `process_foreground_event` (with `unlock_prompt::ask` at **2050**) | **D** |
| 2085–2115 | dispatch the startup window's result | **D** |
| **2117–3101** | the tray event loop | **D** |

### How much `LaunchIntent` / `FirstSurface` already encode

**Less than the plan's framing suggests.** `LaunchIntent` (**8517**, two
variants: `UserLaunch`, `LoginAutostart`) and `FirstSurface` (**8578**,
`ShowTheWindow` / `StayInTheTray`) are today a one-to-one pair — `first_surface`
(8600) is a two-arm `match`. What they decide is **"does a window appear at
startup"**, and nothing else. They do *not* gate:

- the tray icon, the hotkey, the watcher, `bw serve`, the job object, the match
  engine — all of those run on **both** arms, unconditionally, after the
  branch rejoins at 1729;
- the tray event loop, which both arms enter.

So the existing types encode roughly **one of the two axes**: *first window or
not*. The daemon/UI axis — *does this process own the tray at all* — is not
represented anywhere today. `LaunchIntent::Ui(Surface)` (Task 1) is therefore a
genuinely new axis, not a third value on an existing one, and Task 3 has to add
an early return **before line 1832** rather than a new `FirstSurface` variant.

**Contradiction with the plan (mild):** the plan says the startup "interleaves
daemon and UI concerns across roughly 1,200 lines". The interleaving is real but
it is more concentrated than that: **1036–1706** is the mixed region (~670
lines) and **1729–2115** is purely daemon. The genuinely hard part is 1353–1706,
where the cold-start window *produces* the session token and the prepared estate
the daemon then owns.

---

## Step 2 — every site that opens an egui window

An "egui window" here means a call that reaches `eframe::run_native` /
`eframe::run_ui_native`. **All of them block the calling thread for the
window's whole life** (`app_window.rs:1146`, `1936`, `2209` all state this
explicitly), and **all of them are called on the process's main thread**. No
egui window in this crate is opened from a spawned thread.

| # | site | opener | called from | thread | blocks caller |
| --- | --- | --- | --- | --- | --- |
| 1 | `app_window::run_from_working` (`app_window.rs:2184`) | `main.rs:1223` | `main`, warm `ShowTheWindow` launch | main | **yes** |
| 2 | `app_window::run` (`app_window.rs:1094`) | `main.rs:1452` | `main`, cold start, inside `park_and_work` | main | **yes** |
| 3 | `app_window::run_from_vault` (`app_window.rs:1912`) | `main.rs:5772` | `RealVaultOps::open_window` ← `open_vault_window` ← tray loop (2086, 2127, 2231, 2774) | main | **yes — this is what blocks the tray** |
| 4 | `app_window::run_recovery` (`app_window.rs:2449`) | `main.rs:8722` | `wait_for_the_vault(ProbeWindow::InAWindow)` | main | **yes** |
| 5 | `prefs_ui::run` (`prefs_ui.rs:3211`) | `main.rs:2272` | tray loop, `preferences_id` | main | **yes** (the code comment at 2265 says so) |
| 6 | `login_ui::run_login_flow_for` (`login_ui.rs:2602`) | `main.rs:2472`, `6118`, `9166` | tray loop / vault loop / `authenticate_for_switch` | main | **yes** |
| 7 | `login_ui::run_login_flow` (`login_ui.rs:2641`) | `main.rs:9238` (`reauthenticate`) | vault loop | main | **yes**; `exit(1)` on close |
| 8 | `picker_ui::pick_vault_item` (`picker_ui.rs:465`) | `main.rs:8350` (`AddAppFlow::begin`) ← tray loop 2590 region | main | **yes** |
| 9 | `picker_ui::run_picker` (`picker_ui.rs:1067`) | `main.rs:2590` | tray loop, "Add app…" | main | **yes** |
| 10 | `overlay_ui::show_prompt_overlay` (`overlay_ui.rs:185`) | `app.rs:1776` via `REAL_OVERLAY` ← `handle_match` | main | **yes** |
| 11 | `overlay_ui::show_locked_overlay` (`overlay_ui.rs:1073`) | `app.rs:2860` `handle_locked` ← `REAL_LOCKED_CARD` (`main.rs:3728`) | main | **yes** |
| 12 | `overlay_ui::show_save_login_overlay` (`overlay_ui.rs:1585`) | `app.rs:2627` `save_login_flow` ← `handle_no_match` | main | **yes** |
| 13 | `overlay_ui::show_generate_overlay` (`overlay_ui.rs:2352`) | `app.rs:2216` ← the save-login card's *Generate* | main | **yes** |
| 14 | `preflight_host::show_preflight` (`preflight_host.rs:69/86`) | `app.rs:768` `confirmed_by_preflight` ← `fill_from_vault_with` | main | **yes** |
| 15 | `loading_ui::show_while` (`loading_ui.rs:191`) | **no production caller reaches it from `main` any more**; `app_window.rs:6648` pins that. Still reachable via `picker_ui` | main | yes |

**Not window openers** (they draw *inside* an existing frame, and the plan's
list is wrong to imply otherwise): `scratch_window` (the sequence-editor
scratchpad, painted inside the vault window's frame — its own module doc says
so at `scratch_window.rs:11`), `region_overlay` (`region_overlay.rs:27`,
painted inside the vault window), `update_panel` (a panel, not a window),
`vault_window::{rehearsal, record_ui, send_ui, totp_add, folder_modal, …}`.

### `vault_window::run` — the `main.rs:4758` comment

**Two separate answers, and the plan conflates them.**

1. **`vault_window::run` (`vault_window/mod.rs:5193`) has no production
   caller.** Grepping the whole crate finds only doc-comment mentions
   (`main.rs:376, 2295, 2558, 4758, 5670, 6361, 12904, 14138, 14203, 14272,
   16268`, `backend_policy.rs:20`, `login_ui.rs:590`, `picker_ui.rs:230, 1017`,
   `region_overlay.rs:27`, `scratch_window.rs:11`) and the definition itself.
   The tray-click host is now `RealVaultOps::open_window`
   (`main.rs:5593`) → `app_window::run_from_vault` (`main.rs:5772`), which
   `main.rs:5670` says in as many words ("The vault FRAME, not
   `vault_window::run`"). **`main.rs:12904` already records this.**

   **This contradicts Task 2 of the plan**, which names "the call site Task 0
   records for `vault_window::run`" as the thing to change. There is no such
   call site. Task 2 must target `RealVaultOps::open_window` /
   `open_vault_window` (`main.rs:6494`) instead, whose four tray-loop callers
   are `main.rs:2086, 2127, 2231, 2774`.

2. **"Blocked the tray" is still true, but the comment at 4758 is about
   something else that was already fixed.** Read in full, 4740–4765 says the
   thing that blocked the tray was *"a scoped background thread joined right
   after `vault_window::run` returned"* — a backend start-up wait of up to
   ~30s. That join is gone; it is now detached through
   `backend_op_tx`/`backend_op_rx`.

   Separately and still live: `open_vault_window` is called **directly from
   the tray event loop body** and `app_window::run_from_vault` blocks that
   thread for the window's entire life. While the vault window is open the
   daemon's loop does not iterate, so tray clicks queue, `stop_backend_if_idle`
   does not run, the update check does not run, and foreground events pile up
   in the channel. `backend_policy.rs:20` states this as a known property.
   The global hotkey survives only because `WM_HOTKEY` goes to the thread's
   message queue and the egui event loop is pumping that same queue.

   **So yes: the split also fixes a real responsiveness defect, and that
   belongs in the record.** The same is true of every other main-thread window
   in the table — Preferences, "Add app…", the login flow, and *every overlay
   card the autofill path opens*.

---

## Step 3 — what cannot leave the daemon

| thing | where | why it cannot move |
| --- | --- | --- |
| tray icon | `tray::build_tray()`, `main.rs:1836` | owns a hidden Win32 window bound to its creating thread; `main.rs:6511` records that `AppTray` is `!Send` |
| global hotkey | `hotkey::register_fill_hotkey()`, `main.rs:1832` | `RegisterHotKey` binds to the calling thread and `WM_HOTKEY` is delivered only to that thread's queue — `hotkey.rs:343-345` states this and is why the first attempt is on the main thread and not the watcher thread. It is also logon-session-wide first-come-first-served (`hotkey.rs:14`), so a UI process that registered it would *steal* the chord from the daemon |
| `bw serve` + its job object | `job_object::KillOnCloseJob::new()` `main.rs:586`; `start_backend(..)` `main.rs:1085` | the orchestrator owns the backend; the job's kill-on-close is what guarantees no decrypted vault is left served on localhost after a forced takeover (`main.rs:279`) |
| match engine | `MatchEngine::new()` `main.rs:838`, `startup_entries` 860 | consulted synchronously by `process_foreground_event` on every foreground change |
| foreground watcher | `window_watch` thread `main.rs:1971`, channel 1970 | feeds the autofill decision; its `last_active_pid` (2020) is daemon-only state |
| `unlock_prompt::ask` | `main.rs:2050, 2895, 3097` | bare Win32, no GL; and it is the recovery for a locked vault mid-fill |
| `picker_prompt::ask` | `app.rs:2605` via `handle_no_match` | bare Win32; the spec's "every surface lives in exactly one renderer" rule |
| single-instance / app mutex | `main.rs:189, 250, 309` | the daemon *is* the instance |
| `away_lock` session notifications | `main.rs:1850` | registered on this process |
| update check + `update_panel::install_env` | `main.rs:772, 1872-1908` | long-lived, and the updater restarts the daemon |
| clipboard clearing timers | `clipboard::configure` `main.rs:391` | **borderline** — see Step 4's note; the copy is *made* in the vault window (a UI process) but the clearing timer must outlive it |

---

## Step 4 — what each egui surface needs before it can draw

**This is the step that overturns the spec's central claim.** The spec says
(§"The UI needs nothing passed to it") that `settings.json`, the DPAPI session
token and `bw_serve::BW_SERVE_PORT` are sufficient. **That is true for exactly
one surface — the vault window — and false for most of the rest.**

Verified per surface:

### ✅ The vault window — the claim holds for *input*

`vault_window::build_frame` needs: `Arc<VaultCache>`, `FillStats`,
`AccountDetails`, `session_token`, `icon_cache_dir`, `AutoLock`,
`backend_already_running: bool`, `Option<AccountsState>`. Every one is
derivable in a fresh process: the cache is refilled from `bw serve` over HTTP
with the token, `FillStats` and the icon cache are files, `AccountDetails` and
`AccountsState` come from `settings.json` + `accounts::resolve_startup`,
`AutoLock` is a setting, and `backend_already_running` is a probe of the
constant port. **Spec claim verified for startup input.**

**But its *output* is not covered by the spec at all.** `VaultWindowResult`
(`vault_window/mod.rs:~300-390`) carries **`locked`, `needs_reauth`,
`edited_settings`, `switch_to: Option<AccountId>`, `add_account`,
`remove_account`** — plus the initial-search handoff. Today
`run_vault_loop`/`open_vault_window` consumes all six *in the daemon*, and
each drives daemon-owned machinery: a lock tears down `bw serve` and clears
the match engine; a switch re-points `BITWARDENCLI_APPDATA_DIR`, re-points the
session store, re-authenticates and rebuilds the engine; `needs_reauth` runs
`reauthenticate`. **A UI process cannot do any of those and the spec provides
no channel for reporting them.** `edited_settings` and a new token can go
through disk, but *"the vault was locked"* and *"switch to account X"* cannot
— the daemon has to act on them, now, and would never learn.

> **This is the finding most likely to change the plan.** The spec's "no IPC,
> no pipe, no rendezvous" is achievable for *launching* a UI but not for the
> vault window's *result*. Some daemon-visible signal is required. The cheapest
> shape consistent with the spec is the one Task 6 already proposes for
> staleness — **the daemon waits on the child handle and re-reads disk on
> exit** — extended to carry a small non-secret result (an exit code, or a
> status field the UI writes to `settings.json` before exiting). That is a
> design decision the plan currently does not contain.

### ❌ Preferences (`prefs_ui::run`)

Signature is `fn run(settings: Settings) -> Settings` — input is fine (disk).
Output is not: `main.rs:2272-2310` feeds the result through
`apply_disk_cache_change` (which can **refuse**, mutating the cache the daemon
owns), then `persist_preferences`, then `clipboard::configure`, then tray and
backend-policy reconciliation. `prefs_ui` also reads
`prefs_ui::publish_account_status` (`main.rs:1952`), a **process-global** the
daemon's prefetch thread (1955) writes; a fresh UI process would show
"Checking" forever unless it runs its own probe.
**Needs more than settings + token + port.**

### ❌ "Add app…" — `picker_ui::run_picker`

Takes `(Arc<VaultCache>, VaultItem, last_active_pid: Option<u32>,
backend_already_running: bool)`. **`last_active_pid` is daemon-only state**
(`main.rs:2020`, maintained from the foreground watcher) and is what pre-selects
the app the user was just using — the whole point of the feature. It is not on
disk and is not derivable in a fresh process launched *from* the tray (by then
the foreground window is the tray). Its sibling `pick_vault_item`
(`main.rs:8350`) is inside `AddAppFlow`, which captures a **`VaultEra`** off the
live `Arc<VaultCache>` specifically so a stale write cannot land — an identity
that does not survive a process boundary. **Needs more.**

### ❌ The overlay cards — `show_prompt_overlay`, `show_locked_overlay`, `show_save_login_overlay`, `show_generate_overlay`

All four need an **anchor position derived from the foreground window's HWND**
(`app.rs:2196`, `2215`, `2841`, via `PromptPresenter::position`), which only the
daemon's `window_watch` has. `show_save_login_overlay` is pre-filled with a
`SaveLoginForm` whose `username` and **`password: Zeroizing<String>`** were
captured from the observed window — a live secret in daemon memory that is on
no disk. And all four *return an action the daemon must act on immediately*:
the chosen `FillChoice` is typed into the foreground window by the daemon's
`Injector`, which must still be the foreground-adjacent process.
**Needs a great deal more, including a secret.**

### ❌ The send preflight — `preflight_host::show_preflight`

Signature: `fn(PreflightState, Zeroizing<String>) -> Option<PreflightAction>` —
**the second argument is the password about to be typed**. It is called from
`app::confirmed_by_preflight` (`app.rs:714/768`), which is on
`fill_from_vault_with`'s path — i.e. **the daemon's autofill path**
(`main.rs:2837` and `3956`). Moving it to a UI process means passing a live
secret across a process boundary, which the spec forbids on a command line and
provides no other channel for. **This surface either stays in the daemon (and
keeps the GL driver loaded there) or is redrawn in Win32.**

### ⚠️ The login flow — `login_ui::run_login_flow_for`

Input is fine. Output — the session token — can go to disk via
`SessionStore::save`, which is exactly the spec's mechanism. **But** at
`main.rs:9166` (`authenticate_for_switch`) the store has *already been
re-pointed at the target account* and `settings.active_account` has **not** been
updated yet, so a fresh UI process reading `settings.json` would unwrap and
overwrite the **wrong account's** `session.bin`. A UI-mode login must be told
which account it is for. The account id is not a secret, so the command line is
legal — but the spec's "the UI reads the rest itself" is false here.

### ⚠️ `app_window::run` / `run_from_working` — the startup window

These are the first-launch surface, and they run *before* a token exists — the
cold branch's whole purpose is to produce one. They also produce
`outcome.prepared`, the `StartupWork` the daemon's estate is built from
(`main.rs:1624-1692`). Splitting these is materially harder than splitting the
tray-click vault window and should not be assumed equivalent.

### Summary against the spec's claim

| surface | settings + DPAPI token + port sufficient? |
| --- | --- |
| vault window (input) | **yes** |
| vault window (result) | **no** — six daemon-actionable outcomes with no channel |
| Preferences | no — result application + a process-global status |
| Add app… (both windows) | no — `last_active_pid`, `VaultEra` |
| overlay prompt / locked / save-login / generator | no — HWND anchor, live secret, injector proximity |
| send preflight | no — takes the password as an argument |
| login flow | no — needs the target account id when switching |
| startup window | no — runs before a token exists, produces the estate |

---

## Regions I could not map confidently

Stated rather than guessed, as instructed:

1. **`park_and_work` (`main.rs:1417`, `5743`) and `run_the_in_window_teardown`
   (`main.rs:5388` region).** I traced the shape — a worker thread plus a
   blocking event loop on the main thread, with an estate "parked" behind an
   `Arc<Mutex<_>>` and channels forwarding the lock's teardown steps — but I did
   **not** verify every path through the park/unpark handoff. Task 3 touches
   this if it changes what the cold-start branch does, and it deserves its own
   read.

2. **The tray event loop's interior, 2117–3101.** I identified the window-opening
   call sites and the daemon-only state, but the ~1,000 lines of menu-event,
   backend-op and foreground dispatch inside it were not mapped statement by
   statement. Nothing I found suggests a UI concern hides there — every window
   it opens is in the Step 2 table — but I cannot claim exhaustiveness.

3. **`run_vault_loop` (`main.rs:~6494` onward, via `VaultOps`).** I read its
   contract (`VaultFollowUp`, `ResettleOutcome`, `RealVaultOps`'s three methods)
   but not each arm. This is the code Task 2 rewrites, and its `SessionEstate`
   single-owner discipline (`main.rs:13696` pins that a method returning a
   *different* estate silently kills autofill and leaks the `bw serve` port) is
   a hazard an implementer must read directly rather than trust this note for.

4. **Whether any transitive dependency of a "daemon-only" module creates a GL
   context on a path nobody takes.** Task 7 Step 1 already calls for a source
   pin over this; I did not attempt it here.

---

## Things that contradict the spec or the plan

Stated plainly, not softened:

1. **`vault_window::run` is dead code.** Task 2's named target does not exist as
   a call site. Use `open_vault_window` (`main.rs:6494`) and its four tray-loop
   callers (2086, 2127, 2231, 2774), or `RealVaultOps::open_window`
   (`main.rs:5593`).

2. **The spec's "the UI needs nothing passed to it" is false for six of the
   eight surfaces**, and false for the vault window's *result* even though it
   holds for the vault window's *input*. See Step 4. Some daemon-visible return
   channel is required; the plan has no task for it.

3. **The daemon's autofill path opens five egui windows today** — the prompt
   overlay, the locked card, the save-login form, the generator and the send
   preflight. The spec lists only the save-login form and the generator as UI
   surfaces, and describes the daemon's cards as "bare-Win32: the unlock prompt
   and the account picker". That is only two of seven. **On today's code a
   `--autostart` daemon loads the OpenGL driver the first time the user presses
   `CTRL+ALT+B` on an app with more than one credential, or on any locked-vault
   fill.** If Task 7 stops at "Preferences, save-login, generator, sequence
   editor, rehearsal, login flow", the 11.3 MB steady state does not survive
   first use, and the plan's justification evaporates.

4. **The send preflight cannot move without passing a password across a process
   boundary.** Its signature takes `Zeroizing<String>`. This is a direct
   collision between the spec's "nothing secret may ever go on a command line"
   and its "every surface lives in exactly one renderer": the preflight must be
   redrawn in Win32 (a Task the plan does not have), or the daemon keeps GL.

5. **The plan's "roughly 1,200 lines" of interleaving is closer to 670**
   (`main.rs:1036–1706`). Minor, but the mixed region is more concentrated and
   more tangled than a uniform 1,200 lines would be.

6. **Every main-thread window blocks the tray loop, not just the vault window.**
   Preferences, "Add app…", the login flow and all four overlay cards do too.
   The responsiveness win from this split is larger than the plan claims.

---

## Bug noticed, recorded not fixed

`main.rs:9166` `authenticate_for_switch` relies on `SessionStore` having been
re-pointed before it runs. That is correct *in-process* and documented at
`main.rs:9161-9166`. It becomes a **wrong-account token overwrite** the moment
the login flow runs in a separate process that re-derives the store from
`settings.active_account`, because `active_account` still names the account
being left. Task 3/Task 7 must not port this call naively.
