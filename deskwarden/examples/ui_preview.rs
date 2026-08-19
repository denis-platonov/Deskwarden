//! Visual preview of the themed windows.
//!
//! Interactive:
//!
//! ```text
//! cargo run --example ui_preview            # the autofill overlay (design 2a)
//! cargo run --example ui_preview -- --login # the login/unlock window (design 3h)
//! ```
//!
//! The overlay closes on Enter/Esc/✕; the login preview just draws (its
//! Continue does nothing here -- no `bw` is spawned from a preview).
//!
//! Self-screenshotting (for reviewing the design implementation without a
//! human at the keyboard):
//!
//! ```text
//! cargo run --example ui_preview -- --screenshot          # the overlay
//! cargo run --example ui_preview -- --login --screenshot  # the login window
//! cargo run --example ui_preview -- --all                 # EVERY surface below
//! ```
//!
//! `--all` is what CI runs. It walks [`Surface`] in one process -- one
//! `run_native`, resized between surfaces -- and writes a PNG per surface into
//! `target/ui_preview/`. One process rather than one invocation per surface
//! because a window creation and an eframe startup each cost more than the
//! frames they exist to produce, and this job must not double the CI time.
//!
//! # Why this example is worth its length
//!
//! Unit coverage on this crate is around 92% and functional coverage was
//! zero. Three real defects in one day were invisible to 2500 tests -- card
//! editing silently gated off for a fortnight, a rehearsal window that worked
//! and looked wrong, and a text baseline displacement a person spotted in a
//! second. Four separate UI tests were found *structurally blind* to the thing
//! they appeared to check.
//!
//! This does not fix that, and it is deliberately not a golden-image gate:
//! pixel-diffing an anti-aliased egui surface reds on a font-rendering change
//! and gets switched off within a month. It puts every surface where a human
//! can look at it on every change, which is the cheapest thing that would have
//! caught any of the three.
//!
//! # Everything here is a fixture
//!
//! No surface below reads a real vault, touches the network, or spawns `bw`.
//! The items are `serde_json` literals in the wire shape `bw serve` returns --
//! the same shape the deserializer is tested against, so a fixture that stops
//! parsing is a fixture that stopped describing the real thing. The breach
//! cache is handed a check that panics if it is ever called (it is not:
//! `check_breaches` is off), and the preflight is handed a [`SendTarget`]
//! value rather than a real foreground window.
//!
//! Every surface renders the exact draw function the app ships, never a copy,
//! so what these PNGs show is what the real app shows.

use deskwarden::breach::BreachCache;
use deskwarden::hello::HelloState;
use deskwarden::injector::target::SendTarget;
use deskwarden::key_sequence::ResolveSource;
use deskwarden::login_ui::{self, BwStatus, LoginForm};
use deskwarden::vault_bridge::{Folder, VaultItem};
use deskwarden::vault_window::detail::{self, RevealState, TotpState};
use deskwarden::vault_window::detail_edit::{self, EditDraft};
use deskwarden::vault_window::preflight::{self, PreflightState};
use deskwarden::vault_window::record_ui::{self, RecordDraft};
use deskwarden::vault_window::rehearsal;
use deskwarden::vault_window::totp_add::{self, TotpAdd};
use deskwarden::{app_identity::AppIdentityCache, overlay_ui, prefs_ui, scratch_window, theme};
use eframe::egui::{self, Margin};
use std::path::PathBuf;

/// The instant every time-dependent preview is drawn at.
///
/// A literal, not `SystemTime::now()`: the one-time code shot's two most
/// prominent numbers are a code and a countdown, and a screenshot that changes
/// every run is one no reviewer can diff against the last.
const PREVIEW_UNIX: u64 = 1_699_999_980;

/// Where the PNGs go: `$CARGO_TARGET_DIR` when the environment sets one,
/// and the historical relative `target` when it does not.
///
/// **Why the environment has to win.** A bare `target/` is resolved against
/// the process's working directory, which for `cargo run --example` is the
/// package root -- so in a normal checkout this example dropped nine PNGs
/// into `deskwarden/target`, the directory this project forbids writing to
/// because the user runs the shipped app out of it. Anyone who redirects the
/// build away from that directory, which is the whole point of setting
/// `CARGO_TARGET_DIR`, was redirecting everything except this.
///
/// **Why the fallback is unchanged.** CI sets no `CARGO_TARGET_DIR`, so it
/// still gets `target/ui_preview` and its nine-PNG path check in
/// `.github/workflows/ci.yml` keeps working without an edit. The
/// `create_dir_all` at the write site already handled a base that does not
/// exist yet, which is what makes an absolute out-of-tree base safe here.
fn target_dir() -> PathBuf {
    match std::env::var_os("CARGO_TARGET_DIR") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => PathBuf::from("target"),
    }
}

/// One screenshotable surface.
///
/// The list is the point of this file: adding a window to the app and not
/// adding it here is how a surface goes unlooked-at for a year.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Surface {
    /// The autofill overlay (design 2a).
    Overlay,
    /// The autofill overlay with NOTHING to offer (design 3a): a window that
    /// asks for a password and that the vault does not match.
    ///
    /// Its own surface rather than a variation of `Overlay`, because it is a
    /// different card -- no avatar, no row, no Enter chip -- drawn by a
    /// different function, and a state nobody renders is a state nobody looks
    /// at.
    OverlayNoMatch,
    OverlayLocked,
    /// The autofill overlay's save-a-new-login form (design 3c): four rows and
    /// three answers, reached from `OverlayNoMatch`'s *New login* button.
    ///
    /// **By far the tallest state the overlay has**, and the one this list
    /// most needs: a frameless, always-on-top window of a hardcoded height
    /// with no `ScrollArea` anywhere, so a row or a button past the bottom
    /// edge is unreachable, and the geometry tests can only say the card fits
    /// -- not whether it reads as a form somebody would fill in.
    OverlaySaveLogin,
    /// The login window with a vault that exists and is locked.
    LoginUnlock,
    /// The login window with no account yet -- server dropdown and the Hello
    /// opt-in, self-hosted, which is the tallest state it has.
    LoginSignin,
    /// The vault window's read pane for an ordinary login.
    LoginDetail,
    /// The read pane for a **card**: brand mark, masked number, and the
    /// shared expiry/security-code line. The surface whose edit path was
    /// gated off for a fortnight without a test noticing.
    CardDetail,
    /// The same card **with its number revealed** -- the state a user puts the
    /// pane into in order to read the number against the card in their hand.
    ///
    /// A surface of its own because it is the only place the number's digit
    /// grouping can be seen at all: masked, the grouping is a shape made of
    /// dots, and a reviewer looking at `detail_card` cannot tell whether
    /// revealing keeps the groups or runs the digits together. It ran them
    /// together for a release -- "when Bank number masked it has spaces but
    /// when open -- doesn't" -- and no rendered surface showed it.
    CardDetailRevealed,
    /// The edit form with the discard confirmation over it.
    DiscardConfirm,
    /// The record composer -- the Send export form and its seed warning.
    RecordComposer,
    /// **Design 6c/6d**: the by-hand "add a one-time code" form, with a URI
    /// typed in so the confirmation -- the live code, its countdown, the
    /// masked secret and the spelled-out parameters -- is on screen, and
    /// against an item that ALREADY has a code so the replace warning is too.
    /// Those two are the whole surface; a shot of the empty form would show
    /// neither.
    TotpAddConfirm,
    /// **Design 6a's picker**, which is the door every other route to a
    /// one-time code is behind. Four rows in the design's order, the fourth
    /// drawn dead with its reason, and the privacy line pinned under them.
    /// Its own surface rather than a second state of the one above because
    /// nothing of it is on that shot: a confirmation card is what happens
    /// AFTER a route has been chosen.
    TotpAddPicker,
    /// The preflight, allowed: the rule's process is in front and the focused
    /// control is masked, so the hold-to-send is offered.
    PreflightAllowed,
    /// The preflight, refused: a password sequence aimed at the wrong process
    /// and an unmasked control, which is the state that must never grow a
    /// send button.
    PreflightRefused,
    /// The preferences window's Clipboard page, everything switched on --
    /// four live pills, the interval field, the always-on note and the reset
    /// button. The page 3e does not contain, so there is no drawing to
    /// compare it against and a screenshot is the only review there is.
    PrefsClipboard,
    /// The same page with the master switch OFF: three greyed pills and a
    /// greyed field, still present rather than hidden. Its own surface
    /// because "looks disabled" is precisely the claim a picture can check
    /// and a `contains` assertion cannot.
    PrefsClipboardOff,
    /// **Design 4d's rehearsal window, finished.** The twelfth surface, and
    /// the one this example exists for: this window shipped as a raw Win32
    /// dialog with none of the app's theme, tokens or type, and no screenshot
    /// job ever looked at it. It is drawn through `scratch_window::draw` --
    /// the exact function the viewport paints -- with a transcript built by
    /// the real `rehearsal::transcript`, so what this PNG shows is what a user
    /// watching a rehearsal sees.
    Rehearsal,
}

/// The detail pane's exact width in the shipped vault window.
///
/// `WINDOW_SIZE[0] - SIDEBAR_WIDTH - LIST_WIDTH`, i.e. `1240 - 212 - 390`.
/// Spelled out rather than imported because those three are `pub(crate)` in
/// `vault_window::mod` and an example is a separate crate. If that window is
/// ever resized, this is the number that has to follow it.
const PANE_WIDTH: f32 = 1240.0 - 212.0 - 390.0;

/// Tall enough that no pane below scrolls, so a screenshot is the whole
/// surface rather than the top of it. The shipped window is 740 high.
const PANE_HEIGHT: f32 = 740.0;

/// The preferences window's body: its 1000x780 outer size less the 40pt
/// chrome bar `draw_window_chrome` paints above `draw_prefs_body`. Spelled out
/// rather than imported for the same reason [`PANE_WIDTH`] is -- an example is
/// a separate crate and `WINDOW_SIZE` is private to `prefs_ui`.
const PREFS_BODY_WIDTH: f32 = 1000.0;
const PREFS_BODY_HEIGHT: f32 = 780.0 - 40.0;

/// The detail pane's own frame, copied from the `CentralPanel` in
/// `vault_window::mod` that hosts it: `theme::CANVAS` and
/// `Margin::symmetric(20, 18)`. The margin is part of the width the pane's
/// contents get, so guessing it would put the layout back off by tens of
/// pixels -- see [`Surface::size`].
fn pane_frame() -> egui::Frame {
    egui::Frame::new().fill(theme::CANVAS).inner_margin(Margin::symmetric(20, 18))
}

/// Every surface, in the order `--all` walks them.
const ALL: &[Surface] = &[
    Surface::Overlay,
    Surface::OverlayNoMatch,
    Surface::OverlayLocked,
    Surface::OverlaySaveLogin,
    Surface::LoginUnlock,
    Surface::LoginSignin,
    Surface::LoginDetail,
    Surface::CardDetail,
    Surface::CardDetailRevealed,
    Surface::DiscardConfirm,
    Surface::RecordComposer,
    Surface::TotpAddConfirm,
    Surface::TotpAddPicker,
    Surface::PreflightAllowed,
    Surface::PreflightRefused,
    Surface::PrefsClipboard,
    Surface::PrefsClipboardOff,
    Surface::Rehearsal,
];

impl Surface {
    /// The PNG's file stem. Stable, because a human reviewing an artifact
    /// looks for the same name every time.
    fn stem(self) -> &'static str {
        match self {
            Surface::Overlay => "overlay",
            Surface::OverlayNoMatch => "overlay_no_match",
            Surface::OverlayLocked => "overlay_locked",
            Surface::OverlaySaveLogin => "overlay_save_login",
            Surface::LoginUnlock => "login_unlock",
            Surface::LoginSignin => "login_signin",
            Surface::LoginDetail => "detail_login",
            Surface::CardDetail => "detail_card",
            Surface::CardDetailRevealed => "detail_card_revealed",
            Surface::DiscardConfirm => "edit_discard_confirm",
            Surface::RecordComposer => "record_composer",
            Surface::TotpAddConfirm => "totp_add_confirm",
            Surface::TotpAddPicker => "totp_add_picker",
            Surface::PreflightAllowed => "preflight_allowed",
            Surface::PreflightRefused => "preflight_refused",
            Surface::PrefsClipboard => "prefs_clipboard",
            Surface::PrefsClipboardOff => "prefs_clipboard_off",
            Surface::Rehearsal => "rehearsal",
        }
    }

    /// The viewport size this surface is drawn at.
    ///
    /// The two login states are *starting* sizes only: they size to content
    /// exactly as `run_login_flow` does, and the capture waits for that to
    /// settle. Everything else is the real window's size.
    ///
    /// The panes are drawn at [`PANE_WIDTH`] **to the pixel**, for a reason
    /// that is not cosmetic: the card's face lays out through
    /// `detail::card_face_line_fits`, which puts the expiry and the security
    /// code on one line or two depending on the width it is given. A preview
    /// rendered fifty pixels narrow shows a layout the app never produces, and
    /// a screenshot of a layout nobody ships is worse than no screenshot.
    fn size(self) -> egui::Vec2 {
        match self {
            Surface::Overlay | Surface::OverlayNoMatch | Surface::OverlayLocked => {
                egui::vec2(396.0, 164.0)
            }
            // Read off the module rather than written out: 3c is the one
            // overlay state that is NOT 164pt tall, and a preview rendered at
            // the wrong height is a picture of a layout nobody ships.
            Surface::OverlaySaveLogin => egui::vec2(
                overlay_ui::OVERLAY_WIDTH,
                overlay_ui::overlay_height(overlay_ui::SAVE_LOGIN_ROWS),
            ),
            Surface::LoginUnlock | Surface::LoginSignin => egui::vec2(470.0, 588.0),
            Surface::LoginDetail
            | Surface::CardDetail
            | Surface::CardDetailRevealed
            | Surface::DiscardConfirm
            | Surface::RecordComposer
            | Surface::TotpAddConfirm
            | Surface::TotpAddPicker => egui::vec2(PANE_WIDTH, PANE_HEIGHT),
            Surface::PreflightAllowed | Surface::PreflightRefused => egui::vec2(
                deskwarden::preflight_host::PREFLIGHT_WIDTH,
                deskwarden::preflight_host::PREFLIGHT_HEIGHT,
            ),
            // The shipped window is 1000x780 with a 40px chrome bar on top;
            // this draws the BODY, which is what `draw_prefs_body` is, so the
            // page lays out against exactly the width it has in the app.
            Surface::PrefsClipboard | Surface::PrefsClipboardOff => {
                egui::vec2(PREFS_BODY_WIDTH, PREFS_BODY_HEIGHT)
            }
            // The viewport's own inner size, read off the module that builds
            // it -- so a window resized in the app is a preview resized with
            // it, rather than a picture of a layout nobody ships.
            Surface::Rehearsal => {
                egui::vec2(scratch_window::SCRATCH_WIDTH, scratch_window::SCRATCH_HEIGHT)
            }
        }
    }

    /// Whether this surface draws the login window's own titlebar.
    fn is_login_window(self) -> bool {
        matches!(self, Surface::LoginUnlock | Surface::LoginSignin)
    }
}

fn main() -> eframe::Result {
    let arg = |name: &str| std::env::args().any(|a| a == name);
    let all = arg("--all");
    let screenshot = all || arg("--screenshot");
    let signin = arg("--signin");
    let login = signin || arg("--login");

    // `--all` walks the whole list; otherwise the single surface the flags
    // name, exactly as this example has always behaved.
    let queue: Vec<Surface> = if all {
        ALL.to_vec()
    } else if signin {
        vec![Surface::LoginSignin]
    } else if login {
        vec![Surface::LoginUnlock]
    } else {
        vec![Surface::Overlay]
    };
    let first = queue[0];

    // Transparent and undecorated for every surface: the overlay and the
    // preflight need it for their rounded corners, and the login window draws
    // its own titlebar. The panes are drawn on their own opaque CANVAS frame
    // below, so transparency costs them nothing.
    let viewport = egui::ViewportBuilder::default()
        .with_inner_size(first.size())
        .with_resizable(false)
        .with_decorations(false)
        .with_transparent(true)
        .with_icon(theme::window_icon());
    let options = eframe::NativeOptions { viewport, ..Default::default() };

    // Single-surface runs keep their historical file names, because notes and
    // plans elsewhere refer to them by path. `--all` gets a directory, so the
    // artifact upload is one glob and a reviewer sees the set together.
    //
    // The BASE is `target_dir()`, not a bare relative `target/`: this used to
    // write into whatever `./target` the shell happened to be standing over,
    // which in a normal checkout is `deskwarden/target` -- the one directory
    // this project forbids writing to, because the user runs the app out of
    // it. See `target_dir` for the fallback that keeps CI's path unchanged.
    let out: PathBuf = if all {
        target_dir().join("ui_preview")
    } else if signin {
        target_dir().join("ui_preview_signin.png")
    } else if login {
        target_dir().join("ui_preview_login.png")
    } else {
        target_dir().join("ui_preview_overlay.png")
    };

    // Cloned before the closure takes it: the count check below outlives
    // the run.
    let out_dir = out.clone();
    let outcome = eframe::run_native(
        "Deskwarden preview",
        options,
        Box::new(move |_cc| {
            Ok(Box::new(Preview {
                queue,
                at: 0,
                directory: all,
                out,
                form: LoginForm::default(),
                // The app name a real 3c card would have been pre-filled with,
                // and nothing else: the other three rows are what this app can
                // actually know about the window, which is nothing.
                save_login: overlay_ui::SaveLoginForm::new("Atlas Licence"),
                screenshot,
                done: false,
                frames: 0,
                window_height: 0.0,
                styled: false,
                fixtures: Fixtures::new(),
            }))
        }),
    );

    // **The run's own error comes first.** Counting before propagating it
    // turned a real eframe failure into "wrote 0 PNG(s)", which says nothing
    // about why -- measured on a CI runner, where this masked the actual
    // cause for a whole run.
    outcome?;

    // **The walk checks its own arithmetic, so CI does not have to.**
    //
    // The workflow used to assert a hardcoded PNG count. It said nine
    // while `ALL` held eleven, and the screenshots job was red over a
    // number nobody had to touch when a surface was added -- two
    // enumerations obliged to agree, which is the defect this crate keeps
    // losing to. The count now lives where the surfaces do.
    if all {
        let written = std::fs::read_dir(&out_dir)
            .map(|d| {
                d.filter_map(Result::ok)
                    .filter(|e| e.path().extension().is_some_and(|x| x == "png"))
                    .count()
            })
            .unwrap_or(0);
        assert_eq!(
            written,
            ALL.len(),
            "the preview walk wrote {} PNG(s) into {} for {} surface(s) -- a walk that stops part way leaves a perfectly valid, perfectly useless artifact",
            written,
            out_dir.display(),
            ALL.len()
        );
    }
    Ok(())
}

/// How many settled frames a surface gets before it is captured.
///
/// Not one: fonts go live a frame after `set_fonts`, a viewport resize lands
/// asynchronously, and the panes lay out against a width they only learn from
/// the frame they are drawn in. The counter is reset whenever a resize is
/// issued (see `draw_login` and `advance`), so this is "frames since the
/// geometry last moved" rather than "frames since the surface appeared".
const WARMUP_FRAMES: u32 = 12;

struct Preview {
    /// The surfaces to draw, and where in them we are.
    queue: Vec<Surface>,
    at: usize,
    /// Whether `out` names a directory (`--all`) or a single PNG.
    directory: bool,
    out: PathBuf,
    /// Form state for the login preview (typing works; Continue doesn't).
    form: LoginForm,
    /// Form state for design 3c (typing works; Save doesn't -- there is no
    /// vault behind this example).
    save_login: overlay_ui::SaveLoginForm,
    /// Capture and exit, rather than sit there being looked at.
    screenshot: bool,
    /// Every surface has been captured and `Close` has been asked for. The
    /// frames eframe still draws after that must not try to capture a tenth.
    done: bool,
    /// Frames drawn since this surface's geometry last moved.
    frames: u32,
    /// Last applied window height, for the login window's size-to-content.
    window_height: f32,
    /// Whether the theme has been applied yet. Done on the first update
    /// frame, not in the creation context, for the same reason as the real
    /// windows (see login_ui): eframe re-applies its own style after
    /// creation, and egui font sets go live a frame after `set_fonts`.
    styled: bool,
    fixtures: Fixtures,
}

impl Preview {
    fn current(&self) -> Surface {
        self.queue[self.at]
    }

    /// Where this surface's PNG goes.
    fn png_path(&self) -> PathBuf {
        if self.directory {
            self.out.join(format!("{}.png", self.current().stem()))
        } else {
            self.out.clone()
        }
    }

    /// Moves to the next surface, or reports that there is none.
    ///
    /// **`at` is not advanced past the last surface.** `Close` is a request,
    /// not a return: eframe draws at least one more frame after it, and an
    /// index one past the end made that frame panic on `current()` -- after
    /// every PNG had been written, so the artifacts were right and the job
    /// still failed.
    fn advance(&mut self, ctx: &egui::Context) -> bool {
        if self.at + 1 >= self.queue.len() {
            self.done = true;
            return false;
        }
        self.at += 1;
        self.frames = 0;
        self.window_height = 0.0;
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(self.current().size()));
        if self.current().is_login_window() {
            login_ui::round_window_corners("Deskwarden preview");
        }
        true
    }
}

impl eframe::App for Preview {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::TRANSPARENT.to_array()
    }

    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();
        if !self.styled {
            theme::apply(&ctx);
            if self.current().is_login_window() {
                login_ui::round_window_corners("Deskwarden preview");
            }
            self.styled = true;
            ctx.request_repaint();
            return;
        }
        self.frames += 1;

        match self.current() {
            Surface::Overlay => self.draw_overlay(root, &ctx),
            Surface::OverlayNoMatch => self.draw_overlay_no_match(root, &ctx),
            Surface::OverlayLocked => self.draw_overlay_locked(root, &ctx),
            Surface::OverlaySaveLogin => self.draw_overlay_save_login(root, &ctx),
            Surface::LoginUnlock => self.draw_login(root, &ctx, false),
            Surface::LoginSignin => self.draw_login(root, &ctx, true),
            Surface::LoginDetail => self.draw_pane(root, PaneKind::Detail(DetailShot::Login)),
            Surface::CardDetail => self.draw_pane(root, PaneKind::Detail(DetailShot::Card)),
            Surface::CardDetailRevealed => {
                self.draw_pane(root, PaneKind::Detail(DetailShot::CardRevealed))
            }
            Surface::DiscardConfirm => self.draw_pane(root, PaneKind::Discard),
            Surface::RecordComposer => self.draw_pane(root, PaneKind::Composer),
            Surface::TotpAddConfirm => self.draw_pane(root, PaneKind::TotpAdd),
            Surface::TotpAddPicker => self.draw_pane(root, PaneKind::TotpPicker),
            Surface::PreflightAllowed => self.draw_pane(root, PaneKind::Preflight(true)),
            Surface::PreflightRefused => self.draw_pane(root, PaneKind::Preflight(false)),
            Surface::PrefsClipboard => self.draw_prefs(root, true),
            Surface::PrefsClipboardOff => self.draw_prefs(root, false),
            Surface::Rehearsal => self.draw_rehearsal(root),
        }

        if self.screenshot && !self.done {
            if self.frames == WARMUP_FRAMES {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
            }
            let captured = ctx.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::Screenshot { image, .. } => Some(image.clone()),
                    _ => None,
                })
            });
            if let Some(image) = captured {
                let path = self.png_path();
                // The directory is created rather than assumed. A build with
                // `CARGO_TARGET_DIR` pointed elsewhere has no `./target`, and
                // this example used to panic on exactly that -- which is what
                // would happen on a runner that caches its target directory
                // outside the checkout.
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).expect("could not create the screenshot dir");
                }
                save_png(&path, &image).expect("could not write the screenshot PNG");
                println!("wrote {}", path.display());
                if !self.advance(&ctx) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            // Keep frames coming: a hidden/idle window repaints lazily, and
            // the screenshot round-trip needs the pump to keep turning.
            ctx.request_repaint();
        }

        if self.current() == Surface::Overlay
            && ctx.input(|i| i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::Enter))
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

/// Which of the in-window panes `draw_pane` is drawing.
enum PaneKind {
    /// The read pane, in one of its three shots.
    Detail(DetailShot),
    /// The edit form with its discard confirmation up.
    Discard,
    /// The Send record composer.
    Composer,
    /// The "add a one-time code" form, mid-confirmation.
    TotpAdd,
    /// Design 6a's picker, the front door onto the four routes.
    TotpPicker,
    /// The preflight; `true` for the allowed state, `false` for the refusal.
    Preflight(bool),
}

/// Which fixture the read pane is drawn from, and in what reveal state.
///
/// A three-way enum rather than the pair of bools it would otherwise have
/// become: `Detail(true, false)` at a call site names neither of the things it
/// decides, and the reveal flag is the whole reason the third shot exists.
#[derive(Clone, Copy)]
enum DetailShot {
    /// The login fixture, masked -- `detail_login`.
    Login,
    /// The card fixture as it opens: number and code masked -- `detail_card`.
    Card,
    /// The card fixture with the NUMBER revealed and the security code still
    /// masked -- `detail_card_revealed`. The code stays hidden because that is
    /// the state a user reading their number is really in (the two rows have
    /// separate flags), and because a CVV in a checked-in PNG is worth
    /// avoiding even from a fixture.
    CardRevealed,
}

impl Preview {
    fn draw_overlay(&mut self, root: &mut egui::Ui, ctx: &egui::Context) {
        egui::CentralPanel::default().frame(egui::Frame::new()).show(root, |ui| {
            // The preview closes on the dismiss ✕ too, so the affordance can
            // actually be clicked here rather than only looked at.
            if overlay_ui::draw_overlay_card(
                ui,
                "ledgerline.exe",
                "Ledgerline",
                Some("a.novak@ledgerline.com"),
            ) == overlay_ui::OverlayAction::Dismiss
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
    }

    /// Design 3a, drawn by the shipped function rather than re-implemented --
    /// which is why `draw_no_match_card` is public.
    fn draw_overlay_no_match(&mut self, root: &mut egui::Ui, ctx: &egui::Context) {
        egui::CentralPanel::default().frame(egui::Frame::new()).show(root, |ui| {
            if overlay_ui::draw_no_match_card(ui, "Atlas Licence")
                == overlay_ui::OverlayAction::Dismiss
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
    }

    /// Design 3b, drawn by the shipped function for the same reason 3a is:
    /// this card and the no-match one differ by three strings, and a
    /// re-implementation here could show either while the app showed the
    /// other.
    fn draw_overlay_locked(&mut self, root: &mut egui::Ui, ctx: &egui::Context) {
        egui::CentralPanel::default().frame(egui::Frame::new()).show(root, |ui| {
            if overlay_ui::draw_locked_card(ui, "Atlas Licence")
                == overlay_ui::OverlayAction::Dismiss
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
    }

    /// Design 3c, drawn by the shipped function for the same reason 3a and 3b
    /// are.
    ///
    /// The form is drawn **blank apart from the App row**, which is the state
    /// the user is really shown: exactly one of the four rows can be
    /// pre-filled, because `injector::ui_automation` has no username reader
    /// and a password field's contents are not read. A preview that typed
    /// plausible values into the other two would be a picture of a capture
    /// this app does not make.
    fn draw_overlay_save_login(&mut self, root: &mut egui::Ui, ctx: &egui::Context) {
        egui::CentralPanel::default().frame(egui::Frame::new()).show(root, |ui| {
            if overlay_ui::draw_save_login_card(ui, &mut self.save_login)
                != overlay_ui::SaveLoginAction::None
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
    }

    fn draw_login(&mut self, root: &mut egui::Ui, ctx: &egui::Context, signin: bool) {
        // The exact chrome the shipped window draws.
        if login_ui::draw_window_chrome(root, "Log in to Deskwarden")
            == login_ui::ChromeAction::Close
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        let mut resized = false;
        egui::Frame::new()
            .inner_margin(Margin { left: 26, right: 26, top: 24, bottom: 30 })
            .show(root, |ui| {
                ui.set_min_width(ui.available_width());
                // Sample data mirroring the 3h mock (unlock: Hello shown as
                // enrolled so the panel renders; sign-in: available but
                // unenrolled so the opt-in and server dropdown render);
                // actions are ignored -- a preview must never spawn `bw` or
                // pop Hello.
                let (status, email, hello) = if signin {
                    // Self-hosted: the tallest state (URL field + email +
                    // password + Hello panel), which is what overflowed a
                    // fixed-height window.
                    self.form.server_choice = login_ui::ServerChoice::SelfHosted;
                    (
                        BwStatus::Unauthenticated,
                        None,
                        HelloState { available: true, enrolled: false },
                    )
                } else {
                    (
                        BwStatus::Locked,
                        Some("a.novak@ledgerline.com"),
                        HelloState { available: true, enrolled: true },
                    )
                };
                let mut flow_bottom = 0.0;
                let _ = login_ui::draw_login_window(
                    ui,
                    status,
                    email,
                    "vault.ledgerline.eu",
                    hello,
                    &mut self.form,
                    &mut flow_bottom,
                    // Never in flight here: this preview draws the window's
                    // states, it does not run a real sign-in.
                    false,
                    // Not a first run: the preview draws the window an
                    // existing account meets, so the first-run notice is not
                    // part of what this screenshots.
                    false,
                );
                // Size to content, exactly as run_login_flow does, so the
                // screenshot shows the window the app would show.
                let wanted = (flow_bottom + login_ui::FOOTER_RESERVE).ceil();
                if (wanted - self.window_height).abs() > 0.5 {
                    self.window_height = wanted;
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                        470.0, wanted,
                    )));
                    resized = true;
                }
            });
        // The geometry moved, so the warm-up starts again: a capture taken on
        // the frame a resize was asked for shows the PREVIOUS size, which is
        // how a screenshot of a window nobody ships gets into an artifact.
        if resized {
            self.frames = 0;
        }
    }

    /// The preferences window's Clipboard page.
    ///
    /// Draws `prefs_ui::draw_prefs_body` -- the real nav-plus-content shell,
    /// not a reconstruction of it -- on the window's own background, so the
    /// PNG shows the page exactly as the app draws it. The `PrefsState` is
    /// rebuilt each frame rather than held: nothing on this page needs to
    /// survive between frames for a screenshot, and a held state would make
    /// the two surfaces share the interval field's text buffer.
    fn draw_prefs(&mut self, root: &mut egui::Ui, master_on: bool) {
        theme::paint_window_background(root);
        let mut state = prefs_ui::PrefsState::new(deskwarden::settings::Settings {
            clear_clipboard: master_on,
            ..deskwarden::settings::Settings::default()
        });
        state.show(prefs_ui::Section::Clipboard);
        egui::CentralPanel::default()
            .frame(egui::Frame::new())
            .show(root, |ui| prefs_ui::draw_prefs_body(ui, &mut state));
    }

    /// **Design 4d, finished**, drawn exactly as the rehearsal viewport draws
    /// it: `scratch_window::draw` on a root `Ui` filling the window, which is
    /// what `show_viewport_deferred` hands its callback.
    ///
    /// The arrived text is the literal a Win32 edit control would hold after
    /// the design's sequence -- a tab and a Windows line ending -- so the two
    /// glyph substitutions `rehearsal::arrived_panel` makes are in the
    /// picture rather than merely in a unit test.
    ///
    /// Nothing here sends anything: there is no window, no `Injector` and no
    /// plan. The view is a value.
    fn draw_rehearsal(&mut self, root: &mut egui::Ui) {
        let _ = scratch_window::draw(
            root,
            &self.fixtures.rehearsal,
            &mut self.fixtures.rehearsal_arrived,
        );
    }

    /// The surfaces that live *inside* the vault window rather than in one of
    /// their own, drawn on the window's own canvas so the PNG shows them
    /// against the background they actually sit on.
    fn draw_pane(&mut self, root: &mut egui::Ui, kind: PaneKind) {
        let fixtures = &mut self.fixtures;
        egui::CentralPanel::default()
            .frame(pane_frame())
            .show(root, |ui| match kind {
                PaneKind::Detail(shot) => {
                    let card = !matches!(shot, DetailShot::Login);
                    let item = if card { &fixtures.card } else { &fixtures.login };
                    // **Set, never toggled.** One `Fixtures` is shared by the
                    // whole `--all` walk, so a reveal flag left standing would
                    // change a later surface depending on the order the walk
                    // happened to run in. Assigning the whole state makes each
                    // shot independent of the ones before it.
                    fixtures.reveal = RevealState {
                        card_number: matches!(shot, DetailShot::CardRevealed),
                        ..RevealState::default()
                    };
                    // A card has no one-time-code row to have a state for; the
                    // login fixture shows a live code, which is the state that
                    // row spends most of its life in.
                    let no_totp = TotpState::NoSecret;
                    let totp = if card { &no_totp } else { &fixtures.totp };
                    let _ = detail::draw_detail_read(
                        ui,
                        item,
                        Some("Work"),
                        // The preview has no vault behind it, so the kebab's
                        // "Move to folder" submenu says "No folders yet" --
                        // which is a real state of that submenu and the
                        // honest one to show without a folder list.
                        &[],
                        if card { 0 } else { 42 },
                        totp,
                        false,
                        &mut fixtures.reveal,
                        // No favicon texture: the monogram fallback is what
                        // every avatar in this app shows without one, and
                        // loading an image here would mean a file to ship.
                        None,
                        &mut fixtures.apps,
                        // Breach checking OFF -- `detail::should_check` is the
                        // gate, and with it false the cache below is never
                        // asked anything and no worker is started. A preview
                        // must not reach Have I Been Pwned.
                        false,
                        // The TOTP seed row is a preference that is off by
                        // default; the preview draws the default window.
                        false,
                        &mut fixtures.breaches,
                    );
                }
                PaneKind::Discard => {
                    let _ = detail_edit::draw_detail_edit(
                        ui,
                        &mut fixtures.draft,
                        &fixtures.folders,
                        false,
                        &mut fixtures.apps,
                        Some(&fixtures.login),
                        &fixtures.totp,
                    );
                    // The form's Cancel is what sets this in the app; a
                    // preview has nobody to click it, so it is re-armed every
                    // frame. Re-armed rather than set once, because the
                    // confirmation's own KeepEditing arm clears it -- and a
                    // stray Escape delivered to this window would otherwise
                    // take the dialog away before the capture.
                    fixtures.draft.discard_prompt = true;
                }
                PaneKind::Composer => {
                    let _ = record_ui::draw_export_form(
                        ui,
                        &mut fixtures.record,
                        "Ledgerline \u{b7} a.novak@ledgerline.com",
                        false,
                    );
                }
                PaneKind::TotpAdd => {
                    // A FIXED instant, not `SystemTime::now()`: the code and
                    // the countdown are the point of this shot, and a
                    // screenshot whose two most prominent numbers change every
                    // run is a screenshot no reviewer can diff.
                    let _ = totp_add::draw_add_form(ui, &mut fixtures.totp_add, PREVIEW_UNIX);
                }
                PaneKind::TotpPicker => {
                    let _ = totp_add::draw_picker(ui, &mut fixtures.totp_picker);
                }
                PaneKind::Preflight(allowed) => {
                    let state = if allowed { &mut fixtures.allowed } else { &mut fixtures.refused };
                    let _ = preflight::draw(ui, state);
                }
            });
    }
}

/// Everything the panes need, built once from literals.
struct Fixtures {
    login: VaultItem,
    card: VaultItem,
    folders: Vec<Folder>,
    totp: TotpState,
    reveal: RevealState,
    apps: AppIdentityCache,
    breaches: BreachCache,
    draft: EditDraft,
    record: RecordDraft,
    totp_add: TotpAdd,
    totp_picker: TotpAdd,
    allowed: PreflightState,
    refused: PreflightState,
    rehearsal: scratch_window::RehearsalView,
    rehearsal_arrived: String,
}

impl Fixtures {
    fn new() -> Self {
        let login = item(LOGIN_JSON);
        let card = item(CARD_JSON);
        let mut draft = EditDraft::from_item(&login);
        // Dirty, because the confirmation only exists for a dirty draft --
        // showing it over a pristine form would be a picture of a state the
        // app does not reach.
        draft.password.push_str("-edited");
        draft.discard_prompt = true;
        let totp = TotpState::Code { code: "418902".to_string(), seconds_left: 19 };
        // The seed's tick on, so the composer's seed warning -- the sentence
        // that decides whether that tick was a mistake -- is in the picture.
        let mut record = RecordDraft { open: true, ..Default::default() };
        record.set_totp(true);
        // Opened against an item that ALREADY has a code, so the replace
        // warning is in the picture, and with a URI typed whose parameters are
        // all non-default -- 8 digits over 60 seconds under SHA-256 -- because
        // that is the case the confirmation exists to catch and the one a shot
        // of a plain 6/30 card cannot show.
        let mut totp_add = TotpAdd::opening("preview", "Git Host \u{b7} anovak", true);
        totp_add.typed = zeroize::Zeroizing::new(
            "otpauth://totp/Git%20Host:anovak?secret=JBSWY3DPEHPK3PXP&issuer=Git%20Host\
             &digits=8&period=60&algorithm=SHA256"
                .to_string(),
        );
        // A SECOND state of the same form, on design 6a. Carrying a refusal,
        // because a picker with nothing wrong on it shows three of the four
        // things this surface has to get right and not the fourth: a refusal
        // rendered as a sentence that names its reason.
        let mut totp_picker = TotpAdd::opening("preview", "Git Host \u{b7} anovak", true);
        totp_picker.refusal = Some(totp_add::PickerRefusal::NoCode(totp_add::CodeSource::Region));

        Self {
            folders: vec![
                Folder { id: "f-work".into(), name: "Work".into(), other: Default::default() },
                Folder {
                    id: "f-personal".into(),
                    name: "Personal".into(),
                    other: Default::default(),
                },
            ],
            reveal: RevealState::default(),
            apps: AppIdentityCache::default(),
            // A check that is never called: `check_breaches` is false at the
            // call site, so `should_check` refuses before this is consulted.
            // It panics rather than returning a plausible answer, so a future
            // change that starts asking is a loud failure in CI instead of a
            // screenshot quietly claiming a password is safe.
            breaches: BreachCache::new(std::sync::Arc::new(|_, _| {
                unreachable!("a preview must never check a password against a breach corpus")
            })),
            allowed: preflight_state(true, &login, &totp),
            refused: preflight_state(false, &login, &totp),
            rehearsal: rehearsal_view(),
            // What a text field really holds after the design's sequence: the
            // Tab arrived as a tab, the Enter as a Windows line ending.
            rehearsal_arrived: format!(
                "{}	
{}
",
                rehearsal::SAMPLE_USER,
                rehearsal::SAMPLE_PASSWORD
            ),
            record,
            totp_add,
            totp_picker,
            draft,
            login,
            card,
            totp,
        }
    }
}

/// **Design 4d's finished rehearsal, built the way production builds it.**
///
/// `sample_plan` -> `rehearsal_plan` -> `transcript` is the exact chain the
/// window runs, so the acts listed in the PNG are the acts a real rehearsal of
/// the design's sequence produces -- chunking, joining and all -- rather than a
/// hand-written list that could drift from it. No vault is touched: every field
/// in that plan resolves to a fixed sample by construction.
fn rehearsal_view() -> scratch_window::RehearsalView {
    const DESIGN_SEQUENCE: &str = "{USERNAME}{TAB}{DELAY 250}{PASSWORD}{ENTER}";
    let planned = rehearsal::sample_plan(DESIGN_SEQUENCE).expect("the design's sequence plans");
    let sent = rehearsal::transcript(
        &rehearsal::rehearsal_plan(&planned).expect("the substituted sequence re-plans"),
    );
    scratch_window::RehearsalView {
        headline: rehearsal::finished_line(
            std::time::Duration::from_millis(2100),
            sent.len(),
        ),
        finished: true,
        sent,
        failure: None,
    }
}

/// A fixture item, in the wire shape `bw serve` returns.
///
/// Parsed rather than built field by field, for the reason the wire tests are
/// written the same way: a struct literal keeps compiling when the JSON the app
/// actually receives has moved on, and a fixture that no longer describes the
/// real thing is worse than no fixture.
fn item(json: &str) -> VaultItem {
    serde_json::from_str(json).expect("the preview's fixture item must parse as a VaultItem")
}

const LOGIN_JSON: &str = r#"{
  "id": "6f1c2f5e-0000-4a10-9c31-2b7a51d0a001",
  "type": 1,
  "name": "Ledgerline",
  "folderId": "f-work",
  "favorite": true,
  "notes": "Finance approves new seats on the first Monday of the month.",
  "login": {
    "username": "a.novak@ledgerline.com",
    "password": "correct-horse-battery-staple-7",
    "totp": "otpauth://totp/Ledgerline:a.novak?secret=JBSWY3DPEHPK3PXP&issuer=Ledgerline",
    "uris": [{ "uri": "https://app.ledgerline.eu/signin" }]
  },
  "fields": [{ "name": "Employee ID", "value": "LL-40912", "type": 0 }]
}"#;

const CARD_JSON: &str = r#"{
  "id": "6f1c2f5e-0000-4a10-9c31-2b7a51d0a002",
  "type": 3,
  "name": "Ledgerline corporate card",
  "folderId": "f-work",
  "notes": "Expenses only. Anything over EUR 500 needs a purchase order first.",
  "card": {
    "cardholderName": "ANNA NOVAK",
    "brand": "Visa",
    "number": "4111111111111111",
    "expMonth": "11",
    "expYear": "2029",
    "code": "417"
  }
}"#;

/// The two preflight states.
///
/// `allowed`: the rule's own process is in front and the focused control is
/// masked. `refused`: a different process, with an unmasked control -- both
/// facts wrong at once, which is the state whose message has to name both.
fn preflight_state(allowed: bool, item: &VaultItem, totp: &TotpState) -> PreflightState {
    let target = if allowed {
        SendTarget {
            title: "Ledgerline \u{2014} Sign in".to_string(),
            image_name: "ledgerline.exe".to_string(),
            pid: 8124,
            class_name: "Chrome_WidgetWin_1".to_string(),
            focused_is_masked: true,
        }
    } else {
        SendTarget {
            title: "chat \u{2014} #finance".to_string(),
            image_name: "teams.exe".to_string(),
            pid: 5310,
            class_name: "Chrome_WidgetWin_1".to_string(),
            focused_is_masked: false,
        }
    };
    let login = item.login.as_ref();
    let source = ResolveSource {
        username: login.and_then(|l| l.username.as_deref()).unwrap_or(""),
        password: login.and_then(|l| l.password.as_deref()).map(|p| p.as_str()).unwrap_or(""),
        custom: deskwarden::key_sequence::custom_pairs(item),
        totp,
    };
    PreflightState::new(target, "ledgerline.exe", "{USERNAME}{TAB}{PASSWORD}{ENTER}", &source)
}

fn save_png(path: &PathBuf, image: &egui::ColorImage) -> Result<(), Box<dyn std::error::Error>> {
    let [w, h] = image.size;
    let mut data = Vec::with_capacity(w * h * 4);
    for p in &image.pixels {
        data.extend_from_slice(&p.to_array());
    }
    let file = std::fs::File::create(path)?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&data)?;
    Ok(())
}
