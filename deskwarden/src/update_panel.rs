//! The update flow the Updates page drives, and the thread it runs on.
//!
//! **It was the About page's until the Preferences window was reorganised.**
//! Nothing in this module changed for that move -- not a stage, not a
//! message, not a thread. What changed is which `prefs_ui` page calls
//! [`UpdatePanel::pump`], and the answer to "what if the user navigates away
//! mid-download" is unchanged with it: the channel is unbounded, the worker
//! never blocks on a full queue, and everything it sent while nobody was
//! draining is drained in one frame on the way back. See [`UpdatePanel::pump`].
//!
//! # Why this is not in `main.rs`
//!
//! It used to be. `main.rs`'s loop owned an `available_update`, an
//! `update_in_progress` and two channels, and the only control was a tray item
//! that was created disabled, with the label "Update available" already on it.
//! So for the whole life of nearly every session the tray asserted that an
//! update was available and then refused to be clicked, because the label was
//! written at build time and the *enabling* was what the check did. That item
//! is gone; this is what replaced it.
//!
//! # Why it owns its own thread and channel
//!
//! `prefs_ui::run` blocks. It pumps its own event loop (`run_ui_native`) and
//! does not return until the window closes, so for as long as the Updates
//! page is on screen `main.rs`'s loop is not running: it is not calling `try_recv`
//! on anything and it is not spawning anything. A check or a download driven
//! from that page therefore cannot be main's work reported back to main. It
//! has to be work this page starts and this page collects.
//!
//! So an [`UpdatePanel`] holds a `Receiver` and nothing else does. Every frame
//! the Updates page draws, it calls [`UpdatePanel::pump`], which drains whatever
//! has arrived without ever blocking, and asks for another frame while
//! anything is still in flight. Nothing here joins a thread, and nothing here
//! waits: a frame that waited would freeze the window it is trying to report
//! into.
//!
//! **`prefs_ui::run`'s blocking contract is untouched.** That was worth
//! preserving rather than working around -- it is the same contract
//! `open_vault_window` and `picker_ui::run_picker` have, and `main.rs`'s
//! handling of the returned settings depends on it. A per-panel channel
//! polled from the draw needs nothing from it.
//!
//! The same shape serves the preferences *modal* over the vault window for
//! free, because that modal draws the same `draw_updates` inside the vault
//! window's own blocking loop. One flow, two shells, no duplication.
//!
//! **Both shells reach this the same way, and the move did not change that.**
//! `prefs_ui::run` and `vault_window::build_frame` both build a `PrefsState`
//! and both call `draw_prefs_body`, which dispatches on the state's own
//! section; neither shell knows which pages exist. So a section moving from
//! one place in `Section::ALL` to another is invisible to both -- there is no
//! shell-side list of pages to fall out of step.
//!
//! # Why the environment is installed once rather than passed in
//!
//! Everything the flow needs beyond the user's click -- this build's version,
//! the releases API base, the directory downloads go to, and the teardown to
//! run before the process is replaced -- is a fact about the *process*, fixed
//! at startup and identical at both shells. Passing it in would mean threading
//! it through `prefs_ui::run`, `PrefsState::new`, and
//! `vault_window::build_frame`'s closure, which is the parameter list two
//! call sites away from the thing that uses it.
//!
//! It is therefore installed once, by `main.rs`, into [`install_env`]. The
//! cost is honest and worth naming: a global is invisible at the call site,
//! and anything that fails to install it gets a panel that says so instead of
//! a panel that works. That is why [`UpdateStage::Unavailable`] exists as a
//! state the page can render, rather than being an `unwrap`.

use crate::updater::{self, ReleaseInfo};
use semver::Version;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, OnceLock};

/// What the update flow needs from the process it is running in.
///
/// Installed once by `main.rs` (see [`install_env`]) rather than passed down,
/// for the reason in this module's header.
pub struct UpdateEnv {
    /// The version this build is, which the check compares the release
    /// against. Not read from `CARGO_PKG_VERSION` here on purpose: `main.rs`
    /// already parses it once, and two parses of one string are two things
    /// that can disagree.
    pub current_version: Version,
    /// Base URL of the releases API. A field, not a constant, because
    /// `main.rs` owns the real one and because a test drives this against
    /// `mockito`'s loopback server.
    pub api_base: String,
    /// Where a downloaded installer is written. The same cache subdirectory
    /// `updater::cleanup_stale_downloads` sweeps at startup, so an abandoned
    /// download is cleaned up by machinery that already exists.
    pub download_dir: PathBuf,
    /// Run immediately before the installer is launched and this process
    /// exits.
    ///
    /// `main.rs` supplies the teardown that path needs -- clearing the
    /// decrypted vault cache and taking any copied secret back off the
    /// clipboard -- because this module has no business knowing what a vault
    /// cache is. `bw serve` is deliberately NOT in it: it is assigned to a
    /// kill-on-close job object (`job_object`), so the kernel terminates it
    /// when this process dies for any reason, `process::exit` included.
    pub before_install: Arc<dyn Fn() + Send + Sync>,
}

static ENV: OnceLock<UpdateEnv> = OnceLock::new();

/// Installs the process-wide [`UpdateEnv`]. Returns `false` if one was already
/// installed, in which case the new one is dropped and the first one stands.
///
/// Called once, early in `main.rs`. Never called by a test that would then
/// leave a second test's check pointed at a stale base URL -- a `OnceLock` in
/// a shared test process is exactly that hazard, which is why every test in
/// this module drives the state machine through [`UpdatePanel::apply`]
/// instead, and touches neither this nor the network.
pub fn install_env(env: UpdateEnv) -> bool {
    ENV.set(env).is_ok()
}

/// The installed environment, or `None` where nothing installed one -- the
/// screenshot example (`examples/ui_preview`), and any test.
pub fn env() -> Option<&'static UpdateEnv> {
    ENV.get()
}

/// What the Updates page is showing about updates.
///
/// One enum rather than a set of booleans, because the states are exclusive
/// and the previous shape's defect was precisely two independent flags
/// (`available_update` and `update_in_progress`) that could describe a
/// situation neither of them meant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateStage {
    /// Nothing has been asked for yet this session. The page offers the
    /// button and claims nothing.
    Idle,
    /// A check is in flight.
    Checking,
    /// A check came back and this build is the latest.
    ///
    /// **This is the state the tray could never show**, and its absence is the
    /// whole reason for this work: the old item's answer to "is there an
    /// update" was a permanently disabled item labelled "Update available".
    UpToDate,
    /// A newer release exists. Carries its notes, which the page renders as
    /// plain text through [`updater::release_notes_for_display`].
    Available(ReleaseInfo),
    /// The installer is being fetched. `done`/`total` are bytes, `total` being
    /// `None` for a server that declared no length.
    Downloading {
        release: ReleaseInfo,
        done: u64,
        total: Option<u64>,
    },
    /// Downloaded, and its SHA-256 matched the digest the releases API
    /// published for the asset. The page now offers the restart.
    Ready(ReleaseInfo),
    /// Something failed. The message is for the user, and the release (if the
    /// failure happened after one was found) is kept so the retry has
    /// something to retry.
    Failed {
        message: String,
        release: Option<ReleaseInfo>,
    },
    /// No [`UpdateEnv`] was installed, so this build cannot check. Not
    /// reachable in the shipped app -- `main.rs` installs one before any
    /// window opens -- and rendered honestly rather than papered over,
    /// because a button that silently does nothing is the defect this whole
    /// change is about.
    Unavailable,
}

/// What a worker thread reports back.
#[derive(Debug)]
enum UpdateMsg {
    /// A check finished. `Ok(None)` means "already current".
    Checked(Result<Option<ReleaseInfo>, String>),
    /// Bytes so far, and the declared total if the server gave one.
    Progress(u64, Option<u64>),
    /// A download-and-verify finished.
    Downloaded(Result<(), String>),
}

/// The Updates page's update section, and the only owner of the channel its
/// worker threads report on.
pub struct UpdatePanel {
    stage: UpdateStage,
    /// `Some` exactly while a worker may still report. Dropped when the work
    /// finishes, which is also what makes [`UpdatePanel::is_busy`] answerable
    /// without a second flag to keep in step.
    rx: Option<Receiver<UpdateMsg>>,
}

impl Default for UpdatePanel {
    fn default() -> Self {
        Self { stage: UpdateStage::Idle, rx: None }
    }
}

impl UpdatePanel {
    /// A panel parked in an arbitrary stage and wired to nothing.
    ///
    /// For `examples/ui_preview`, which has to render every state of this page
    /// without a network, a thread or a clock. Deliberately cannot start work:
    /// there is no receiver, so nothing can arrive, and the states it draws
    /// are exactly the states named.
    pub fn parked(stage: UpdateStage) -> Self {
        Self { stage, rx: None }
    }

    pub fn stage(&self) -> &UpdateStage {
        &self.stage
    }

    /// True while a worker thread may still report into this panel. The About
    /// page uses it to keep asking for frames -- an egui window repaints on
    /// input, and a download nobody is typing over would otherwise show a
    /// progress bar that only moves when the mouse does.
    pub fn is_busy(&self) -> bool {
        self.rx.is_some()
    }

    /// Drains everything that has arrived, without ever blocking.
    ///
    /// Returns true if the stage changed, which is the caller's cue that the
    /// frame it is drawing is out of date and another is worth asking for.
    /// A disconnected channel with nothing in it is not an error and not a
    /// failure to report: it means the worker finished and its outcome has
    /// already been applied, so the receiver is simply retired.
    ///
    /// # What happens while nobody is calling this
    ///
    /// Only the Updates page pumps, so a user who starts a download and then
    /// clicks another section in the nav stops draining. **The download does
    /// not stop, and nothing it says is lost.** The channel is an unbounded
    /// `mpsc`, so the worker's `send` never blocks on a queue nobody is
    /// reading; the bytes keep arriving, the file keeps being written, and
    /// the progress messages queue. Coming back to the page drains the whole
    /// backlog in this one loop before the frame paints, so the bar is drawn
    /// at where the transfer *is* -- not where it was when the user left, and
    /// never replayed as an animation.
    ///
    /// That is the same answer as before the Preferences window was
    /// reorganised, and worth stating because the question changed shape: the
    /// panel used to be a card on About, so "navigate away" already meant
    /// "stop pumping" for every other section. Updates being its own page
    /// makes leaving it likelier, not different.
    ///
    /// The two things that genuinely pause are cosmetic and self-correcting:
    /// the repaint request (asked for by the page, not here) and the stage,
    /// which is only ever a rendering of messages already sent.
    pub fn pump(&mut self) -> bool {
        let mut changed = false;
        loop {
            let Some(rx) = self.rx.as_ref() else { return changed };
            match rx.try_recv() {
                Ok(msg) => {
                    changed |= self.apply(msg);
                }
                Err(TryRecvError::Empty) => return changed,
                Err(TryRecvError::Disconnected) => {
                    self.rx = None;
                    return changed;
                }
            }
        }
    }

    /// One message's effect on the stage, and the whole of it.
    ///
    /// Separate from [`pump`](Self::pump) and from every thread in this module
    /// so the state machine is testable as a state machine: the tests below
    /// hand it messages in sequences a real run would produce, and in
    /// sequences a real run would not, without a socket, a file or a spawn
    /// anywhere. Nothing about a transition lives in the worker threads.
    fn apply(&mut self, msg: UpdateMsg) -> bool {
        let next = match msg {
            UpdateMsg::Checked(Ok(Some(release))) => UpdateStage::Available(release),
            UpdateMsg::Checked(Ok(None)) => UpdateStage::UpToDate,
            UpdateMsg::Checked(Err(message)) => UpdateStage::Failed { message, release: None },
            // **Progress that arrives outside a download is dropped, not
            // rendered.** A `Progress` landing after the `Downloaded` that
            // followed it -- the two are sent from one thread but read here
            // after the fact -- would otherwise reopen a finished download and
            // put the page back on a progress bar for a file already verified.
            UpdateMsg::Progress(done, total) => match &self.stage {
                UpdateStage::Downloading { release, .. } => UpdateStage::Downloading {
                    release: release.clone(),
                    done,
                    total,
                },
                _ => return false,
            },
            UpdateMsg::Downloaded(Ok(())) => match &self.stage {
                UpdateStage::Downloading { release, .. } => UpdateStage::Ready(release.clone()),
                // Same reasoning: a success for a download this panel is no
                // longer on cannot promote it to Ready, because there is no
                // release here to say Ready *to*.
                _ => return false,
            },
            UpdateMsg::Downloaded(Err(message)) => UpdateStage::Failed {
                message,
                release: match &self.stage {
                    UpdateStage::Downloading { release, .. } => Some(release.clone()),
                    _ => None,
                },
            },
        };
        let changed = next != self.stage;
        self.stage = next;
        changed
    }

    /// Starts a check on a background thread.
    ///
    /// **Deliberately not gated on `Settings::check_for_updates`.** That
    /// setting governs the check this app makes *by itself* -- at startup and
    /// once a day thereafter (`main.rs`) -- and turning it off is a request
    /// not to be talked to, not a request to be refused. A click on this
    /// button is the user initiating the request in the same breath as
    /// consenting to it, and a button that silently did nothing because of a
    /// pill on another page is the exact "control that refuses to be clicked"
    /// this change exists to delete. `PRIVACY.md` says so, under "Update
    /// checks -- api.github.com", because a privacy policy that describes
    /// fewer requests than the software makes is worse than no policy.
    ///
    /// A no-op while something is already in flight: two concurrent checks
    /// would race to be the one whose answer the page shows.
    pub fn begin_check(&mut self) {
        if self.is_busy() {
            return;
        }
        let Some(env) = env() else {
            self.stage = UpdateStage::Unavailable;
            return;
        };
        let (tx, rx) = mpsc::channel();
        let base = env.api_base.clone();
        let version = env.current_version.clone();
        std::thread::spawn(move || {
            let agent = updater::build_api_agent();
            let outcome = updater::check_for_update(&base, &version, &agent);
            match &outcome {
                Ok(Some(r)) => log::info!("manual update check: v{} is available", r.version),
                Ok(None) => log::info!("manual update check: v{version} is the latest"),
                Err(e) => log::warn!("manual update check failed: {e}"),
            }
            // The receiver is gone if the user closed Preferences mid-check.
            // That is a normal way for this to end, not an error.
            let _ = tx.send(UpdateMsg::Checked(outcome));
        });
        self.rx = Some(rx);
        self.stage = UpdateStage::Checking;
    }

    /// Starts downloading and verifying the release this panel is showing.
    ///
    /// A no-op unless the panel is on a release that has not been fetched --
    /// [`UpdateStage::Available`], or a [`UpdateStage::Failed`] that still
    /// remembers one, which is what makes the failure state's retry work.
    ///
    /// If the user closes Preferences while this runs, the thread finishes
    /// into a dropped receiver and the part-file (or the verified installer)
    /// is left in the cache directory, where `updater::cleanup_stale_downloads`
    /// removes it at the next startup. Nothing is leaked and nothing is
    /// launched: launching is a separate, explicit click.
    pub fn begin_download(&mut self) {
        if self.is_busy() {
            return;
        }
        let release = match &self.stage {
            UpdateStage::Available(r) => r.clone(),
            UpdateStage::Failed { release: Some(r), .. } => r.clone(),
            _ => return,
        };
        let Some(env) = env() else {
            self.stage = UpdateStage::Unavailable;
            return;
        };
        let (tx, rx) = mpsc::channel();
        let dest = env.download_dir.clone();
        let for_thread = release.clone();
        let progress_tx: Sender<UpdateMsg> = tx.clone();
        std::thread::spawn(move || {
            let agent = updater::build_download_agent();
            let outcome = updater::download_and_verify(
                // No expected-value argument: the digest to check against
                // travels inside `for_thread`, from the same API response
                // that supplied the download URL. This call site used to
                // pass `updater::EXPECTED_SIGNER_THUMBPRINT`, and a call site
                // that hands over the value a check is made against is a
                // call site that could hand over a different one.
                &for_thread,
                &dest,
                &agent,
                // Called between reads on this thread. A `send` into an
                // unbounded channel, so the download's pace is not set by how
                // often the window happens to repaint.
                &move |done, total| {
                    let _ = progress_tx.send(UpdateMsg::Progress(done, total));
                },
            )
            // The path is deliberately dropped: `apply_update` reconstructs it
            // from the release and re-verifies it, so no caller can name the
            // file that eventually gets launched.
            .map(|_installer_path| ());
            if let Err(e) = &outcome {
                log::error!("update download failed: {e}");
            }
            let _ = tx.send(UpdateMsg::Downloaded(outcome));
        });
        self.rx = Some(rx);
        self.stage = UpdateStage::Downloading { release, done: 0, total: None };
    }

    /// Launches the verified installer and ends this process.
    ///
    /// **Does not return on success.** The installer replaces this binary and
    /// relaunches it, so there is nothing for this process to go back to;
    /// `updater::apply_update` re-hashes the file and starts it, and
    /// [`UpdateEnv::before_install`] runs first so the decrypted vault cache
    /// and the clipboard are cleared before the handover. `bw serve` needs no
    /// entry there: it is in a kill-on-close job object and the kernel takes
    /// it down with this process.
    ///
    /// Runs on the UI thread rather than on a worker, and that is the one
    /// place in this module where blocking a frame is right. `apply_update`
    /// re-hashes ~6 MB before it spawns anything, so this can cost a moment --
    /// but it is the last frame this process will ever draw, and putting the
    /// exit on a background thread would mean a window still taking clicks
    /// while its process was being replaced.
    pub fn install_now(&mut self) {
        let release = match &self.stage {
            UpdateStage::Ready(r) => r.clone(),
            _ => return,
        };
        let Some(env) = env() else {
            self.stage = UpdateStage::Unavailable;
            return;
        };
        log::info!("installing update v{}; shutting down for it", release.version);
        (env.before_install)();
        match updater::apply_update(&env.download_dir, &release) {
            Ok(()) => std::process::exit(0),
            Err(message) => {
                log::error!("update install failed: {message}");
                self.stage = UpdateStage::Failed { message, release: Some(release) };
            }
        }
    }
}

/// How far a download has got, as a fraction, or `None` when the server
/// declared no length.
///
/// Pure, so "does the bar fill" is answerable without a window. A declared
/// total of zero yields `None` rather than a division by zero, and a stream
/// that overran its declared length is clamped rather than reported as more
/// than whole.
pub fn download_fraction(done: u64, total: Option<u64>) -> Option<f32> {
    match total {
        Some(t) if t > 0 => Some((done as f32 / t as f32).clamp(0.0, 1.0)),
        _ => None,
    }
}

/// The progress line under the bar: "3.2 MB of 6.1 MB", or just "3.2 MB" when
/// the total is unknown.
///
/// Megabytes throughout rather than a unit that changes with the number: this
/// text is repainted many times a second, and a label that switches between KB
/// and MB mid-download flickers between two widths.
pub fn download_label(done: u64, total: Option<u64>) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    let mb = |b: u64| format!("{:.1} MB", b as f64 / MB);
    match total {
        Some(t) if t > 0 => format!("{} of {}", mb(done), mb(t)),
        _ => mb(done),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(v: &str) -> ReleaseInfo {
        ReleaseInfo {
            version: Version::parse(v).unwrap(),
            installer_download_url: format!("https://example.invalid/deskwarden-{v}-installer.exe"),
            installer_sha256: updater::parse_asset_digest(&format!("sha256:{}", "b".repeat(64)))
                .unwrap(),
            body: "notes".to_string(),
        }
    }

    /// **The state the tray could not express.**
    ///
    /// The item this replaced was built as `MenuItem::new("Update available",
    /// false, None)`: the words were there from startup and only the *enabling*
    /// waited on the check, so a session with no update showed a permanent
    /// claim that there was one. A check that comes back empty must land on a
    /// stage that says so.
    #[test]
    fn a_check_that_finds_nothing_says_so_rather_than_claiming_an_update() {
        let mut panel = UpdatePanel::default();
        assert_eq!(panel.stage(), &UpdateStage::Idle);

        panel.apply(UpdateMsg::Checked(Ok(None)));

        assert_eq!(panel.stage(), &UpdateStage::UpToDate);
    }

    #[test]
    fn a_check_that_finds_a_release_carries_its_notes_into_the_stage() {
        let mut panel = UpdatePanel::default();

        panel.apply(UpdateMsg::Checked(Ok(Some(release("9.9.9")))));

        match panel.stage() {
            UpdateStage::Available(r) => {
                assert_eq!(r.version, Version::parse("9.9.9").unwrap());
                assert_eq!(r.body, "notes", "the page has nothing to render without these");
            }
            other => panic!("expected an available release, got {other:?}"),
        }
    }

    #[test]
    fn a_failed_check_reports_its_reason_and_has_nothing_to_retry() {
        let mut panel = UpdatePanel::default();

        panel.apply(UpdateMsg::Checked(Err("failed to reach GitHub".to_string())));

        assert_eq!(
            panel.stage(),
            &UpdateStage::Failed {
                message: "failed to reach GitHub".to_string(),
                release: None
            }
        );
    }

    /// The whole happy path as a sequence of messages, which is the only form
    /// it has: nothing in this module transitions except through `apply`.
    #[test]
    fn the_download_walks_from_available_through_progress_to_ready() {
        let mut panel = UpdatePanel::parked(UpdateStage::Downloading {
            release: release("9.9.9"),
            done: 0,
            total: None,
        });

        panel.apply(UpdateMsg::Progress(512, Some(2048)));
        assert!(
            matches!(panel.stage(), UpdateStage::Downloading { done: 512, total: Some(2048), .. }),
            "progress must reach the page: {:?}",
            panel.stage()
        );

        panel.apply(UpdateMsg::Progress(2048, Some(2048)));
        panel.apply(UpdateMsg::Downloaded(Ok(())));

        match panel.stage() {
            UpdateStage::Ready(r) => assert_eq!(r.version, Version::parse("9.9.9").unwrap()),
            other => panic!("expected the restart prompt, got {other:?}"),
        }
    }

    /// **A failed download keeps the release, so the retry has something to
    /// retry.** Without this the failure state is a dead end: the message says
    /// what went wrong and the only way back to the same release is another
    /// full check.
    #[test]
    fn a_failed_download_keeps_the_release_and_can_be_retried_from_the_failure() {
        let mut panel = UpdatePanel::parked(UpdateStage::Downloading {
            release: release("9.9.9"),
            done: 10,
            total: Some(2048),
        });

        panel.apply(UpdateMsg::Downloaded(Err("the link went away".to_string())));

        match panel.stage() {
            UpdateStage::Failed { message, release: Some(r) } => {
                assert_eq!(message, "the link went away");
                assert_eq!(r.version, Version::parse("9.9.9").unwrap());
            }
            other => panic!("a failure that forgets the release cannot retry: {other:?}"),
        }
    }

    /// A `Progress` read after the `Downloaded` that followed it must not put
    /// a verified download back onto a progress bar. The two are sent from one
    /// thread, but they are *read* here one at a time and the page renders
    /// whatever the last one left.
    #[test]
    fn progress_arriving_after_the_download_finished_does_not_reopen_it() {
        let mut panel = UpdatePanel::parked(UpdateStage::Downloading {
            release: release("9.9.9"),
            done: 0,
            total: Some(2048),
        });
        panel.apply(UpdateMsg::Downloaded(Ok(())));

        let changed = panel.apply(UpdateMsg::Progress(1024, Some(2048)));

        assert!(!changed, "a late progress report changed the stage");
        assert!(
            matches!(panel.stage(), UpdateStage::Ready(_)),
            "the page went back to downloading a file it had already verified: {:?}",
            panel.stage()
        );
    }

    /// `pump` never blocks, and a channel whose sender has gone is a finished
    /// worker rather than a failure to report.
    #[test]
    fn pump_drains_without_blocking_and_retires_a_finished_worker() {
        let (tx, rx) = mpsc::channel();
        let mut panel = UpdatePanel { stage: UpdateStage::Checking, rx: Some(rx) };

        tx.send(UpdateMsg::Checked(Ok(Some(release("9.9.9"))))).unwrap();
        assert!(panel.pump(), "the message was not drained");
        assert!(matches!(panel.stage(), UpdateStage::Available(_)));
        assert!(panel.is_busy(), "the sender is still alive, so the panel is still working");

        // Nothing queued and the sender still alive: returns immediately,
        // which is the property that keeps a frame from freezing.
        assert!(!panel.pump());

        drop(tx);
        panel.pump();
        assert!(!panel.is_busy(), "a hung-up channel must retire, or the page spins forever");
    }

    /// Two checks at once would race to be the one whose answer is shown.
    #[test]
    fn a_second_start_is_refused_while_something_is_in_flight() {
        let (tx, rx) = mpsc::channel::<UpdateMsg>();
        let mut panel = UpdatePanel { stage: UpdateStage::Checking, rx: Some(rx) };

        panel.begin_check();
        panel.begin_download();

        assert_eq!(panel.stage(), &UpdateStage::Checking);
        drop(tx);
    }

    /// With no environment installed -- the screenshot example, and every test
    /// in this crate -- the button says so instead of doing nothing. It also
    /// proves the negative that matters: **no test in this crate can make this
    /// module open a socket**, because reaching the network requires an
    /// `UpdateEnv` and nothing but `main.rs` installs one.
    #[test]
    fn without_an_installed_environment_the_flow_refuses_rather_than_reaching_the_network() {
        assert!(env().is_none(), "a test installed a process-wide UpdateEnv; nothing may");

        let mut panel = UpdatePanel::default();
        panel.begin_check();

        assert_eq!(panel.stage(), &UpdateStage::Unavailable);
        assert!(!panel.is_busy(), "no worker may exist without an environment to work in");
    }

    #[test]
    fn the_bar_fills_only_on_a_total_it_can_believe() {
        assert_eq!(download_fraction(512, Some(1024)), Some(0.5));
        assert_eq!(download_fraction(1024, Some(1024)), Some(1.0));
        assert_eq!(download_fraction(0, Some(0)), None, "a zero total is not a full bar");
        assert_eq!(download_fraction(10, None), None);
        assert_eq!(
            download_fraction(2048, Some(1024)),
            Some(1.0),
            "a stream past its declared length is whole, never more than whole"
        );
    }

    #[test]
    fn the_progress_line_keeps_one_unit_so_it_does_not_flicker_between_widths() {
        assert_eq!(download_label(0, Some(6_291_456)), "0.0 MB of 6.0 MB");
        assert_eq!(download_label(3_355_443, Some(6_291_456)), "3.2 MB of 6.0 MB");
        assert_eq!(download_label(1024, None), "0.0 MB");
    }
}
