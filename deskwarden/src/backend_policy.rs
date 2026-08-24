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
/// # It does not yet know about [`choose`]
///
/// When [`choose`] answers [`VaultBackendChoice::DirectRest`] there is no
/// `bw serve` to run at all, and the honest signature for this function then
/// takes the choice and answers `false` unconditionally. It deliberately does
/// **not**, yet, and the reason is that a half-wired startup is worse than an
/// unwired one: nothing in this crate constructs a
/// [`crate::rest::backend::RestBackend`], so a `settings.json` that already
/// carried `use_official_bw_crypto: false` would stop `bw serve` from being
/// started and put nothing at all in its place. The parameter is added by
/// whoever wires the construction, in the same change.
pub fn should_run(keep_backend_running: bool) -> bool {
    keep_backend_running
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

#[cfg(test)]
mod tests {
    use super::{choose, is_self_hosted, should_run, VaultBackendChoice};

    #[test]
    fn keeping_it_running_says_yes() {
        assert!(should_run(true));
    }

    #[test]
    fn saving_memory_says_no_at_idle() {
        assert!(!should_run(false));
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
