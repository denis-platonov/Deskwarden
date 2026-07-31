//! The vault window's detail pane in edit mode, and the "+ New" creation
//! flow (they share one form: creating is editing against an empty draft
//! with no item id yet). See `detail.rs` for why this is a separate file
//! from read mode.

use crate::theme;
use crate::vault_bridge::{Folder, NewLoginItem, VaultItem};
#[cfg(test)]
use crate::vault_bridge::{LoginData, UriEntry};
use eframe::egui::{self, CornerRadius, Margin, RichText, Stroke};

#[derive(Debug, Clone, Default)]
pub struct EditDraft {
    pub name: String,
    pub username: String,
    pub password: String,
    pub folder_id: Option<String>,
    /// Persistent toggle state for the password field's "Show"/"Hide"
    /// control -- see `theme::password_field`'s doc comment. Must live on
    /// the draft (not a local in `draw_detail_edit`) so it survives across
    /// egui frames, matching `login_ui::LoginForm::reveal_password`.
    pub reveal_password: bool,
}

impl EditDraft {
    pub fn from_item(item: &VaultItem) -> Self {
        let login = item.login.as_ref();
        Self {
            name: item.name.clone(),
            username: login.and_then(|l| l.username.clone()).unwrap_or_default(),
            password: login
                .and_then(|l| l.password.as_deref())
                .map(|p| p.to_owned())
                .unwrap_or_default(),
            folder_id: item.folder_id.clone(),
            reveal_password: false,
        }
    }

    pub fn empty() -> Self {
        Self::default()
    }

    /// A name is the one thing `bw serve`'s create/edit endpoints reject an
    /// empty item without -- everything else (blank username/password) is
    /// legitimate for e.g. a placeholder entry.
    pub fn is_valid(&self) -> bool {
        !self.name.trim().is_empty()
    }

    /// Applies this draft onto a clone of `item`, preserving every field the
    /// draft doesn't know about (id, favorite, type, fields, TOTP, uris, ...)
    /// via the same "clone then overwrite known fields" pattern
    /// `vault_bridge::with_app_match` already uses, so a save from this form
    /// can never silently drop data the design doesn't expose an editor for.
    pub fn apply_to(&self, item: &VaultItem) -> VaultItem {
        let mut updated = item.clone();
        updated.name = self.name.clone();
        updated.folder_id = self.folder_id.clone();
        let mut login = updated.login.unwrap_or_default();
        login.username = if self.username.is_empty() { None } else { Some(self.username.clone()) };
        login.password = if self.password.is_empty() {
            None
        } else {
            Some(self.password.clone().into())
        };
        updated.login = Some(login);
        updated
    }

    pub fn to_new_item(&self) -> NewLoginItem {
        NewLoginItem {
            name: self.name.clone(),
            username: self.username.clone(),
            password: self.password.clone(),
            folder_id: self.folder_id.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditAction {
    None,
    Save,
    Cancel,
}

pub fn draw_detail_edit(
    ui: &mut egui::Ui,
    draft: &mut EditDraft,
    folders: &[Folder],
    creating: bool,
) -> EditAction {
    let mut action = EditAction::None;

    ui.label(theme::bold(if creating { "New login" } else { "Edit login" }, 19.0).color(theme::INK));
    ui.add_space(12.0);

    egui::Frame::new()
        .fill(theme::CARD)
        .corner_radius(CornerRadius::same(10))
        .stroke(Stroke::new(1.0, theme::HAIRLINE))
        .inner_margin(Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());

            theme::field_label(ui, "Name");
            theme::text_field(ui, &mut draft.name, false);
            ui.add_space(10.0);

            theme::field_label(ui, "Username");
            theme::text_field(ui, &mut draft.username, false);
            ui.add_space(10.0);

            theme::field_label(ui, "Password");
            theme::password_field(ui, &mut draft.password, &mut draft.reveal_password);
            ui.add_space(10.0);

            theme::field_label(ui, "Folder");
            egui::ComboBox::from_id_salt("edit-folder")
                .selected_text(
                    folders
                        .iter()
                        .find(|f| Some(&f.id) == draft.folder_id.as_ref())
                        .map(|f| f.name.as_str())
                        .unwrap_or("No folder"),
                )
                .show_ui(ui, |ui| {
                    if ui.selectable_label(draft.folder_id.is_none(), "No folder").clicked() {
                        draft.folder_id = None;
                    }
                    for folder in folders {
                        let selected = draft.folder_id.as_deref() == Some(folder.id.as_str());
                        if ui.selectable_label(selected, &folder.name).clicked() {
                            draft.folder_id = Some(folder.id.clone());
                        }
                    }
                });
        });

    if !draft.is_valid() {
        ui.add_space(6.0);
        ui.label(RichText::new("Name is required.").size(12.0).color(theme::ERROR));
    }

    ui.add_space(12.0);
    ui.horizontal(|ui| {
        let save = egui::Button::new(if draft.is_valid() { "Save" } else { "Save (needs a name)" });
        if ui.add_enabled(draft.is_valid(), save).clicked() {
            action = EditAction::Save;
        }
        if theme::secondary_button(ui, "Cancel").clicked() {
            action = EditAction::Cancel;
        }
    });

    action
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item() -> VaultItem {
        let mut login_other = serde_json::Map::new();
        login_other.insert("fido2Credentials".into(), serde_json::json!(["cred-1"]));

        let mut item_other = serde_json::Map::new();
        item_other.insert("collectionIds".into(), serde_json::json!(["c1"]));

        VaultItem {
            id: "1".into(),
            name: "Ledgerline".into(),
            fields: vec![],
            login: Some(LoginData {
                username: Some("a@b.com".into()),
                password: Some("p".to_string().into()),
                totp: Some("SEED".into()),
                uris: vec![UriEntry { uri: Some("https://ledgerline.example".into()), other: serde_json::Map::new() }],
                other: login_other,
            }),
            item_type: Some(1),
            folder_id: Some("f1".into()),
            favorite: true,
            other: item_other,
        }
    }

    #[test]
    fn from_item_pulls_the_current_credentials() {
        let draft = EditDraft::from_item(&item());
        assert_eq!(draft.name, "Ledgerline");
        assert_eq!(draft.username, "a@b.com");
        assert_eq!(draft.folder_id.as_deref(), Some("f1"));
    }

    #[test]
    fn from_item_starts_with_the_password_masked() {
        // `reveal_password` must be the persistent toggle state
        // `theme::password_field` expects (see its doc comment), and a
        // freshly-opened edit form should start masked, matching
        // `login_ui::LoginForm`'s own default.
        let draft = EditDraft::from_item(&item());
        assert!(!draft.reveal_password);
    }

    #[test]
    fn empty_draft_is_invalid() {
        assert!(!EditDraft::empty().is_valid());
    }

    #[test]
    fn a_named_draft_is_valid() {
        let draft = EditDraft { name: "New".into(), ..EditDraft::empty() };
        assert!(draft.is_valid());
    }

    #[test]
    fn apply_to_preserves_fields_the_form_never_touched() {
        let draft = EditDraft {
            name: "Renamed".into(),
            username: "new@b.com".into(),
            password: "np".into(),
            folder_id: None,
            ..EditDraft::empty()
        };
        let updated = draft.apply_to(&item());

        assert_eq!(updated.name, "Renamed");
        assert_eq!(updated.folder_id, None);
        assert!(updated.favorite, "favorite must survive an edit the form doesn't expose");
        assert_eq!(updated.item_type, Some(1));
        let login = updated.login.as_ref().unwrap();
        assert_eq!(login.totp.as_deref(), Some("SEED"));
        assert_eq!(
            login.uris.len(),
            1,
            "login.uris must survive an edit the form doesn't expose a URI editor for"
        );
        assert_eq!(login.uris[0].uri.as_deref(), Some("https://ledgerline.example"));
        assert_eq!(
            login.other.get("fido2Credentials"),
            Some(&serde_json::json!(["cred-1"])),
            "login's flattened extras must survive a save"
        );
        assert_eq!(
            updated.other.get("collectionIds"),
            Some(&serde_json::json!(["c1"])),
            "the item's flattened extras must survive a save"
        );
    }

    #[test]
    fn to_new_item_carries_the_drafts_fields() {
        let draft = EditDraft {
            name: "New".into(),
            username: "u".into(),
            password: "p".into(),
            folder_id: Some("f2".into()),
            ..EditDraft::empty()
        };
        let new_item = draft.to_new_item();
        assert_eq!(new_item.name, "New");
        assert_eq!(new_item.folder_id.as_deref(), Some("f2"));
    }
}
