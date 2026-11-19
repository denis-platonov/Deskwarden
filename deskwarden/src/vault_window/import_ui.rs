//! Spec B section 3: the Import wizard -- source, file, progress, done.
//!
//! The file is read, parsed and de-duplicated here ([`Wizard::begin`]); the
//! writes then run on the window's one shared worker ([`launch_on_cache`]) so
//! the counter on the progress step moves while items land. What each format
//! maps to, and why the items are created one at a time through the existing
//! create path rather than posted to the bulk endpoint, is
//! `import_formats`' business and the slice plan's Rulings.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::Duration;

use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Vec2};
use zeroize::Zeroizing;

use super::import_formats::{self, ParseError, Plan, Report, Sinks, ImportSource};
use crate::kit::controls::{self, ButtonKind, ButtonSize};
use crate::kit::icon;
use crate::kit::modal::{Dialog, ModalOutcome};
use crate::theme;
use crate::vault_bridge::{VaultError, VaultItem};
use crate::vault_cache::VaultCache;

/// The scrim's area name (listed in `item_list::MODAL_SCRIM_AREAS`).
pub const ID: &str = "import";
pub const WIDTH: f32 = 620.0;
pub const SOURCE_TITLE: &str = "Import passwords";
pub const SOURCE_SUB: &str = "Where are they now?";
pub const PROGRESS_TITLE: &str = "Importing\u{2026}";
pub const PROGRESS_SUB: &str = "This takes a few seconds";
pub const DONE_TITLE: &str = "All set";
pub const DROP_HEADLINE: &str = "Drop the file here";
pub const DROP_SUB: &str = "or click to choose it";
pub const DELETE_NOTE: &str = "Delete the exported file once you're done \u{2014} it isn't encrypted.";
pub const COUNT_LABEL: &str = "items brought in";
pub const BACK: &str = "Back";
pub const DONE: &str = "Done";
pub const SEE_ITEMS: &str = "See my items";
/// The glyph for "Import passwords" in the palette: a tray with an arrow in.
pub const DOWNLOAD_TRAY: &str = "M12 4v11M7 10l5 5 5-5M4 15v4a1 1 0 0 0 1 1h14a1 1 0 0 0 1-1v-4";
const UPLOAD: &str = "M12 16V5M7 10l5-5 5 5M4 16v3a1 1 0 0 0 1 1h14a1 1 0 0 0 1-1v-3";

const EDGE: Color32 = Color32::from_rgb(0xe8, 0xe6, 0xe3);
const TILE_BG: Color32 = Color32::from_rgb(0xf1, 0xf0, 0xee);
const NAVY: Color32 = Color32::from_rgb(0x14, 0x30, 0x7a);
const MUTED: Color32 = Color32::from_rgb(0x7d, 0x79, 0x79);
const NOTE_INK: Color32 = Color32::from_rgb(0x8a, 0x87, 0x85);
const GREEN: Color32 = Color32::from_rgb(0x1b, 0x7a, 0x3f);
const GREEN_WASH: Color32 = Color32::from_rgb(0xe8, 0xf3, 0xec);
const BAR: Color32 = Color32::from_rgb(0xee, 0xec, 0xe9);
const GAP: f32 = 10.0;
const TILE_HEIGHT: f32 = 104.0;
const ZONE_HEIGHT: f32 = 220.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    ImportSource,
    File,
    Progress,
    Done,
}

/// What the wizard says it did, on the last step.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub imported: usize,
    pub duplicates: usize,
    pub failed: usize,
    pub left_out: usize,
    pub stopped_early: bool,
}

struct Job {
    progress: Arc<AtomicUsize>,
    cancel: Arc<AtomicBool>,
    total: usize,
    rx: Receiver<Report>,
}

/// The wizard's own state while it is up.
pub struct Wizard {
    pub step: Step,
    pub source: Option<ImportSource>,
    /// Why the last file was refused, shown on the file step.
    pub error: Option<&'static str>,
    pub summary: Option<Summary>,
    job: Option<Job>,
}

impl Default for Wizard {
    fn default() -> Self {
        Self { step: Step::ImportSource, source: None, error: None, summary: None, job: None }
    }
}

/// What the wizard did this frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    Open,
    Closed,
    /// The user wants the file dialog; the window opens it between frames.
    PickFile,
    /// A file was dropped on the zone.
    File(std::path::PathBuf),
    /// "See my items": close, and show the whole vault.
    SeeItems,
}

/// The write side's hand-back: the first error, so the window can notice a
/// signed-out session.
pub struct Finished {
    pub first_error: Option<VaultError>,
}

impl Wizard {
    pub fn title(&self) -> String {
        match self.step {
            Step::ImportSource => SOURCE_TITLE.to_string(),
            Step::File => format!("From {}", self.source.map_or("", ImportSource::label)),
            Step::Progress => PROGRESS_TITLE.to_string(),
            Step::Done => DONE_TITLE.to_string(),
        }
    }

    pub fn sub(&self) -> Option<String> {
        match self.step {
            Step::ImportSource => Some(SOURCE_SUB.to_string()),
            Step::File => {
                let label = self.source.map_or("", ImportSource::label);
                Some(format!("Export a file from {label}, then add it here."))
            }
            Step::Progress => Some(PROGRESS_SUB.to_string()),
            Step::Done => None,
        }
    }

    /// A wizard parked on `step`, for the preview example: nothing is read or
    /// written. `progress` is `(landed, total)` for the progress step.
    pub fn preview(step: Step, source: ImportSource, summary: Summary, progress: (usize, usize)) -> Self {
        let job = (step == Step::Progress).then(|| {
            // The sender is dropped at once, so the job never finishes.
            let (_, rx) = mpsc::channel();
            Job {
                progress: Arc::new(AtomicUsize::new(progress.0)),
                cancel: Arc::new(AtomicBool::new(false)),
                total: progress.1,
                rx,
            }
        });
        Self { step, source: Some(source), error: None, summary: Some(summary), job }
    }

    pub fn is_running(&self) -> bool {
        self.job.is_some()
    }

    /// How many items have landed, and how many will.
    pub fn counted(&self) -> (usize, usize) {
        self.job.as_ref().map_or((0, 0), |j| (j.progress.load(Ordering::Relaxed), j.total))
    }

    /// Reads the file at `path` and starts the import. A file that cannot be
    /// used leaves the wizard on the file step with the reason.
    pub fn begin(
        &mut self,
        path: &Path,
        existing: &[VaultItem],
        launch: impl FnOnce(Plan, Arc<AtomicUsize>, Arc<AtomicBool>) -> Receiver<Report>,
    ) {
        let bytes = match read_file(path) {
            Ok(bytes) => bytes,
            Err(e) => {
                self.error = Some(e.sentence());
                return;
            }
        };
        self.begin_from_bytes(&bytes, existing, launch);
    }

    /// [`Self::begin`] after the read; separate so a test can hand it bytes.
    pub fn begin_from_bytes(
        &mut self,
        bytes: &[u8],
        existing: &[VaultItem],
        launch: impl FnOnce(Plan, Arc<AtomicUsize>, Arc<AtomicBool>) -> Receiver<Report>,
    ) {
        let Some(source) = self.source else { return };
        let parsed = match import_formats::parse(source, bytes) {
            Ok(parsed) => parsed,
            Err(e) => {
                self.error = Some(e.sentence());
                return;
            }
        };
        let plan = import_formats::plan(parsed, existing);
        self.error = None;
        let total = plan.to_create.len();
        if total == 0 {
            // Everything was already here: nothing to write, so no progress.
            self.summary = Some(Summary {
                duplicates: plan.duplicates,
                left_out: plan.left_out,
                ..Summary::default()
            });
            self.step = Step::Done;
            return;
        }
        let progress = Arc::new(AtomicUsize::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let rx = launch(plan, progress.clone(), cancel.clone());
        self.job = Some(Job { progress, cancel, total, rx });
        self.step = Step::Progress;
    }

    /// Answers a finished import, once, on the frame it finishes.
    pub fn poll(&mut self) -> Option<Finished> {
        let report = self.job.as_ref()?.rx.try_recv().ok()?;
        self.job = None;
        self.summary = Some(Summary {
            imported: report.imported,
            duplicates: report.duplicates,
            failed: report.failed,
            left_out: report.left_out,
            stopped_early: report.stopped_early,
        });
        self.step = Step::Done;
        Some(Finished { first_error: report.first_error })
    }

    /// Asks a running import to stop after the item in hand.
    pub fn cancel(&self) {
        if let Some(job) = &self.job {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }
}

/// The file's bytes, held so they are wiped when dropped.
fn read_file(path: &Path) -> Result<Zeroizing<Vec<u8>>, ParseError> {
    let meta = std::fs::metadata(path).map_err(|_| ParseError::Unreadable)?;
    if meta.len() > import_formats::MAX_FILE_BYTES {
        return Err(ParseError::TooLarge);
    }
    std::fs::read(path).map(Zeroizing::new).map_err(|_| ParseError::Unreadable)
}

/// The production launcher: the writes go through `cache`, on the window's one
/// shared worker.
pub fn launch_on_cache(
    ctx: egui::Context,
    cache: Arc<VaultCache>,
) -> impl FnOnce(Plan, Arc<AtomicUsize>, Arc<AtomicBool>) -> Receiver<Report> {
    move |plan, progress, cancel| {
        let (tx, rx) = mpsc::channel();
        let work = move || {
            let existing_folders = cache.folders();
            let mut create_folder = |name: &str| cache.create_folder(name);
            let mut create_item = |item: &crate::vault_bridge::NewItem| cache.create_item(item).map(|_| ());
            import_formats::run(
                plan,
                Sinks { existing_folders, create_folder: &mut create_folder, create_item: &mut create_item },
                &progress,
                &cancel,
            )
        };
        // A worker that panics is reported as a stopped import with nothing
        // known to have landed beyond the counter.
        super::spawn_reporting(ctx, tx, work, || Report { stopped_early: true, failed: 1, ..Report::default() });
        rx
    }
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

pub fn tile_id(source: ImportSource) -> egui::Id {
    egui::Id::new(("import-tile", source.label()))
}
pub fn zone_id() -> egui::Id {
    egui::Id::new("import-zone")
}
pub fn back_id() -> egui::Id {
    egui::Id::new("import-back")
}
pub fn done_id() -> egui::Id {
    egui::Id::new("import-done")
}
pub fn see_id() -> egui::Id {
    egui::Id::new("import-see")
}

fn button_size(height: f32, pad_x: f32) -> ButtonSize {
    ButtonSize { height, pad_x, radius: 14, font_px: 16.0 }
}

/// "128 items imported".
pub fn imported_headline(count: usize) -> String {
    format!("{count} {} imported", if count == 1 { "item" } else { "items" })
}

/// The sentences under the headline: duplicates first (the spec's), then what
/// did not land.
pub fn done_lines(summary: &Summary) -> Vec<(String, bool)> {
    let plural = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let mut lines = Vec::new();
    if summary.duplicates > 0 {
        let verb = if summary.duplicates == 1 { "was" } else { "were" };
        lines.push((format!("{} {verb} skipped", plural(summary.duplicates, "duplicate", "duplicates")), false));
    }
    if summary.left_out > 0 {
        let verb = if summary.left_out == 1 { "was" } else { "were" };
        lines.push((format!("{} {verb} left out", plural(summary.left_out, "entry", "entries")), false));
    }
    if summary.failed > 0 {
        lines.push((
            format!("{} couldn't be imported", plural(summary.failed, "item", "items")),
            true,
        ));
    }
    if summary.stopped_early && summary.failed == 0 {
        lines.push(("The import was stopped before it finished".to_string(), true));
    }
    lines
}

pub fn draw(ctx: &egui::Context, wizard: &mut Wizard) -> Answer {
    let title = wizard.title();
    let sub = wizard.sub();
    let mut dialog = Dialog::new(ID, WIDTH).title(&title);
    if let Some(sub) = sub.as_deref() {
        dialog = dialog.sub(sub);
    }
    let (outcome, answer) = dialog.show(ctx, |ui| match wizard.step {
        Step::ImportSource => source_step(ui, wizard),
        Step::File => file_step(ui, wizard),
        Step::Progress => {
            progress_step(ui, wizard);
            Answer::Open
        }
        Step::Done => done_step(ui, wizard),
    });
    if wizard.step == Step::Progress {
        // The counter moves without any input, so ask for the next frame.
        ctx.request_repaint_after(Duration::from_millis(100));
    }
    match (answer, outcome) {
        (Answer::Open, ModalOutcome::Closed) if wizard.step == Step::Progress => {
            // Closing mid-import stops it after the item in hand; the dialog
            // stays to say what landed.
            wizard.cancel();
            Answer::Open
        }
        (Answer::Open, ModalOutcome::Closed) => Answer::Closed,
        (answer, _) => answer,
    }
}

fn source_step(ui: &mut egui::Ui, wizard: &mut Wizard) -> Answer {
    let width = ui.available_width();
    let tile_w = ((width - 2.0 * GAP) / 3.0).floor();
    for row in ImportSource::ALL.chunks(3) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            for source in row {
                if tile(ui, *source, tile_w) {
                    wizard.source = Some(*source);
                    wizard.error = None;
                    wizard.step = Step::File;
                }
            }
        });
        ui.add_space(GAP - ui.spacing().item_spacing.y);
    }
    Answer::Open
}

/// One source's tile; true when clicked.
fn tile(ui: &mut egui::Ui, source: ImportSource, width: f32) -> bool {
    let (slot, response) = ui.allocate_exact_size(Vec2::new(width, TILE_HEIGHT), Sense::click());
    let _ = ui.interact(slot, tile_id(source), Sense::hover());
    let hovered = response.hovered();
    let rect = if hovered { slot.translate(Vec2::new(0.0, -3.0)) } else { slot };
    if hovered {
        let lift = egui::Shadow { offset: [0, 10], blur: 22, spread: 0, color: Color32::from_rgba_unmultiplied(27, 63, 160, 31) };
        ui.painter().add(lift.as_shape(rect, CornerRadius::same(16)));
    }
    let edge = if hovered { theme::BLUE } else { EDGE };
    ui.painter().rect(rect, CornerRadius::same(16), theme::CARD, Stroke::new(2.0, edge), StrokeKind::Inside);
    let badge = Rect::from_center_size(egui::pos2(rect.center().x, rect.top() + 14.0 + 21.0), Vec2::splat(42.0));
    ui.painter().rect_filled(badge, CornerRadius::same(12), TILE_BG);
    ui.painter().text(
        badge.center(),
        egui::Align2::CENTER_CENTER,
        source.initials(),
        egui::FontId::new(13.0, egui::FontFamily::Name(theme::EXTRABOLD.into())),
        theme::INK,
    );
    ui.painter().text(
        egui::pos2(rect.center().x, badge.bottom() + 10.0 + 9.0),
        egui::Align2::CENTER_CENTER,
        source.label(),
        egui::FontId::new(15.0, egui::FontFamily::Name(theme::BOLD.into())),
        theme::INK,
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

fn file_step(ui: &mut egui::Ui, wizard: &mut Wizard) -> Answer {
    let mut answer = Answer::Open;
    let (slot, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ZONE_HEIGHT), Sense::click());
    let _ = ui.interact(slot, zone_id(), Sense::hover());
    let hovered = response.hovered();
    let dragging_over = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
    let lit = hovered || dragging_over;
    let (border, fill) = if lit {
        (theme::BLUE, Color32::from_rgb(0xee, 0xf2, 0xfc))
    } else {
        (Color32::from_rgb(0xb8, 0xc7, 0xea), Color32::from_rgb(0xf5, 0xf7, 0xfd))
    };
    ui.painter().rect_filled(slot, CornerRadius::same(20), fill);
    let outline = rounded_outline(slot.shrink(1.0), 20.0);
    ui.painter().extend(Shape::dashed_line(&outline, Stroke::new(2.0, border), 9.0, 6.0));
    let tile = Rect::from_center_size(egui::pos2(slot.center().x, slot.top() + 62.0), Vec2::splat(58.0));
    let shadow = egui::Shadow { offset: [0, 6], blur: 16, spread: 0, color: Color32::from_rgba_unmultiplied(27, 63, 160, 36) };
    ui.painter().add(shadow.as_shape(tile, CornerRadius::same(17)));
    ui.painter().rect_filled(tile, CornerRadius::same(17), theme::CARD);
    icon::stroke(ui.painter(), Rect::from_center_size(tile.center(), Vec2::splat(26.0)), UPLOAD, 2.2, theme::BLUE);
    ui.painter().text(
        egui::pos2(slot.center().x, tile.bottom() + 28.0),
        egui::Align2::CENTER_CENTER,
        DROP_HEADLINE,
        egui::FontId::new(18.0, egui::FontFamily::Name(theme::EXTRABOLD.into())),
        NAVY,
    );
    ui.painter().text(
        egui::pos2(slot.center().x, tile.bottom() + 54.0),
        egui::Align2::CENTER_CENTER,
        DROP_SUB,
        egui::FontId::proportional(15.0),
        Color32::from_rgb(0x4a, 0x47, 0x45),
    );
    if response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        answer = Answer::PickFile;
    }
    if let Some(path) = ui.ctx().input(|i| i.raw.dropped_files.iter().find_map(|f| f.path.clone())) {
        answer = Answer::File(path);
    }
    if let Some(error) = wizard.error {
        ui.add_space(12.0);
        ui.label(egui::RichText::new(error).size(14.0).color(theme::ERROR));
    }
    ui.add_space(16.0);
    ui.horizontal(|ui| {
        let back = controls::button(ui, ButtonKind::Outline, button_size(48.0, 20.0), BACK, None);
        let _ = ui.interact(back.rect, back_id(), Sense::hover());
        if back.clicked() {
            wizard.step = Step::ImportSource;
            wizard.error = None;
        }
        ui.add_space(8.0);
        ui.add(egui::Label::new(egui::RichText::new(DELETE_NOTE).size(14.0).color(NOTE_INK)).wrap());
    });
    answer
}

/// A rounded rectangle's outline as a polyline, for the dashed border.
fn rounded_outline(rect: Rect, radius: f32) -> Vec<Pos2> {
    let mut points = Vec::new();
    let corners = [
        (egui::pos2(rect.right() - radius, rect.top() + radius), -90.0_f32),
        (egui::pos2(rect.right() - radius, rect.bottom() - radius), 0.0),
        (egui::pos2(rect.left() + radius, rect.bottom() - radius), 90.0),
        (egui::pos2(rect.left() + radius, rect.top() + radius), 180.0),
    ];
    for (centre, start) in corners {
        for step in 0..=8 {
            let angle = (start + step as f32 * 90.0 / 8.0).to_radians();
            points.push(centre + Vec2::new(angle.cos(), angle.sin()) * radius);
        }
    }
    points.push(points[0]);
    points
}

fn progress_step(ui: &mut egui::Ui, wizard: &Wizard) {
    let (done, total) = wizard.counted();
    ui.vertical_centered(|ui| {
        ui.add_space(30.0);
        ui.label(
            egui::RichText::new(done.to_string())
                .font(egui::FontId::new(56.0, egui::FontFamily::Name(theme::EXTRABOLD.into())))
                .color(NAVY),
        );
        ui.add_space(16.0);
        ui.label(egui::RichText::new(COUNT_LABEL).size(16.0).color(MUTED));
        ui.add_space(16.0);
        let (bar, _) = ui.allocate_exact_size(Vec2::new(320.0, 5.0), Sense::hover());
        ui.painter().rect_filled(bar, CornerRadius::same(3), BAR);
        let fraction = if total == 0 { 0.0 } else { (done as f32 / total as f32).clamp(0.0, 1.0) };
        let fill = Rect::from_min_size(bar.min, Vec2::new(bar.width() * fraction, bar.height()));
        ui.painter().rect_filled(fill, CornerRadius::same(3), theme::BLUE);
        ui.add_space(30.0);
    });
}

fn done_step(ui: &mut egui::Ui, wizard: &Wizard) -> Answer {
    let mut answer = Answer::Open;
    let summary = wizard.summary.clone().unwrap_or_default();
    ui.vertical_centered(|ui| {
        ui.add_space(8.0);
        let (ring, _) = ui.allocate_exact_size(Vec2::splat(80.0), Sense::hover());
        ui.painter().circle_filled(ring.center(), 40.0, GREEN_WASH);
        icon::stroke(ui.painter(), Rect::from_center_size(ring.center(), Vec2::splat(40.0)), icon::glyph::CHECK, 2.6, GREEN);
        ui.add_space(14.0);
        ui.label(
            egui::RichText::new(imported_headline(summary.imported))
                .font(egui::FontId::new(26.0, egui::FontFamily::Name(theme::EXTRABOLD.into())))
                .color(theme::INK),
        );
        for (line, bad) in done_lines(&summary) {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(line).size(16.0).color(if bad { theme::ERROR } else { MUTED }));
        }
        ui.add_space(14.0);
    });
    let size = button_size(50.0, 18.0);
    ui.columns(2, |columns| {
        let done = controls::button(&mut columns[0], ButtonKind::Outline, size, DONE, None);
        let _ = columns[0].interact(done.rect, done_id(), Sense::hover());
        if done.clicked() {
            answer = Answer::Closed;
        }
        let see = controls::button(&mut columns[1], ButtonKind::Primary, size, SEE_ITEMS, None);
        let _ = columns[1].interact(see.rect, see_id(), Sense::hover());
        if see.clicked() {
            answer = Answer::SeeItems;
        }
    });
    answer
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testing::{self, click_at, Painted};

    const SCREEN: Vec2 = egui::vec2(1280.0, 900.0);

    fn frame(ctx: &egui::Context, time: f64, events: Vec<egui::Event>, wizard: &mut Wizard) -> (Answer, Painted) {
        let mut answer = Answer::Open;
        let painted = testing::frame(ctx, time, SCREEN, events, |ui| answer = draw(ui.ctx(), wizard));
        (answer, painted)
    }

    fn settle(ctx: &egui::Context, wizard: &mut Wizard, from: f64) -> f64 {
        for i in 0..3 {
            let _ = frame(ctx, from + i as f64 * 0.1, vec![], wizard);
        }
        from + 0.3
    }

    fn has(p: &Painted, text: &str) -> bool {
        p.texts.iter().any(|(t, _, _)| t == text)
    }

    fn click(ctx: &egui::Context, wizard: &mut Wizard, id: egui::Id, time: f64) -> Answer {
        let at = ctx.read_response(id).expect("the control is drawn").rect.center();
        frame(ctx, time, click_at(at), wizard).0
    }

    /// A launcher that runs the import at once against fakes and hands back
    /// its report -- the same `run` the production worker calls.
    fn immediately(
        created: std::rc::Rc<std::cell::RefCell<usize>>,
    ) -> impl FnOnce(Plan, Arc<AtomicUsize>, Arc<AtomicBool>) -> Receiver<Report> {
        move |plan, progress, cancel| {
            let (tx, rx) = mpsc::channel();
            let mut create_folder = |name: &str| {
                Ok(crate::vault_bridge::Folder { id: format!("id-{name}"), name: name.to_string(), other: Default::default() })
            };
            let mut create_item = |_: &crate::vault_bridge::NewItem| {
                *created.borrow_mut() += 1;
                Ok(())
            };
            let report = import_formats::run(
                plan,
                Sinks { existing_folders: vec![], create_folder: &mut create_folder, create_item: &mut create_item },
                &progress,
                &cancel,
            );
            tx.send(report).unwrap();
            rx
        }
    }

    const CHROME: &str = "name,url,username,password\nA,https://a.test,u,p\nB,https://b.test,u,p\n";

    #[test]
    fn the_source_step_has_six_tiles_in_two_rows_of_three() {
        let ctx = testing::ctx();
        let mut wizard = Wizard::default();
        let t = settle(&ctx, &mut wizard, 0.0);
        let (_, p) = frame(&ctx, t, vec![], &mut wizard);
        assert!(has(&p, SOURCE_TITLE) && has(&p, SOURCE_SUB));
        for source in ImportSource::ALL {
            assert!(has(&p, source.label()) && has(&p, source.initials()), "no tile for {source:?}");
        }
        let rect = |s: ImportSource| ctx.read_response(tile_id(s)).expect("tile").rect;
        assert!(rect(ImportSource::Bitwarden).left() < rect(ImportSource::OnePassword).left());
        assert!((rect(ImportSource::Bitwarden).top() - rect(ImportSource::Keeper).top()).abs() < 1.0);
        assert!(rect(ImportSource::LastPass).top() > rect(ImportSource::Bitwarden).bottom());
        assert_eq!(rect(ImportSource::Bitwarden).height(), TILE_HEIGHT);
    }

    #[test]
    fn the_card_is_620_wide_on_a_wide_window() {
        let ctx = testing::ctx();
        let mut wizard = Wizard::default();
        let _ = settle(&ctx, &mut wizard, 0.0);
        let card = ctx.read_response(crate::kit::modal::card_id(ID)).expect("the card").rect;
        assert!((card.width() - WIDTH).abs() < 1.0, "{}", card.width());
    }

    #[test]
    fn a_tile_leads_to_its_file_step_and_back_returns() {
        let ctx = testing::ctx();
        let mut wizard = Wizard::default();
        let t = settle(&ctx, &mut wizard, 0.0);
        let _ = click(&ctx, &mut wizard, tile_id(ImportSource::Browser), t);
        assert_eq!(wizard.step, Step::File);
        assert_eq!(wizard.source, Some(ImportSource::Browser));
        let t = settle(&ctx, &mut wizard, t + 0.1);
        let (_, p) = frame(&ctx, t, vec![], &mut wizard);
        assert!(has(&p, "From Chrome or Edge"));
        assert!(has(&p, "Export a file from Chrome or Edge, then add it here."));
        assert!(has(&p, DROP_HEADLINE) && has(&p, DROP_SUB));
        let _ = click(&ctx, &mut wizard, back_id(), t + 0.1);
        assert_eq!(wizard.step, Step::ImportSource);
    }

    #[test]
    fn clicking_the_zone_asks_for_the_file_dialog() {
        let ctx = testing::ctx();
        let mut wizard = Wizard { step: Step::File, source: Some(ImportSource::Keeper), ..Wizard::default() };
        let t = settle(&ctx, &mut wizard, 0.0);
        assert_eq!(click(&ctx, &mut wizard, zone_id(), t), Answer::PickFile);
    }

    #[test]
    fn a_dropped_file_is_taken_from_the_zone() {
        let ctx = testing::ctx();
        let mut wizard = Wizard { step: Step::File, source: Some(ImportSource::Keeper), ..Wizard::default() };
        let t = settle(&ctx, &mut wizard, 0.0);
        let mut answer = Answer::Open;
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
                time: Some(t),
                dropped_files: vec![egui::DroppedFile { path: Some("C:/x/export.csv".into()), ..Default::default() }],
                ..Default::default()
            },
            |ui| answer = draw(ui.ctx(), &mut wizard),
        );
        assert_eq!(answer, Answer::File("C:/x/export.csv".into()));
    }

    #[test]
    fn a_file_that_is_not_the_format_stays_on_the_file_step_with_the_reason() {
        let mut wizard = Wizard { step: Step::File, source: Some(ImportSource::LastPass), ..Wizard::default() };
        let created = Default::default();
        wizard.begin_from_bytes(CHROME.as_bytes(), &[], immediately(created));
        assert_eq!(wizard.step, Step::File);
        assert_eq!(wizard.error, Some(ParseError::NotThisFormat.sentence()));
        let ctx = testing::ctx();
        let t = settle(&ctx, &mut wizard, 0.0);
        let (_, p) = frame(&ctx, t, vec![], &mut wizard);
        assert!(has(&p, ParseError::NotThisFormat.sentence()));
    }

    #[test]
    fn a_good_file_imports_and_the_done_step_counts_it_and_the_duplicates() {
        let mut wizard = Wizard { step: Step::File, source: Some(ImportSource::Browser), ..Wizard::default() };
        let created: std::rc::Rc<std::cell::RefCell<usize>> = Default::default();
        let mut existing = import_formats::parse(ImportSource::Browser, b"name,url,username,password\nA,https://a.test,u,p\n")
            .unwrap()
            .items
            .remove(0)
            .item;
        existing.id = "have".into();
        wizard.begin_from_bytes(CHROME.as_bytes(), &[existing], immediately(created.clone()));
        assert_eq!(wizard.step, Step::Progress);
        assert!(wizard.is_running());
        assert_eq!(wizard.counted(), (1, 1));
        let finished = wizard.poll().expect("the worker has answered");
        assert!(finished.first_error.is_none());
        assert!(wizard.poll().is_none(), "a finish is answered once");
        assert_eq!(*created.borrow(), 1, "the duplicate was not created");
        assert_eq!(wizard.step, Step::Done);

        let ctx = testing::ctx();
        let t = settle(&ctx, &mut wizard, 0.0);
        let (_, p) = frame(&ctx, t, vec![], &mut wizard);
        assert!(has(&p, DONE_TITLE));
        assert!(has(&p, "1 item imported"));
        assert!(has(&p, "1 duplicate was skipped"));
        assert_eq!(click(&ctx, &mut wizard, see_id(), t + 0.1), Answer::SeeItems);
        assert_eq!(click(&ctx, &mut wizard, done_id(), t + 0.2), Answer::Closed);
    }

    #[test]
    fn a_file_of_nothing_new_goes_straight_to_done() {
        let mut wizard = Wizard { step: Step::File, source: Some(ImportSource::Browser), ..Wizard::default() };
        let existing: Vec<VaultItem> = import_formats::parse(ImportSource::Browser, CHROME.as_bytes())
            .unwrap()
            .items
            .into_iter()
            .map(|e| e.item)
            .collect();
        wizard.begin_from_bytes(CHROME.as_bytes(), &existing, immediately(Default::default()));
        assert_eq!(wizard.step, Step::Done);
        assert_eq!(wizard.summary.as_ref().map(|s| (s.imported, s.duplicates)), Some((0, 2)));
    }

    #[test]
    fn the_progress_step_shows_the_counter_and_closing_it_cancels_without_leaving() {
        let (_tx, rx) = mpsc::channel();
        let progress = Arc::new(AtomicUsize::new(3));
        let cancel = Arc::new(AtomicBool::new(false));
        let mut wizard = Wizard {
            step: Step::Progress,
            source: Some(ImportSource::Browser),
            job: Some(Job { progress, cancel: cancel.clone(), total: 12, rx }),
            ..Wizard::default()
        };
        let ctx = testing::ctx();
        let t = settle(&ctx, &mut wizard, 0.0);
        let (_, p) = frame(&ctx, t, vec![], &mut wizard);
        assert!(has(&p, PROGRESS_TITLE) && has(&p, PROGRESS_SUB));
        assert!(has(&p, "3") && has(&p, COUNT_LABEL));
        assert!(p.repaint_delay < Duration::from_secs(1), "the counter keeps moving");
        let mark = ctx.read_response(crate::kit::modal::close_id(ID)).expect("the close mark").rect.center();
        let (answer, _) = frame(&ctx, t + 0.1, click_at(mark), &mut wizard);
        assert_eq!(answer, Answer::Open, "the dialog stays to say what landed");
        assert!(cancel.load(Ordering::Relaxed), "and the worker is asked to stop");
    }

    #[test]
    fn the_done_lines_say_what_did_not_land() {
        let lines = done_lines(&Summary { imported: 5, duplicates: 3, failed: 2, left_out: 1, stopped_early: true });
        let texts: Vec<&str> = lines.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(texts, ["3 duplicates were skipped", "1 entry was left out", "2 items couldn't be imported"]);
        assert!(!lines[0].1 && lines[2].1, "only the failure is painted as one");
        assert_eq!(imported_headline(128), "128 items imported");
        assert!(done_lines(&Summary { imported: 1, ..Summary::default() }).is_empty());
    }
}
