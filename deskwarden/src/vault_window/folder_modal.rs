//! The vault window's "Edit folder" modal (rename + delete), opened from a
//! sidebar folder row's edit-pencil icon. A dedicated small file rather than
//! folded into `sidebar.rs` or `mod.rs`: it owns its own state and drawing,
//! and neither of those files needs to know how it's built internally --
//! `vault_window::mod` only needs [`FolderEditState`] and
//! [`draw_folder_edit_modal`]'s returned [`FolderEditAction`].

use crate::theme;
use eframe::egui::{self, CornerRadius, Margin, RichText, Stroke};

/// The modal's own per-open state: which folder is being edited, the name
/// draft (seeded from the folder's current name, edited in place), and a
/// two-click delete confirm. The delete confirm is a simple bool rather than
/// `vault_window::mod`'s timed `confirm_click` pattern -- the modal already
/// requires a deliberate "open this folder's editor" step before Delete is
/// even reachable, so the fast-double-click concern that motivated the timed
/// version elsewhere doesn't apply here to the same degree, but a bare
/// single click still shouldn't delete outright.
pub struct FolderEditState {
    pub folder_id: String,
    pub name: String,
    delete_armed: bool,
    /// Set by the caller when a save or delete comes back an error.
    ///
    /// Without somewhere to put it, a failed action logged a warning and
    /// left the modal sitting open exactly as it was -- indistinguishable,
    /// from the outside, from the click not having registered at all.
    pub error: Option<String>,
}

impl FolderEditState {
    pub fn new(folder_id: String, name: String) -> Self {
        Self {
            folder_id,
            name,
            delete_armed: false,
            error: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderEditAction {
    None,
    Save,
    Delete,
    Cancel,
}

/// Draws the modal as a centered card over a dimmed scrim covering the whole
/// window. Pure view plus the tiny bit of state (`delete_armed`) the confirm
/// button needs -- the caller performs the actual rename/delete against
/// `VaultBridge` and closes the modal in response to the returned action.
pub fn draw_folder_edit_modal(ctx: &egui::Context, state: &mut FolderEditState) -> FolderEditAction {
    let mut action = FolderEditAction::None;

    // Dimmed scrim: a full-window click-catcher (so a click outside the
    // card can't reach whatever is behind it) painted at a low alpha, on the
    // `Foreground` layer so it sits above the sidebar/list/detail panels
    // regardless of draw order.
    egui::Area::new(egui::Id::new("folder-edit-scrim"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::Pos2::ZERO)
        .show(ctx, |ui| {
            let screen = ctx.content_rect();
            ui.allocate_response(screen.size(), egui::Sense::click());
            ui.painter()
                .rect_filled(screen, CornerRadius::ZERO, egui::Color32::from_black_alpha(90));
        });

    egui::Area::new(egui::Id::new("folder-edit-modal"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(theme::CARD)
                .corner_radius(CornerRadius::same(10))
                .stroke(Stroke::new(1.0, theme::BORDER))
                .inner_margin(Margin::same(20))
                .show(ui, |ui| {
                    ui.set_width(320.0);
                    ui.label(theme::bold("Edit folder", 15.0).color(theme::INK));
                    ui.add_space(14.0);
                    theme::field_label(ui, "Folder name");
                    ui.add_space(6.0);
                    theme::text_field(ui, &mut state.name, false);
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new("Use \"/\" to nest, e.g. Work/Clients")
                            .size(11.0)
                            .color(theme::TEXT_GHOST),
                    );
                    if let Some(error) = &state.error {
                        ui.add_space(8.0);
                        ui.label(RichText::new(error).size(12.0).color(theme::ERROR));
                    }
                    ui.add_space(18.0);
                    ui.horizontal(|ui| {
                        let delete_label = if state.delete_armed { "Confirm delete" } else { "Delete" };
                        let delete_clicked = ui
                            .add(
                                egui::Button::new(RichText::new(delete_label).size(12.0).color(theme::ERROR))
                                    .fill(theme::CARD)
                                    .stroke(Stroke::new(1.0, theme::ERROR))
                                    .corner_radius(CornerRadius::same(7))
                                    .min_size(egui::vec2(0.0, 32.0)),
                            )
                            .clicked();
                        if delete_clicked {
                            if state.delete_armed {
                                action = FolderEditAction::Delete;
                            } else {
                                state.delete_armed = true;
                            }
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if theme::primary_button(ui, "Save", None).clicked() {
                                action = FolderEditAction::Save;
                            }
                            ui.add_space(8.0);
                            if theme::secondary_button(ui, "Cancel").clicked() {
                                action = FolderEditAction::Cancel;
                            }
                        });
                    });
                });
        });

    // Esc cancels, same as every other transient overlay in this app.
    if action == FolderEditAction::None && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        action = FolderEditAction::Cancel;
    }

    action
}
