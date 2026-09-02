//! Acquiring the Bitwarden CLI, at the moment a server is chosen that needs
//! it, and never before.
//!
//! # What this module is for
//!
//! `bw serve` is still the vault for accounts whose backend is
//! [`crate::backend_policy::VaultBackendChoice::BwServe`] -- which is every
//! account on `bitwarden.com` and `bitwarden.eu`, because this app's own
//! direct-REST client is deliberately not pointed at Bitwarden's production
//! servers. So `bw.exe` has stopped being a property of *the product* and
//! become a property of *one choice inside it*.
//!
//! The installer used to fetch it unconditionally, at install time, before
//! the user had chosen anything: a wizard page, a 370-line PowerShell
//! bootstrap, a minute of waiting and 37 MB of somebody's connection, spent
//! on a dependency that only some users need. That is gone. This module is
//! what replaced it, and it runs from the sign-in window at the moment the
//! user presses Continue on an official server.
//!
//! # Nothing here is silent
//!
//! The owner's ruling, verbatim:
//!
//! > "yes, no silent - we say that it is requared period"
//!
//! and, refining it:
//!
//! > "installer should not install it but when user attempts to login to bw
//! > servers - prompt (modal probably - Yes, No), installation,
//! > configuration, login, working"
//!
//! So the app **names the Bitwarden CLI**, says it is required for the server
//! the user just chose, and asks -- with a Yes/No modal -- before a single
//! byte is fetched. Not a bare spinner, not "Setting up...", not a euphemism,
//! and not an explanation offered afterwards. Every refusal below names three
//! things: the Bitwarden CLI, that `bitwarden.com` requires it, and that a
//! self-hosted server does not. That third clause is the actionable half.
//!
//! **A future reader will be tempted to cite the project's other standing
//! rule -- "users should not know about how it works underhood" -- to soften
//! these strings. Do not.** The two rules do not conflict, and the test that
//! separates them is:
//!
//! > **Does the user have a decision to make about this fact?** If yes, say
//! > it plainly and name the thing. If no, it is machinery and it stays
//! > inside.
//!
//! The "underhood" rule bans exposing internal machinery the user cannot act
//! on: which process draws a window, whether a vault read came from `bw
//! serve` or a REST call, that a `bw status` spawn fell back. This is not
//! that. A third-party program is being downloaded from the internet and
//! installed on the user's machine, as a hard requirement of a choice they
//! just made, at a cost in bandwidth they are about to pay. All three are
//! theirs to know. See
//! `docs/superpowers/specs/2026-08-31-the-cli-arrives-when-it-is-needed-design.md`.
//!
//! # Trust: what is actually proved, and what is not
//!
//! Established from primary sources on 2026-08-31, and the distinction is
//! load-bearing:
//!
//! * **Bitwarden publishes no signature file and no checksum file.** All 13
//!   assets of `cli-v2026.8.0` were enumerated; none is one. What their docs
//!   call a checksum is GitHub's own per-asset `digest` field.
//! * That `digest` arrives over TLS from `api.github.com`; the file arrives
//!   over TLS from GitHub's CDN. Same trust root. It catches a truncated,
//!   corrupted or partially-written download. **It is INTEGRITY ONLY. It is
//!   not an independent authenticity proof and nothing here claims it is** --
//!   anyone able to substitute the asset could substitute the digest.
//! * **The load-bearing check is the Authenticode signature on the `bw.exe`
//!   inside the zip** (`O=Bitwarden Inc.`, DigiCert-chained). A public CA had
//!   to validate that organization name, and the proof does not depend on how
//!   the file reached the machine.
//!
//! Two checks, in the only safe order: digest first (cheap, catches the
//! boring failure), Authenticode second, and **the binary is never
//! executed** -- not even `bw --version` to "check it works". Verify, then
//! install; never run to test. [`nothing_in_this_module_starts_a_process`]
//! pins that over this file's production half.
//!
//! A failed signature **deletes and does not retry**. A retry loop against a
//! substituted artefact is a loop.
//!
//! # The organization list is not ours
//!
//! [`crate::signature::TRUSTED_BW_SIGNER_ORGANIZATIONS`] is read, never
//! copied. It is the same list `main`'s startup check grades against, and the
//! two must not be able to disagree -- the direction they would disagree in
//! is "this module installs a binary that startup then refuses to run".
//! [`the_trusted_organizations_are_not_duplicated_in_this_module`] pins it.
//!
//! # Where the file goes, and why exactly there
//!
//! [`crate::bw_path::install_bin_candidate`], and nowhere else.
//! `bw_path::VERIFIED_BW_EXE` is a first-wins `OnceLock`, and `main` fills it
//! **unconditionally at startup -- including when the file does not exist**,
//! because `resolve_bw_exe` returns the expected install path in that case.
//! So this module runs in a process already holding a path to a file that was
//! not there, and its job is to make that path correct by putting the file
//! underneath it. A second spelling of that path would leave the process
//! trusting one location while the binary sat at another.
//!
//! Two consequences follow, and the module turns on both:
//!
//! 1. The destination is `bw_path`'s own answer, asserted by agreement rather
//!    than by string ([`the_install_destination_is_the_path_the_resolver_already_recorded`]).
//! 2. **This module owns the signature check for the file it installs**,
//!    because startup's did not run -- there was nothing to check. It
//!    verifies *before* the copy, so a file that fails is never at the
//!    recorded path at all.
//!
//! # Uninstall now removes both, and why that reversed
//!
//! This paragraph moved here from `installer/deskwarden.iss` when the
//! bootstrap was deleted, and it used to read:
//!
//! > Uninstall does not remove `bw.exe` or its `PATH` entry. The user may be
//! > using `bw` independently of deskwarden -- it is a general-purpose tool
//! > that predates this app on many machines -- and silently deleting a
//! > working command-line program because an unrelated tray app was
//! > uninstalled is worse than leaving a 40 MB file behind.
//!
//! **That reasoning was written for a file this app did not own, and it no
//! longer describes the file it guards.** It dates from `bootstrap-bw.ps1`,
//! when the CLI could plausibly be one the user already had. What this module
//! installs is not that: [`install_destination`] puts it under
//! `<InstallDir>\bin`, a directory THIS APP creates inside its own install
//! location, from a download THIS APP made, on a `PATH` entry THIS APP wrote.
//! Nobody's independent copy of `bw` lives there. So the sentence protected a
//! case that cannot arise, at the price of one that does: leaving a 40 MB
//! binary and a `PATH` entry pointing into the install directory of an app
//! that has been uninstalled.
//!
//! The uninstaller therefore removes `<InstallDir>\bin` and takes that one
//! `PATH` entry back out, comparing it the same way [`add_to_user_path`]
//! compared it going in. A `bw` the user installed anywhere else is untouched,
//! which is the part of the old reasoning that was always the real point.
//!
//! **None of that is this module's job, and the test below still says so.**
//! The running app must never delete the CLI -- the uninstaller is a different
//! program, run deliberately, at a moment the user asked for exactly this.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::backend_policy::{choose, VaultBackendChoice};
use crate::http_agent::{StallBounded, TotalBounded};
use crate::updater::{parse_asset_digest, Sha256Digest};

// ---- the gate ---------------------------------------------------------------

/// Whether this sign-in needs the Bitwarden CLI at all.
///
/// **A thin reading of [`crate::backend_policy::choose`], not a second
/// decision.** It could have been written `!is_self_hosted(server)`, which
/// agrees today and would drift the first time `choose` gains an input --
/// and the two disagreeing means either "the app refuses to sign in to a
/// server it can serve" or "the app signs in to a server whose vault it then
/// cannot open". [`the_gate_is_exactly_the_backend_policy_decision`] walks
/// the whole table in both directions.
///
/// Note what this means for a **self-hosted server with the official-CLI
/// setting left on**, which is the default: `choose` answers `BwServe`, so
/// that account genuinely does run `bw serve` and genuinely does need the
/// binary, and this returns `true` for it. The design document's failure
/// matrix says "self-hosted never acquires", which is true only of the
/// direct-REST arm; following it literally would refuse the CLI to a
/// self-hoster whose vault is served by it, which is a brick. Reality wins,
/// and it is `choose` that knows it.
#[must_use]
pub fn this_sign_in_needs_the_cli(server_url: Option<&str>, use_official_bw_crypto: bool) -> bool {
    choose(server_url, use_official_bw_crypto) == VaultBackendChoice::BwServe
}

// ---- what the resolver settles on -------------------------------------------

/// The artefact the resolver settled on: where it is, and what it must hash
/// to.
///
/// The URL and the digest are read out of **one asset object of one
/// release**, never by two independent searches -- the reason
/// `updater::installer_from` gives, unchanged: two searches can disagree
/// about which asset they found, and a digest taken from a different asset
/// than the URL is a check that passes on the wrong file or fails on the
/// right one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BwArtefact {
    pub url: String,
    pub digest: Sha256Digest,
    pub asset_name: String,
}

/// One release, as much of it as this module reads.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub assets: Vec<ReleaseAsset>,
}

/// One asset of one release.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
    /// GitHub's per-asset digest, `sha256:<64 hex>`. `Option` because the
    /// field may be absent -- and an absent one is a **refusal**, not a
    /// permission to skip the check. See [`pick_artefact`].
    #[serde(default)]
    pub digest: Option<String>,
    #[serde(default)]
    pub browser_download_url: String,
}

/// The tag prefix that identifies a CLI release in a monorepo whose `cli`,
/// `desktop`, `browser` and `web` releases interleave by date.
///
/// **Anchored, and matched with `strip_prefix` rather than `contains`.** The
/// repo's generic "latest" release is not the CLI's, which is the finding the
/// deleted `bootstrap-bw.ps1` recorded first and this preserves.
const CLI_TAG_PREFIX: &str = "cli-v";

/// The asset-name prefix of the Windows build.
///
/// **Anchored at the FRONT, and that is the whole point of the constant.**
/// `bw-oss-windows-<version>.zip` sits beside `bw-windows-<version>.zip` in
/// every release and sorts *before* it, so a `contains("windows")` glob, or a
/// "first match" scan, picks the OSS build -- which lacks paid-tier features
/// and would fail for users in a way nobody could diagnose from the symptom.
const WINDOWS_ASSET_PREFIX: &str = "bw-windows-";

/// ...and the suffix, so a future `.zip.sig` or `.zip.sha256` beside it
/// cannot be mistaken for the archive.
const WINDOWS_ASSET_SUFFIX: &str = ".zip";

/// The name `bw.exe` is stored under inside the archive.
const BW_EXE_NAME: &str = "bw.exe";

/// The releases listing. `per_page=50` for `updater`'s reason: the CLI's
/// releases interleave with three other products', so the newest `cli-v*` can
/// be well down the first page.
const RELEASES_URL: &str = "https://api.github.com/repos/bitwarden/clients/releases?per_page=50";

/// Bound for the API call: a small response where a late answer is a useless
/// answer. Same shape `updater` uses for the same host.
const API_CONNECT: Duration = Duration::from_secs(10);
const API_TOTAL: Duration = Duration::from_secs(30);

/// Bounds for the transfer: ~37 MB, so a *total* cap is the wrong shape (see
/// `http_agent`). What is bounded is time without progress.
const DOWNLOAD_CONNECT: Duration = Duration::from_secs(15);
const DOWNLOAD_STALL: Duration = Duration::from_secs(30);

// ---- why acquisition stopped ------------------------------------------------

/// Why acquisition stopped.
///
/// One variant per row of the design's failure matrix, because the user reads
/// a different sentence for each -- and because [`Self::retryable`] differs
/// between them in a way a single `String` error could not express.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcquireRefusal {
    /// The user answered No to the prompt. Not an error, and it costs
    /// nothing: the prompt precedes the first byte, so nothing was fetched
    /// and nothing is left behind.
    Declined,
    /// No network, DNS failure, connect timeout, or a transfer that stopped
    /// moving. Refused **before credentials leave the window**.
    Offline(String),
    /// No `cli-v*` release, no `bw-windows-*.zip` asset, or no `sha256:`
    /// digest on it. Fail-closed.
    NoArtefact(String),
    /// The bytes that arrived are not the bytes GitHub described.
    DigestMismatch,
    /// A valid signature, by somebody who is not Bitwarden -- or no signature
    /// at all. **The one that does not retry.**
    NotBitwardenSigned { subject_dn: Option<String> },
    /// The signature check could not run. A different refusal from
    /// [`Self::NotBitwardenSigned`] precisely because this one is retryable
    /// and that one is not.
    Unverifiable(String),
    /// Everything arrived and verified, and the file could not be put where
    /// it belongs.
    CouldNotInstall(String),
}

impl AcquireRefusal {
    /// The sentence shown in the sign-in card.
    ///
    /// **Every one of these names three things**: the Bitwarden CLI (what is
    /// required), `bitwarden.com` (what requires it), and self-hosting (the
    /// way out). The third clause is the actionable half and it is the one a
    /// euphemism would swallow;
    /// [`every_refusal_names_the_cli_the_server_and_the_alternative`] holds
    /// that by the file rather than by review.
    ///
    /// An earlier draft of this module asserted the OPPOSITE -- that only the
    /// signature failure should name the CLI, everything else being
    /// euphemised to "setup" -- reasoning from the "underhood" rule. It was
    /// wrong; see this module's own docs for why the two rules do not
    /// conflict. It is recorded here so nobody re-derives it.
    ///
    /// The strings name `bitwarden.com` rather than the server actually
    /// chosen. That is deliberate and it is the ruling's own wording: these
    /// are self-contained sentences with no context parameter, and
    /// `bitwarden.com` is the case that is true for nearly every reader of
    /// them. See [`this_sign_in_needs_the_cli`] for the one account shape
    /// (self-hosted, official-CLI setting on) where the second clause reads
    /// as less precise than it could -- the first and third stay exactly
    /// true, and those are the ones that tell the user what to do.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::Declined => "Deskwarden did not install the Bitwarden CLI, so it cannot sign \
                 in to bitwarden.com, which requires it. Press Continue again if you change \
                 your mind. A self-hosted server can be used without it."
                .to_string(),
            Self::Offline(_) => "Deskwarden couldn't download the Bitwarden CLI, which \
                 bitwarden.com requires. Check your connection and try again. A self-hosted \
                 server can be used without it."
                .to_string(),
            Self::NoArtefact(_) => "Deskwarden couldn't find the Bitwarden CLI download, which \
                 bitwarden.com requires. Try again later, or install the CLI yourself from \
                 bitwarden.com/help/cli/. A self-hosted server can be used without it."
                .to_string(),
            Self::DigestMismatch => "The Bitwarden CLI download didn't arrive intact, so \
                 Deskwarden discarded it. Try again. bitwarden.com requires the Bitwarden CLI; \
                 a self-hosted server does not."
                .to_string(),
            Self::NotBitwardenSigned { .. } => "Deskwarden downloaded the Bitwarden CLI but \
                 could not confirm it came from Bitwarden, so it did not install it and did \
                 not run it. bitwarden.com requires the Bitwarden CLI, so you cannot sign in \
                 to it until this is resolved. You can install the CLI yourself from \
                 bitwarden.com/help/cli/, or use a self-hosted server, which does not need it."
                .to_string(),
            Self::Unverifiable(_) => "Deskwarden couldn't check who signed the Bitwarden CLI it \
                 downloaded, so it discarded it rather than install an unverified program. Try \
                 again. bitwarden.com requires the Bitwarden CLI; a self-hosted server does not."
                .to_string(),
            Self::CouldNotInstall(_) => "Deskwarden downloaded and verified the Bitwarden CLI \
                 but could not install it. Try again. bitwarden.com requires the Bitwarden \
                 CLI; a self-hosted server does not."
                .to_string(),
        }
    }

    /// Whether Continue offers another attempt.
    ///
    /// `false` for [`Self::NotBitwardenSigned`] alone. Everything else here
    /// is a transient or environmental failure whose natural remedy is to try
    /// again; that one is a statement about the *artefact*, and retrying
    /// against a substituted artefact is a loop that ends when the user gives
    /// up or when one attempt happens to slip through.
    #[must_use]
    pub fn retryable(&self) -> bool {
        !matches!(self, Self::NotBitwardenSigned { .. })
    }
}

/// What landed, for the modal's third state.
///
/// **Both fields are measured, not requested.** The owner's requirement is
/// that the confirmation names the version and the size of what was
/// installed; if the release moves, or GitHub serves something other than
/// what was asked for, this must show what is actually on disk. So `bytes`
/// is `metadata()` of the installed file and `version` is read off the asset
/// that was actually downloaded and verified -- never off a constant in this
/// file. [`the_confirmation_reflects_the_file_that_landed`] pins that a
/// changed artefact changes both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcquiredCli {
    pub path: PathBuf,
    /// `2026.8.0`, read out of the asset name that was verified. `None` if
    /// the name did not carry one -- stated as "unknown version" rather than
    /// guessed, because a wrong version in a confirmation is worse than an
    /// absent one.
    pub version: Option<String>,
    pub bytes: u64,
}

impl AcquiredCli {
    /// The confirmation line: what was installed, which version, how big.
    #[must_use]
    pub fn summary(&self) -> String {
        let version = match &self.version {
            Some(v) => format!("version {v}"),
            None => "an unknown version".to_string(),
        };
        format!(
            "The Bitwarden CLI was installed ({version}, {}).",
            megabytes(self.bytes)
        )
    }
}

/// `37.7 MB`. Megabytes throughout, matching `update_panel::download_label`
/// rather than inventing a second size format for the same kind of number.
fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
}

/// `bw-windows-2026.8.0.zip` -> `2026.8.0`.
///
/// A pure reading of the asset name that was verified, so the confirmation
/// cannot drift from the artefact. Returns `None` rather than a guess when
/// the name is not the shape this module resolved.
fn version_from_asset(asset_name: &str) -> Option<String> {
    let rest = asset_name.strip_prefix(WINDOWS_ASSET_PREFIX)?;
    let version = rest.strip_suffix(WINDOWS_ASSET_SUFFIX)?;
    (!version.is_empty()).then(|| version.to_string())
}

/// What acquisition is doing right now, for the one status line in the
/// sign-in card.
///
/// Named stages rather than a bare spinner, because the ruling is that the
/// app says what it is doing. [`Self::Downloading`] carries `(done, total)`
/// straight through to [`crate::update_panel::download_fraction`], which is
/// reused rather than reimplemented -- a spinner would hide the size, and the
/// size is the part that costs the user something.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcquireStage {
    Resolving,
    Downloading { done: u64, total: Option<u64> },
    Verifying,
    Installing,
}

impl AcquireStage {
    /// The line under the progress bar. Names the Bitwarden CLI at every
    /// stage, and names Bitwarden as the source at the one that matters.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Resolving => "Finding the Bitwarden CLI download\u{2026}",
            Self::Downloading { .. } => "Downloading the Bitwarden CLI from Bitwarden\u{2026}",
            // The reassuring half, and the user watching an unknown binary
            // arrive is owed it.
            Self::Verifying => "Checking it is signed by Bitwarden\u{2026}",
            Self::Installing => "Installing the Bitwarden CLI\u{2026}",
        }
    }
}

// ---- the modal, as three states of one modal --------------------------------

/// The sign-in window's CLI-setup modal.
///
/// **One modal across three states, not three dialogs.** The owner's flow:
///
/// > "1. Modal - for using BW servers you require to use oficial CLI tool,
/// > press Ok to continue - Cancel to return
/// > 2. Ok - Installation in progress...
/// > 3. The CLI was installed (version, size), OK to continue"
///
/// It opens at [`Self::Asking`] **before the first byte is downloaded**, which
/// is the property the whole disclosure rule turns on: the user is told what
/// is required, and what it will cost, while they can still say no. It closes
/// once, at the end, and the sign-in they were already attempting carries on.
///
/// [`Self::Failed`] replaces [`Self::Working`] in the same modal rather than
/// opening a second one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliSetupState {
    /// **State 0, and only on a self-hosted sign-in: the CHOICE.**
    ///
    /// On bitwarden.com there is no choice to put -- the official CLI is the
    /// only client that opens that vault, and [`Self::Asking`] says so. On a
    /// self-hosted server both clients work, so which one runs is a decision
    /// with costs on both sides, and until now the app made it silently:
    /// `accounts::official_cli_after_sign_in` answered "built-in" for a fresh
    /// mint and nothing said so. The owner's instruction was the opposite --
    ///
    /// > "If self-hosted selected - show another modal prompting to install
    /// > CLI or offering using build in with pros and conses - Use A or use
    /// > B, progress bar, ok"
    ///
    /// -- and, on finding it absent, "there were no question when login and
    /// it started using bw by default - should ask".
    ///
    /// **A state of this modal rather than a second modal.** It reaches
    /// `login_ui::draw_cli_setup_modal` unchanged, which is what makes the
    /// choice inherit the allocated button row, the card-height measurement
    /// and `cli_setup_modal_layout_tests` -- the apparatus that exists
    /// because the requirement modal shipped blank. Choosing the CLI moves
    /// this same modal to [`Self::Working`] and the SAME acquisition worker
    /// runs; choosing the built-in client downloads nothing and spawns
    /// nothing.
    ///
    /// **Shown on EVERY self-hosted sign-in, established account included.**
    /// The owner's rule, verbatim: "always either notify (if bw.com) or
    /// prompt which one to use (self-hosted) when login".
    ///
    /// It is not nagging, because both answers are one keystroke away and
    /// the answer the sign-in would otherwise take silently is on the screen
    /// as a labelled button rather than as a default. What the repetition
    /// buys is that the answer becomes changeable at the one moment changing
    /// it is free: a sign-in re-settles the backend from the record it is
    /// about (`login_ui::direct_login_for_this_sign_in` ->
    /// `backend_policy::resettle_for`), which is the same settlement that
    /// clears or writes `userkey.bin`, and it re-authenticates anyway. A
    /// change made in Preferences has to ask for a restart and a fresh
    /// sign-in to reach that state; a change made here is already in it.
    ///
    /// **No "don't ask again".** The silent default is the defect this state
    /// exists to remove, and a checkbox would put it back one click later.
    ///
    /// **One shape, always -- no fields.** It carried two: `in_use`, the
    /// client this sign-in would otherwise take, and `established`, whether
    /// the account had signed in before. Between them they varied a sentence
    /// of the body and the ORDER of the buttons. The owner's ruling on that
    /// was to make it "same all the time", so a user who signs in twice reads
    /// the same words in the same order and finds the same button under the
    /// same finger -- and this modal has one layout to fit in the window
    /// rather than four.
    ///
    /// **Nothing about the stored answer is lost**, because this modal was
    /// never where it was honoured: the sign-in reads
    /// `accounts::official_cli_after_sign_in`, and the button pressed here is
    /// the `chosen_backend` handed to it. What went is the presentation
    /// varying, not the record.
    Choosing,
    /// **State 1.** The ask. Nothing has been fetched.
    Asking,
    /// **State 1, reached from the other direction.** The same ask, worded
    /// for the user who is ALREADY SIGNED IN and whose `bw.exe` has gone --
    /// quarantined by antivirus, cleaned out of `%LOCALAPPDATA%`, lost to a
    /// failed CLI self-update, or missing from a restored machine image.
    ///
    /// **A separate variant rather than a parameter on [`Self::Asking`],
    /// because the two differ only in what they must NOT say.** The sign-in
    /// ask offers "go back and choose a different server", which is a real
    /// way out at the moment a server is being typed. It is not one here:
    /// the server was chosen on a previous launch and this user has no form
    /// in front of them. Offering it would be telling somebody to press a
    /// control that is not on their screen.
    ///
    /// Everything below this state is shared with [`Self::Asking`] --
    /// [`acquire_if_needed`], the six-function seam, the verification gate,
    /// the install path -- which is the whole reason this is a state of this
    /// modal rather than a second dialog with its own downloader.
    AskingToRecover,
    /// **State 2.** Downloading, then verifying, then installing -- with a
    /// determinate bar, not a spinner.
    Working { stage: AcquireStage },
    /// **State 3.** What landed, named. Required: not auto-dismissed on
    /// success, because the owner wants to see what arrived.
    Installed(AcquiredCli),
    /// A refusal, in the same modal.
    Failed(AcquireRefusal),
}

/// **What acquiring the CLI costs, said once.**
///
/// Three states put this sentence to the user -- [`CliSetupState::Asking`],
/// [`CliSetupState::AskingToRecover`] and [`CliSetupState::Choosing`] -- and
/// it is the sentence the whole disclosure rests on: what is fetched, from
/// whom, what is checked, how big it is, and how often. Three copies would
/// drift, and the copy that drifted would be the one quoting the wrong size
/// to the user deciding whether to accept it.
///
/// Written to follow a modal verb ("Deskwarden will ...", "Choosing the
/// Bitwarden CLI will ...") so the same clause serves a statement of intent
/// and a consequence of a button. The lead-in is the caller's; this constant
/// starts at the bare infinitive and must keep doing so.
const DOWNLOAD_DISCLOSURE: &str = "download it from Bitwarden, check that Bitwarden signed it, \
     and install it. It is about 37 MB, and this happens once.";

/// The two clients, named once each.
///
/// Both names appear in a button label and in prose, and the button is what
/// the user is looking for when they read the prose. Two spellings of one
/// thing -- "the Bitwarden CLI" in a sentence and "Official CLI" on the
/// button -- is a user hunting for a control that is on screen.
const OFFICIAL_CLI_NAME: &str = "the Bitwarden CLI";
const BUILT_IN_NAME: &str = "Deskwarden's built-in client";

/// What the user did in the modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliModalAction {
    /// State 1's OK: begin.
    Begin,
    /// [`CliSetupState::Choosing`]'s "Use the built-in client".
    ///
    /// **Downloads nothing and starts nothing.** It records the answer
    /// against the account and lets the sign-in the user was already
    /// attempting carry on -- which is the whole difference between this
    /// button and the other one, and what
    /// `login_ui::choosing_the_built_in_client_spawns_nothing` holds it to.
    UseBuiltIn,
    /// [`CliSetupState::Choosing`]'s "Use the Bitwarden CLI".
    ///
    /// Records the answer and then runs the SAME acquisition [`Begin`] runs
    /// -- literally the same match arm and the same worker, so there is one
    /// downloader in this app and not two. See `login_ui`'s
    /// `CliModalAction::Begin | CliModalAction::UseOfficialCli` arm.
    ///
    /// [`Begin`]: Self::Begin
    UseOfficialCli,
    /// State 1's Cancel. Returns the user to the server choice -- not an
    /// error card and not a dead end.
    Cancel,
    /// State 3's OK, or a failure's dismissal. Closes the modal.
    Close,
}

impl CliSetupState {
    /// The modal's heading.
    #[must_use]
    pub fn title(&self) -> &'static str {
        match self {
            // A question, and phrased as one. Not "Choose a backend": the
            // user did not come here to pick an implementation, they came to
            // sign in, and the word for the thing that opens their vault is
            // the thing it does.
            Self::Choosing { .. } => "Which client should open this vault?",
            Self::Asking => "The Bitwarden CLI is required",
            Self::AskingToRecover => "The Bitwarden CLI is missing",
            Self::Working { .. } => "Installing the Bitwarden CLI",
            Self::Installed(_) => "The Bitwarden CLI is installed",
            Self::Failed(_) => "The Bitwarden CLI was not installed",
        }
    }

    /// The modal's body, as the lines it paints.
    ///
    /// State 1 names the thing, says it is required for Bitwarden's servers,
    /// says what will happen, and says what it costs -- in that order,
    /// because the cost is the part the user is being asked to accept.
    #[must_use]
    pub fn body(&self) -> Vec<String> {
        match self {
            // **Two clients, each with its cost, in numbers.** `prefs_ui`'s
            // rule, applied to the one screen where the user picks between
            // them: name what the thing does and what it costs.
            //
            // The built-in client gets its case first and gets the BLUE
            // button (see `buttons`), because running in about a fifth of the
            // memory with no second process is what this app is FOR. What
            // keeps that from being a sales pitch is the third paragraph:
            // it is the built-in client's own costs, not the CLI's, and it is
            // the paragraph a modal selling one answer would drop.
            //
            // **Two things this paragraph deliberately does NOT say**, both
            // of which it said in 0.15.5 and both of which were false as
            // costs:
            //
            //  * "it sends text only". True of the built-in client and
            //    equally true of the CLI path -- `send::plan_to_invocation`
            //    builds a text Send and nothing in this app builds any other
            //    kind, which `rest::send`'s module doc states outright: this
            //    is "parity with the other backend and not a subtraction from
            //    it".
            //  * "it cannot ask for an email address before one can be
            //    opened". `rest::send::email_gated` refuses such a link BY
            //    NAME, which reads as a limitation -- but the CLI path is
            //    `bw send receive [--passwordenv X] <url>`
            //    (`send::receive_invocation`) and carries no e-mail or OTP
            //    route either. The direct path is the one that names the
            //    case; naming it is not losing it.
            //
            // A cost that both options share is not a cost of choosing this
            // one, and printing it beside a blue button is how a modal ends
            // up looking even-handed while misleading.
            //
            // The one that IS real stays: `user_key_store`'s own module doc
            // -- "a master key does not expire and cannot be revoked" -- and
            // it is worded exactly as `prefs_ui::official_crypto_description`
            // words it, because a user who meets this decision twice must not
            // meet two accounts of it. It is a property of the stored KEY,
            // not a claim about when the vault locks, which is a separate
            // Preferences setting and is not what this sentence is about.
            //
            // **The figures are `README.md`'s measured table**, not
            // estimates and not the round numbers they were asked for:
            // ~10 MB for Deskwarden beside ~118 MB for `bw serve`, ~21 MB for
            // the built-in client with nothing beside it, and the backend's
            // ~8 s cold start after a restart. A user can check every one of
            // them in Task Manager, which is why they are the measured ones.
            Self::Choosing => vec![
                format!(
                    "{BUILT_IN_NAME} talks to your server itself: about 21 MB of memory, \
                     no second program, and no wait after a restart. Running small is what \
                     this app is for."
                ),
                // **Not `{OFFICIAL_CLI_NAME}` at the front.** That
                // constant is the phrase "the Bitwarden CLI", written to sit
                // MID-sentence, and this paragraph opened with it: the
                // rendered modal read "the Bitwarden CLI is Bitwarden's own
                // program", lowercase, at the start of a paragraph. Caught by
                // looking at the picture, which is the only place a
                // capital letter lives. `no_body_line_starts_in_lower_case`
                // is the assertion that means the next one is not.
                format!(
                    "The Bitwarden CLI is Bitwarden's own program, run beside Deskwarden: \
                     about 118 MB of memory on top of Deskwarden's 10 MB, and about 8 \
                     seconds after a restart before the vault answers. Choosing \
                     {OFFICIAL_CLI_NAME} will {DOWNLOAD_DISCLOSURE}"
                ),
                "What it costs: the key that unlocks your vault is kept on this PC, \
                 protected by Windows, and unlike a session it never expires, so anyone \
                 who can run programs as you on this PC can use it. And its cryptography \
                 is checked against Bitwarden's published test vectors, but it is not \
                 Bitwarden's code."
                    .to_string(),
            ],
            Self::Asking => vec![
                "Signing in to a Bitwarden server requires the official Bitwarden CLI."
                    .to_string(),
                format!("Deskwarden will {DOWNLOAD_DISCLOSURE}"),
                "Press OK to continue, or Cancel to go back and choose a different server. \
                 A self-hosted server can be used without it."
                    .to_string(),
            ],
            Self::AskingToRecover => vec![
                "Your vault is served by the official Bitwarden CLI, and it is no longer on \
                 this computer. Antivirus software, a cleanup tool, or a failed CLI update \
                 can remove it."
                    .to_string(),
                format!("Deskwarden will {DOWNLOAD_DISCLOSURE}"),
                "Press OK to reinstall it now, or Cancel to start without your vault. \
                 Deskwarden will still open, and you can sign out or switch accounts from \
                 the tray."
                    .to_string(),
            ],
            Self::Working { stage } => {
                let mut lines = vec![stage.label().to_string()];
                if let AcquireStage::Downloading { done, total } = stage {
                    lines.push(crate::update_panel::download_label(*done, *total));
                }
                lines
            }
            Self::Installed(cli) => vec![
                cli.summary(),
                format!("It is at {}.", cli.path.display()),
                "Press OK to continue signing in.".to_string(),
            ],
            Self::Failed(refusal) => vec![refusal.message()],
        }
    }

    /// How far the bar is filled, or `None` for an indeterminate moment.
    ///
    /// **Reuses [`crate::update_panel::download_fraction`]** rather than
    /// reimplementing the arithmetic -- including its decisions that a
    /// declared total of zero is `None` rather than a division by zero, and
    /// that an overrun is clamped rather than reported as more than whole.
    ///
    /// Verifying and installing report a full bar rather than `None`,
    /// deliberately: the bytes really have all arrived by then, and a bar
    /// that emptied itself after the download finished would read as the
    /// transfer restarting. The *label* is what says the stage changed, which
    /// is why the label is never a bare spinner.
    #[must_use]
    pub fn progress(&self) -> Option<f32> {
        match self {
            Self::Working { stage } => match stage {
                AcquireStage::Resolving => None,
                AcquireStage::Downloading { done, total } => {
                    crate::update_panel::download_fraction(*done, *total)
                }
                AcquireStage::Verifying | AcquireStage::Installing => Some(1.0),
            },
            _ => None,
        }
    }

    /// The buttons this state offers, left to right: label, what pressing it
    /// means, and whether it is the PRIMARY -- the one
    /// [`crate::login_ui::draw_cli_setup_modal`] paints with
    /// `theme::primary_button`, the app's blue filled action button.
    ///
    /// The flag rather than a rule in the drawing code ("the last one is
    /// blue"), because only one state has an answer worth leaning on and the
    /// others must not acquire a blue button by sitting last. At most one
    /// `true` per state -- `at_most_one_button_is_the_primary` holds that,
    /// since two blue buttons name no preference at all.
    ///
    /// A failed signature offers no retry: see [`AcquireRefusal::retryable`].
    #[must_use]
    pub fn buttons(&self) -> Vec<(&'static str, CliModalAction, bool)> {
        match self {
            // **Two answers and no Cancel, because there is no third
            // outcome.** Every other state of this modal has one thing to do
            // and a way out of it; this one is a fork, and both prongs
            // continue the sign-in. A Cancel here would be a button that
            // returns the user to a form whose Continue leads straight back
            // to this modal.
            //
            // **The same two buttons in the same places, every time.** The
            // order used to move: the client already in use was placed last,
            // which `draw_cli_setup_modal`'s right-to-left row puts at the
            // right edge. The owner's ruling was "same all the time", and a
            // fixed row is the stronger position anyway -- a control that
            // swaps sides between sign-ins is a control that can be pressed
            // from muscle memory and mean the other thing.
            //
            // **The built-in client is the primary**, the third element, and
            // `draw_cli_setup_modal` paints it with `theme::primary_button` --
            // the same blue as the login card's Continue, not a colour minted
            // here. It sits last, so the right-to-left row puts it at the
            // right edge: the affirmative position AND the blue one, which is
            // the whole of how this modal says which way it leans. It says it
            // in weight rather than in words, and
            // `the_choice_names_both_costs_and_recommends_neither` still
            // holds the PROSE to naming neither.
            //
            // Both labels name their own answer in full, so neither is "OK"
            // or "Continue" -- the two words that would make position and
            // colour the only information.
            Self::Choosing => vec![
                ("Use the Bitwarden CLI", CliModalAction::UseOfficialCli, false),
                ("Use the built-in client", CliModalAction::UseBuiltIn, true),
            ],
            Self::Asking | Self::AskingToRecover => {
                vec![
                    ("Cancel", CliModalAction::Cancel, false),
                    ("OK", CliModalAction::Begin, false),
                ]
            }
            // **No Cancel during state 2.** Not offered rather than offered
            // and unsafe: the transfer runs on a worker that owns the temp
            // file, and a cancel that returned the window to the form while
            // that worker carried on writing, extracting and possibly
            // COPYING TO THE INSTALL PATH would be the one outcome this
            // module must not have -- a half-installed, unverified binary at
            // the path the process already trusts. Closing the window is
            // still available and is safe: the copy is the last step, so a
            // process that goes away before it leaves nothing behind, and the
            // scratch directory is swept at the next attempt.
            Self::Working { .. } => Vec::new(),
            Self::Installed(_) => vec![("OK", CliModalAction::Close, false)],
            Self::Failed(_) => vec![("Close", CliModalAction::Close, false)],
        }
    }
}

// ---- the seam ---------------------------------------------------------------

/// The `fn`-pointer seam, same shape and same discipline as
/// [`crate::updater::UpdaterEnv`].
///
/// # `fn` pointers rather than `impl Fn`
///
/// A seam that is itself unpinned only MOVES the hole. A `fn` pointer has an
/// address, so [`production_holds_the_real_six`] can assert by identity --
/// `std::ptr::fn_addr_eq` -- that [`BwAcquireEnv::production`] hands over the
/// real functions. A wrapper, a forwarder, a rename or a flag-gated no-op is
/// a different address and fails there, whatever it is spelled. That matters
/// most for `verify`: a substitute that returned `Ok(())` would make the gate
/// below always agree with itself.
///
/// Fields are private and [`Self::production`] is the only constructor a
/// shipping build compiles. The test-only substitute is an inherent impl
/// written in `mod tests`, deliberately below every source guard in this
/// file.
pub struct BwAcquireEnv {
    /// [`resolve_present_and_trusted`]: is there already a `bw.exe` at the
    /// path this process recorded at startup? `Some` short-circuits
    /// everything below it, and that is the common case.
    already_present: fn() -> Option<PathBuf>,
    /// [`resolve_artefact`]: which release, which asset, which digest.
    resolve: fn(&TotalBounded) -> Result<BwArtefact, AcquireRefusal>,
    /// [`download_artefact`]: stream it to a temp file and check the digest.
    download: fn(
        &StallBounded,
        &BwArtefact,
        &Path,
        &dyn Fn(u64, Option<u64>),
    ) -> Result<PathBuf, AcquireRefusal>,
    /// [`extract_bw_exe`]: pull `bw.exe` out of the archive, into temp.
    extract: fn(&Path, &Path) -> Result<PathBuf, AcquireRefusal>,
    /// [`verify_is_bitwardens`]: the load-bearing check, run on the extracted
    /// file *before* it is copied anywhere.
    verify: fn(&Path) -> Result<(), AcquireRefusal>,
    /// [`install_at_the_resolver_path`]: the copy, and only after `verify`
    /// said yes.
    install: fn(&Path) -> Result<PathBuf, AcquireRefusal>,
}

impl BwAcquireEnv {
    /// The real world.
    #[must_use]
    pub fn production() -> Self {
        Self {
            already_present: resolve_present_and_trusted,
            resolve: resolve_artefact,
            download: download_artefact,
            extract: extract_bw_exe,
            verify: verify_is_bitwardens,
            install: install_at_the_resolver_path,
        }
    }
}

// ---- the whole feature, as one call ----------------------------------------

/// Acquire the Bitwarden CLI if it is not already there.
///
/// `Ok(None)` means "already present, nothing to do" and is the common case.
/// `Ok(Some(path))` means a Bitwarden-signed `bw.exe` is now at the path this
/// process already recorded, so the sign-in that triggered this can carry
/// straight on into `bw config server` and `bw login` without the user
/// retrying anything.
///
/// # The ordering is the whole function
///
/// Resolve, download, check the digest, extract, **verify**, and only then
/// install. `install` is unreachable except through `verify`, and there is no
/// other path to the install destination in this module. A verified-after-
/// install implementation would pass a test that checked the final file's
/// signature and still leave an unverified binary at the path this process
/// already trusts, for however long the check took.
///
/// Nothing is executed, at any point, on any arm. See
/// [`nothing_in_this_module_starts_a_process`].
pub fn acquire_if_needed(
    env: &BwAcquireEnv,
    on_stage: &dyn Fn(AcquireStage),
) -> Result<AcquireOutcome, AcquireRefusal> {
    if let Some(existing) = (env.already_present)() {
        return Ok(AcquireOutcome::AlreadyPresent(existing));
    }

    on_stage(AcquireStage::Resolving);
    let api = crate::http_agent::bounded_total(API_CONNECT, API_TOTAL);
    let artefact = (env.resolve)(&api)?;

    let scratch = scratch_dir()?;
    // Sweep first, the way `updater::cleanup_stale_downloads` sweeps
    // installers: a window closed mid-download leaves a partial archive, and
    // nothing else ever removes it.
    sweep_scratch(&scratch);

    let downloads = crate::http_agent::bounded_stall(DOWNLOAD_CONNECT, DOWNLOAD_STALL);
    let archive = (env.download)(&downloads, &artefact, &scratch, &|done, total| {
        on_stage(AcquireStage::Downloading { done, total })
    })?;

    // Every arm from here on deletes the archive. It is a network-supplied
    // file in a predictable location and there is no reason to keep one after
    // the one thing it was fetched for has happened or failed.
    let extracted = match (env.extract)(&archive, &scratch) {
        Ok(path) => path,
        Err(e) => {
            discard(&archive);
            return Err(e);
        }
    };
    discard(&archive);

    on_stage(AcquireStage::Verifying);
    // **The gate.** Not a predicate pinned in isolation somewhere else -- a
    // `?` in this body, in a gating position, with `install` below it and
    // nothing else in this module able to reach the install destination. The
    // lesson `updater::apply_update_with` paid for applies unchanged: a pin
    // on a pure decision cannot see whether the decision is in a gating
    // position.
    if let Err(e) = (env.verify)(&extracted) {
        // Deleted, not quarantined and not retried. See
        // `AcquireRefusal::retryable`.
        discard(&extracted);
        return Err(e);
    }

    on_stage(AcquireStage::Installing);
    let installed = (env.install)(&extracted);
    discard(&extracted);
    let path = installed?;
    // Measured off the file that actually landed, not off the artefact
    // record: the confirmation the user reads must describe what is on their
    // disk. A metadata failure is not worth refusing a verified, installed
    // binary over, so the size falls back to 0 and the summary says so by
    // showing `0.0 MB` rather than inventing the artefact's claimed length.
    let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    Ok(AcquireOutcome::Installed(AcquiredCli {
        version: version_from_asset(&artefact.asset_name),
        path,
        bytes,
    }))
}

/// What [`acquire_if_needed`] settled.
///
/// Two variants and not an `Option`, because the sign-in window does
/// genuinely different things with them: [`Self::AlreadyPresent`] shows no
/// modal at all and goes straight to the login, while [`Self::Installed`] has
/// a confirmation to paint before it does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcquireOutcome {
    /// There was already a `bw.exe` at the recorded path. The common case:
    /// no modal, no request, nothing said.
    AlreadyPresent(PathBuf),
    /// One was downloaded, verified and installed just now.
    Installed(AcquiredCli),
}

impl AcquireOutcome {
    /// Where the CLI is, on either arm.
    #[must_use]
    pub fn path(&self) -> &Path {
        match self {
            Self::AlreadyPresent(path) => path,
            Self::Installed(cli) => &cli.path,
        }
    }
}

// ---- the six production functions ------------------------------------------

/// `Some(path)` when a `bw.exe` already exists at the path this process
/// recorded at startup.
///
/// # Why "exists" and not "exists and is Bitwarden-signed"
///
/// Because the `OnceLock` has already been set and cannot be re-set. If a
/// file exists at the recorded path, `main`'s `check_bw_signature` **has
/// already graded it** -- refusing outright if it is unsigned or tampered,
/// asking the user if it is validly signed by an unrecognised organization --
/// and this process will use that file whatever this module does. Installing
/// a second copy somewhere the process is not going to look would be theatre.
///
/// So acquisition triggers on **absence**, which is exactly the state the
/// deleted installer bootstrap used to prevent and now nothing does.
pub fn resolve_present_and_trusted() -> Option<PathBuf> {
    crate::bw_path::verified_bw_exe()
        .filter(|path| path.exists())
        .map(Path::to_path_buf)
}

/// The releases listing, filtered down to one artefact.
///
/// Every transport failure maps to [`AcquireRefusal::Offline`] -- including a
/// stall, which `http_agent::bounded_stall` surfaces as an I/O error on the
/// body read. The user-visible difference between "no DNS" and "the transfer
/// stopped" is nothing they can act on differently.
fn resolve_artefact(agent: &TotalBounded) -> Result<BwArtefact, AcquireRefusal> {
    let releases: Vec<Release> = agent
        .get(RELEASES_URL)
        // GitHub rejects an API request with no User-Agent. Same header the
        // updater sends, for the same reason.
        .set("User-Agent", concat!("deskwarden/", env!("CARGO_PKG_VERSION")))
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| AcquireRefusal::Offline(format!("failed to reach the releases API: {e}")))?
        .into_json()
        .map_err(|e| AcquireRefusal::Offline(format!("failed to parse the releases list: {e}")))?;
    pick_artefact(&releases)
}

/// The newest published `cli-v*` release's Windows asset, with the digest
/// GitHub published for **that same asset object**.
///
/// Pure, so the whole of the interesting behaviour is testable without a
/// socket -- and it is where the house defect would hide if it hid anywhere
/// in this module. A resolver that "found an asset" passes a naive test while
/// returning `bw-oss-windows`, or `bw-linux`, or a prerelease.
fn pick_artefact(releases: &[Release]) -> Result<BwArtefact, AcquireRefusal> {
    // The monorepo filter. Anchored `strip_prefix`, drafts and prereleases
    // excluded, ordered by the version in the tag rather than by position in
    // the list -- GitHub's ordering is by date across four products and is
    // not this module's to rely on.
    let mut candidates: Vec<(semver::Version, &Release)> = releases
        .iter()
        .filter(|r| !r.draft && !r.prerelease)
        .filter_map(|r| {
            let rest = r.tag_name.strip_prefix(CLI_TAG_PREFIX)?;
            // A tag this build cannot read as a version is SKIPPED rather
            // than fatal, carrying `updater::check_for_update`'s decision:
            // one odd historical tag must not take the whole feature down.
            Some((semver::Version::parse(rest).ok()?, r))
        })
        .collect();
    candidates.sort_by(|a, b| b.0.cmp(&a.0));

    let Some((version, release)) = candidates.first() else {
        return Err(AcquireRefusal::NoArtefact(
            "the releases listing contains no published Bitwarden CLI release".to_string(),
        ));
    };

    // ONE asset object, bound once, both fields read from it. Deliberately
    // not two searches -- see `BwArtefact`.
    let asset = release
        .assets
        .iter()
        .find(|a| {
            a.name.starts_with(WINDOWS_ASSET_PREFIX) && a.name.ends_with(WINDOWS_ASSET_SUFFIX)
        })
        .ok_or_else(|| {
            AcquireRefusal::NoArtefact(format!(
                "Bitwarden CLI release {version} has no {WINDOWS_ASSET_PREFIX}*\
                 {WINDOWS_ASSET_SUFFIX} asset"
            ))
        })?;

    // **Absent is a refusal, not a permission to skip the check.** This
    // carries `updater::ReleaseInfo::installer_sha256`'s decision -- the
    // digest is a required value rather than an `Option` by the time anything
    // downstream sees it -- so "we could not check" cannot become "proceed"
    // by anyone forgetting a branch.
    let digest_field = asset.digest.as_deref().ok_or_else(|| {
        AcquireRefusal::NoArtefact(format!(
            "Bitwarden CLI asset '{}' carries no digest, so the download could not be \
             verified at all; refusing to fetch it",
            asset.name
        ))
    })?;
    let digest = parse_asset_digest(digest_field)
        .map_err(|e| AcquireRefusal::NoArtefact(format!("Bitwarden CLI asset digest: {e}")))?;

    if asset.browser_download_url.is_empty() {
        return Err(AcquireRefusal::NoArtefact(format!(
            "Bitwarden CLI asset '{}' has no download URL",
            asset.name
        )));
    }

    Ok(BwArtefact {
        url: asset.browser_download_url.clone(),
        digest,
        asset_name: asset.name.clone(),
    })
}

/// Streams the archive into `dir` and refuses anything whose SHA-256 is not
/// the one GitHub published for the asset.
///
/// The digest comes from `artefact` -- i.e. from the API response that also
/// supplied the URL just downloaded from -- and never from a parameter. A
/// caller that picks the value a check is made against picks the answer;
/// `updater::download_and_verify` removed exactly that weakness and this does
/// not reintroduce it.
///
/// Remember what this proves and what it does not: **integrity, not
/// authenticity**. See the module docs.
fn download_artefact(
    agent: &StallBounded,
    artefact: &BwArtefact,
    dir: &Path,
    on_progress: &dyn Fn(u64, Option<u64>),
) -> Result<PathBuf, AcquireRefusal> {
    let dest = dir.join(&artefact.asset_name);
    let response = agent
        .get(&artefact.url)
        .call()
        .map_err(|e| AcquireRefusal::Offline(format!("failed to download the CLI: {e}")))?;
    // Advisory only: a server may omit it and a chunked response has none, so
    // it is passed on as an `Option` rather than defaulted to zero and
    // silently reported as "0 bytes total".
    let total = response
        .header("Content-Length")
        .and_then(|v| v.parse::<u64>().ok());
    let mut reader = response.into_reader();
    let mut file = std::fs::File::create(&dest).map_err(|e| {
        AcquireRefusal::CouldNotInstall(format!("could not create {}: {e}", dest.display()))
    })?;
    // Reused, not reimplemented: the contract `updater` pins for this
    // function -- the final call always reports the total bytes actually
    // written -- is what stops the bar sticking at 97% on a finished
    // transfer.
    if let Err(e) = crate::updater::copy_reporting(&mut reader, &mut file, total, on_progress) {
        drop(file);
        discard(&dest);
        return Err(AcquireRefusal::Offline(format!("the CLI download stopped: {e}")));
    }
    drop(file);

    // Both arms delete, and writing them as two arms of one decision rather
    // than as a `?` is the point: a hash that could not be COMPUTED is
    // exactly as much a refusal as a hash that did not MATCH.
    let actual = match crate::updater::file_sha256(&dest) {
        Ok(actual) => actual,
        Err(_) => {
            discard(&dest);
            return Err(AcquireRefusal::DigestMismatch);
        }
    };
    if actual != artefact.digest {
        discard(&dest);
        return Err(AcquireRefusal::DigestMismatch);
    }
    Ok(dest)
}

/// Pulls `bw.exe` out of the archive into `dir`.
///
/// The entry is matched on its **file name**, and any leading directory
/// components in the archive are discarded rather than joined onto `dir` --
/// a zip entry named `..\..\Windows\System32\bw.exe` is a well-known way to
/// write outside the extraction directory, and this module refuses to be the
/// thing that does it. The one file that comes out lands at `dir/bw.exe` and
/// nowhere else, by construction rather than by sanitising a string.
///
/// Nothing extracted here is executed. See the module docs.
fn extract_bw_exe(archive: &Path, dir: &Path) -> Result<PathBuf, AcquireRefusal> {
    let file = std::fs::File::open(archive).map_err(|e| {
        AcquireRefusal::NoArtefact(format!("could not open {}: {e}", archive.display()))
    })?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| {
        AcquireRefusal::NoArtefact(format!("the Bitwarden CLI download is not a zip archive: {e}"))
    })?;

    let mut index = None;
    for i in 0..zip.len() {
        let entry = zip
            .by_index(i)
            .map_err(|e| AcquireRefusal::NoArtefact(format!("unreadable archive entry: {e}")))?;
        // `enclosed_name` is `None` for exactly the traversal shapes above;
        // the file-name comparison then makes the match independent of any
        // directory prefix the archive chose.
        let matches = entry
            .enclosed_name()
            .and_then(|p| p.file_name().map(|n| n.eq_ignore_ascii_case(BW_EXE_NAME)))
            .unwrap_or(false);
        if matches {
            index = Some(i);
            break;
        }
    }
    let Some(index) = index else {
        return Err(AcquireRefusal::NoArtefact(format!(
            "the Bitwarden CLI download contains no {BW_EXE_NAME}"
        )));
    };

    let mut entry = zip
        .by_index(index)
        .map_err(|e| AcquireRefusal::NoArtefact(format!("unreadable archive entry: {e}")))?;
    let dest = dir.join(BW_EXE_NAME);
    let mut out = std::fs::File::create(&dest).map_err(|e| {
        AcquireRefusal::CouldNotInstall(format!("could not create {}: {e}", dest.display()))
    })?;
    std::io::copy(&mut entry, &mut out).map_err(|e| {
        AcquireRefusal::CouldNotInstall(format!("could not extract {BW_EXE_NAME}: {e}"))
    })?;
    Ok(dest)
}

/// **The load-bearing check.** Is this file carrying a valid Authenticode
/// signature whose `O=` names Bitwarden?
///
/// In-process, via `signature::verify_authenticode` -> `WinVerifyTrust`, and
/// **not** via PowerShell's `Get-AuthenticodeSignature`. That cmdlet lives in
/// `Microsoft.PowerShell.Security` and fails with "the module could not be
/// loaded" wherever autoloading is unavailable, which is a trust gate that
/// cannot answer. The deleted `bootstrap-bw.ps1` used the cmdlet, and
/// hand-maintained a second X.500 DN parser to go with it -- two parsers of
/// one grammar, in two languages, held together by a comment. Moving
/// acquisition into the app collapsed both mechanisms into one, and
/// `signature::verification_needs_no_external_process` pins the survivor from
/// the other side.
fn verify_is_bitwardens(path: &Path) -> Result<(), AcquireRefusal> {
    judge(&crate::signature::verify_authenticode(path))
}

/// The verdict, as a pure function of what the OS said, so the three-way
/// distinction is testable without a signed file.
///
/// Three outcomes and not two: "the OS said no" and "the OS could not say"
/// are different facts about the world, one of them worth retrying and one of
/// them not.
fn judge(
    result: &Result<crate::signature::SignatureInfo, String>,
) -> Result<(), AcquireRefusal> {
    match result {
        Ok(info) => {
            if crate::signature::is_trusted_organization(
                info,
                crate::signature::TRUSTED_BW_SIGNER_ORGANIZATIONS,
            ) {
                Ok(())
            } else {
                // Covers both "validly signed by someone else" and "not
                // validly signed at all": `is_trusted_organization` returns
                // false for `valid: false` before it looks at the DN, so a
                // tampered binary carrying `O=Bitwarden Inc.` lands here too.
                // Unlike `main`'s startup grading, this does not ask the user
                // -- a file this app fetched thirty seconds ago has no excuse
                // for being unrecognised.
                Err(AcquireRefusal::NotBitwardenSigned {
                    subject_dn: info.subject_dn.clone(),
                })
            }
        }
        Err(e) => Err(AcquireRefusal::Unverifiable(e.clone())),
    }
}

/// Copies the verified binary to the path this process already recorded, and
/// puts its directory on the user's `PATH`.
///
/// Both effects, because both are what the deleted bootstrap wrote. An
/// existing user who deletes their `bw.exe` must get the replacement in the
/// same spot their `HKCU\Environment` entry already points at, not a
/// differently-placed one that leaves a stale entry aimed at nothing.
fn install_at_the_resolver_path(verified: &Path) -> Result<PathBuf, AcquireRefusal> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .ok_or_else(|| {
            AcquireRefusal::CouldNotInstall(
                "Deskwarden could not work out its own install directory".to_string(),
            )
        })?;
    let dest = install_destination(&exe_dir);
    let bin = dest.parent().ok_or_else(|| {
        AcquireRefusal::CouldNotInstall("the install destination has no parent".to_string())
    })?;
    std::fs::create_dir_all(bin).map_err(|e| {
        AcquireRefusal::CouldNotInstall(format!("could not create {}: {e}", bin.display()))
    })?;
    std::fs::copy(verified, &dest).map_err(|e| {
        AcquireRefusal::CouldNotInstall(format!("could not write {}: {e}", dest.display()))
    })?;
    // Best-effort, and deliberately not fatal: the app itself reaches the CLI
    // by absolute path (`bw_path::bw_command_in`), so a failed PATH write
    // costs the user a `bw` on their own command line and costs Deskwarden
    // nothing. Turning it into a refusal would discard a verified binary over
    // a convenience.
    if let Err(e) = add_to_user_path(bin) {
        log::warn!("could not add {} to the user PATH: {e}", bin.display());
    }
    Ok(dest)
}

/// `<install dir>\bin\bw.exe`, from `bw_path`'s own function.
///
/// A one-line forwarder on purpose: it gives the tests a name to assert
/// *agreement* against, and it means this module contains no second spelling
/// of the path. See the module docs for why a second spelling is the defect
/// worth pinning against.
fn install_destination(exe_dir: &Path) -> PathBuf {
    crate::bw_path::install_bin_candidate(exe_dir)
}

/// Where partial downloads and extracted binaries live before they are judged.
///
/// Beside the updater's own download directory, under the OS cache location,
/// so nothing here is ever written into the install tree until it has passed.
fn scratch_dir() -> Result<PathBuf, AcquireRefusal> {
    let dirs = directories::ProjectDirs::from("", "", "deskwarden").ok_or_else(|| {
        AcquireRefusal::CouldNotInstall("could not locate a cache directory".to_string())
    })?;
    let dir = dirs.cache_dir().join("bw-cli");
    std::fs::create_dir_all(&dir).map_err(|e| {
        AcquireRefusal::CouldNotInstall(format!("could not create {}: {e}", dir.display()))
    })?;
    Ok(dir)
}

/// Deletes everything in the scratch directory.
///
/// A window closed mid-download leaves a partial archive, and **nothing is
/// retried in the background** -- the next sign-in states the requirement
/// again from the top -- so a leftover would otherwise live forever. Same
/// best-effort shape as `updater::cleanup_stale_downloads`: a file that
/// cannot be removed is logged and skipped, never fatal.
fn sweep_scratch(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.path().is_file() {
            discard(&entry.path());
        }
    }
}

/// Deletes a file this module has finished with or refused, and says so.
///
/// **A refused binary must not be left behind.** Best-effort, and
/// deliberately does not turn a failed delete into a different error: the
/// caller is already returning a refusal and that refusal is the important
/// half.
fn discard(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => log::warn!("could not delete {}: {e}", path.display()),
    }
}

/// Appends `dir` to the user's `PATH` in `HKCU\Environment` and tells the
/// shell about it, if it is not already there.
///
/// The same two effects the deleted `bootstrap-bw.ps1` had. The broadcast is
/// what makes an already-running Explorer notice; without it the entry is
/// real but nothing inherits it until the next sign-out.
///
/// **`WM_SETTINGCHANGE` and `HWND_BROADCAST` are the `windows` crate's named
/// constants, never numeric literals** -- the crate-wide rule, and the reason
/// for it is that a mistyped `0x001A` is indistinguishable from a correct one
/// at a glance.
fn add_to_user_path(dir: &Path) -> Result<(), String> {
    use windows::core::w;
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
        KEY_READ, KEY_WRITE, REG_EXPAND_SZ, REG_SZ, REG_VALUE_TYPE,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
    };

    let wanted = dir.to_string_lossy().to_string();
    let mut key = HKEY::default();
    // SAFETY: `w!("Environment")` is a static NUL-terminated wide literal and
    // `key` is a live local the call writes exactly one handle into.
    unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            w!("Environment"),
            0,
            KEY_READ | KEY_WRITE,
            &mut key,
        )
        .ok()
        .map_err(|e| format!("could not open HKCU\\Environment: {e}"))?;
    }

    let result = (|| -> Result<(), String> {
        // Read the existing value, and its TYPE: an existing `REG_EXPAND_SZ`
        // must be written back as `REG_EXPAND_SZ`, or every `%VAR%` already
        // in the user's PATH stops expanding. That is a way to break a
        // machine while adding one entry to it.
        let mut kind = REG_VALUE_TYPE::default();
        let mut bytes: u32 = 0;
        // SAFETY: a size query -- null data pointer with a live length out
        // parameter, which is the documented way to ask.
        let queried = unsafe {
            RegQueryValueExW(
                key,
                w!("Path"),
                None,
                Some(&mut kind),
                None,
                Some(&mut bytes),
            )
        };
        let (existing, kind) = if queried.is_ok() && bytes > 0 {
            let mut buf = vec![0u16; (bytes as usize).div_ceil(2)];
            let mut len = bytes;
            // SAFETY: `buf` is sized from the query above and `len` is its
            // byte length; both outlive the call.
            unsafe {
                RegQueryValueExW(
                    key,
                    w!("Path"),
                    None,
                    Some(&mut kind),
                    Some(buf.as_mut_ptr().cast()),
                    Some(&mut len),
                )
            }
            .ok()
            .map_err(|e| format!("could not read the user PATH: {e}"))?;
            let chars = (len as usize / 2).min(buf.len());
            let text = String::from_utf16_lossy(&buf[..chars]);
            (text.trim_end_matches('\0').to_string(), kind)
        } else {
            (String::new(), REG_SZ)
        };

        // Already there? Compared entry by entry and case-insensitively, with
        // trailing separators stripped, because `C:\dir\` and `C:\dir` are the
        // same directory and appending a second copy every launch is how a
        // PATH becomes a mile long.
        let already = existing.split(';').any(|entry| {
            entry.trim().trim_end_matches('\\').eq_ignore_ascii_case(
                wanted.trim_end_matches('\\'),
            )
        });
        if already {
            return Ok(());
        }

        let updated = if existing.trim().is_empty() {
            wanted.clone()
        } else {
            format!("{};{wanted}", existing.trim_end_matches(';'))
        };
        let mut wide: Vec<u16> = updated.encode_utf16().collect();
        wide.push(0);
        let kind = if kind == REG_EXPAND_SZ { REG_EXPAND_SZ } else { REG_SZ };
        // SAFETY: `wide` is a live NUL-terminated buffer and the byte length
        // handed over is its own.
        unsafe {
            RegSetValueExW(
                key,
                w!("Path"),
                0,
                kind,
                Some(std::slice::from_raw_parts(
                    wide.as_ptr().cast::<u8>(),
                    wide.len() * 2,
                )),
            )
        }
        .ok()
        .map_err(|e| format!("could not write the user PATH: {e}"))?;

        // Tell everything already running. `SendMessageTimeoutW` with
        // `SMTO_ABORTIFHUNG` rather than `SendMessageW`, because
        // `HWND_BROADCAST` reaches every top-level window on the desktop and
        // one hung application would otherwise block this thread forever.
        // SAFETY: the wide string is a static literal and outlives the call;
        // the timeout bounds it.
        unsafe {
            SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                WPARAM(0),
                LPARAM(w!("Environment").as_ptr() as isize),
                SMTO_ABORTIFHUNG,
                5_000,
                None,
            );
        }
        Ok(())
    })();

    // SAFETY: `key` came from a successful `RegOpenKeyExW` above and is
    // closed exactly once, on every arm.
    unsafe {
        let _ = RegCloseKey(key);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Task 1: the gate, the seam, and the words -------------------------

    /// **The gate is `backend_policy`'s answer, not a second one.**
    ///
    /// Without this, [`this_sign_in_needs_the_cli`] could be written as
    /// `!is_self_hosted(server)` -- which agrees today and drifts the first
    /// time `choose` gains an input. The whole table, both directions.
    #[test]
    fn the_gate_is_exactly_the_backend_policy_decision() {
        let mut saw_true = false;
        let mut saw_false = false;
        for server in [
            None,
            Some(""),
            Some("https://vault.example.com"),
            Some("https://vault.bitwarden.com"),
            Some("https://bitwarden.eu"),
            Some("https://vault.bitwarden.community"),
        ] {
            for use_official in [true, false] {
                let expected = choose(server, use_official) == VaultBackendChoice::BwServe;
                assert_eq!(
                    this_sign_in_needs_the_cli(server, use_official),
                    expected,
                    "the gate disagrees with `backend_policy::choose` for \
                     server={server:?} use_official={use_official}"
                );
                saw_true |= expected;
                saw_false |= !expected;
            }
        }
        // **Positive control on the table, not on the gate.** A `choose` that
        // answered one way for every row would make the loop above pass
        // against a `this_sign_in_needs_the_cli` hardcoded to that same
        // answer. Both verdicts have to occur in the walked table for the
        // agreement to mean anything.
        assert!(saw_true, "control: no row in the table needs the CLI at all");
        assert!(saw_false, "control: every row in the table needs the CLI");
    }

    /// The one arm that must never acquire, asserted on its own so a refactor
    /// of the table above cannot quietly lose it.
    #[test]
    fn a_self_hosted_account_on_the_built_in_client_never_needs_the_cli() {
        assert!(!this_sign_in_needs_the_cli(Some("https://vault.example.com"), false));
        // Positive control: the same server with the official CLI chosen DOES
        // need it, so the assertion above is reading both inputs and not just
        // the URL. This is also the row the design document got wrong -- see
        // `this_sign_in_needs_the_cli`.
        assert!(this_sign_in_needs_the_cli(Some("https://vault.example.com"), true));
    }

    /// **What the sign-in window actually asks, composed the way it composes
    /// it**, rather than the gate alone.
    ///
    /// The gate takes a `bool`, and the tests above pin it against `choose`
    /// for every value of that bool. Neither says where the bool comes from --
    /// and that is the half the acquisition modal now turns on, because a
    /// *new* account has no server yet and so no property of its record can
    /// answer the question. `login_ui` passes
    /// `accounts::official_cli_after_sign_in(account, typed_server)`, so this
    /// walks the same composition over the cases that differ.
    ///
    /// Each row is a `(what the user is doing, does the CLI get downloaded)`
    /// pair, and the three rows are three different reasons for the answer.
    #[test]
    fn the_acquisition_modal_fires_for_official_servers_and_not_for_new_self_hosted_ones() {
        use crate::accounts::{Account, AccountId, official_cli_after_sign_in};

        let id = || AccountId::parse(&"a".repeat(32)).expect("a 32-hex test id");
        // What `prepare_new_account` produces: no address, no server.
        let mint = Account {
            id: id(),
            email: String::new(),
            server_url: None,
            use_official_bw_crypto: true,
        };
        // A self-hoster who has been on `bw serve` all along.
        let established_on_the_cli = Account {
            id: id(),
            email: "me@example.com".to_string(),
            server_url: Some("https://vault.example.com".to_string()),
            use_official_bw_crypto: true,
        };

        let needs_cli = |account: Option<&Account>, typed: Option<&str>| {
            this_sign_in_needs_the_cli(typed, official_cli_after_sign_in(account, typed, None))
        };

        // **A new account signing in to a self-hosted server: no download.**
        // This is the row the whole change exists for.
        assert!(
            !needs_cli(Some(&mint), Some("https://vault.example.com")),
            "a new self-hosted account was asked to download the Bitwarden CLI, which is the \
             download the built-in client exists to avoid"
        );

        // **A new account signing in to bitwarden.com: the modal fires.**
        // `None` is what the form leaves for the official cloud, and the
        // explicit cloud URL is what it leaves when the user types one.
        assert!(
            needs_cli(Some(&mint), None),
            "a new bitwarden.com account no longer acquires the CLI, so official servers get \
             a backend this app does not implement for them"
        );
        assert!(
            needs_cli(Some(&mint), Some("https://vault.bitwarden.com")),
            "a new account against an official Bitwarden host no longer acquires the CLI"
        );

        // **An established self-hosted account on `bw serve` still needs it.**
        // Its vault is held by the CLI; answering "no" here would be the
        // upgrade regression showing up as a missing binary instead.
        assert!(
            needs_cli(Some(&established_on_the_cli), Some("https://vault.example.com")),
            "an existing self-hosted account served by `bw serve` was refused the CLI its \
             vault is actually held by"
        );

        // And the typed address does NOT override an established account:
        // same record, official URL typed, still the record's answer.
        assert!(
            needs_cli(Some(&established_on_the_cli), None),
            "an established account's backend followed the typed server rather than its own \
             record"
        );
    }

    /// `production()` holds the real functions.
    ///
    /// Copied in shape from
    /// `updater::production_holds_the_real_hash_and_the_real_launch`, which
    /// exists because a `production()` quietly wired to a stub is a seam that
    /// tests everything except what ships. All six named, because the one
    /// left out is the one that gets substituted.
    #[test]
    fn production_holds_the_real_six() {
        let env = BwAcquireEnv::production();
        let present: fn() -> Option<PathBuf> = resolve_present_and_trusted;
        let resolve: fn(&TotalBounded) -> Result<BwArtefact, AcquireRefusal> = resolve_artefact;
        let download: fn(
            &StallBounded,
            &BwArtefact,
            &Path,
            &dyn Fn(u64, Option<u64>),
        ) -> Result<PathBuf, AcquireRefusal> = download_artefact;
        let extract: fn(&Path, &Path) -> Result<PathBuf, AcquireRefusal> = extract_bw_exe;
        let verify: fn(&Path) -> Result<(), AcquireRefusal> = verify_is_bitwardens;
        let install: fn(&Path) -> Result<PathBuf, AcquireRefusal> = install_at_the_resolver_path;
        assert!(std::ptr::fn_addr_eq(env.already_present, present));
        assert!(std::ptr::fn_addr_eq(env.resolve, resolve));
        assert!(std::ptr::fn_addr_eq(env.download, download));
        assert!(std::ptr::fn_addr_eq(env.extract, extract));
        assert!(std::ptr::fn_addr_eq(env.verify, verify));
        assert!(std::ptr::fn_addr_eq(env.install, install));
    }

    /// **Every refusal names the Bitwarden CLI, the server, and the way out.**
    ///
    /// The owner's ruling -- "yes, no silent - we say that it is requared
    /// period" -- held by the file rather than by review.
    #[test]
    fn every_refusal_names_the_cli_the_server_and_the_alternative() {
        let all = [
            AcquireRefusal::Declined,
            AcquireRefusal::Offline("x".into()),
            AcquireRefusal::NoArtefact("x".into()),
            AcquireRefusal::DigestMismatch,
            AcquireRefusal::Unverifiable("x".into()),
            AcquireRefusal::CouldNotInstall("x".into()),
            AcquireRefusal::NotBitwardenSigned { subject_dn: None },
        ];
        for r in &all {
            let m = r.message();
            assert!(m.contains("Bitwarden CLI"), "{m:?} does not name what is required");
            assert!(m.contains("bitwarden.com"), "{m:?} does not name the server that requires it");
            assert!(m.contains("self-hosted"), "{m:?} does not name the way out");
            // An empty or one-word message passes a `contains` test against
            // nothing, so the length floor is part of the assertion.
            assert!(m.len() > 60, "{m:?} is too short to have said any of that");
        }

        // **Positive control for the PREDICATE, not for the messages.** A
        // euphemism of the kind the ruling forbids must FAIL the same check
        // the seven above pass -- otherwise the loop is asserting nothing.
        let euphemism = "Something went wrong while setting up. Try again in a moment, please.";
        assert!(euphemism.len() > 60, "control: the euphemism clears the length floor");
        assert!(
            !euphemism.contains("Bitwarden CLI"),
            "control: the predicate does not distinguish a euphemism from a real message"
        );
    }

    /// **The decline path is held to the same standard.** The owner's ruling
    /// covers it explicitly: a user who says No is still told what they
    /// declined, that bitwarden.com requires it, and that self-hosting does
    /// not. Asserted separately from the loop above so that dropping
    /// `Declined` from that array cannot silently drop this.
    #[test]
    fn declining_is_told_what_was_declined_and_what_to_do_instead() {
        let m = AcquireRefusal::Declined.message();
        assert!(m.contains("Bitwarden CLI"));
        assert!(m.contains("bitwarden.com"));
        assert!(m.contains("self-hosted"));
        // And it says the door is still open, which the other six do not need
        // to say because their causes are not the user's choice.
        assert!(
            m.contains("Continue"),
            "a declined user is not told how to change their mind: {m:?}"
        );
    }

    /// Retryability is per-variant, and the signature failure is the one that
    /// is not: a retry loop against a substituted artefact is a loop.
    #[test]
    fn only_the_signature_refusal_refuses_to_retry() {
        assert!(!AcquireRefusal::NotBitwardenSigned { subject_dn: None }.retryable());
        // Control: the others do, so the assertion above is reading the
        // variant and not a `retryable()` hardcoded to false.
        for r in [
            AcquireRefusal::Declined,
            AcquireRefusal::Offline("x".into()),
            AcquireRefusal::NoArtefact("x".into()),
            AcquireRefusal::DigestMismatch,
            AcquireRefusal::Unverifiable("x".into()),
            AcquireRefusal::CouldNotInstall("x".into()),
        ] {
            assert!(r.retryable(), "{:?} should offer another attempt", r.message());
        }
    }

    /// Every shape of the choice modal, for the tests below.
    ///
    /// **There is exactly one**, and this function stays as a function
    /// rather than being inlined so that the tests below keep reading "every
    /// shape" -- if the state ever regains a field, they widen by widening
    /// this, and no assertion below has to be remembered.
    fn every_choice() -> Vec<CliSetupState> {
        vec![CliSetupState::Choosing]
    }

    /// **The choice states both options honestly and steers toward neither.**
    ///
    /// `prefs_ui`'s rules, applied to the one screen where a user picks
    /// between two clients: name what each does and what it costs, never the
    /// word "secure", and no marketing. The specific claims asserted here are
    /// the ones a reader cannot check for themselves and would be worse off
    /// not being told.
    #[test]
    fn the_choice_names_both_costs_and_recommends_neither() {
        for state in every_choice() {
            let body = state.body().join(" ");

            // **The built-in client's real costs.** The first two are the
            // brief's own list; the third is the one the brief did not have
            // and `prefs_ui::official_crypto_description` does -- the stored
            // vault key. An omission the user cannot discover is worse than
            // a stated limitation, and this is the sentence that states it.
            assert!(
                body.contains("never expires") && body.contains("kept on this PC"),
                "{state:?} does not say the built-in client keeps a non-expiring vault key \
                 on this PC. That is the cost the user cannot find out any other way, and \
                 it is the one Preferences already tells them about: {body:?}"
            );
            // **And what it must NOT claim.** This assertion was the
            // opposite one in 0.15.5: it required the modal to say the
            // built-in client "cannot put a file in a Send". That is true and
            // is not a COST, because the CLI path in this app builds text
            // Sends too (`rest::send`'s module doc: "parity with the other
            // backend and not a subtraction from it"), and neither path can
            // receive an e-mail-gated Send -- `send::receive_invocation` is
            // `bw send receive [--passwordenv X] <url>` and has no e-mail or
            // OTP route.
            //
            // Reversed rather than deleted, and reversed knowingly: a modal
            // that hands one answer the blue button and then invents
            // drawbacks for it is not being even-handed, it is being wrong,
            // and the wrongness runs in the direction that flatters the OTHER
            // answer. The direction of this guard is the whole point of it.
            for parity in ["text only", "file in a Send", "email address", "e-mail address"] {
                assert!(
                    !body.contains(parity),
                    "{state:?} presents {parity:?} as something the built-in client gives \
                     up. Both clients are equal on it in this app, so it is not a cost of \
                     this choice: {body:?}"
                );
            }

            // **The CLI's real case, including the part that argues against
            // the other option.** A choice screen that would not say
            // "Deskwarden's client is not Bitwarden's code" is a choice
            // screen selling one of its answers.
            assert!(
                body.contains("not Bitwarden's code"),
                "{state:?} never says the built-in client is not Bitwarden's own code, \
                 which is the honest case FOR the CLI: {body:?}"
            );
            assert!(
                body.contains("test vectors"),
                "{state:?} claims the built-in client's cryptography without saying what \
                 checks it: {body:?}"
            );
            // The cost of the other side, in the same words the requirement
            // modal uses.
            assert!(
                body.contains("37 MB") && body.contains("Bitwarden signed it"),
                "{state:?} asks for a download without disclosing its size or the check: \
                 {body:?}"
            );

            // **The numbers, which are the whole point of the rewrite.**
            // A cost named without its size is not a cost the user can weigh,
            // and these four are `README.md`'s measured figures. Asserted
            // individually so a paragraph that drops one fails naming it.
            for figure in ["21 MB", "118 MB", "10 MB", "8 seconds"] {
                assert!(
                    body.contains(figure),
                    "{state:?} does not say {figure:?}. The choice between these two \
                     clients IS the resource cost, and a user cannot weigh it against a \
                     paragraph of adjectives: {body:?}"
                );
            }

            // **No steering IN THE PROSE.** "secure" is `prefs_ui`'s named
            // ban; "recommend" and "best" would pick for the user in words.
            //
            // The modal does now lean -- the built-in client holds the blue
            // primary button (see `buttons`) and its case is stated first.
            // That is deliberate, and this guard is deliberately NOT relaxed
            // to match: a lean carried by weight is one a user can see and
            // discount, and a lean carried by the word "recommended" is a
            // claim about which client is better that this app has no
            // standing to make. The costs are stated either way, which is
            // what makes the weight honest rather than a sales pitch.
            for banned in ["secure", "Secure", "recommend", "Recommend", "best", "safer"] {
                assert!(
                    !body.contains(banned),
                    "{state:?} uses {banned:?}, which either makes a claim this app cannot \
                     stand behind or chooses for the user: {body:?}"
                );
            }
        }
    }

    /// **One row, one order, and the built-in client is the blue one.**
    ///
    /// The behaviour that replaced the preselection-by-position this modal
    /// shipped with in 0.15.5. The order no longer depends on anything, so
    /// there is nothing left to parameterise over -- what is asserted instead
    /// is that the row is what it claims to be: two answers, no Cancel, and
    /// the affirmative slot (LAST, because `draw_cli_setup_modal` lays the
    /// row out right-to-left) held by the built-in client AND marked primary.
    #[test]
    fn the_choice_puts_the_built_in_client_in_the_blue_affirmative_slot() {
        for state in every_choice() {
            let buttons = state.buttons();
            assert_eq!(buttons.len(), 2, "{state:?} is a fork and must offer two answers");

            let (label, action, primary) = *buttons.last().expect("two buttons");
            assert_eq!(
                (action, primary),
                (CliModalAction::UseBuiltIn, true),
                "{state:?} put {label:?} in the affirmative slot. The built-in client \
                 belongs there, and it belongs there blue: it is what this app is for, \
                 and the button is the only place the modal says so"
            );

            // The other one is offered, and offered plainly.
            let (other_label, other_action, other_primary) =
                *buttons.first().expect("two buttons");
            assert_eq!(
                (other_action, other_primary),
                (CliModalAction::UseOfficialCli, false),
                "{state:?} offers {other_label:?} as its first button. Two primaries name \
                 no preference, and a missing CLI answer is not a fork"
            );

            // No Cancel: both prongs continue the sign-in, and a Cancel
            // would return the user to a form whose Continue leads straight
            // back here.
            let actions: Vec<_> = buttons.iter().map(|(_, a, _)| *a).collect();
            assert!(
                !actions.contains(&CliModalAction::Cancel),
                "{state:?} offers a Cancel that has nowhere to go"
            );

            // **And the copy no longer varies with the account.** The
            // sentence that did -- naming the client in use -- is the one
            // the owner cut, and its absence is asserted rather than assumed
            // because reinstating it is a one-line change that no other test
            // here would notice.
            let body = state.body().join(" ");
            assert!(
                !body.contains("account is using"),
                "{state:?} tells the user which client the account is using, which is the \
                 per-account sentence this modal was flattened to remove: {body:?}"
            );
        }
    }

    /// **No paragraph starts in lower case, in any state.**
    ///
    /// The defect this catches is specific and has happened twice in this
    /// modal: [`OFFICIAL_CLI_NAME`] is the phrase "the Bitwarden CLI",
    /// written to sit mid-sentence, and a paragraph interpolating it at the
    /// front renders "the Bitwarden CLI is Bitwarden's own program" -- which
    /// compiles, reads fine in the source, and is visible only in the picture.
    /// The sibling defect was a missing verb ("Choosing it **download** it"),
    /// which no test could have found either.
    ///
    /// Every state, not just the choice: the constants are shared, so any
    /// state can acquire this the same way.
    #[test]
    fn no_body_line_starts_in_lower_case() {
        let states = every_choice().into_iter().chain([
            CliSetupState::Asking,
            CliSetupState::AskingToRecover,
            CliSetupState::Installed(AcquiredCli {
                path: std::path::PathBuf::from(r"C:\invented\bw.exe"),
                version: Some("2026.1.0".to_string()),
                bytes: 37 * 1024 * 1024,
            }),
            CliSetupState::Failed(AcquireRefusal::Declined),
        ]);
        for state in states {
            for line in state.body() {
                let first = line.chars().next().expect("a body line is not empty");
                assert!(
                    !first.is_lowercase(),
                    "{state:?} paints a paragraph starting {first:?}: {line:?}. A shared \
                     name constant is written for the middle of a sentence"
                );
            }
        }
    }

    /// **At most one blue button per state**, which is what makes the flag
    /// mean "lean this way" rather than "this is a button".
    #[test]
    fn at_most_one_button_is_the_primary() {
        for state in every_choice().into_iter().chain([
            CliSetupState::Asking,
            CliSetupState::AskingToRecover,
            CliSetupState::Working { stage: AcquireStage::Verifying },
            CliSetupState::Failed(AcquireRefusal::Declined),
        ]) {
            let primaries = state.buttons().iter().filter(|(_, _, p)| *p).count();
            assert!(
                primaries <= 1,
                "{state:?} marks {primaries} buttons primary; two blue buttons express no \
                 preference between them"
            );
        }
    }

    /// Every stage names the Bitwarden CLI, so the status line cannot decay
    /// into "Setting up...". The download stage additionally names Bitwarden
    /// as the source, because that is the sentence that says where the file
    /// on its way to the machine came from.
    #[test]
    fn every_stage_says_what_it_is_doing_and_names_the_cli() {
        for stage in [
            AcquireStage::Resolving,
            AcquireStage::Downloading { done: 0, total: None },
            AcquireStage::Verifying,
            AcquireStage::Installing,
        ] {
            let label = stage.label();
            assert!(!label.is_empty(), "a bare spinner is not a stage label");
            let names_it = label.contains("Bitwarden");
            assert!(names_it, "{label:?} does not name Bitwarden at all");
        }
        assert!(AcquireStage::Downloading { done: 1, total: Some(2) }
            .label()
            .contains("from Bitwarden"));
        // Control: the labels are not all the same string, so the loop above
        // is reading four values rather than one.
        assert_ne!(AcquireStage::Verifying.label(), AcquireStage::Installing.label());
    }

    /// **The confirmation describes the file that landed, not the request.**
    ///
    /// The owner's requirement is that the third modal state names the
    /// version and the size of what was installed. A test that asserted the
    /// confirmation merely *contains* a version would pass against a build
    /// that printed a constant, which is precisely the vacuous kind: it would
    /// keep saying `2026.8.0` forever after the release moved.
    #[test]
    fn the_confirmation_reflects_the_file_that_landed() {
        let landed = AcquiredCli {
            path: PathBuf::from(r"C:\x\bin\bw.exe"),
            version: version_from_asset("bw-windows-2026.8.0.zip"),
            bytes: 38_695_474,
        };
        let summary = landed.summary();
        assert!(summary.contains("2026.8.0"), "{summary:?} does not name the version");
        assert!(summary.contains("36.9 MB"), "{summary:?} does not name the size");
        assert!(summary.contains("Bitwarden CLI"), "{summary:?} does not name what was installed");

        // **The control that makes the two above mean something**: a
        // DIFFERENT artefact of a DIFFERENT size produces a different
        // sentence. A summary built from constants would print the same
        // string for both.
        let other = AcquiredCli {
            path: PathBuf::from(r"C:\x\bin\bw.exe"),
            version: version_from_asset("bw-windows-2027.1.2.zip"),
            bytes: 12_345_678,
        };
        assert_ne!(summary, other.summary());
        assert!(other.summary().contains("2027.1.2"));
        assert!(!other.summary().contains("2026.8.0"), "the version is a constant, not a reading");
    }

    /// A name this module did not resolve yields no version rather than a
    /// wrong one -- a confirmation that states the wrong version is worse
    /// than one that admits it does not know.
    #[test]
    fn an_unrecognised_asset_name_yields_no_version() {
        assert_eq!(version_from_asset("bw-oss-windows-2026.8.0.zip"), None);
        assert_eq!(version_from_asset("something-else.zip"), None);
        assert_eq!(version_from_asset("bw-windows-.zip"), None);
        // Control: the shape this module actually resolves DOES parse, so the
        // assertions above are about anchoring and not about a broken parser.
        assert_eq!(
            version_from_asset("bw-windows-2026.8.0.zip"),
            Some("2026.8.0".to_string())
        );
        assert!(AcquiredCli {
            path: PathBuf::from("x"),
            version: None,
            bytes: 1,
        }
        .summary()
        .contains("unknown version"));
    }

    // ---- the modal's three states ------------------------------------------

    fn body_of(state: &CliSetupState) -> String {
        state.body().join(" ")
    }

    /// **State 1 names the thing, before anything is fetched.**
    ///
    /// This is the state the owner's ruling turns on, and the property that
    /// makes it a disclosure rather than a retroactive explanation is
    /// structural: [`CliSetupState::Asking`] is what the Submit path sets,
    /// and acquisition does not begin until the user presses OK. So the test
    /// is about the words -- that they name the CLI, say it is required, say
    /// what will happen to the machine, and say what it costs.
    #[test]
    fn the_ask_names_the_cli_the_requirement_the_action_and_the_cost() {
        let body = body_of(&CliSetupState::Asking);
        assert!(body.contains("Bitwarden CLI"), "{body:?} does not name what is required");
        assert!(body.contains("requires"), "{body:?} does not say it is required");
        assert!(body.contains("download"), "{body:?} does not say what will happen");
        assert!(body.contains("install"), "{body:?} does not say it will be installed");
        assert!(body.contains("37 MB"), "{body:?} does not say what it costs");
        assert!(body.contains("signed"), "{body:?} does not say it will be checked");
        assert!(body.contains("self-hosted"), "{body:?} does not name the alternative");
        assert!(
            CliSetupState::Asking.title().contains("Bitwarden CLI"),
            "the modal's own heading does not name what it is about"
        );
    }

    /// **State 1 offers exactly OK and Cancel**, and Cancel is a way back
    /// rather than a failure.
    #[test]
    fn the_ask_offers_ok_and_cancel_and_nothing_else() {
        let buttons = CliSetupState::Asking.buttons();
        assert_eq!(buttons.len(), 2, "state 1 offers {buttons:?}, not two buttons");
        assert_eq!(buttons[0], ("Cancel", CliModalAction::Cancel, false));
        assert_eq!(buttons[1], ("OK", CliModalAction::Begin, false));
    }

    /// **State 2 has a determinate bar and never a bare spinner.**
    #[test]
    fn progress_is_a_fraction_and_the_label_always_says_what_is_happening() {
        let half = CliSetupState::Working {
            stage: AcquireStage::Downloading { done: 50, total: Some(100) },
        };
        assert_eq!(half.progress(), Some(0.5));
        assert!(body_of(&half).contains("Downloading the Bitwarden CLI"));
        // The size is shown, because the size is the part that costs the user
        // something and a spinner would hide it.
        assert!(body_of(&half).contains("of"), "the download does not report how far it is");

        // An unknown total stays unknown rather than becoming zero -- the
        // decision `download_fraction` already made, reused rather than
        // re-decided.
        let unknown = CliSetupState::Working {
            stage: AcquireStage::Downloading { done: 50, total: None },
        };
        assert_eq!(unknown.progress(), None);
        assert_eq!(
            CliSetupState::Working { stage: AcquireStage::Downloading { done: 1, total: Some(0) } }
                .progress(),
            None,
            "a declared total of zero became a division rather than an unknown"
        );

        // **Verification is visible as its own stage**, rather than the bar
        // freezing at 100% with the download's label still under it.
        let verifying = CliSetupState::Working { stage: AcquireStage::Verifying };
        assert_eq!(verifying.progress(), Some(1.0));
        assert!(body_of(&verifying).contains("signed by Bitwarden"));
        assert_ne!(body_of(&verifying), body_of(&half));

        // No buttons while work is in flight -- see `buttons()` for why a
        // Cancel here is not offered rather than offered and unsafe.
        assert!(verifying.buttons().is_empty());
    }

    /// **State 3 is required, names the version and the size, and offers one
    /// OK.**
    #[test]
    fn the_confirmation_names_what_landed_and_offers_one_button() {
        let state = CliSetupState::Installed(AcquiredCli {
            path: PathBuf::from(r"C:\x\bin\bw.exe"),
            version: Some("2026.8.0".to_string()),
            bytes: 38_695_474,
        });
        let body = body_of(&state);
        assert!(body.contains("2026.8.0"), "{body:?} does not name the version");
        assert!(body.contains("36.9 MB"), "{body:?} does not name the size");
        assert!(body.contains("continue"), "{body:?} does not say what OK does");
        assert_eq!(state.buttons(), vec![("OK", CliModalAction::Close, false)]);
        // Control: a DIFFERENT install produces a different body, so the
        // state is reading the value it was handed rather than printing a
        // fixed confirmation.
        let other = CliSetupState::Installed(AcquiredCli {
            path: PathBuf::from(r"C:\x\bin\bw.exe"),
            version: Some("2027.1.2".to_string()),
            bytes: 1,
        });
        assert_ne!(body_of(&other), body);
    }

    /// **Every failure state keeps the failure-matrix discipline**, and the
    /// signature failure offers no retry.
    #[test]
    fn a_failure_state_names_the_cli_the_server_and_the_alternative() {
        for refusal in [
            AcquireRefusal::Offline("x".into()),
            AcquireRefusal::DigestMismatch,
            AcquireRefusal::NotBitwardenSigned { subject_dn: None },
        ] {
            let state = CliSetupState::Failed(refusal.clone());
            let body = body_of(&state);
            assert!(body.contains("Bitwarden CLI"), "{body:?}");
            assert!(body.contains("bitwarden.com"), "{body:?}");
            assert!(body.contains("self-hosted"), "{body:?}");
            assert_eq!(state.buttons().len(), 1, "a failure offers {:?}", state.buttons());
            assert!(state.progress().is_none(), "a failed setup is still showing progress");
        }
        // The signature failure says it is a security event rather than
        // "something went wrong", and says the CLI was NOT run.
        let signed = CliSetupState::Failed(AcquireRefusal::NotBitwardenSigned { subject_dn: None });
        let body = body_of(&signed);
        assert!(
            body.contains("could not confirm it came from Bitwarden"),
            "the signature failure is euphemised: {body:?}"
        );
        assert!(body.contains("did not run it"), "{body:?}");
    }

    /// The four states are four different modals-worth of words. A body that
    /// was the same in two of them would mean one of them says nothing.
    #[test]
    fn the_states_do_not_say_the_same_thing() {
        let states = [
            CliSetupState::Asking,
            CliSetupState::Working { stage: AcquireStage::Verifying },
            CliSetupState::Installed(AcquiredCli {
                path: PathBuf::from("x"),
                version: Some("1.2.3".into()),
                bytes: 1,
            }),
            CliSetupState::Failed(AcquireRefusal::DigestMismatch),
        ];
        for (i, a) in states.iter().enumerate() {
            assert!(!a.body().is_empty(), "{a:?} paints nothing");
            for b in states.iter().skip(i + 1) {
                assert_ne!(body_of(a), body_of(b), "two states say the same thing");
                assert_ne!(a.title(), b.title(), "two states share a heading");
            }
        }
    }

    // ---- Task 2: the right release, the right asset ------------------------

    /// The real digest GitHub publishes for `bw-windows-2026.8.0.zip`,
    /// recorded 2026-08-31.
    const WINDOWS_ASSET_DIGEST: &str =
        "sha256:26a6bb9a88ca9eeaad9e59db1816dcceb3ce6cc80a30b33e1324b0642f4a0f32";
    /// A DIFFERENT value, standing in for the OSS build's digest, so the
    /// "carried the right digest" test has something to be wrong about.
    const OSS_WINDOWS_ASSET_DIGEST: &str =
        "sha256:11111111111111111111111111111111111111111111111111111111111111ff";

    fn asset(name: &str, digest: Option<&str>) -> ReleaseAsset {
        ReleaseAsset {
            name: name.to_string(),
            digest: digest.map(str::to_string),
            browser_download_url: format!(
                "https://github.invalid/bitwarden/clients/releases/download/x/{name}"
            ),
        }
    }

    /// The real `cli-v2026.8.0` asset list, verified 2026-08-31: thirteen
    /// assets, with `bw-oss-windows-2026.8.0.zip` sorted BEFORE
    /// `bw-windows-2026.8.0.zip`, so a "first match containing windows"
    /// implementation picks the wrong one.
    fn stable_cli_release_2026_8_0() -> Release {
        Release {
            tag_name: "cli-v2026.8.0".to_string(),
            prerelease: false,
            draft: false,
            assets: vec![
                asset("bw-linux-2026.8.0.zip", Some(OSS_WINDOWS_ASSET_DIGEST)),
                asset("bw-macos-2026.8.0.zip", Some(OSS_WINDOWS_ASSET_DIGEST)),
                asset("bw-oss-linux-2026.8.0.zip", Some(OSS_WINDOWS_ASSET_DIGEST)),
                asset("bw-oss-macos-2026.8.0.zip", Some(OSS_WINDOWS_ASSET_DIGEST)),
                asset("bw-oss-windows-2026.8.0.zip", Some(OSS_WINDOWS_ASSET_DIGEST)),
                asset("bw-windows-2026.8.0.zip", Some(WINDOWS_ASSET_DIGEST)),
                asset("bw-oss-linux-sha256-2026.8.0.txt", Some(OSS_WINDOWS_ASSET_DIGEST)),
                asset("bw-oss-macos-sha256-2026.8.0.txt", Some(OSS_WINDOWS_ASSET_DIGEST)),
                asset("bw-oss-windows-sha256-2026.8.0.txt", Some(OSS_WINDOWS_ASSET_DIGEST)),
                asset("bw-linux-sha256-2026.8.0.txt", Some(OSS_WINDOWS_ASSET_DIGEST)),
                asset("bw-macos-sha256-2026.8.0.txt", Some(OSS_WINDOWS_ASSET_DIGEST)),
                asset("bw-windows-sha256-2026.8.0.txt", Some(OSS_WINDOWS_ASSET_DIGEST)),
                asset("bitwarden-cli-2026.8.0.tgz", Some(OSS_WINDOWS_ASSET_DIGEST)),
            ],
        }
    }

    fn cli_release(tag: &str, prerelease: bool, draft: bool) -> Release {
        let version = tag.trim_start_matches("cli-v");
        Release {
            tag_name: tag.to_string(),
            prerelease,
            draft,
            assets: vec![
                asset(&format!("bw-oss-windows-{version}.zip"), Some(OSS_WINDOWS_ASSET_DIGEST)),
                asset(&format!("bw-windows-{version}.zip"), Some(WINDOWS_ASSET_DIGEST)),
            ],
        }
    }

    fn desktop_release_newer_than_the_cli() -> Release {
        Release {
            tag_name: "desktop-v2026.12.0".to_string(),
            prerelease: false,
            draft: false,
            assets: vec![asset("bw-windows-2026.12.0.zip", Some(WINDOWS_ASSET_DIGEST))],
        }
    }

    /// **The OSS build is beside the real one and a glob matches both.**
    #[test]
    fn the_oss_windows_build_is_never_the_one_chosen() {
        let picked = pick_artefact(&[stable_cli_release_2026_8_0()]).expect("an asset");
        assert_eq!(picked.asset_name, "bw-windows-2026.8.0.zip");
        assert!(!picked.asset_name.contains("oss"));
        // Control on the FIXTURE: the decoy really is present and really does
        // sort first, or this test is not about anything.
        let names: Vec<String> = stable_cli_release_2026_8_0()
            .assets
            .iter()
            .map(|a| a.name.clone())
            .collect();
        let oss = names.iter().position(|n| n == "bw-oss-windows-2026.8.0.zip");
        let real = names.iter().position(|n| n == "bw-windows-2026.8.0.zip");
        assert!(oss.is_some() && real.is_some(), "control: the fixture lost an asset");
        assert!(oss < real, "control: the OSS decoy no longer sorts before the real asset");
    }

    /// Positive control for the test above: with `bw-windows-*` removed from
    /// the same fixture it REFUSES, rather than falling back to the OSS build
    /// or to a Linux one. Without this, an implementation that always
    /// returned the last matching asset would pass the test above.
    #[test]
    fn a_release_without_the_windows_build_is_refused_not_substituted() {
        let mut release = stable_cli_release_2026_8_0();
        release.assets.retain(|a| a.name != "bw-windows-2026.8.0.zip");
        // Control: the OSS build IS still in the fixture, so a substitution
        // was available and was not taken.
        assert!(release.assets.iter().any(|a| a.name.contains("oss-windows")));
        assert!(matches!(pick_artefact(&[release]), Err(AcquireRefusal::NoArtefact(_))));
    }

    /// The monorepo problem: `cli`, `desktop`, `browser` and `web` releases
    /// interleave by date, so "newest release" is not "newest CLI".
    #[test]
    fn a_newer_desktop_release_does_not_win_over_the_newest_cli() {
        let picked = pick_artefact(&[
            desktop_release_newer_than_the_cli(),
            stable_cli_release_2026_8_0(),
        ])
        .expect("the cli release");
        assert_eq!(picked.asset_name, "bw-windows-2026.8.0.zip");
        // Control: the desktop release carries a matching asset name, so the
        // filter is reading the TAG and not merely failing to find an asset.
        assert!(desktop_release_newer_than_the_cli()
            .assets
            .iter()
            .any(|a| a.name.starts_with(WINDOWS_ASSET_PREFIX)));
    }

    /// A prerelease tagged `cli-v*` ahead of its stable promotion is skipped.
    #[test]
    fn a_newer_cli_prerelease_loses_to_the_older_stable_one() {
        let picked = pick_artefact(&[
            cli_release("cli-v2026.9.0", true, false),
            stable_cli_release_2026_8_0(),
        ])
        .expect("the stable release");
        assert_eq!(picked.asset_name, "bw-windows-2026.8.0.zip");
    }

    /// **Positive control for the flag, not the ordering.** With the
    /// prerelease flag cleared on the same newer release it DOES win -- so
    /// the test above is reading `prerelease` and not merely preferring the
    /// second element of the slice.
    #[test]
    fn the_prerelease_test_is_reading_the_flag_and_not_the_order() {
        let picked = pick_artefact(&[
            cli_release("cli-v2026.9.0", false, false),
            stable_cli_release_2026_8_0(),
        ])
        .expect("the newer release");
        assert_eq!(picked.asset_name, "bw-windows-2026.9.0.zip");
    }

    /// Drafts, same shape, same pair.
    #[test]
    fn a_draft_cli_release_is_skipped() {
        let picked = pick_artefact(&[
            cli_release("cli-v2026.9.0", false, true),
            stable_cli_release_2026_8_0(),
        ])
        .expect("the stable release");
        assert_eq!(picked.asset_name, "bw-windows-2026.8.0.zip");
        // Control: cleared, the same release wins.
        let picked = pick_artefact(&[
            cli_release("cli-v2026.9.0", false, false),
            stable_cli_release_2026_8_0(),
        ])
        .expect("the newer release");
        assert_eq!(picked.asset_name, "bw-windows-2026.9.0.zip");
    }

    /// **Fail-closed on a missing digest**, carrying `updater`'s existing
    /// decision that the digest is required rather than optional. An asset
    /// with no `sha256:` digest is refused, never downloaded unchecked.
    #[test]
    fn an_asset_with_no_digest_is_refused() {
        let mut release = stable_cli_release_2026_8_0();
        release.assets.iter_mut().for_each(|a| a.digest = None);
        assert!(matches!(pick_artefact(&[release]), Err(AcquireRefusal::NoArtefact(_))));
        // Control: with the digests restored, the very same fixture resolves.
        assert!(pick_artefact(&[stable_cli_release_2026_8_0()]).is_ok());
    }

    /// A digest that is present but malformed is also refused, rather than
    /// being silently treated as absent-and-therefore-fine.
    #[test]
    fn a_malformed_digest_is_refused() {
        let mut release = stable_cli_release_2026_8_0();
        for a in &mut release.assets {
            if a.name.starts_with(WINDOWS_ASSET_PREFIX) {
                a.digest = Some("md5:0123".to_string());
            }
        }
        assert!(matches!(pick_artefact(&[release]), Err(AcquireRefusal::NoArtefact(_))));
    }

    /// The digest that comes back is the WINDOWS asset's, not some other
    /// asset's. The failure this catches -- right file, wrong hash -- makes
    /// every download fail forever with a mismatch nobody can explain.
    #[test]
    fn the_digest_belongs_to_the_asset_that_was_picked() {
        let picked = pick_artefact(&[stable_cli_release_2026_8_0()]).expect("an asset");
        assert_eq!(picked.digest, parse_asset_digest(WINDOWS_ASSET_DIGEST).expect("a digest"));
        // Positive control: the decoy's digest is a DIFFERENT value, so the
        // assertion above would fail if the wrong one were carried.
        assert_ne!(WINDOWS_ASSET_DIGEST, OSS_WINDOWS_ASSET_DIGEST);
        assert_ne!(
            picked.digest,
            parse_asset_digest(OSS_WINDOWS_ASSET_DIGEST).expect("a digest")
        );
    }

    /// An empty listing is a refusal, not a panic and not a default.
    #[test]
    fn an_empty_listing_is_refused() {
        assert!(matches!(pick_artefact(&[]), Err(AcquireRefusal::NoArtefact(_))));
    }

    /// The tag filter is a PREFIX and not a `contains`. A release tagged
    /// `web-v...-cli-v9.9.9` must not be mistaken for a CLI release.
    #[test]
    fn the_tag_filter_is_anchored_at_the_front() {
        let mut decoy = cli_release("cli-v2026.9.0", false, false);
        decoy.tag_name = "web-v2026.9.0-cli-v2026.9.0".to_string();
        let picked = pick_artefact(&[decoy, stable_cli_release_2026_8_0()]).expect("the cli");
        assert_eq!(picked.asset_name, "bw-windows-2026.8.0.zip");
    }

    /// The releases URL names Bitwarden's repository and asks for enough of
    /// the list to find a CLI release among three other products'.
    #[test]
    fn the_releases_url_is_bitwardens_client_monorepo() {
        assert!(RELEASES_URL.starts_with("https://"));
        assert!(RELEASES_URL.contains("bitwarden/clients"));
        assert!(RELEASES_URL.contains("per_page="));
    }

    // ---- Task 3 + 4: the digest gate, and verify-before-install ------------

    /// The destination is `bw_path`'s own, not a second spelling.
    #[test]
    fn the_install_destination_is_the_path_the_resolver_already_recorded() {
        let exe_dir = Path::new(r"C:\deskwarden-test\app");
        // Asserted against the RESOLVER, not against a string: this is about
        // agreement between two modules, and a hand-written path in this test
        // would only prove this test agrees with itself.
        assert_eq!(
            install_destination(exe_dir),
            crate::bw_path::install_bin_candidate(exe_dir)
        );
        // Control on the shape, so a `bw_path` that started returning
        // `exe_dir` itself would be noticed here rather than at runtime.
        assert_eq!(install_destination(exe_dir), exe_dir.join("bin").join("bw.exe"));
    }

    fn signed(valid: bool, subject: Option<&str>) -> crate::signature::SignatureInfo {
        crate::signature::SignatureInfo {
            valid,
            thumbprint: Some("80375A0C9630A51ECB7EC79B37A8174C8DACCCED".to_string()),
            subject_dn: subject.map(str::to_string),
        }
    }

    /// A validly signed binary whose `O=` is somebody else is refused --
    /// three spellings a substring check would wave through.
    #[test]
    fn a_binary_signed_by_someone_else_is_refused() {
        for subject in [
            "CN=Not Bitwarden\r\nO=Not Bitwarden Ltd\r\nC=US",
            "CN=Bitwarden Inc.\r\nO=Bitwarden Solutions LLC\r\nC=US",
            "CN=x\r\nOU=bitwarden-integration\r\nO=Someone Else\r\nC=US",
        ] {
            assert!(
                matches!(
                    judge(&Ok(signed(true, Some(subject)))),
                    Err(AcquireRefusal::NotBitwardenSigned { .. })
                ),
                "{subject} was accepted"
            );
        }
    }

    /// **Positive control**: the real subject measured on 2026-08-10 IS
    /// accepted. Without this, the test above passes against a `judge` that
    /// refuses everything, and the feature would never install anything at
    /// all -- which is the failure mode a "wrong signers are rejected" suite
    /// is blind to by construction.
    #[test]
    fn the_real_bitwarden_subject_is_accepted() {
        let real = "OID.1.3.6.1.4.1.311.60.2.1.3=US\r\n\
                    OID.1.3.6.1.4.1.311.60.2.1.2=Delaware\r\n\
                    OID.2.5.4.15=Private Organization\r\n\
                    SERIALNUMBER=7654941\r\n\
                    C=US\r\n\
                    S=California\r\n\
                    L=Santa Barbara\r\n\
                    O=Bitwarden Inc.\r\n\
                    CN=Bitwarden Inc.";
        assert!(judge(&Ok(signed(true, Some(real)))).is_ok());
    }

    /// An invalid signature is refused **even when the `O=` is right** -- the
    /// tampered-with case, which an organization check alone would accept.
    #[test]
    fn a_tampered_binary_with_the_right_name_is_still_refused() {
        assert!(matches!(
            judge(&Ok(signed(false, Some("O=Bitwarden Inc.")))),
            Err(AcquireRefusal::NotBitwardenSigned { .. })
        ));
        // Control: the identical DN with `valid: true` is accepted, so the
        // assertion above is reading the validity flag.
        assert!(judge(&Ok(signed(true, Some("O=Bitwarden Inc.")))).is_ok());
    }

    /// An unsigned binary -- no signer certificate at all -- is refused.
    #[test]
    fn an_unsigned_binary_is_refused() {
        assert!(matches!(
            judge(&Ok(signed(false, None))),
            Err(AcquireRefusal::NotBitwardenSigned { .. })
        ));
    }

    /// A check that could not run at all is `Unverifiable`, a DIFFERENT
    /// refusal from `NotBitwardenSigned`, because one is retryable and the
    /// other is deliberately not.
    #[test]
    fn a_check_that_could_not_run_is_its_own_refusal() {
        let refusal = judge(&Err("the file could not be read".to_string()))
            .expect_err("an unreadable file is not a pass");
        assert!(matches!(refusal, AcquireRefusal::Unverifiable(_)));
        assert!(refusal.retryable());
        // Control: the neighbouring refusal is NOT retryable, so the two are
        // genuinely different outcomes rather than two names for one.
        assert!(!AcquireRefusal::NotBitwardenSigned { subject_dn: None }.retryable());
    }

    /// **Acquisition reads `signature`'s list and does not carry its own.**
    #[test]
    fn the_trusted_organizations_are_not_duplicated_in_this_module() {
        let production = production_slice();
        for spelling in ["8bit Solutions", "Bitwarden, Inc.", "Bitwarden Inc\""] {
            assert!(
                !production.contains(spelling),
                "the trusted-organization list is duplicated in this module ({spelling:?}); \
                 a second list is how the two come to disagree"
            );
        }
        // Positive control: the module DOES name the shared constant, so a
        // module that consulted no list at all cannot pass the assertions
        // above by simply not caring.
        assert!(
            production.contains("TRUSTED_BW_SIGNER_ORGANIZATIONS"),
            "this module consults no trusted-organization list at all"
        );
    }

    // ---- Task 6: the pin that says nothing is ever executed -----------------

    /// The `cfg` attribute that makes a module test-only, split so this
    /// constant is not itself one.
    const CUT_GATE: &str = concat!("#[cfg(", "test)]");
    /// The literal the guards cut this file at, split for the same reason.
    const CUT_MARKER: &str = concat!("mod te", "sts {");

    /// This file's production half: everything above the test module.
    ///
    /// The cut is anchored by UNIQUENESS rather than by position, which is
    /// the lesson `updater::production_slice` paid for: a cut chosen by
    /// "first occurrence" can be MOVED by production text, and production
    /// text is exactly what the guard is supposed to be judging.
    fn production_slice() -> String {
        let source = include_str!("bw_acquire.rs");
        let markers = source.matches(CUT_MARKER).count();
        assert_eq!(
            markers, 1,
            "bw_acquire.rs contains {markers} occurrences of the test-module marker, not 1. \
             The production slice the source guards read is cut here, so a SECOND one -- in a \
             raw string, a doc comment or a fixture -- would let production code choose where \
             the guards stop reading"
        );
        let gates = source.matches(CUT_GATE).count();
        assert_eq!(
            gates, 1,
            "bw_acquire.rs contains {gates} occurrences of the test gate, not 1; half a forged \
             marker moves the cut just as well as a whole one"
        );
        let cut = source.find(CUT_MARKER).expect("the marker was counted above");
        let slice = source[..cut].to_string();
        assert!(
            slice.len() < source.len(),
            "control: the cut kept the whole file, so the guards read their own fixtures"
        );
        slice
    }

    /// The source with the CONTENTS of comments and string literals erased,
    /// so what is left is only what the programmer wrote as syntax.
    ///
    /// Deliberately conservative and deliberately small: it erases `//`
    /// comments, `/* */` comments, ordinary `"..."` strings and raw strings.
    /// Counting identifiers over this cannot be inflated by string data --
    /// which matters here, because this module's doc comments discuss
    /// `Command` and `spawn` at length and must be free to.
    fn code_without_literals(source: &str) -> String {
        let chars: Vec<char> = source.chars().collect();
        let mut out = String::with_capacity(source.len());
        let mut i = 0usize;
        let at = |k: usize| chars.get(k).copied().unwrap_or('\0');
        while i < chars.len() {
            let c = chars[i];
            if c == '/' && at(i + 1) == '/' {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            if c == '/' && at(i + 1) == '*' {
                i += 2;
                while i < chars.len() && !(chars[i] == '*' && at(i + 1) == '/') {
                    i += 1;
                }
                i = (i + 2).min(chars.len());
                continue;
            }
            // A raw string: `r`, some `#`s, then a quote. The contents are
            // skipped up to the matching close.
            if c == 'r' {
                let mut k = i + 1;
                let mut hashes = 0usize;
                while at(k) == '#' {
                    hashes += 1;
                    k += 1;
                }
                if at(k) == '"' {
                    out.push('"');
                    i = k + 1;
                    loop {
                        if i >= chars.len() {
                            break;
                        }
                        if chars[i] == '"' {
                            let mut got = 0usize;
                            while got < hashes && at(i + 1 + got) == '#' {
                                got += 1;
                            }
                            if got == hashes {
                                i += 1 + hashes;
                                break;
                            }
                        }
                        i += 1;
                    }
                    out.push('"');
                    continue;
                }
            }
            if c == '"' {
                out.push('"');
                i += 1;
                while i < chars.len() {
                    if chars[i] == '\\' {
                        i += 2;
                        continue;
                    }
                    if chars[i] == '"' {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
                out.push('"');
                continue;
            }
            out.push(c);
            i += 1;
        }
        out
    }

    /// **This module downloads and verifies. It never runs anything.**
    ///
    /// Not even `bw --version` to "check it works": a binary that has not
    /// been proven to be Bitwarden's must not execute, and one that has does
    /// not need a smoke test. This is the strongest single statement the
    /// feature makes.
    #[test]
    fn nothing_in_this_module_starts_a_process() {
        let code = code_without_literals(&production_slice());
        // Needles SPLIT ACROSS `concat!` arguments, for two reasons. The
        // crate-wide scanner in `job_object` reads every `.rs` file for these
        // same spellings and would report this line as an offender if they
        // were written whole. And a needle written as one literal would match
        // its own declaration through `production_slice`, which is the dead
        // guard `picker_ui.rs` records this crate having actually shipped.
        for forbidden in [
            concat!("Comm", "and"),
            concat!(".spa", "wn("),
            concat!(".out", "put("),
            concat!(".sta", "tus("),
            concat!("proc", "ess::"),
        ] {
            assert!(
                !code.contains(forbidden),
                "{forbidden} appears in bw_acquire's production code. The fix is to REMOVE the \
                 process start, never to loosen this scanner: a failure here is a design \
                 violation, not a test problem"
            );
        }
    }

    /// **Positive control for the scanner**, not for the module. A scanner
    /// that returned `""` -- or one whose comment-stripping ate the whole
    /// file -- would pass the test above against any code at all.
    #[test]
    fn the_scanner_would_catch_a_process_start_if_one_were_added() {
        // **The forged process start is assembled at run time, never written
        // whole in this file.**
        //
        // It used to be one literal, and that was a real defect rather than a
        // style point: `job_object::the_two_job_bearing_modules_can_start_a_
        // child_only_through_this_one` scans EVERY `.rs` file in the tree for
        // the three `Command` methods that start a child, and it does not
        // care that this one is inside a test fixture. It reported this line
        // as an offender. The needle spellings are therefore split the way
        // `http_agent`'s guard splits its own, and reassembled below -- so
        // the text handed to the scanner under test is exactly what it would
        // see in real code, while this file contains no such spelling.
        let command_word = concat!("Comm", "and");
        let spawn_call = concat!(".spa", "wn(");
        let sneaked = format!(
            "{}\nfn zz() {{ std::process::{command_word}::new(\"x\"){spawn_call}); }}\n",
            production_slice()
        );
        let code = code_without_literals(&sneaked);
        for forbidden in [command_word, spawn_call, concat!("proc", "ess::")] {
            assert!(
                code.contains(forbidden),
                "control: the scanner did not see a {forbidden} it was handed"
            );
        }
        // And it must NOT be blind in the other direction: the scanner has to
        // leave real code behind, or the test above is comparing two empty
        // strings.
        assert!(
            code.contains("fn pick_artefact"),
            "control: the scanner erased the module's own code, not just its literals"
        );
        // ...while genuinely erasing literal contents, which is the whole
        // reason this module's prose is free to discuss processes.
        let erased = code_without_literals("let s = \"Command::new spawn\";");
        assert!(
            !erased.contains("Command"),
            "control: the scanner does not erase string contents, so the guard would fire on \
             a doc string and the fix would be to reword prose"
        );
    }

    /// Bare `ureq` never appears here. `http_agent`'s own guard covers the
    /// whole crate; this restates it at the one new call site so a reviewer
    /// reading this file alone can see it.
    #[test]
    fn every_request_goes_through_a_bounded_agent() {
        let code = code_without_literals(&production_slice());
        for forbidden in [
            concat!("Agent", "Builder"),
            concat!("ureq:", ":get("),
            concat!("ureq:", ":agent("),
        ] {
            assert!(!code.contains(forbidden), "{forbidden} reaches around http_agent");
        }
        // Positive control: this module DOES make requests, through the two
        // bounded constructors, so "no bare ureq" is not satisfied by making
        // no requests at all.
        assert!(code.contains("bounded_total"), "control: no total-bounded agent is built");
        assert!(code.contains("bounded_stall"), "control: no stall-bounded agent is built");
    }

    /// The install destination is named exactly once, through `bw_path`.
    ///
    /// A second spelling would leave the process trusting one path while the
    /// binary sat at another -- see the module docs. Pinned rather than
    /// commented, because the failure is invisible until a user's `PATH`
    /// entry points at nothing.
    #[test]
    fn the_install_path_is_never_spelled_twice() {
        let code = code_without_literals(&production_slice());
        assert_eq!(
            code.matches("install_bin_candidate").count(),
            1,
            "the install path is computed in more than one place in this module"
        );
        assert!(
            !code.contains("join(\"bin\")"),
            "this module spells the install path itself instead of asking `bw_path` for it"
        );
    }

    /// **The install writes BOTH things the deleted bootstrap wrote**: the
    /// `bin` directory, and the user's `PATH`.
    ///
    /// Neither is optional any more, and neither has another author.
    /// `deskwarden.iss` never created `<InstallDir>\bin` -- `bootstrap-bw.ps1`
    /// did, with `New-Item -ItemType Directory`, and it also wrote
    /// `HKCU\Environment`'s `Path` with `SetEnvironmentVariable`. That script
    /// is deleted, so on a machine that never ran it the directory does not
    /// exist at all and the copy would fail with a bare "not found".
    ///
    /// The PATH entry is **user-visible behaviour, not an implementation
    /// detail**: the app itself reaches the CLI by absolute path
    /// (`bw_path::resolve_bw_exe` deliberately never spells a bare `bw`), so
    /// nothing here depends on it -- but the user's ability to type `bw` in
    /// their own terminal does, and that is a capability they have today and
    /// would otherwise silently lose.
    ///
    /// Pinned as a source assertion rather than by writing to a real
    /// registry: this is a test suite that must not touch
    /// `HKCU\Environment`, and the failure being guarded against is a
    /// deletion, which source order sees perfectly well.
    #[test]
    fn installing_creates_the_bin_directory_and_writes_the_user_path() {
        let code = code_without_literals(&production_slice());
        let start = code
            .find("fn install_at_the_resolver_path")
            .expect("control: the install function is gone, so this pins nothing");
        let body = &code[start..];
        let create = body
            .find("create_dir_all")
            .expect("the install no longer creates <InstallDir>\\bin, and nothing else does");
        let copy = body.find("fs::copy").expect("the install no longer copies anything");
        assert!(
            create < copy,
            "the install copies into <InstallDir>\\bin before creating it"
        );
        assert!(
            body.contains("add_to_user_path"),
            "the install no longer puts <InstallDir>\\bin on the user's PATH. Nothing else \
             does either since the installer bootstrap was deleted, so the user would lose \
             the ability to run `bw` in their own terminal"
        );

        // The broadcast, by NAMED CONSTANT. A numeric literal here is
        // indistinguishable from a correct one at a glance, which is the
        // crate-wide rule this restates at the one new Win32 call site.
        assert!(code.contains("WM_SETTINGCHANGE"), "the PATH change is not broadcast");
        assert!(code.contains("HWND_BROADCAST"), "the broadcast has no recipient");
        for literal in ["0x001A", "0x1a", "26u32", "0xffff"] {
            assert!(
                !code.contains(literal),
                "{literal} is a Win32 constant written as a number"
            );
        }
    }

    /// **Nothing here deletes a CLI**, and that is now the whole of what this
    /// asserts. The UNINSTALLER removes `<InstallDir>\bin` and its PATH entry
    /// (see the module docs for why that reversed); this module is the
    /// RUNNING APP, which must never do either. The distinction is the point:
    /// a deletion the user triggered by uninstalling is not the same act as a
    /// deletion a tray app performs on its own while they are using it.
    ///
    /// The `discard` calls in this file are all on the SCRATCH directory: a
    /// partial download, a rejected archive, an extracted binary that failed
    /// verification. None of them may ever be pointed at the install
    /// destination, which is the one deletion that would take a working
    /// command-line tool away from somebody who did not ask.
    #[test]
    fn nothing_here_removes_the_installed_cli_or_its_path_entry() {
        let code = code_without_literals(&production_slice());
        assert!(
            !code.contains("remove_dir"),
            "this module removes a directory. Removing <InstallDir>\\bin is the UNINSTALLER's \
             job and only its job -- the running app taking a command-line tool away from \
             somebody who is still using it is a different act entirely"
        );
        // Every `discard` is reachable only with a scratch path. Asserted by
        // the absence of the one composition that would not be: discarding
        // what `install_destination` computed.
        assert!(
            !code.contains("discard(&dest)") || !code.contains("discard(&install"),
            "a discard is aimed at the install destination"
        );
        assert!(
            !code.contains("discard(&installed"),
            "the installed CLI is deleted by this module"
        );
        // Control: the module DOES discard things, so the assertions above
        // are about WHERE rather than about a module that deletes nothing and
        // therefore leaves rejected binaries lying around.
        assert!(code.contains("fn discard"), "control: nothing is discarded at all");
    }

    /// The whole trust ordering, as a source-order assertion over
    /// `acquire_if_needed`: **verify is reached before install, and install
    /// is reached from nowhere else.**
    ///
    /// A routing test over the seam proves the same thing for one input; this
    /// proves there is no second path. Together they are what stops a
    /// verified-after-install implementation, which would pass a test that
    /// checked the final file's signature while leaving an unverified binary
    /// at the trusted path for however long the check took.
    #[test]
    fn install_is_reachable_only_through_verify() {
        let code = code_without_literals(&production_slice());
        let body_start = code
            .find("pub fn acquire_if_needed")
            .expect("control: acquire_if_needed is gone, so this pins nothing");
        let body = &code[body_start..];
        let verify = body.find("env.verify").expect("acquire_if_needed does not verify at all");
        let install = body.find("env.install").expect("acquire_if_needed does not install at all");
        assert!(
            verify < install,
            "acquisition reaches the install seam before the signature seam"
        );
        // And exactly one call to each, so a second, ungated install cannot
        // be written beside the gated one.
        assert_eq!(code.matches("env.install").count(), 1, "there is a second install call site");
        assert_eq!(code.matches("env.verify").count(), 1, "there is a second verify call site");
    }

    // ---- routing: the gate is in a gating position -------------------------

    /// Records which seams were reached, so the routing tests below assert
    /// about REACHABILITY rather than about return values.
    use std::sync::atomic::{AtomicUsize, Ordering};
    static REACHED_RESOLVE: AtomicUsize = AtomicUsize::new(0);
    static REACHED_INSTALL: AtomicUsize = AtomicUsize::new(0);
    static REACHED_VERIFY: AtomicUsize = AtomicUsize::new(0);

    /// **The counters above are process-global and the test runner is
    /// parallel**, so every test that reads them holds this first.
    ///
    /// Written down rather than fixed by "it passed on my machine": the first
    /// version of these routing tests had no lock, and
    /// `an_existing_binary_short_circuits_before_the_resolver` failed with
    /// `left: 2, right: 1` -- its control counting a resolve call made by
    /// `the_stages_are_reported_as_they_happen` running beside it. That is a
    /// flake that would have appeared roughly one run in three, in a suite
    /// whose whole discipline is that a failure is real until proven
    /// otherwise.
    ///
    /// A seam carried in a `static` is the reason. The alternative -- giving
    /// each test its own counters -- needs a distinct `fn` item per test,
    /// because these are `fn` pointers and not closures, and that trade
    /// (three lines of lock against a dozen near-identical `fn`s) went this
    /// way.
    static COUNTER_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Takes the lock and zeroes the counters. The guard is returned rather
    /// than dropped, so the caller holds it for the body of the test --
    /// dropping it here would be the same race with an extra step.
    ///
    /// `unwrap_or_else(PoisonError::into_inner)`: a panicking test poisons
    /// the lock, and every later test would then fail for a reason that has
    /// nothing to do with it. The data behind the lock is three counters this
    /// function is about to overwrite anyway.
    fn counters_locked() -> std::sync::MutexGuard<'static, ()> {
        let guard = COUNTER_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        REACHED_RESOLVE.store(0, Ordering::SeqCst);
        REACHED_INSTALL.store(0, Ordering::SeqCst);
        REACHED_VERIFY.store(0, Ordering::SeqCst);
        guard
    }

    fn reset_counters() {
        REACHED_RESOLVE.store(0, Ordering::SeqCst);
        REACHED_INSTALL.store(0, Ordering::SeqCst);
        REACHED_VERIFY.store(0, Ordering::SeqCst);
    }

    impl BwAcquireEnv {
        /// The test-only substitute, written here as an inherent impl and
        /// deliberately BELOW every source guard in this file, so a
        /// test-gated item up there could not truncate the slice those guards
        /// read. Same arrangement `updater` uses.
        fn substitute(
            already_present: fn() -> Option<PathBuf>,
            verify: fn(&Path) -> Result<(), AcquireRefusal>,
        ) -> Self {
            Self {
                already_present,
                resolve: |_| {
                    REACHED_RESOLVE.fetch_add(1, Ordering::SeqCst);
                    Err(AcquireRefusal::Offline("stub".into()))
                },
                download: |_, _, _, _| Err(AcquireRefusal::Offline("stub".into())),
                extract: |_, _| Err(AcquireRefusal::NoArtefact("stub".into())),
                verify: {
                    // Wrapped so reaching it is observable; the real verdict
                    // still comes from the function under test.
                    let _ = verify;
                    verify
                },
                install: |_| {
                    REACHED_INSTALL.fetch_add(1, Ordering::SeqCst);
                    Err(AcquireRefusal::CouldNotInstall("stub".into()))
                },
            }
        }
    }

    fn present_at_a_path() -> Option<PathBuf> {
        Some(PathBuf::from(r"C:\deskwarden-test\app\bin\bw.exe"))
    }
    fn absent() -> Option<PathBuf> {
        None
    }
    fn verify_counting_ok(_: &Path) -> Result<(), AcquireRefusal> {
        REACHED_VERIFY.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    /// **An existing binary short-circuits before any network call.** The
    /// common case, and the one a user on a slow connection notices
    /// immediately if it is wrong.
    #[test]
    fn an_existing_binary_short_circuits_before_the_resolver() {
        let _guard = counters_locked();
        let env = BwAcquireEnv::substitute(present_at_a_path, verify_counting_ok);
        let result = acquire_if_needed(&env, &|_| {});
        assert!(
            matches!(result, Ok(AcquireOutcome::AlreadyPresent(_))),
            "an existing binary is not an acquisition"
        );
        assert_eq!(
            REACHED_RESOLVE.load(Ordering::SeqCst),
            0,
            "a machine that already has the CLI made a network request anyway"
        );
        assert_eq!(REACHED_INSTALL.load(Ordering::SeqCst), 0);

        // **Positive control**: with the binary absent, the very same call
        // DOES reach the resolver. Without this, the assertion above passes
        // against a build where acquisition is wired to nothing at all.
        reset_counters();
        let env = BwAcquireEnv::substitute(absent, verify_counting_ok);
        let _ = acquire_if_needed(&env, &|_| {});
        assert_eq!(
            REACHED_RESOLVE.load(Ordering::SeqCst),
            1,
            "control: acquisition never reaches the resolver even with no CLI present"
        );
    }

    /// A resolver failure stops before install, and reports it as itself
    /// rather than as something vaguer.
    #[test]
    fn a_resolver_failure_never_reaches_the_install() {
        let _guard = counters_locked();
        let env = BwAcquireEnv::substitute(absent, verify_counting_ok);
        let refusal = acquire_if_needed(&env, &|_| {}).expect_err("the stub resolver fails");
        assert!(matches!(refusal, AcquireRefusal::Offline(_)));
        assert_eq!(
            REACHED_INSTALL.load(Ordering::SeqCst),
            0,
            "acquisition installed something despite never resolving an artefact"
        );
        assert_eq!(
            REACHED_VERIFY.load(Ordering::SeqCst),
            0,
            "acquisition verified something it never downloaded"
        );
    }

    /// Stages are reported in order, and the first thing the user is told is
    /// not a bare spinner.
    #[test]
    fn the_stages_are_reported_as_they_happen() {
        let _guard = counters_locked();
        let seen = std::sync::Mutex::new(Vec::new());
        let env = BwAcquireEnv::substitute(absent, verify_counting_ok);
        let _ = acquire_if_needed(&env, &|stage| seen.lock().unwrap().push(stage));
        let seen = seen.into_inner().unwrap();
        assert_eq!(
            seen.first(),
            Some(&AcquireStage::Resolving),
            "the first thing reported was not the resolve stage: {seen:?}"
        );
        // Control: an existing binary reports NOTHING, because there is no
        // work to narrate -- so the assertion above is reading real progress.
        let quiet = std::sync::Mutex::new(Vec::new());
        let env = BwAcquireEnv::substitute(present_at_a_path, verify_counting_ok);
        let _ = acquire_if_needed(&env, &|stage| quiet.lock().unwrap().push(stage));
        assert!(
            quiet.into_inner().unwrap().is_empty(),
            "a machine that already has the CLI was shown setup progress"
        );
    }
}
