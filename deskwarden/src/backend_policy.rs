//! When `bw serve` should be running.
//!
//! A pure function so the policy is table-testable rather than something
//! that has to be verified by opening windows. The lifecycle itself lives
//! in `main`; this only decides.

/// Whether the backend should be running right now.
///
/// With `keep_backend_running` the answer is always yes -- today's
/// behaviour, everything instant, ~111 MB held at idle.
///
/// Without it the backend runs only while the vault window is open. That is
/// deliberately *not* "per operation": TOTP polls once a second and writes
/// are frequent while the window is open, so tearing down between them
/// would be pathological. Reads are served by `VaultCache`, so autofill is
/// unaffected either way and idle -- the state that lasts hours -- costs
/// nothing.
pub fn should_run(keep_backend_running: bool, vault_window_open: bool) -> bool {
    keep_backend_running || vault_window_open
}

#[cfg(test)]
mod tests {
    use super::should_run;

    #[test]
    fn keeping_it_running_ignores_whether_a_window_is_open() {
        assert!(should_run(true, false));
        assert!(should_run(true, true));
    }

    #[test]
    fn saving_memory_ties_the_backend_to_the_vault_window() {
        assert!(should_run(false, true));
        assert!(!should_run(false, false));
    }

    #[test]
    fn the_only_state_that_stops_the_backend_is_idle_while_saving_memory() {
        // Spelled out as a table so a future change that accidentally makes
        // the default mode shut down is a failing test, not a surprise.
        let cases = [
            ((true, true), true),
            ((true, false), true),
            ((false, true), true),
            ((false, false), false),
        ];
        for ((keep, open), expected) in cases {
            assert_eq!(
                should_run(keep, open),
                expected,
                "keep_backend_running={keep}, vault_window_open={open}"
            );
        }
    }
}
