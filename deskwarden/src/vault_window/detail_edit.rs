//! The vault window's detail pane in edit mode, and the "+ New" creation
//! flow (they share one form: creating is editing against an empty draft
//! with no item id yet). See `detail.rs` for why this is a separate file
//! from read mode.

use crate::theme;
use crate::vault_bridge::{Folder, ItemKind, NewItem, VaultItem};
#[cfg(test)]
use crate::vault_bridge::{LoginData, UriEntry};
use crate::vault_window::sidebar;
use eframe::egui::{self, CornerRadius, Margin, RichText, Stroke};
use zeroize::Zeroizing;

/// The card-specific half of a draft (`type: 3`).
///
/// Plain `String`s, including the two secrets, exactly as
/// [`EditDraft::password`] has always been: a text field's buffer is a
/// `String` and egui owns copies of it in its galley cache regardless. This
/// deliberately does **not** widen the zeroize guarantee -- see the recorded
/// decision in `deskwarden/README.md`. The model side (`CardData::number`,
/// `CardData::code`) is `Zeroizing` and stays that way.
#[derive(Debug, Clone, Default)]
pub struct CardDraft {
    pub cardholder_name: String,
    pub brand: String,
    pub number: String,
    pub exp_month: String,
    pub exp_year: String,
    pub code: String,
    /// Persistent reveal state for the number, for the same reason
    /// [`EditDraft::reveal_password`] lives on the draft: a local `bool`
    /// inside the frame closure resets every frame, which is a toggle that
    /// visibly does nothing. That bug has already been found and fixed once
    /// in this file; the two card secrets get their own flags rather than
    /// sharing one, so revealing the number cannot reveal the security code.
    pub reveal_number: bool,
    pub reveal_code: bool,
}

/// The identity-specific half of a draft (`type: 4`). One `String` per
/// modelled key of [`crate::vault_bridge::IdentityData`], including
/// `address3` -- see that struct's doc for why it is modelled despite being
/// absent from the captured template.
#[derive(Debug, Clone, Default)]
pub struct IdentityDraft {
    pub title: String,
    pub first_name: String,
    pub middle_name: String,
    pub last_name: String,
    pub address1: String,
    pub address2: String,
    pub address3: String,
    pub city: String,
    pub state: String,
    pub postal_code: String,
    pub country: String,
    pub company: String,
    pub email: String,
    pub phone: String,
    pub ssn: String,
    pub username: String,
    pub passport_number: String,
    pub license_number: String,
}

/// The edit form's state, for **one kind of item**.
///
/// [`Self::kind`] is what makes this safe. Before it existed, `apply_to`
/// unconditionally did `updated.login.unwrap_or_default()`, so saving a card
/// from this form gave it an empty `login` object it never had -- an item
/// carrying two type objects. The kind-specific fields are all present on the
/// one struct (rather than an enum payload) because `EditDraft` is also the
/// *create* form's state, where the kind is chosen after the draft exists,
/// and because `vault_window/mod.rs` holds it in `DetailMode::Edit`/`Create`
/// across frames.
#[derive(Debug, Clone)]
pub struct EditDraft {
    /// Which kind this draft edits. Set from the item by [`Self::from_item`]
    /// and fixed thereafter -- an item's type cannot be changed after
    /// creation. [`Self::apply_to`] writes back **only** this kind's object.
    pub kind: ItemKind,
    pub name: String,
    pub folder_id: Option<String>,
    /// What the item's folder was when the form opened.
    ///
    /// Kept because `bw serve` (CLI 2026.7.0) **cannot un-file an item**:
    /// omitting `folderId`, sending `null` and sending `""` all leave the
    /// folder unchanged, proven against a control field that did change in
    /// the same request (`.superpowers/sdd/put-semantics-capture.md`). So
    /// "No folder" is offered only when it is achievable -- see
    /// [`Self::may_unfile`].
    original_folder_id: Option<String>,
    pub username: String,
    pub password: String,
    /// Persistent toggle state for the password field's "Show"/"Hide"
    /// control -- see `theme::password_field`'s doc comment. Must live on
    /// the draft (not a local in `draw_detail_edit`) so it survives across
    /// egui frames, matching `login_ui::LoginForm::reveal_password`.
    pub reveal_password: bool,
    pub card: CardDraft,
    pub identity: IdentityDraft,
    /// Item-level `notes`, which is where a secure note's entire body lives.
    /// Populated for every kind but written back only for
    /// [`ItemKind::SecureNote`], because that is the only kind this form
    /// offers a notes editor for; on every other kind the item's own notes
    /// ride the clone untouched.
    pub note_body: String,
}

impl Default for EditDraft {
    /// A blank **login** draft. `ItemKind` has no `Default` of its own on
    /// purpose (nothing should be able to guess an item's type), so this is
    /// the one place the create form's default kind is stated.
    fn default() -> Self {
        Self {
            kind: ItemKind::Login,
            name: String::new(),
            folder_id: None,
            original_folder_id: None,
            username: String::new(),
            password: String::new(),
            reveal_password: false,
            card: CardDraft::default(),
            identity: IdentityDraft::default(),
            note_body: String::new(),
        }
    }
}

/// One field's new value, following the form's "blank means absent"
/// convention **without** collapsing absent and empty.
///
/// A non-empty draft is the new value. A blank draft normally means the key
/// is absent -- the convention `create_item` and the login path already
/// follow -- *except* when the item already held an explicit empty string, in
/// which case blank means "unchanged" and the empty string is kept. Empty is
/// not absent: collapsing the two rewrites an item's shape on a save that
/// changed nothing, which is the exact class of silent drift the round-trip
/// tests in `vault_bridge.rs` exist to catch.
fn edited(current: Option<&str>, draft: &str) -> Option<String> {
    if draft.is_empty() {
        current.filter(|c| c.is_empty()).map(str::to_string)
    } else {
        Some(draft.to_string())
    }
}

/// [`edited`] for a secret, re-wrapped in `Zeroizing` so the model side keeps
/// the wipe-on-drop guarantee the draft's plain `String` does not have.
fn edited_secret(current: Option<&str>, draft: &str) -> Option<Zeroizing<String>> {
    edited(current, draft).map(Zeroizing::new)
}

/// The other direction: a modelled `Option` becomes the draft's `String`.
fn drafted(current: Option<&str>) -> String {
    current.unwrap_or_default().to_string()
}

impl EditDraft {
    pub fn from_item(item: &VaultItem) -> Self {
        let login = item.login.as_ref();
        let card = item.card.as_ref();
        let identity = item.identity.as_ref();
        // Every kind's fields are populated from whatever objects the item
        // actually has, rather than only the matching one: a login simply has
        // no `card` object, so its `CardDraft` comes out blank, and
        // `apply_to` gates on `kind` anyway. One branch fewer to get wrong.
        Self {
            kind: ItemKind::of(item),
            name: item.name.clone(),
            folder_id: item.folder_id.clone(),
            original_folder_id: item.folder_id.clone(),
            username: drafted(login.and_then(|l| l.username.as_deref())),
            password: drafted(login.and_then(|l| l.password.as_deref()).map(|p| p.as_str())),
            reveal_password: false,
            card: CardDraft {
                cardholder_name: drafted(card.and_then(|c| c.cardholder_name.as_deref())),
                brand: drafted(card.and_then(|c| c.brand.as_deref())),
                number: drafted(card.and_then(|c| c.number.as_deref()).map(|n| n.as_str())),
                exp_month: drafted(card.and_then(|c| c.exp_month.as_deref())),
                exp_year: drafted(card.and_then(|c| c.exp_year.as_deref())),
                code: drafted(card.and_then(|c| c.code.as_deref()).map(|c| c.as_str())),
                reveal_number: false,
                reveal_code: false,
            },
            identity: IdentityDraft {
                title: drafted(identity.and_then(|i| i.title.as_deref())),
                first_name: drafted(identity.and_then(|i| i.first_name.as_deref())),
                middle_name: drafted(identity.and_then(|i| i.middle_name.as_deref())),
                last_name: drafted(identity.and_then(|i| i.last_name.as_deref())),
                address1: drafted(identity.and_then(|i| i.address1.as_deref())),
                address2: drafted(identity.and_then(|i| i.address2.as_deref())),
                address3: drafted(identity.and_then(|i| i.address3.as_deref())),
                city: drafted(identity.and_then(|i| i.city.as_deref())),
                state: drafted(identity.and_then(|i| i.state.as_deref())),
                postal_code: drafted(identity.and_then(|i| i.postal_code.as_deref())),
                country: drafted(identity.and_then(|i| i.country.as_deref())),
                company: drafted(identity.and_then(|i| i.company.as_deref())),
                email: drafted(identity.and_then(|i| i.email.as_deref())),
                phone: drafted(identity.and_then(|i| i.phone.as_deref())),
                ssn: drafted(identity.and_then(|i| i.ssn.as_deref())),
                username: drafted(identity.and_then(|i| i.username.as_deref())),
                passport_number: drafted(identity.and_then(|i| i.passport_number.as_deref())),
                license_number: drafted(identity.and_then(|i| i.license_number.as_deref())),
            },
            note_body: drafted(item.notes.as_deref().map(|n| n.as_str())),
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

    /// Whether the form may offer "No folder".
    ///
    /// **Only when the item is already unfiled**, in which case the option is
    /// a no-op and is honest. An item that *has* a folder cannot be un-filed
    /// through this backend at all: `.superpowers/sdd/put-semantics-capture.md`
    /// records a controlled experiment against the user's own `bw serve` (CLI
    /// 2026.7.0) in which omitting `folderId`, sending `null`, sending `""`
    /// and sending a fully round-tripped object all left the folder unchanged,
    /// while a name change in the very same PUT applied. The write succeeds
    /// and does nothing.
    ///
    /// Offering the option anyway is what this form did before: the pane
    /// showed the item unfiled, the server ignored it, and the next sync
    /// reverted it -- a silent lie. Withholding it makes the limitation
    /// visible instead. This is not a fix for the backend limitation; nothing
    /// in this crate can fix it. Re-test after a CLI upgrade: Bitwarden's
    /// current `main` clears the folder on an explicit null.
    ///
    /// Moving *between* folders, and *into* one from unfiled, both work and
    /// are unaffected.
    pub fn may_unfile(&self) -> bool {
        self.original_folder_id.is_none()
    }

    /// Applies this draft onto a clone of `item`, preserving every field the
    /// draft doesn't know about (id, favorite, type, fields, TOTP, uris,
    /// `sshKey`, and everything riding the `#[serde(flatten)] other`
    /// catch-alls) via the same "clone then overwrite known fields" pattern
    /// `vault_bridge::with_app_match` already uses, so a save from this form
    /// can never silently drop data the design doesn't expose an editor for.
    ///
    /// Only **this draft's kind's** object is written. Writing more than one
    /// is how an item ends up carrying two type objects, which is the bug
    /// this method was carrying before it became kind-aware.
    pub fn apply_to(&self, item: &VaultItem) -> VaultItem {
        let mut updated = item.clone();
        updated.name = self.name.clone();
        // The un-file that cannot happen (see `may_unfile`) is refused here
        // too, not only in the widget: the caller writes this value straight
        // into its in-memory list, so honouring a clear the server will
        // ignore would leave the pane disagreeing with the vault until the
        // next sync. Every other transition is the draft's to make.
        updated.folder_id = match (&self.folder_id, &self.original_folder_id) {
            (None, Some(original)) => Some(original.clone()),
            (chosen, _) => chosen.clone(),
        };

        // The item is the authority on its own type, not the draft: if the
        // two ever disagree (a draft carried across a selection change, say)
        // writing this draft's kind's object onto that item would create the
        // very two-type-object item this method exists to prevent. Name and
        // folder are kind-agnostic and have already been applied.
        if self.kind != ItemKind::of(item) {
            return updated;
        }

        match self.kind {
            ItemKind::Login => {
                let mut login = updated.login.take().unwrap_or_default();
                login.username = edited(login.username.as_deref(), &self.username);
                login.password =
                    edited_secret(login.password.as_deref().map(|p| p.as_str()), &self.password);
                updated.login = Some(login);
            }
            ItemKind::Card => {
                let d = &self.card;
                let mut card = updated.card.take().unwrap_or_default();
                card.cardholder_name = edited(card.cardholder_name.as_deref(), &d.cardholder_name);
                card.brand = edited(card.brand.as_deref(), &d.brand);
                card.number =
                    edited_secret(card.number.as_deref().map(|n| n.as_str()), &d.number);
                card.exp_month = edited(card.exp_month.as_deref(), &d.exp_month);
                card.exp_year = edited(card.exp_year.as_deref(), &d.exp_year);
                card.code = edited_secret(card.code.as_deref().map(|c| c.as_str()), &d.code);
                updated.card = Some(card);
            }
            ItemKind::Identity => {
                let d = &self.identity;
                let mut identity = updated.identity.take().unwrap_or_default();
                identity.title = edited(identity.title.as_deref(), &d.title);
                identity.first_name = edited(identity.first_name.as_deref(), &d.first_name);
                identity.middle_name = edited(identity.middle_name.as_deref(), &d.middle_name);
                identity.last_name = edited(identity.last_name.as_deref(), &d.last_name);
                identity.address1 = edited(identity.address1.as_deref(), &d.address1);
                identity.address2 = edited(identity.address2.as_deref(), &d.address2);
                identity.address3 = edited(identity.address3.as_deref(), &d.address3);
                identity.city = edited(identity.city.as_deref(), &d.city);
                identity.state = edited(identity.state.as_deref(), &d.state);
                identity.postal_code = edited(identity.postal_code.as_deref(), &d.postal_code);
                identity.country = edited(identity.country.as_deref(), &d.country);
                identity.company = edited(identity.company.as_deref(), &d.company);
                identity.email = edited(identity.email.as_deref(), &d.email);
                identity.phone = edited(identity.phone.as_deref(), &d.phone);
                identity.ssn = edited(identity.ssn.as_deref(), &d.ssn);
                identity.username = edited(identity.username.as_deref(), &d.username);
                identity.passport_number =
                    edited(identity.passport_number.as_deref(), &d.passport_number);
                identity.license_number =
                    edited(identity.license_number.as_deref(), &d.license_number);
                updated.identity = Some(identity);
            }
            ItemKind::SecureNote => {
                // A secure note has no object of its own to write: its
                // `secureNote` key is a `{"type": 0}` discriminator that
                // rides `VaultItem::other`, and the body is item-level
                // `notes`.
                updated.notes = edited_secret(
                    updated.notes.as_deref().map(|n| n.as_str()),
                    &self.note_body,
                );
            }
            // Edits nothing but the name and folder already applied above.
            //
            // SSH keys: this build has no `SshKeyData` (`type: 5`'s wire
            // shape is being modelled separately), so the whole `sshKey`
            // object rides `VaultItem::other` and the clone preserves it
            // untouched. When the field lands, the clone preserves it for
            // exactly the same reason -- *because nothing here touches it*.
            // Do not add an arm that writes it without a form to fill it.
            //
            // Unknown: an item type this build does not understand. There is
            // by definition nothing safe to write into it.
            //
            // Both are spelled out rather than reached by a catch-all `_ =>`,
            // which `ItemKind`'s own doc forbids: a catch-all would silently
            // hand a future variant whichever behaviour happened to sit next
            // to it.
            ItemKind::SshKey | ItemKind::Unknown(_) => {}
        }
        updated
    }

    /// The create payload. **Login-shaped**, because the create form has no
    /// type selector yet: that is the UI half of the plan's Task 5, and this
    /// draft therefore always comes from [`Self::empty`], whose kind is
    /// `Login`. [`NewItem`] itself can now express every kind, so adding the
    /// selector is a change here and not in `vault_bridge`.
    pub fn to_new_item(&self) -> NewItem {
        NewItem::login(
            self.name.clone(),
            self.username.clone(),
            self.password.clone(),
            self.folder_id.clone(),
        )
    }
}

/// The form's heading. Pure so the wording is asserted directly rather than
/// inferred from a screenshot; the old hardcoded "Edit login" was the read
/// pane's `kind_offers_edit` stopgap made visible.
fn form_title(kind: ItemKind, creating: bool) -> String {
    let noun = match kind {
        ItemKind::Login => "login",
        ItemKind::SecureNote => "secure note",
        ItemKind::Card => "card",
        ItemKind::Identity => "identity",
        ItemKind::SshKey => "SSH key",
        // "item" rather than the kind's own "Unsupported item" label, which
        // reads as "Edit unsupported item".
        ItemKind::Unknown(_) => "item",
    };
    format!("{} {noun}", if creating { "New" } else { "Edit" })
}

/// The identity form's rows, in the same order and grouping the read pane
/// uses. One place, so a field cannot be modelled, drafted and then never
/// offered.
fn identity_rows(d: &mut IdentityDraft) -> Vec<(&'static str, &mut String)> {
    vec![
        ("Title", &mut d.title),
        ("First name", &mut d.first_name),
        ("Middle name", &mut d.middle_name),
        ("Last name", &mut d.last_name),
        ("Email", &mut d.email),
        ("Phone", &mut d.phone),
        ("Username", &mut d.username),
        ("Company", &mut d.company),
        ("Address", &mut d.address1),
        ("Address 2", &mut d.address2),
        ("Address 3", &mut d.address3),
        ("City", &mut d.city),
        ("State", &mut d.state),
        ("Postal code", &mut d.postal_code),
        ("Country", &mut d.country),
        ("SSN", &mut d.ssn),
        ("Passport number", &mut d.passport_number),
        ("Licence number", &mut d.license_number),
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditAction {
    None,
    Save,
    Cancel,
}

/// The folders this form may actually move an item **into**: the folder list
/// as the backend sends it, minus `bw serve`'s virtual "No Folder" bucket.
///
/// The CLI reports that bucket alongside real folders, as a folder with an
/// empty id (see [`sidebar::is_virtual_folder`], the one definition of
/// "virtual" -- an inline `id.is_empty()` here would be a second one).
/// Listing it as a destination offered the user two near-identical rows --
/// this form's own hardcoded "No folder" and the server's "No Folder" -- and
/// choosing the second wrote `folderId: ""`, which is neither a folder the
/// item is in nor unfiled: `SidebarFilter::Unfiled` means `folder_id: None`,
/// so the item showed up under no folder row and no virtual row, visible only
/// under "All items", while the combo box's own label resolved the empty id
/// back to the bucket's name and so reported success.
///
/// A separate pure function rather than a filter inline in the combo box
/// because nothing in this crate can call [`draw_detail_edit`] (it needs an
/// egui context), and an untestable seam is exactly what let the unfiltered
/// loop ship. `the_folder_dropdown_offers_exactly_the_assignable_folders` is
/// the source-text guard tying the two together.
///
/// Note what this does *not* do: it does not make un-filing possible. That is
/// a separate, backend-side limitation handled by [`EditDraft::may_unfile`],
/// which greys out the hardcoded "No folder" row. The two are independent --
/// this one removes a destination that was never real, that one disables a
/// transition the CLI ignores.
pub fn assignable_folders(folders: &[Folder]) -> Vec<&Folder> {
    folders.iter().filter(|folder| !sidebar::is_virtual_folder(folder)).collect()
}

pub fn draw_detail_edit(
    ui: &mut egui::Ui,
    draft: &mut EditDraft,
    folders: &[Folder],
    creating: bool,
) -> EditAction {
    let mut action = EditAction::None;
    // Read before the closure borrows `draft` mutably.
    let may_unfile = draft.may_unfile();

    ui.label(theme::bold(form_title(draft.kind, creating), 19.0).color(theme::INK));
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

            // Exhaustive, no catch-all: `ItemKind`'s doc forbids one, and a
            // `_ =>` here would render a login's username and password box
            // over whatever kind Bitwarden ships next.
            match draft.kind {
                ItemKind::Login => {
                    theme::field_label(ui, "Username");
                    theme::text_field(ui, &mut draft.username, false);
                    ui.add_space(10.0);

                    theme::field_label(ui, "Password");
                    theme::password_field(ui, &mut draft.password, &mut draft.reveal_password);
                    ui.add_space(10.0);
                }
                ItemKind::Card => {
                    let card = &mut draft.card;
                    theme::field_label(ui, "Cardholder name");
                    theme::text_field(ui, &mut card.cardholder_name, false);
                    ui.add_space(10.0);

                    theme::field_label(ui, "Brand");
                    theme::text_field(ui, &mut card.brand, false);
                    ui.add_space(10.0);

                    theme::field_label(ui, "Number");
                    theme::password_field(ui, &mut card.number, &mut card.reveal_number);
                    ui.add_space(10.0);

                    theme::field_label(ui, "Expiry month");
                    theme::text_field(ui, &mut card.exp_month, false);
                    ui.add_space(10.0);

                    theme::field_label(ui, "Expiry year");
                    theme::text_field(ui, &mut card.exp_year, false);
                    ui.add_space(10.0);

                    theme::field_label(ui, "Security code");
                    theme::password_field(ui, &mut card.code, &mut card.reveal_code);
                    ui.add_space(10.0);
                }
                ItemKind::Identity => {
                    for (label, value) in identity_rows(&mut draft.identity) {
                        theme::field_label(ui, label);
                        theme::text_field(ui, value, false);
                        ui.add_space(10.0);
                    }
                }
                ItemKind::SecureNote => {
                    theme::field_label(ui, "Note");
                    // A multiline box rather than `theme::text_field`: a
                    // secure note's body is the whole item and is routinely
                    // several lines. `theme` has no multiline helper.
                    ui.add(
                        egui::TextEdit::multiline(&mut draft.note_body)
                            .desired_width(ui.available_width())
                            .desired_rows(8),
                    );
                    ui.add_space(10.0);
                }
                ItemKind::SshKey | ItemKind::Unknown(_) => {
                    ui.label(
                        RichText::new(
                            "Deskwarden can change only the name and folder of this item. Its \
                             contents are left exactly as they are -- open it in the Bitwarden \
                             web vault or app to edit them.",
                        )
                        .size(12.0)
                        .color(theme::TEXT_FAINT),
                    );
                    ui.add_space(10.0);
                }
            }

            theme::field_label(ui, "Folder");
            // Both the label and the rows read the *assignable* list, not the
            // raw one, and the label matters as much as the rows: resolving a
            // draft's folder id against the virtual bucket is what let an
            // item carrying `folderId: ""` display the bucket's name and look
            // correctly filed while belonging to nothing. Unresolvable now
            // falls through to "No folder", which is at least a state the
            // sidebar agrees exists.
            let assignable = assignable_folders(folders);
            egui::ComboBox::from_id_salt("edit-folder")
                .selected_text(
                    assignable
                        .iter()
                        .find(|f| Some(&f.id) == draft.folder_id.as_ref())
                        .map(|f| f.name.as_str())
                        .unwrap_or("No folder"),
                )
                .show_ui(ui, |ui| {
                    // "No folder" is offered only when it can actually take
                    // effect -- see `EditDraft::may_unfile`. Shown disabled
                    // rather than hidden, so the option's absence is a
                    // visible limitation instead of a missing row.
                    let unfile = egui::Button::selectable(draft.folder_id.is_none(), "No folder");
                    if ui.add_enabled(may_unfile, unfile).clicked() {
                        draft.folder_id = None;
                    }
                    for folder in &assignable {
                        let selected = draft.folder_id.as_deref() == Some(folder.id.as_str());
                        if ui.selectable_label(selected, &folder.name).clicked() {
                            draft.folder_id = Some(folder.id.clone());
                        }
                    }
                });

            if !may_unfile {
                ui.add_space(6.0);
                ui.label(
                    RichText::new(
                        "This item can be moved to another folder, but the Bitwarden CLI this \
                         app talks to cannot remove an item from a folder. Un-file it in the \
                         web vault or app.",
                    )
                    .size(11.0)
                    .color(theme::TEXT_FAINT),
                );
            }
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
                totp: Some("SEED".to_string().into()),
                uris: vec![UriEntry { uri: Some("https://ledgerline.example".into()), other: serde_json::Map::new() }],
                other: login_other,
            }),
            card: None,
            identity: None,
            notes: None,
            item_type: Some(1),
            folder_id: Some("f1".into()),
            favorite: true,
            other: item_other,
        }
    }

    fn folder(id: &str, name: &str) -> Folder {
        Folder { id: id.into(), name: name.into(), other: serde_json::Map::new() }
    }

    #[test]
    fn the_virtual_no_folder_bucket_is_not_offered_as_a_destination() {
        // The regression this guards: `bw serve` reports "no folder" as a
        // folder with an EMPTY id, and this form's combo box listed the
        // folder slice verbatim. That put two near-identical rows in the
        // dropdown -- the hardcoded "No folder" and the server's "No Folder"
        // -- and choosing the second wrote `folderId: ""`. Such an item then
        // matches no folder row (its id is "") and no `Unfiled` row (that is
        // `None`), so it is visible only under "All items", while the pane's
        // own `selected_text` resolves the empty id to the bucket's name and
        // reports success.
        let folders = [folder("", "No Folder"), folder("f1", "Banking"), folder("f2", "Work")];
        let offered = assignable_folders(&folders);
        assert_eq!(
            offered.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(),
            vec!["f1", "f2"],
            "the virtual bucket is not a folder and cannot be one, so it is not a destination"
        );
    }

    #[test]
    fn a_real_folder_named_no_folder_is_still_offered() {
        // The other half of `sidebar::is_virtual_folder`'s reason for keying
        // on the ID rather than the name: "No Folder" is user-facing text a
        // real folder can be called, and filtering by name would lock the
        // user out of a folder they actually own.
        let folders = [folder("", "No Folder"), folder("f9", "No Folder")];
        let offered = assignable_folders(&folders);
        assert_eq!(offered.len(), 1);
        assert_eq!(offered[0].id, "f9");
    }

    #[test]
    fn the_folder_dropdown_offers_exactly_the_assignable_folders() {
        // `draw_detail_edit` needs an egui context, so no test in this crate
        // can click that combo box -- which is precisely how the unfiltered
        // loop survived. A source-text guard over this file instead (the
        // same device as `settings.rs`'s
        // `the_config_path_still_matches_the_one_main_resolves`): the pure
        // function above is only worth anything if the dropdown is the thing
        // calling it.
        let source = include_str!("detail_edit.rs");
        assert!(
            source.contains("let assignable = assignable_folders(folders);"),
            "the folder combo box no longer builds its rows from `assignable_folders`, so the \
             virtual \"No Folder\" bucket is being offered as a destination again"
        );
        // Assembled rather than written out, or this assertion's own needle
        // would be the match it is looking for.
        let raw_loop = format!("for folder in {}", "folders");
        assert!(
            !source.contains(&raw_loop),
            "something in this file iterates the raw folder slice again -- that slice includes \
             `bw serve`'s virtual bucket; go through `assignable_folders`"
        );
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
        assert_eq!(login.totp.as_deref().map(|t| t.as_str()), Some("SEED"));
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

    /// Fixtures transcribed from `.superpowers/sdd/item-shapes-capture.md`.
    ///
    /// They deliberately carry **real values in every catch-all** -- item
    /// level (`object`, `key`, `reprompt`, `passwordHistory`, ...), inside the
    /// type object (`futureCardKey`, `passwordRevisionDate: null`), and inside
    /// `fields` elements (`type`, `linkedId`). An earlier review found the
    /// empty-catch-all version of this test proved nothing, because an item
    /// with nothing to lose cannot demonstrate that nothing was lost.
    const LOGIN_WITH_EXTRAS: &str = r#"{
        "object":"item","id":"11111111-1111-1111-1111-111111111111","name":"Ledgerline",
        "type":1,"favorite":true,"folderId":"f1","reprompt":0,"key":"ITEMKEY",
        "creationDate":"2026-01-02T03:04:05.000Z","revisionDate":"2026-02-03T04:05:06.000Z",
        "attachments":null,"collectionIds":["c1"],
        "passwordHistory":[{"lastUsedDate":"2026-01-01T00:00:00.000Z","password":"old"}],
        "notes":"a note",
        "fields":[{"name":"deskwarden:app-match","value":"exe:code.exe","type":0,"linkedId":null}],
        "login":{"username":"a@b.com","password":"p","totp":"SEED",
                 "passwordRevisionDate":null,"fido2Credentials":[],
                 "uris":[{"uri":"https://ledgerline.example","match":null}]}
    }"#;

    const CARD_WITH_EXTRAS: &str = r#"{
        "object":"item","id":"22222222-2222-2222-2222-222222222222","name":"Visa",
        "type":3,"favorite":false,"folderId":"f1","reprompt":0,"key":"ITEMKEY",
        "creationDate":"2026-01-02T03:04:05.000Z","revisionDate":"2026-02-03T04:05:06.000Z",
        "attachments":null,"collectionIds":[],"passwordHistory":null,
        "notes":"the PIN is elsewhere",
        "fields":[{"name":"issuer","value":"Bank","type":0,"linkedId":null}],
        "card":{"cardholderName":"John Doe","brand":"visa","number":"4242424242424242",
                "expMonth":"04","expYear":"2028","code":"123","futureCardKey":{"deep":true}}
    }"#;

    /// Seventeen captured keys; `address3` is absent (it is not in the
    /// template) and `middleName` is **present but empty**, which is the shape
    /// that separates "blank means absent" from "blank means unchanged".
    const IDENTITY_WITH_EXTRAS: &str = r#"{
        "object":"item","id":"33333333-3333-3333-3333-333333333333","name":"Me",
        "type":4,"favorite":false,"reprompt":0,"key":"ITEMKEY",
        "creationDate":"2026-01-02T03:04:05.000Z","revisionDate":"2026-02-03T04:05:06.000Z",
        "attachments":null,"collectionIds":[],"passwordHistory":null,
        "fields":[],
        "identity":{"title":"Ms","firstName":"Ada","middleName":"","lastName":"Lovelace",
                    "address1":"12 Analytical Way","address2":"","city":"London","state":"",
                    "postalCode":"NW1","country":"GB","company":"","email":"ada@example.com",
                    "phone":"","ssn":"","username":"ada","passportNumber":"","licenseNumber":"",
                    "futureIdentityKey":7}
    }"#;

    const NOTE_WITH_EXTRAS: &str = r#"{
        "object":"item","id":"44444444-4444-4444-4444-444444444444","name":"Wifi",
        "type":2,"favorite":true,"reprompt":0,"key":"ITEMKEY",
        "creationDate":"2026-01-02T03:04:05.000Z","revisionDate":"2026-02-03T04:05:06.000Z",
        "attachments":null,"collectionIds":[],"passwordHistory":null,
        "notes":"the passphrase","fields":[],"secureNote":{"type":0}
    }"#;

    /// `VaultItem` has no `ssh_key` field in this build, so the whole `sshKey`
    /// object rides `VaultItem::other`. When it becomes a modelled field it
    /// will ride the clone instead, and this fixture must keep passing either
    /// way -- that is the point of it.
    const SSH_WITH_EXTRAS: &str = r#"{
        "object":"item","id":"55555555-5555-5555-5555-555555555555","name":"deploy key",
        "type":5,"favorite":false,"reprompt":0,"key":"ITEMKEY",
        "creationDate":"2026-01-02T03:04:05.000Z","revisionDate":"2026-02-03T04:05:06.000Z",
        "attachments":null,"collectionIds":[],"passwordHistory":null,"fields":[],
        "sshKey":{"privateKey":"PRIV","publicKey":"PUB","keyFingerprint":"SHA256:FP"}
    }"#;

    const UNKNOWN_WITH_EXTRAS: &str = r#"{
        "object":"item","id":"66666666-6666-6666-6666-666666666666","name":"From the future",
        "type":6,"favorite":false,"reprompt":0,"key":"ITEMKEY",
        "creationDate":"2026-01-02T03:04:05.000Z","revisionDate":"2026-02-03T04:05:06.000Z",
        "attachments":null,"collectionIds":[],"passwordHistory":null,"fields":[],
        "somethingNew":{"deep":true}
    }"#;

    const EVERY_KIND: [&str; 6] = [
        LOGIN_WITH_EXTRAS,
        CARD_WITH_EXTRAS,
        IDENTITY_WITH_EXTRAS,
        NOTE_WITH_EXTRAS,
        SSH_WITH_EXTRAS,
        UNKNOWN_WITH_EXTRAS,
    ];

    fn parse(raw: &str) -> VaultItem {
        serde_json::from_str(raw).expect("fixture is not a VaultItem")
    }

    #[test]
    fn editing_a_card_does_not_give_it_a_login_object() {
        // The live bug: `apply_to` unconditionally did
        // `updated.login.unwrap_or_default()`, so saving a card from the edit
        // form gave it an empty `login` -- an item carrying two type objects.
        let card = parse(CARD_WITH_EXTRAS);
        assert!(card.login.is_none());
        let draft = EditDraft::from_item(&card);
        let saved = draft.apply_to(&card);
        assert!(saved.login.is_none(), "editing a card gave it a login object");
        assert!(saved.card.is_some(), "editing a card dropped its card object");
    }

    #[test]
    fn an_edit_that_changes_nothing_produces_a_byte_identical_item() {
        // The strongest property in the plan: open the edit form, touch
        // nothing, save -- and the bytes on the wire must be what came off it.
        for raw in EVERY_KIND {
            let item = parse(raw);
            let draft = EditDraft::from_item(&item);
            let saved = draft.apply_to(&item);
            let before: serde_json::Value = serde_json::from_str(raw).unwrap();
            let after = serde_json::to_value(&saved).unwrap();
            assert_eq!(before, after, "an unchanged edit altered a {:?}", ItemKind::of(&item));
        }
    }

    #[test]
    fn from_item_reads_the_kind_off_the_item() {
        let kinds: Vec<ItemKind> =
            EVERY_KIND.iter().map(|raw| EditDraft::from_item(&parse(raw)).kind).collect();
        assert_eq!(
            kinds,
            vec![
                ItemKind::Login,
                ItemKind::Card,
                ItemKind::Identity,
                ItemKind::SecureNote,
                ItemKind::SshKey,
                ItemKind::Unknown(6),
            ]
        );
    }

    #[test]
    fn a_real_edit_of_a_card_touches_only_what_it_changed() {
        let item = parse(CARD_WITH_EXTRAS);
        let mut draft = EditDraft::from_item(&item);
        draft.name = "Visa (personal)".into();
        draft.card.number = "4000000000000002".into();
        let saved = draft.apply_to(&item);

        assert_eq!(saved.name, "Visa (personal)");
        assert!(saved.login.is_none(), "a card edit invented a login object");
        let card = saved.card.as_ref().unwrap();
        assert_eq!(card.number.as_deref().map(|n| n.as_str()), Some("4000000000000002"));
        assert_eq!(card.cardholder_name.as_deref(), Some("John Doe"));
        assert_eq!(card.code.as_deref().map(|c| c.as_str()), Some("123"));
        assert_eq!(
            card.other.get("futureCardKey"),
            Some(&serde_json::json!({"deep": true})),
            "an unmodelled key inside `card` was dropped by an edit"
        );
        assert_eq!(
            saved.notes.as_deref().map(|n| n.as_str()),
            Some("the PIN is elsewhere"),
            "the form offers no notes editor on a card, so the note must ride the clone"
        );
        assert!(saved.favorite.eq(&false) && saved.item_type == Some(3));
        assert_eq!(
            saved.other.get("passwordHistory"),
            Some(&serde_json::Value::Null),
            "a present-but-null item-level key was dropped by an edit"
        );
        assert_eq!(saved.fields.len(), 1);
        assert_eq!(saved.fields[0].other.get("type"), Some(&serde_json::json!(0)));
    }

    #[test]
    fn a_real_edit_of_a_login_still_writes_only_the_login_object() {
        let item = parse(LOGIN_WITH_EXTRAS);
        let mut draft = EditDraft::from_item(&item);
        draft.username = "new@b.com".into();
        let saved = draft.apply_to(&item);

        let login = saved.login.as_ref().unwrap();
        assert_eq!(login.username.as_deref(), Some("new@b.com"));
        assert_eq!(login.totp.as_deref().map(|t| t.as_str()), Some("SEED"));
        assert_eq!(login.uris.len(), 1);
        assert!(saved.card.is_none(), "a login edit invented a card object");
        assert!(saved.identity.is_none(), "a login edit invented an identity object");
        assert_eq!(saved.notes.as_deref().map(|n| n.as_str()), Some("a note"));
    }

    #[test]
    fn editing_a_secure_note_writes_the_body_to_item_level_notes() {
        let item = parse(NOTE_WITH_EXTRAS);
        let mut draft = EditDraft::from_item(&item);
        assert_eq!(draft.note_body, "the passphrase");
        draft.note_body = "a different passphrase".into();
        let saved = draft.apply_to(&item);

        assert_eq!(saved.notes.as_deref().map(|n| n.as_str()), Some("a different passphrase"));
        assert!(saved.login.is_none(), "editing a note gave it a login object");
        assert!(saved.card.is_none());
        assert_eq!(
            saved.other.get("secureNote"),
            Some(&serde_json::json!({"type": 0})),
            "the secureNote discriminator was dropped by an edit"
        );
    }

    #[test]
    fn editing_an_ssh_key_or_an_unknown_type_changes_only_the_name() {
        // Neither kind has an editor, and neither may grow one by accident:
        // an SSH key's `sshKey` object (unmodelled in this build, so it rides
        // `VaultItem::other`) and an unknown type's payload must come through
        // an edit untouched.
        for raw in [SSH_WITH_EXTRAS, UNKNOWN_WITH_EXTRAS] {
            let item = parse(raw);
            let mut draft = EditDraft::from_item(&item);
            draft.name = "Renamed".into();
            // Fields no form offers for these kinds; setting them must have
            // no effect whatsoever.
            draft.username = "leak".into();
            draft.password = "leak".into();
            draft.card.number = "leak".into();
            draft.note_body = "leak".into();
            let saved = draft.apply_to(&item);

            assert_eq!(saved.name, "Renamed");
            assert!(saved.login.is_none(), "an unsupported kind gained a login object");
            assert!(saved.card.is_none(), "an unsupported kind gained a card object");
            assert!(saved.identity.is_none());
            assert!(saved.notes.is_none(), "an unsupported kind gained notes");

            let before: serde_json::Value = serde_json::from_str(raw).unwrap();
            let mut expected = before.clone();
            expected["name"] = serde_json::json!("Renamed");
            assert_eq!(
                expected,
                serde_json::to_value(&saved).unwrap(),
                "renaming this item changed something other than its name"
            );
        }
    }

    #[test]
    fn a_draft_never_writes_its_kinds_object_onto_an_item_of_another_kind() {
        // A draft carried across a selection change would otherwise graft its
        // own type object on -- the same two-type-object failure, one layer
        // up from the bug this method was carrying.
        let card = parse(CARD_WITH_EXTRAS);
        let mut login_draft = EditDraft::from_item(&parse(LOGIN_WITH_EXTRAS));
        login_draft.name = "Renamed".into();
        let saved = login_draft.apply_to(&card);

        assert_eq!(saved.name, "Renamed");
        assert!(saved.login.is_none(), "a login draft grafted a login onto a card");
        assert_eq!(
            saved.card.as_ref().and_then(|c| c.number.as_deref()).map(|n| n.as_str()),
            Some("4242424242424242"),
            "the card's own object was disturbed"
        );
    }

    #[test]
    fn blank_means_absent_but_does_not_erase_an_explicit_empty_string() {
        // Clearing a value the item had drops the key, the convention
        // `create_item` and the login path already follow.
        assert_eq!(edited(Some("old"), ""), None);
        assert_eq!(edited(Some("old"), "new"), Some("new".to_string()));
        assert_eq!(edited(None, "new"), Some("new".to_string()));
        assert_eq!(edited(None, ""), None);
        // But an item that ARRIVED carrying an empty string keeps it: empty
        // is not absent, and an untouched save must not rewrite the shape.
        assert_eq!(edited(Some(""), ""), Some(String::new()));
    }

    #[test]
    fn every_modelled_identity_field_is_offered_by_the_form() {
        // A field that is modelled, drafted, and then never rendered is
        // invisible data loss waiting to happen: the pane shows it, the form
        // silently cannot change it.
        let item = parse(r#"{"id":"1","name":"Me","type":4,"fields":[],"identity":{}}"#);
        let mut draft = EditDraft::from_item(&item);
        let rows = identity_rows(&mut draft.identity);
        assert_eq!(rows.len(), 18, "IdentityData has eighteen modelled fields");
        for (i, (_, value)) in rows.into_iter().enumerate() {
            *value = format!("v{i}");
        }
        let saved = draft.apply_to(&item);
        let json = serde_json::to_value(saved.identity.as_ref().unwrap()).unwrap();
        for key in [
            "title",
            "firstName",
            "middleName",
            "lastName",
            "address1",
            "address2",
            "address3",
            "city",
            "state",
            "postalCode",
            "country",
            "company",
            "email",
            "phone",
            "ssn",
            "username",
            "passportNumber",
            "licenseNumber",
        ] {
            assert!(
                json.get(key).is_some(),
                "`{key}` is modelled on IdentityData but the edit form never offers it"
            );
        }
    }

    #[test]
    fn the_card_secrets_do_not_share_one_reveal_flag_and_both_start_masked() {
        let mut draft = EditDraft::from_item(&parse(CARD_WITH_EXTRAS));
        assert!(!draft.card.reveal_number);
        assert!(!draft.card.reveal_code);
        draft.card.reveal_number = true;
        assert!(!draft.card.reveal_code, "revealing the number revealed the security code");
    }

    #[test]
    fn an_item_already_in_a_folder_is_not_offered_no_folder() {
        // `bw serve` (CLI 2026.7.0) cannot un-file an item: null, "" and
        // omission all leave the folder unchanged, proven against a control
        // that did change in the same PUT. Offering the option would show a
        // change the server ignores and the next sync reverts.
        assert!(!EditDraft::from_item(&parse(CARD_WITH_EXTRAS)).may_unfile());
        // An item with no folder may stay that way -- the option is a no-op
        // and honest, and it is what the create form needs.
        assert!(EditDraft::from_item(&parse(NOTE_WITH_EXTRAS)).may_unfile());
        assert!(EditDraft::empty().may_unfile());
    }

    #[test]
    fn apply_to_refuses_an_unfile_the_backend_would_ignore() {
        // Belt and braces behind the widget: the caller writes this value
        // straight into its in-memory list, so a clear the server drops must
        // not leave the pane disagreeing with the vault.
        let item = parse(CARD_WITH_EXTRAS);
        let mut draft = EditDraft::from_item(&item);
        draft.folder_id = None;
        assert_eq!(draft.apply_to(&item).folder_id.as_deref(), Some("f1"));

        // Every other transition is the draft's to make.
        let mut moved = EditDraft::from_item(&item);
        moved.folder_id = Some("f2".into());
        assert_eq!(moved.apply_to(&item).folder_id.as_deref(), Some("f2"));

        let unfiled = parse(NOTE_WITH_EXTRAS);
        let mut filing = EditDraft::from_item(&unfiled);
        filing.folder_id = Some("f2".into());
        assert_eq!(filing.apply_to(&unfiled).folder_id.as_deref(), Some("f2"));
    }

    #[test]
    fn the_form_titles_name_the_kind_and_only_a_login_says_login() {
        assert_eq!(form_title(ItemKind::Login, true), "New login");
        assert_eq!(form_title(ItemKind::Login, false), "Edit login");
        assert_eq!(form_title(ItemKind::Card, false), "Edit card");
        assert_eq!(form_title(ItemKind::SecureNote, false), "Edit secure note");
        assert_eq!(form_title(ItemKind::Identity, false), "Edit identity");
        assert_eq!(form_title(ItemKind::SshKey, false), "Edit SSH key");
        assert_eq!(form_title(ItemKind::Unknown(6), false), "Edit item");
        for kind in [
            ItemKind::SecureNote,
            ItemKind::Card,
            ItemKind::Identity,
            ItemKind::SshKey,
            ItemKind::Unknown(6),
        ] {
            assert!(
                !form_title(kind, false).contains("login"),
                "{kind:?}'s edit form still calls itself a login"
            );
        }
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
        // `NewItem` is an enum with no shared `name`/`folder_id` fields to
        // read, so the same two facts are asserted one step further along, on
        // the payload those fields produce. If anything, this is the stronger
        // form of the original assertion.
        let payload = draft.to_new_item().to_payload();
        assert_eq!(payload["name"], serde_json::json!("New"));
        assert_eq!(payload["folderId"], serde_json::json!("f2"));
        assert_eq!(payload["type"], serde_json::json!(1));
    }
}
