//! Which backend holds the vault, and when `bw serve` should be running.
//!
//! Pure functions, so the policy is table-testable rather than something that
//! has to be verified by opening windows or by watching a subprocess. The
//! lifecycles themselves live in `main`; this only decides.

/// Whether the backend should be running right now.
///
/// With `keep_backend_running` the answer is always yes -- today's
/// behaviour, everything instant, ~111 MB held at idle. Without it, the
/// backend is only started for the operations that genuinely need it
/// (`open_vault_window`, the tray's Sync item, "Add app...") and torn back
/// down again once idle (`stop_backend_if_idle`, called from `main`'s own
/// loop). Reads are served by `VaultCache` either way, so autofill is
/// unaffected and idle -- the state that lasts hours -- costs nothing in
/// save-memory mode.
///
/// This used to also take a `vault_window_open` parameter ("keep it running
/// this instant even without the setting, because a window needs it"), but
/// `vault_window::run` blocks the main loop for as long as the window is
/// open -- so by the time this function's one call site (`main`'s idle
/// reconciliation) can run at all, no window can possibly still be open, and
/// the parameter was always `false`. Removed rather than kept "for a future
/// non-blocking window": nothing here is building one, and a parameter
/// nothing can ever set is worse than no parameter, since it reads as live
/// policy that isn't.
///
/// # It knows about [`choose`] now, and the parameter is not optional
///
/// This used to take `keep_backend_running` alone, with a note saying the
/// honest signature takes the choice as well and that adding it before
/// anything constructed a [`crate::rest::backend::RestBackend`] would be a
/// half-wired startup. That construction is wired, so the parameter is here.
///
/// `DirectRest` answers `false` **whatever the setting says**, and the order
/// of the two tests below is the whole of that: `keep_backend_running` is a
/// memory/latency trade about a subprocess this account does not have, and a
/// `true` there must not be able to start one. It is the same answer
/// [`bw_serve_is_selected`] gives the eleven entry points that start the
/// backend; this is the twelfth-and-a-half, the one that stops it.
pub fn should_run(choice: VaultBackendChoice, keep_backend_running: bool) -> bool {
    match choice {
        VaultBackendChoice::DirectRest => false,
        VaultBackendChoice::BwServe => keep_backend_running,
    }
}

// ---- which backend holds the vault -----------------------------------------

/// Which of the two [`crate::vault_backend::VaultBackend`] implementations an
/// account's vault is served by.
///
/// Two variants and not three: there is no "either" and no "not decided yet".
/// The worst state available on this branch is one account with both backends
/// live -- reads answered by a `bw serve` the app forgot to stop, writes sent
/// straight to the server, and a [`crate::vault_cache::VaultCache`] holding
/// whichever answered first -- so the decision is a total function of two
/// inputs and every caller gets the same answer out of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultBackendChoice {
    /// [`crate::vault_bridge::VaultBridge`] against the `bw serve`
    /// subprocess. `bw` holds the master password and the keys derived from
    /// it; this process holds only a session token, and one that expires.
    BwServe,
    /// [`crate::rest::backend::RestBackend`] against the Bitwarden server
    /// directly. No subprocess -- and **this process holds the user's master
    /// key**, which does not expire. See [`crate::user_key_store`] for what
    /// that means at rest.
    DirectRest,
}

/// Whether `server_url` is *positively* a self-hosted server.
///
/// `None`, an empty string, and anything whose host cannot be read are all
/// **false** -- unknown counts as official. That direction is the owner's
/// rule ("setting should be disabled if not self-hosted vault to avoid issues
/// with Bitwarden") and it is the safe one: guessing "self-hosted" wrongly
/// points the direct-REST backend at bitwarden.com, which is the case this
/// branch is explicitly not trying to serve; guessing "official" wrongly only
/// leaves a self-hoster on the `bw serve` path they already have.
///
/// The host test is [`crate::favicon::bitwarden_cloud`], reached through
/// [`crate::favicon::host_from_url`], and it is *reached* rather than
/// repeated: that function's own doc says "one host test, two callers -- a
/// second copy is how the two come to disagree". This is the third caller,
/// not a fourth copy. In particular it is an exact-or-suffix host match and
/// never a substring, so the unrelated self-hosted domain
/// `vault.bitwarden.community` is self-hosted here too.
#[must_use]
pub fn is_self_hosted(server_url: Option<&str>) -> bool {
    let Some(url) = server_url else {
        // `accounts::Account::server_url` is `None` for bitwarden.com by
        // definition, not for "not known yet".
        return false;
    };
    let host = crate::favicon::host_from_url(url);
    !host.is_empty() && crate::favicon::bitwarden_cloud(&host).is_none()
}

/// Which backend an account gets, from its server URL and one setting.
///
/// **Direct REST needs both halves**, and the two halves are not the same
/// question:
///
/// * [`is_self_hosted`] is about the *server*. Official Bitwarden is out of
///   scope for this backend, and so is a server this app cannot identify.
/// * `!use_official_bw_crypto` is about the *user's choice*. That setting is
///   on by default, so an account that has never opened Preferences -- and a
///   `settings.json` written before the key existed, which loads as `true` --
///   stays on `bw serve`.
///
/// A pure function of exactly those two values, with no `Settings`, no
/// `Account` and no I/O in the signature, so the table in the tests below can
/// walk every combination without a server, a subprocess or a file. Same
/// shape as [`should_run`] above and as `app::disposition`.
///
/// **Nothing calls this yet.** The startup path that would act on it is not
/// written; see [`should_run`]'s own note for why that is deliberate rather
/// than forgotten.
#[must_use]
pub fn choose(server_url: Option<&str>, use_official_bw_crypto: bool) -> VaultBackendChoice {
    if is_self_hosted(server_url) && !use_official_bw_crypto {
        VaultBackendChoice::DirectRest
    } else {
        VaultBackendChoice::BwServe
    }
}

// ---- the choice, as a fact about this process ------------------------------

/// Everything this process needs to act on [`choose`]'s answer.
///
/// # Why a published environment and not twelve parameters
///
/// `bw serve` has **twelve reachable entry points** (see
/// [`selected`]'s own doc for the enumeration and for how it was proved to be
/// all of them), spread over startup's two arms, the readiness recovery, the
/// lock and re-auth recoveries, the tray's Sync item, the vault window's, the
/// "Add app..." fallback and the account switch. They do not share a call
/// stack, they do not share a struct, and several of them run on detached
/// worker threads that could not borrow a `main` local if they wanted to.
///
/// Threading the choice to all twelve is how eleven of them get it and the
/// twelfth does not -- which is precisely the "reads from one backend, writes
/// to the other" state this whole branch exists to avoid, and it would arrive
/// silently. So the choice is a **fact about the process**, established once,
/// read by the one function every one of the twelve funnels through, and
/// pinned by a test that reads this crate's source rather than by a list.
///
/// It is installed by `main` before the first window, which is not a
/// convention here but a rule with a test behind it:
/// `every_published_environment_is_installed_before_the_first_window` finds
/// this module by its `install_env` entry point, exactly as it finds
/// [`crate::update_panel`] and [`crate::breach_scan`], and fails if `main`
/// installs it late or not at all.
///
/// # It is replaceable, and the others are not
///
/// [`crate::update_panel`] and [`crate::breach_scan`] use `OnceLock`: what
/// they publish is a fact about the *build* and the *machine*. This is a fact
/// about the **account**, and the account changes without the process
/// restarting -- `main`'s `switch_to_account` re-points the profile directory,
/// the session store and the vault, and the backend choice has to move with
/// them or a switch from a self-hosted account to a bitwarden.com one would
/// leave `bw serve` gated off with nothing serving the vault.
///
/// The *setting* half of the choice is a different matter and does not move:
/// changing `use_official_bw_crypto` takes effect on the next launch, and
/// Preferences says so. See [`crate::prefs_ui`]'s row for why.
pub struct BackendEnv {
    /// [`choose`]'s answer for the active account.
    pub choice: VaultBackendChoice,
    /// `Some` exactly when `choice` is [`VaultBackendChoice::DirectRest`].
    ///
    /// The pairing is checked by [`install_env`] rather than trusted: an
    /// environment that said `DirectRest` with no way to log in would gate
    /// `bw serve` off and put nothing in its place, which is the one outcome
    /// this module's own docs have called worse than not shipping at all.
    pub direct: Option<crate::login_ui::DirectRestLogin>,
}

/// The installed environment.
///
/// A `RwLock` rather than a `OnceLock` for the reason in [`BackendEnv`]'s doc:
/// an account switch replaces it. Poisoning is recovered from rather than
/// propagated -- what is guarded is a choice and a `fn` pointer, not a
/// half-updated invariant, and an `unwrap` here would be an `unwrap` on the
/// path that decides whether a subprocess starts.
static ENV: std::sync::RwLock<Option<BackendEnv>> = std::sync::RwLock::new(None);

/// Installs (or replaces) the process-wide [`BackendEnv`].
///
/// Returns `false` and installs **nothing** when the environment is not
/// self-consistent -- `DirectRest` with no [`crate::login_ui::DirectRestLogin`]
/// -- because the safe direction on a malformed choice is the one that leaves
/// `bw serve` running, which is what the previous, un-wired state did.
///
/// Called from `main` at startup and again on every account switch. A test
/// that installs one **must** put it back (see [`uninstall_env`]): this is
/// process-wide state in a suite that runs its tests in parallel, and a stale
/// `DirectRest` left behind would gate `bw serve` off for every later test in
/// the same process.
pub fn install_env(env: BackendEnv) -> bool {
    if env.choice == VaultBackendChoice::DirectRest && env.direct.is_none() {
        log::error!(
            "refusing to select the direct-REST vault backend with no way to log in to it; \
             staying on `bw serve`"
        );
        return false;
    }
    match ENV.write() {
        Ok(mut slot) => *slot = Some(env),
        Err(poisoned) => *poisoned.into_inner() = Some(env),
    }
    true
}

/// Removes the installed environment, putting this process back on
/// `bw serve`.
///
/// Exists for the tests that install one, and for the account switch's
/// rollback, which has to be able to say "no direct-REST account is active"
/// as distinctly as it can say which one is.
pub fn uninstall_env() {
    match ENV.write() {
        Ok(mut slot) => *slot = None,
        Err(poisoned) => *poisoned.into_inner() = None,
    }
}

fn with_env<T>(f: impl FnOnce(Option<&BackendEnv>) -> T) -> T {
    match ENV.read() {
        Ok(slot) => f(slot.as_ref()),
        Err(poisoned) => f(poisoned.into_inner().as_ref()),
    }
}

/// **Which backend this process is serving the active account's vault from,
/// and therefore whether `bw serve` may start.**
///
/// [`VaultBackendChoice::BwServe`] when nothing has been installed, which is
/// every test process, `examples/ui_preview`, and any launch whose account is
/// not both self-hosted and opted in. That default is the direction this
/// module's [`is_self_hosted`] already argues for: guessing "`bw serve`"
/// wrongly leaves a user on the path they already had.
///
/// # The twelve entry points, and how they were found
///
/// `bw serve` is spawned by exactly one expression in this crate --
/// `bw_serve::bw_serve_command`, whose one production caller is `main`'s
/// `try_start_backend` -- and **twelve call paths reach that function**:
///
///  1. `main`'s cached-session arm, through `start_backend`.
///  2. the startup window's worker (`app_window::run_from_working`'s start
///     closure), through `try_start_backend`.
///  3. `spawn_backend_start`, from the tray's Sync item.
///  4. `spawn_backend_start`, from the vault window's Sync.
///  5. `apply_backend_op`'s in-line start for "Add app...".
///  6. `spawn_sync`'s start-if-not-running arm.
///  7. `restart_backend_after_unlock`, from the away-lock recovery.
///  8. `open_vault_window`'s lock/re-auth recovery.
///  9. `recover_from_failed_vault_wait`, at startup.
/// 10. `reauthenticate`'s restart, after a fresh master password.
/// 11. the account switch's resettle.
/// 12. the startup worker's orphan-adoption retry.
///
/// **The list is not what is trusted.** It is written down because a reader
/// deserves to know what was counted, but the guard is
/// `no_path_reaches_bw_serve_without_consulting_the_policy` in `main.rs`,
/// which reads this crate's own source, finds every caller of
/// `bw_serve_command` and of `try_start_backend`, and fails if any of them can
/// reach a spawn without this function's answer being between it and the
/// process. A thirteenth added next year is covered on the day it is written;
/// a list would not be.
#[must_use]
pub fn selected() -> VaultBackendChoice {
    with_env(|env| env.map_or(VaultBackendChoice::BwServe, |env| env.choice))
}

/// Whether `bw serve` is the backend for the active account.
///
/// The same answer as [`selected`], as the boolean the call sites want, so
/// that no caller writes its own comparison against a variant and no caller
/// gets the sense of it backwards.
#[must_use]
pub fn bw_serve_is_selected() -> bool {
    selected() == VaultBackendChoice::BwServe
}

/// The direct-REST login for the active account, for
/// [`crate::login_ui`]'s sign-in worker.
///
/// `None` on every `bw serve` account and in every process with no
/// environment installed, which is what makes the sign-in path byte-for-byte
/// what it was before this branch on those accounts.
#[must_use]
pub fn direct_rest_login() -> Option<crate::login_ui::DirectRestLogin> {
    with_env(|env| {
        // **Gated on the choice and not merely on the field.** The two are
        // kept in step by `install_env`, so this cannot differ from
        // `env.direct` today -- which is the point: it is the sign-in worker
        // that reads this, and a login handed out for an account the app
        // decided to serve through `bw serve` would derive and store a master
        // key nothing was going to use. One test asserts the pairing from
        // this side so that a later `install_env` which relaxed it would be
        // caught here rather than at a `userkey.bin` written for no reason.
        env.filter(|env| env.choice == VaultBackendChoice::DirectRest)
            .and_then(|env| env.direct.clone())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    // ---- the published environment -----------------------------------------

    /// **One test at a time in here.**
    ///
    /// [`ENV`] is process-wide and this suite runs its tests in parallel, so
    /// two tests installing environments at once would each see the other's.
    /// Every test below takes this and puts the environment back on its way
    /// out, so a stale `DirectRest` cannot gate `bw serve` off for a later
    /// test in the same process.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn hold_the_env_lock() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A direct-REST login with no server and no network behind it: the `fn`
    /// pointer refuses, and nothing here can call it anyway.
    fn a_login() -> crate::login_ui::DirectRestLogin {
        fn never(
            _server_url: &str,
            _email: &str,
            _device_id: &str,
            _password: &[u8],
        ) -> Result<crate::rest::api::Authenticated, String> {
            Err("this fixture never logs in".to_string())
        }
        crate::login_ui::DirectRestLogin {
            server_url: "https://vault.example.com".to_string(),
            email: "someone@example.com".to_string(),
            device_id: "00000000-0000-0000-0000-000000000000".to_string(),
            authenticate: never,
            adopt: std::sync::Arc::new(|_authenticated| {}),
        }
    }

    /// **The default is `bw serve`, and every process that installs nothing
    /// gets it.** That is every test process, `examples/ui_preview`, and any
    /// launch whose account is not both self-hosted and opted in.
    #[test]
    fn nothing_installed_means_bw_serve() {
        let _guard = hold_the_env_lock();
        uninstall_env();
        assert_eq!(selected(), VaultBackendChoice::BwServe);
        assert!(bw_serve_is_selected());
        assert!(direct_rest_login().is_none());
    }

    #[test]
    fn installing_direct_rest_gates_bw_serve_off_and_publishes_the_login() {
        let _guard = hold_the_env_lock();
        assert!(install_env(BackendEnv {
            choice: VaultBackendChoice::DirectRest,
            direct: Some(a_login()),
        }));
        assert_eq!(selected(), VaultBackendChoice::DirectRest);
        assert!(!bw_serve_is_selected(), "`bw serve` is still selected on a direct-REST account");
        let login = direct_rest_login().expect("the login window has nothing to derive with");
        assert_eq!(login.email, "someone@example.com");
        assert_eq!(login.server_url, "https://vault.example.com");
        uninstall_env();
        assert!(bw_serve_is_selected(), "the environment outlived the test that installed it");
    }

    /// **A switch replaces the environment rather than being refused.**
    ///
    /// The one way this differs from `update_panel`'s and `breach_scan`'s
    /// `OnceLock`s, and the reason: those publish facts about the build and
    /// the machine, and this publishes a fact about the *account*, which
    /// changes without the process restarting.
    #[test]
    fn an_account_switch_replaces_the_environment_in_both_directions() {
        let _guard = hold_the_env_lock();
        assert!(install_env(BackendEnv {
            choice: VaultBackendChoice::DirectRest,
            direct: Some(a_login()),
        }));
        assert_eq!(selected(), VaultBackendChoice::DirectRest);

        // Switching to a `bw serve` account.
        assert!(install_env(BackendEnv {
            choice: VaultBackendChoice::BwServe,
            direct: None,
        }));
        assert_eq!(selected(), VaultBackendChoice::BwServe);
        assert!(
            direct_rest_login().is_none(),
            "the previous account's direct-REST login survived the switch, so a master \
             password typed for THIS account would derive a key against the other one's server"
        );

        // And back again.
        assert!(install_env(BackendEnv {
            choice: VaultBackendChoice::DirectRest,
            direct: Some(a_login()),
        }));
        assert_eq!(selected(), VaultBackendChoice::DirectRest);
        uninstall_env();
    }

    /// **A self-inconsistent environment is refused, and refused whole.**
    ///
    /// `DirectRest` with no way to log in would gate `bw serve` off and put
    /// nothing in its place, which is the one outcome this module's own docs
    /// call worse than not shipping the feature. The safe direction on a
    /// malformed choice is the one that leaves `bw serve` running.
    #[test]
    fn direct_rest_without_a_login_is_refused_and_changes_nothing() {
        let _guard = hold_the_env_lock();
        uninstall_env();
        assert!(!install_env(BackendEnv {
            choice: VaultBackendChoice::DirectRest,
            direct: None,
        }));
        assert_eq!(
            selected(),
            VaultBackendChoice::BwServe,
            "a `DirectRest` choice with no login was installed anyway, so this process now \
             starts no `bw serve` and has nothing to serve the vault with"
        );
        assert!(direct_rest_login().is_none());

        // The positive control: the same call WITH a login is accepted, so
        // the refusal above is about the missing half and not about
        // `install_env` refusing everything.
        assert!(install_env(BackendEnv {
            choice: VaultBackendChoice::DirectRest,
            direct: Some(a_login()),
        }));
        assert_eq!(selected(), VaultBackendChoice::DirectRest);
        uninstall_env();
    }

    /// **A `BwServe` environment hands out no login, even carrying one.**
    ///
    /// `install_env` keeps the two in step, so this state is unreachable from
    /// production today. It is asserted from the other side anyway: it is the
    /// sign-in worker that reads `direct_rest_login`, and a login handed out
    /// for an account this process decided to serve through `bw serve` would
    /// derive and store a master key nothing was going to use.
    #[test]
    fn a_bw_serve_environment_hands_out_no_login_even_if_one_is_attached() {
        let _guard = hold_the_env_lock();
        assert!(install_env(BackendEnv {
            choice: VaultBackendChoice::BwServe,
            direct: Some(a_login()),
        }));
        assert!(bw_serve_is_selected());
        assert!(
            direct_rest_login().is_none(),
            "a login was handed to the sign-in worker for an account this process is serving \
             through `bw serve`"
        );

        // The positive control: the same login under the choice that selects
        // it IS handed out, so the assertion above is about the choice and
        // not about a function that answers `None` to everything.
        assert!(install_env(BackendEnv {
            choice: VaultBackendChoice::DirectRest,
            direct: Some(a_login()),
        }));
        assert!(direct_rest_login().is_some());
        uninstall_env();
    }


    #[test]
    fn keeping_it_running_says_yes() {
        assert!(should_run(VaultBackendChoice::BwServe, true));
    }

    #[test]
    fn saving_memory_says_no_at_idle() {
        assert!(!should_run(VaultBackendChoice::BwServe, false));
    }

    /// The arm this parameter was added for: an account served over direct
    /// REST has no `bw serve`, and `keep_backend_running` -- a trade about a
    /// subprocess that does not exist -- cannot conjure one.
    #[test]
    fn direct_rest_never_runs_the_backend_however_the_setting_is_set() {
        assert!(!should_run(VaultBackendChoice::DirectRest, true));
        assert!(!should_run(VaultBackendChoice::DirectRest, false));
    }

    // ---- is_self_hosted ----------------------------------------------------

    #[test]
    fn no_server_url_is_the_default_cloud_and_so_not_self_hosted() {
        assert!(!is_self_hosted(None));
    }

    #[test]
    fn both_official_clouds_and_their_subdomains_are_not_self_hosted() {
        for url in [
            "https://bitwarden.com",
            "https://vault.bitwarden.com",
            "https://vault.bitwarden.com/",
            "https://bitwarden.eu",
            "https://vault.bitwarden.eu",
            "http://vault.bitwarden.com:8443/api",
        ] {
            assert!(!is_self_hosted(Some(url)), "{url} is an official cloud");
        }
    }

    #[test]
    fn a_self_hosted_url_is_self_hosted() {
        for url in [
            "https://vault.example.com",
            "https://vault.example.com/",
            "https://vault.example.com:8443",
            "http://localhost:8080",
            "https://bw.napps.pw",
        ] {
            assert!(is_self_hosted(Some(url)), "{url} is self-hosted");
        }
    }

    /// The substring trap `favicon::bitwarden_cloud` exists to avoid,
    /// asserted again from this side: `vault.bitwarden.community` *contains*
    /// `bitwarden.com` and is somebody else's domain entirely.
    #[test]
    fn a_domain_merely_containing_bitwarden_com_is_still_self_hosted() {
        assert!(is_self_hosted(Some("https://vault.bitwarden.community")));
        assert!(is_self_hosted(Some("https://bitwarden.com.evil.example")));
    }

    /// Empty, blank and hostless strings are "unknown", and unknown counts as
    /// official. This is the arm that keeps a garbled or half-written
    /// `settings.json` from pointing the direct-REST backend somewhere nobody
    /// chose.
    #[test]
    fn an_unreadable_server_url_counts_as_official() {
        for url in ["", "   ", "https://", "http://", "/just/a/path", "?query"] {
            assert!(!is_self_hosted(Some(url)), "{url:?} should count as official");
        }
    }

    // ---- choose ------------------------------------------------------------

    /// Every combination of the two inputs, as one table. Seven of the eight
    /// rows are `BwServe`; the single `DirectRest` row is the whole of what
    /// this branch turns on.
    #[test]
    fn the_whole_decision_table() {
        let self_hosted = Some("https://vault.example.com");
        let official = Some("https://vault.bitwarden.com");
        let unknown = Some("");

        // Setting ON (the default): always `bw serve`, whatever the server.
        assert_eq!(choose(self_hosted, true), VaultBackendChoice::BwServe);
        assert_eq!(choose(official, true), VaultBackendChoice::BwServe);
        assert_eq!(choose(unknown, true), VaultBackendChoice::BwServe);
        assert_eq!(choose(None, true), VaultBackendChoice::BwServe);

        // Setting OFF: only a positively self-hosted server switches.
        assert_eq!(choose(self_hosted, false), VaultBackendChoice::DirectRest);
        assert_eq!(choose(official, false), VaultBackendChoice::BwServe);
        assert_eq!(choose(unknown, false), VaultBackendChoice::BwServe);
        assert_eq!(choose(None, false), VaultBackendChoice::BwServe);
    }

    /// The shipped default, spelled out here rather than left to be inferred:
    /// a fresh install is on `bw serve` even on a self-hosted server, because
    /// the setting it would need is on.
    #[test]
    fn the_shipped_default_never_selects_rest() {
        let default_setting = crate::settings::Settings::default().use_official_bw_crypto;
        assert!(default_setting);
        assert_eq!(
            choose(Some("https://vault.example.com"), default_setting),
            VaultBackendChoice::BwServe
        );
    }
}
