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
use deskwarden::{app_identity::AppIdentityCache, overlay_ui, theme};
use eframe::egui::{self, Margin};
use std::path::PathBuf;

/// One screenshotable surface.
///
/// The list is the point of this file: adding a window to the app and not
/// adding it here is how a surface goes unlooked-at for a year.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Surface {
    /// The autofill overlay (design 2a).
    Overlay,
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
    /// The edit form with the discard confirmation over it.
    DiscardConfirm,
    /// The record composer -- the Send export form and its seed warning.
    RecordComposer,
    /// The preflight, allowed: the rule's process is in front and the focused
    /// control is masked, so the hold-to-send is offered.
    PreflightAllowed,
    /// The preflight, refused: a password sequence aimed at the wrong process
    /// and an unmasked control, which is the state that must never grow a
    /// send button.
    PreflightRefused,
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
    Surface::LoginUnlock,
    Surface::LoginSignin,
    Surface::LoginDetail,
    Surface::CardDetail,
    Surface::DiscardConfirm,
    Surface::RecordComposer,
    Surface::PreflightAllowed,
    Surface::PreflightRefused,
];

impl Surface {
    /// The PNG's file stem. Stable, because a human reviewing an artifact
    /// looks for the same name every time.
    fn stem(self) -> &'static str {
        match self {
            Surface::Overlay => "overlay",
            Surface::LoginUnlock => "login_unlock",
            Surface::LoginSignin => "login_signin",
            Surface::LoginDetail => "detail_login",
            Surface::CardDetail => "detail_card",
            Surface::DiscardConfirm => "edit_discard_confirm",
            Surface::RecordComposer => "record_composer",
            Surface::PreflightAllowed => "preflight_allowed",
            Surface::PreflightRefused => "preflight_refused",
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
            Surface::Overlay => egui::vec2(396.0, 164.0),
            Surface::LoginUnlock | Surface::LoginSignin => egui::vec2(470.0, 588.0),
            Surface::LoginDetail
            | Surface::CardDetail
            | Surface::DiscardConfirm
            | Surface::RecordComposer => egui::vec2(PANE_WIDTH, PANE_HEIGHT),
            Surface::PreflightAllowed | Surface::PreflightRefused => egui::vec2(
                deskwarden::preflight_host::PREFLIGHT_WIDTH,
                deskwarden::preflight_host::PREFLIGHT_HEIGHT,
            ),
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
    let out: PathBuf = if all {
        PathBuf::from("target/ui_preview")
    } else if signin {
        PathBuf::from("target/ui_preview_signin.png")
    } else if login {
        PathBuf::from("target/ui_preview_login.png")
    } else {
        PathBuf::from("target/ui_preview_overlay.png")
    };

    eframe::run_native(
        "Deskwarden preview",
        options,
        Box::new(move |_cc| {
            Ok(Box::new(Preview {
                queue,
                at: 0,
                directory: all,
                out,
                form: LoginForm::default(),
                screenshot,
                done: false,
                frames: 0,
                window_height: 0.0,
                styled: false,
                fixtures: Fixtures::new(),
            }))
        }),
    )
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
            Surface::LoginUnlock => self.draw_login(root, &ctx, false),
            Surface::LoginSignin => self.draw_login(root, &ctx, true),
            Surface::LoginDetail => self.draw_pane(root, PaneKind::Detail(false)),
            Surface::CardDetail => self.draw_pane(root, PaneKind::Detail(true)),
            Surface::DiscardConfirm => self.draw_pane(root, PaneKind::Discard),
            Surface::RecordComposer => self.draw_pane(root, PaneKind::Composer),
            Surface::PreflightAllowed => self.draw_pane(root, PaneKind::Preflight(true)),
            Surface::PreflightRefused => self.draw_pane(root, PaneKind::Preflight(false)),
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
    /// The read pane; `true` for the card fixture, `false` for the login one.
    Detail(bool),
    /// The edit form with its discard confirmation up.
    Discard,
    /// The Send record composer.
    Composer,
    /// The preflight; `true` for the allowed state, `false` for the refusal.
    Preflight(bool),
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

    /// The surfaces that live *inside* the vault window rather than in one of
    /// their own, drawn on the window's own canvas so the PNG shows them
    /// against the background they actually sit on.
    fn draw_pane(&mut self, root: &mut egui::Ui, kind: PaneKind) {
        let fixtures = &mut self.fixtures;
        egui::CentralPanel::default()
            .frame(pane_frame())
            .show(root, |ui| match kind {
                PaneKind::Detail(card) => {
                    let item = if card { &fixtures.card } else { &fixtures.login };
                    // A card has no one-time-code row to have a state for; the
                    // login fixture shows a live code, which is the state that
                    // row spends most of its life in.
                    let no_totp = TotpState::NoSecret;
                    let totp = if card { &no_totp } else { &fixtures.totp };
                    let _ = detail::draw_detail_read(
                        ui,
                        item,
                        Some("Work"),
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
    allowed: PreflightState,
    refused: PreflightState,
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
            record,
            draft,
            login,
            card,
            totp,
        }
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
