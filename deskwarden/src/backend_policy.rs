//! When `bw serve` should be running.
//!
//! A pure function so the policy is table-testable rather than something
//! that has to be verified by opening windows. The lifecycle itself lives
//! in `main`; this only decides.

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
pub fn should_run(keep_backend_running: bool) -> bool {
    keep_backend_running
}

#[cfg(test)]
mod tests {
    use super::should_run;

    #[test]
    fn keeping_it_running_says_yes() {
        assert!(should_run(true));
    }

    #[test]
    fn saving_memory_says_no_at_idle() {
        assert!(!should_run(false));
    }
}
