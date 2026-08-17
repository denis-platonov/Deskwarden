//! The vault window's detail pane in edit mode, and the "+ New" creation
//! flow (they share one form: creating is editing against an empty draft
//! with no item id yet). See `detail.rs` for why this is a separate file
//! from read mode.

use crate::app_identity::{self, AppIdentityCache};
use crate::app_match::{AppMatch, TriggerMode};
use crate::key_sequence::{self, FieldRef, PreviewPart, ResolveSource, Token};
use crate::theme;
use crate::vault_bridge::{
    CardData, Folder, GenerateRequest, IdentityData, ItemKind, NewItem, PassphraseRecipe,
    PasswordRecipe, VaultItem,
};
#[cfg(test)]
use crate::vault_bridge::{LoginData, UriEntry};
use crate::vault_window::{detail, sidebar};
use eframe::egui::{self, CornerRadius, Margin, RichText, Stroke};
use std::time::Duration;
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

/// The SSH-key-specific half of a draft (`type: 5`).
///
/// The three field names are the wire keys captured from a real type-5 item on
/// 2026-08-01 (`.superpowers/sdd/item-shapes-capture.md`): `privateKey`,
/// `publicKey`, `keyFingerprint`. [`NewItem::ssh_key`] takes the three strings
/// directly, so this draft is written against that constructor.
///
/// **Create only.** [`EditDraft::apply_to`] never writes these back, and
/// [`EditDraft::from_item`] never reads them: `form_body` gives an existing
/// SSH key `FormBody::UneditableNotice`, so there is no form to fill this
/// draft from or to save it out of. An existing key's data is preserved
/// precisely *because* nothing here touches it. That is also why an SSH key
/// is creatable but not editable -- see
/// [`detail::kind_offers_edit`](super::detail::kind_offers_edit).
#[derive(Debug, Clone, Default)]
pub struct SshKeyDraft {
    pub private_key: String,
    pub public_key: String,
    pub key_fingerprint: String,
    /// Persistent reveal state for the private key, for the same reason
    /// [`EditDraft::reveal_password`] and [`CardDraft::reveal_number`] exist:
    /// a frame-local `bool` resets every frame, which is a toggle that
    /// visibly does nothing. The public key and the fingerprint are not
    /// secrets and get no flag.
    pub reveal_private_key: bool,
}

/// What this form may do with one element of [`VaultItem::fields`].
///
/// Bitwarden's custom fields carry a `type`: `0` text, `1` hidden, `2`
/// boolean, `3` linked. This form offers boxes for the first two only, and
/// **the other two are carried through untouched rather than converted**. A
/// boolean is a checkbox whose wire value is the strings `"true"`/`"false"`,
/// and a linked field's `value` is `null` with the real payload in `linkedId`
/// -- putting either in a text box would let a save rewrite it into something
/// the type no longer describes. That is the exact failure
/// `vault_bridge::VaultField`'s own doc records happening on a real 1656-item
/// vault, and this enum is what keeps this form from being a second cause of
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldRole {
    /// `type: 0`, or no `type` at all. An ordinary box.
    Text,
    /// `type: 1`. A **secret**, drawn with the same masked box and reveal
    /// toggle as the password -- see [`FieldDraft::reveal`].
    Hidden,
    /// A type this form has no editor for (boolean, linked, or a `type` this
    /// build has never seen), or a field with no `name` to label it by.
    /// Listed, so the user can see it is there and that the form left it
    /// alone, and written back byte for byte from [`FieldDraft::original`].
    Preserved,
    /// A `deskwarden:`-prefixed field: ours, not the user's.
    ///
    /// **Not drawn at all**, matching `key_sequence::field_palette`, which
    /// skips the same prefix so the keystroke builder never offers to type
    /// our own bookkeeping. `deskwarden:app-match` already has a dedicated
    /// editor further down this very form, and a second, raw one beside it
    /// would let the two disagree -- a hand-typed `process` there and a
    /// picked one in the app block, with whichever ran last winning.
    ///
    /// It still occupies a slot in [`EditDraft::fields`], and that is the
    /// point: `vault_bridge::with_app_match` replaces the app-match field
    /// **in place** precisely so saving does not reshuffle the user's own
    /// fields, and a draft that dropped the slot would move it to the end on
    /// the next save.
    Internal,
}

/// One element of [`VaultItem::fields`], as the edit form holds it.
///
/// **Everything the server sent that this form does not model rides
/// [`Self::original`]**, and the write in [`Self::to_field`] rebuilds only
/// `name` and `value` on top of the original's `other` map -- the same
/// "clone then overwrite what is known" rule `EditDraft::apply_to` and
/// `vault_bridge::with_app_match` both follow. `type` and `linkedId` are in
/// that map, which is how a hidden field stays hidden through an edit.
#[derive(Debug, Clone)]
pub struct FieldDraft {
    pub name: String,
    pub value: String,
    /// Persistent reveal state for a [`FieldRole::Hidden`] field, for the
    /// same reason [`EditDraft::reveal_password`] exists: a frame-local
    /// `bool` resets every frame. Never consulted for the other roles.
    pub reveal: bool,
    role: FieldRole,
    /// **This row's identity, and the only thing on this struct that is not
    /// about the field.**
    ///
    /// Nothing else here can serve: a name is empty on a row the user has
    /// just added and duplicable on any other, a value likewise, and the
    /// position in `EditDraft::fields` is exactly what changes when a row is
    /// removed. So the row's `id_salt` used to be its index, and the comment
    /// on the deferred remove in [`custom_fields_block`] claimed that
    /// deferring the removal to the end of the frame avoided "a shifted list
    /// would hand one row the next row's widget state". **It did not.**
    /// Deferring avoids invalidating the iterator; it says nothing about the
    /// NEXT frame, on which every row below the removed one answers to its
    /// predecessor's id and inherits that row's egui `TextEdit` state -- the
    /// caret, the selection and the undo buffer.
    ///
    /// A counter rather than a hash of the contents, because two blank rows
    /// added in a row are identical and must still be two rows. Process-local
    /// and never compared across runs: it is a salt, not a key.
    ///
    /// **It never reaches the wire.** [`Self::to_field`] builds the
    /// `VaultField` out of `name`, `value`, `role` and `original` and touches
    /// nothing else, and `EditDraft::to_new_item` carries no fields at all;
    /// `an_untouched_save_is_byte_for_byte_what_was_read` and the round-trip
    /// tests would see it if it did.
    row_id: u64,
    /// What arrived, or `None` for a field this form created.
    ///
    /// Private, and read only by [`Self::to_field`]: it is the byte-identity
    /// guarantee, and a caller that could swap it could swap a hidden
    /// field's `type` for a text one's.
    original: Option<crate::vault_bridge::VaultField>,
}

/// The prefix that marks a custom field as Deskwarden's own bookkeeping.
///
/// The same test `key_sequence::field_palette` applies. Stated here as a
/// `const` rather than a literal so the two cannot drift apart in silence;
/// `the_editor_hides_the_same_prefix_the_palette_hides` holds them together.
const OURS_PREFIX: &str = "deskwarden:";

/// The next [`FieldDraft::row_id`].
///
/// **Starts at 1, and that is not a sentinel.** This doc used to say that a
/// `0` read anywhere was a draft built without going through one of the two
/// constructors. It cannot be: `row_id` is private, `FieldDraft` derives no
/// `Default`, and both struct literals that build one -- in
/// [`FieldDraft::from_field`] and [`FieldDraft::new_of`] -- call this
/// function for the value. A `0` is unconstructible, so the check that
/// sentence invited would be dead code, and the claim was wider than the
/// thing backing it. The count starts at 1 because a counter has to start
/// somewhere.
fn next_field_row_id() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl FieldDraft {
    /// Reads one field off an item, deciding by its `type` what this form may
    /// do with it.
    ///
    /// A missing `type` reads as text, which is what `bw` itself does with
    /// the field this app writes when it creates one (see
    /// `an_app_match_field_added_to_a_fresh_item_carries_no_extra_keys`) --
    /// and reading it as `Preserved` instead would make a freshly bound
    /// item's own field uneditable for one sync.
    fn from_field(field: &crate::vault_bridge::VaultField) -> Self {
        let name = field.name.clone().unwrap_or_default();
        let role = if field.name.is_none() {
            FieldRole::Preserved
        } else if name.starts_with(OURS_PREFIX) {
            FieldRole::Internal
        } else {
            match field.other.get("type") {
                None => FieldRole::Text,
                Some(t) if t == &serde_json::json!(0) => FieldRole::Text,
                Some(t) if t == &serde_json::json!(1) => FieldRole::Hidden,
                Some(_) => FieldRole::Preserved,
            }
        };
        Self {
            name,
            value: drafted(field.value.as_ref().map(|v| v.as_str())),
            reveal: false,
            role,
            row_id: next_field_row_id(),
            original: Some(field.clone()),
        }
    }

    /// A field the user just asked for, of one of the two roles this form can
    /// make.
    ///
    /// The new field carries an explicit `type`, unlike the freshly created
    /// app-match field, and the difference is deliberate rather than an
    /// inconsistency: `type` is *what the user chose*. A hidden field written
    /// without it is a text field, and the secret the user typed into a box
    /// labelled hidden would be visible in every Bitwarden client. Nothing
    /// else is invented -- `linkedId` is not written, because nobody asked
    /// for a linked field and it is not observed on a field `bw` has not
    /// normalised yet.
    fn new_of(role: FieldRole) -> Self {
        Self {
            name: String::new(),
            value: String::new(),
            reveal: false,
            role,
            row_id: next_field_row_id(),
            original: None,
        }
    }

    pub fn role(&self) -> FieldRole {
        self.role
    }

    /// This row's `id_salt` -- see [`Self::row_id`]. Read by
    /// [`custom_fields_block`] and by the test that pins it.
    fn row_id(&self) -> u64 {
        self.row_id
    }

    /// Whether this form draws boxes for this field.
    fn is_editable(&self) -> bool {
        matches!(self.role, FieldRole::Text | FieldRole::Hidden)
    }

    /// This draft as one element of the item's `fields` array.
    ///
    /// `Preserved` and `Internal` answer with exactly what arrived, so the
    /// two roles this form cannot edit cost nothing to carry.
    ///
    /// The editable roles go through [`edited`], the same blank rule the rest
    /// of this form uses: a box left empty on a field that arrived with no
    /// `value` key writes no `value` key, so an untouched save is byte for
    /// byte what was read.
    fn to_field(&self) -> crate::vault_bridge::VaultField {
        // The uneditable roles: whatever arrived, unchanged. There is always
        // an original -- `new_of` only ever makes the two editable roles --
        // and the fallback is written out rather than `unwrap()`ed because a
        // panic in a save path is a worse answer than an honest empty field.
        if !self.is_editable() {
            return self.original.clone().unwrap_or(crate::vault_bridge::VaultField {
                name: Some(self.name.clone()),
                value: None,
                other: serde_json::Map::new(),
            });
        }
        let other = match &self.original {
            Some(o) => o.other.clone(),
            None => {
                let mut other = serde_json::Map::new();
                other.insert(
                    "type".to_string(),
                    serde_json::json!(if self.role == FieldRole::Hidden { 1 } else { 0 }),
                );
                other
            }
        };
        crate::vault_bridge::VaultField {
            // Always `Some`. A field that arrived with no `name` key at all
            // is `FieldRole::Preserved` (see `from_field`) and has already
            // returned above, so this cannot inject a `"name": ""` onto one.
            name: Some(self.name.clone()),
            // `edited_secret`, not `edited`: a custom field's value is now
            // `Zeroizing` on the model side, and this is the save path that
            // builds it (copy F of the trace in `VaultField::value`'s doc).
            value: edited_secret(
                self.original.as_ref().and_then(|o| o.value.as_ref()).map(|v| v.as_str()),
                &self.value,
            ),
            other,
        }
    }
}

/// One running window, as the edit form's process picker shows it.
///
/// A plain, owned, `Clone` copy of the fields of
/// [`crate::window_list::WindowInfo`] this form uses -- not the type itself,
/// which is built by a Win32 callback and is neither `Clone` nor `Debug`, and
/// which the draft has to be both of. **The enumeration itself is not
/// re-implemented**: [`running_app_rows`] is a thin map over
/// `window_list::list_windows`, which stays the one place that decides what
/// counts as a window a user could point at (visible, titled, not cloaked, not
/// a tool window, attributed through the frame host rather than named after
/// it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppWindowRow {
    pub title: String,
    pub exe_name: String,
    pub exe_path: String,
    /// See [`crate::window_list::WindowInfo::hosted`]. Carried through
    /// verbatim because it cannot be re-derived later, and because it is the
    /// single thing that decides whether this row's title may ever be matched
    /// on.
    pub hosted: bool,
    pub pid: u32,
    /// The window handle, which is this row's identity. NOT the pid: every
    /// unattributable Microsoft Store frame on a machine shares the host's
    /// pid, so two different rows can carry the same one -- the same reason
    /// `picker_ui::selected_window` keys on the handle.
    pub hwnd: isize,
}

/// The windows the process picker offers.
///
/// Excludes this process, so the form cannot offer to match Deskwarden to
/// itself -- the same argument `picker_ui::run_picker` passes.
pub fn running_app_rows() -> Vec<AppWindowRow> {
    crate::window_list::list_windows(std::process::id())
        .into_iter()
        .map(|w| AppWindowRow {
            title: w.title,
            exe_name: w.exe_name,
            exe_path: w.exe_path,
            hosted: w.hosted,
            pid: w.pid,
            hwnd: w.hwnd,
        })
        .collect()
}

/// Why a row must not be chosen, or `None` when it may be.
///
/// The one refusal, and it is the same one `picker_ui::host_process_refusal`
/// makes for the same reason: a row still showing `ApplicationFrameHost.exe`
/// is a Microsoft Store frame whose app could not be identified, and matching
/// the host would fill this item into **every** Store app. Shorter wording
/// than the tray picker's because this row is one of a list rather than the
/// single thing the window is about, and it is shown on the row itself.
///
/// `Option<&'static str>` rather than a `bool` so the refusal and its reason
/// cannot drift apart -- a disabled row with no explanation is the silent
/// no-op this crate keeps being patched for.
pub fn window_row_refusal(row: &AppWindowRow) -> Option<&'static str> {
    crate::window_watch::is_host_process(&row.exe_name).then_some(
        "Windows is not saying which app is inside this window. Restore it from the taskbar \
         and refresh.",
    )
}

/// The app-binding half of a draft: the `deskwarden:app-match` custom field,
/// broken out into the boxes the form edits.
///
/// **`None` on [`EditDraft`] means the item carries no binding**, and the form
/// draws no app block at all. Creating a binding is still the tray's "Add
/// app..." flow: this form edits one that exists, which is exactly what was
/// asked for ("when clicked Edit on a login **if app present**"). Offering a
/// create here as well would put a second producer of a first binding in a
/// second file, and the two would have to agree forever about a capture that
/// only makes sense against a live window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppMatchDraft {
    /// Cleared by Remove. The binding's fields are deliberately **kept** when
    /// it is cleared, so the block can say what is about to be removed, and so
    /// that a Remove followed by Cancel loses nothing.
    pub bound: bool,
    pub process: String,
    pub title: String,
    pub hosted: bool,
    pub path: String,
    pub args: String,
    /// The stored autofill trigger, **carried and never offered**.
    ///
    /// No control in this form writes it: what a matched foreground window
    /// does is one global preference (`settings::Settings::prompt_on_match`)
    /// and nothing in this build reads this field. It is on the draft so that
    /// [`Self::to_match`] can write back the value the item already had --
    /// v0.5.0's `trigger` has no `#[serde(default)]`, so an edit that dropped
    /// it, or that invented a value for it, would make the binding unreadable
    /// or wrong to a build the user may roll back to. See
    /// [`AppMatch::trigger`] and
    /// `an_edit_carries_the_stored_trigger_through_untouched`.
    pub trigger: TriggerMode,
    /// Whether the running-window list is open.
    pub picking: bool,
    /// The rows that list is showing.
    ///
    /// Captured when the list is opened and when Refresh is clicked -- **never
    /// per frame**. Enumerating every top-level window on the desktop is an
    /// `EnumWindows` walk with a `DwmGetWindowAttribute`, a
    /// `GetWindowThreadProcessId`, a process-name lookup and an image-path
    /// lookup per window; doing that on every repaint of an open form is the
    /// per-frame I/O this feature is otherwise careful to avoid.
    pub windows: Vec<AppWindowRow>,
    pub window_filter: String,
    /// The keystroke sequence, **as the stored string** rather than as a
    /// parsed value.
    ///
    /// This is what makes the byte-for-byte promise in
    /// [`AppMatch::sequence`]'s doc true rather than aspirational: a sequence
    /// this build does not fully understand -- one KeePass wrote, one a later
    /// version wrote -- is carried through an edit of the item's *name*
    /// without ever passing through this build's renderer, so there is no
    /// spelling it could be changed into. The chips are a VIEW of this string
    /// (`key_sequence::parse`), and every edit is a whole-string replacement
    /// (`key_sequence::render`) applied only when the user actually clicks
    /// something.
    pub sequence: String,
    /// Whether the eye is open, i.e. whether the sequence is being previewed
    /// resolved against the item.
    ///
    /// **A `bool`, and it is important that it is only a `bool`.** The resolved
    /// preview is built and dropped inside the frame that paints it (see
    /// [`key_sequence::PreviewPart`]); nothing about it is kept here, so
    /// closing the pane, cancelling the edit or simply moving on leaves no
    /// copy of a password or a one-time code behind on the draft. The same
    /// care [`detail::DetailAction`]'s secret variants take, for the same
    /// reason.
    pub previewing: bool,
    /// The literal-text box, holding what the user is about to add. Not part
    /// of the sequence until Add is clicked, so a half-typed word is not a
    /// chip.
    pub literal_draft: String,
    /// The wait box, **in seconds** -- the unit the user asked in ("Wait N
    /// sec"). Converted to the format's milliseconds by
    /// [`key_sequence::wait_ms_from_seconds`] at the moment Add is clicked.
    pub wait_draft: String,
    /// Whether the keystroke builder is open.
    ///
    /// **Shut by default, and the pane's width is the reason.** The detail
    /// pane is 298pt at the app's minimum size and the edit form's card is
    /// already wider than that (`aae9429`); a palette of every field, every
    /// key, a text box and a wait box is several hundred points of height that
    /// would push Save, Cancel and Remove down the scroll on a form the user
    /// opened to rename something. Closed, the block is a heading, a
    /// one-line summary of what would be typed, and the button that opens it
    /// -- so the sequence is still *visible* without being in the way.
    pub sequence_open: bool,
    /// Whether the sequence is being shown as the template STRING rather than
    /// as the step list.
    ///
    /// A view flag and nothing more. Both views read and write
    /// [`Self::sequence`], so switching between them is not a mode change with
    /// its own state to reconcile -- it is the same string, drawn twice.
    pub template_view: bool,
    /// The text box behind the template view, seeded from [`Self::sequence`]
    /// **verbatim** every time the view is entered.
    ///
    /// Separate from `sequence` only because egui's `TextEdit` needs a
    /// `&mut String` it owns across a frame; the moment it changes, its bytes
    /// are copied into `sequence` unaltered. Nothing here is ever rendered
    /// through [`key_sequence::render`], so a template the user did not touch
    /// leaves `sequence` byte for byte as it arrived -- the promise
    /// [`AppMatch::sequence`] makes.
    pub template_draft: String,
    /// Whether the user has actually edited the template box.
    ///
    /// **The gate on the save refusal, and it has to be here.** A vault may
    /// already hold a sequence this build cannot read back (an unterminated
    /// brace another tool wrote); blocking Save on it would make renaming such
    /// an item impossible, which is a worse outcome than carrying the string.
    /// So the refusal applies to what the user WROTE, in the field that says
    /// it will not parse, and never retroactively to what they inherited.
    pub template_touched: bool,
}

/// The `trigger` a binding created in this form is born with.
///
/// **A constant, not a choice**, and the same constant `picker_ui`'s
/// `RETAINED_TRIGGER` is, for its reasons: the three-way Auto / Prompt / Hotkey
/// control was removed from both windows because nothing in this build reads
/// [`AppMatch::trigger`] -- what a matched foreground window does is the one
/// global `settings::Settings::prompt_on_match`. The KEY is still written,
/// because v0.5.0 has no `#[serde(default)]` on it and cannot parse an
/// `AppMatch` that lacks it, so a binding this build creates must carry a
/// value or a rollback would find the item unreadable.
///
/// `Prompt` rather than `Auto` because it is the value the other creation path
/// already writes, so a binding made here and a binding made from the tray
/// picker are byte for byte the same shape.
const NEW_BINDING_TRIGGER: TriggerMode = TriggerMode::Prompt;

impl AppMatchDraft {
    /// A brand-new, empty binding -- what the "Add an app" button hands to
    /// [`app_block`].
    ///
    /// **`bound: true`, and it is deliberate.** `bound` is not "has this draft
    /// got values in it": it is the Remove flag (see the field's doc), and a
    /// draft created `false` would open on the "this app will stop filling"
    /// notice and an Undo button, which is a sentence about a binding that
    /// never existed. `true` is what draws the block the user came for -- the
    /// path box, Browse, the running-app list, the arguments and the
    /// keystrokes -- and it is the SAME block an existing binding gets, which
    /// is the point: there is one app block in this form, not an add one and
    /// an edit one.
    ///
    /// What stops a click on Add from writing an empty binding to the vault is
    /// [`Self::is_blank`], read by [`app_match_edit`] -- not a second `bool`
    /// here, because a second bool would be a state the block would then have
    /// to render differently and the two paths would diverge again.
    pub fn unbound() -> Self {
        Self {
            bound: true,
            process: String::new(),
            title: String::new(),
            hosted: false,
            path: String::new(),
            args: String::new(),
            trigger: NEW_BINDING_TRIGGER,
            picking: false,
            windows: Vec::new(),
            window_filter: String::new(),
            sequence: String::new(),
            previewing: false,
            literal_draft: String::new(),
            wait_draft: DEFAULT_WAIT_SECONDS.to_string(),
            sequence_open: false,
            template_view: false,
            template_draft: String::new(),
            template_touched: false,
        }
    }

    /// Whether this draft names no app at all -- the state
    /// [`Self::unbound`] starts in and stays in until the user picks
    /// something.
    ///
    /// **`process` is the field that decides it**, with `title` beside it for
    /// the Store-app case where the process is the frame host and the title is
    /// the identity. A binding is *matched on* `process`, so a match without
    /// one can never fire; `path` is not consulted because a path with no file
    /// name leaves `process` alone (see [`Self::set_path`]) and a draft holding
    /// only `C:\` is still nothing.
    ///
    /// **No control in this form can turn a real binding blank.**
    /// [`Self::set_path`] only ever *writes* `process`, and
    /// [`Self::choose_window`] copies one off the row -- so this answering
    /// `true` means the draft was created by the Add button and never filled
    /// in, which is exactly the case [`app_match_edit`] must not write.
    pub fn is_blank(&self) -> bool {
        self.process.is_empty() && self.title.is_empty()
    }

    pub fn from_match(m: &AppMatch) -> Self {
        Self {
            bound: true,
            process: m.process.clone(),
            title: m.title.clone(),
            hosted: m.hosted,
            path: m.path.clone(),
            args: m.args.clone(),
            trigger: m.trigger,
            picking: false,
            windows: Vec::new(),
            window_filter: String::new(),
            // Verbatim. See the field's doc: this is the only copy, and it is
            // the one written back.
            sequence: m.sequence.clone(),
            // Closed. The eye is a reveal, and a reveal that is on by default
            // is not a reveal.
            previewing: false,
            literal_draft: String::new(),
            wait_draft: DEFAULT_WAIT_SECONDS.to_string(),
            sequence_open: false,
            template_view: false,
            template_draft: String::new(),
            template_touched: false,
        }
    }

    /// The template fault that must stop a save, or `None`.
    ///
    /// See [`template_fault`] for what "will not parse" means against a parser
    /// that cannot fail, and [`Self::template_touched`] for why an inherited
    /// string is not judged by it.
    pub fn template_fault(&self) -> Option<&'static str> {
        if !self.template_touched {
            return None;
        }
        template_fault(&self.sequence)
    }

    /// The binding this draft would save.
    ///
    /// `args` is passed straight through -- **not trimmed, not re-quoted, not
    /// split**. See [`AppMatch::args`]: the string is the user's, and this app
    /// has no tokenisation of a Windows command line that it could apply
    /// without guessing. The one thing done to it is the same thing done to
    /// `title`, in the mirror-image case: it is **dropped when it can no longer
    /// apply**.
    pub fn to_match(&self) -> AppMatch {
        AppMatch {
            process: self.process.clone(),
            // A title only ever rides a HOSTED match. Review 31's Important 1
            // as an invariant of this type rather than of one call site: an
            // unhosted title is inert, and an inert title in a saved field is
            // indistinguishable from the batch of them one shipped commit
            // wrote, which is why `MatchEngine::rebuild` refuses all of them.
            title: if self.hosted { self.title.clone() } else { String::new() },
            hosted: self.hosted,
            path: self.path.clone(),
            // ...and the same for the arguments, for the same reason and in
            // the other direction. A Store app is not started by path, so
            // there is no command line to give it: the form draws the args row
            // for a hosted binding as a DISABLED box saying so, which means an
            // `args` string already in the draft -- from a binding that was
            // unhosted when the user typed one, and then re-pointed at a Store
            // app -- becomes invisible and unclearable while still being
            // written on every save. Zeroed here rather than in
            // `choose_window`, so that a hosted binding written by an older
            // build is cleaned up too, and so that Cancel still puts the old
            // arguments back if the user re-points at a real executable.
            args: if self.hosted { String::new() } else { self.args.clone() },
            // Passed through untouched, and NOT dropped for a hosted binding
            // the way `args` above is. The two look alike and are not: a
            // command line is meaningless for a Store app because nothing is
            // started by path, whereas a Store app is typed into exactly like
            // any other window -- so its sequence is as applicable as any.
            sequence: self.sequence.clone(),
            trigger: self.trigger,
        }
    }

    /// Points this binding at `path`, **deriving `process` from it**.
    ///
    /// This is the answer to the question a hand-typed or browsed-for path
    /// raises: `AppMatch::launchable_path` requires the path's file name to be
    /// the very `process` the match is keyed on, and a user editing one of the
    /// two boxes would otherwise be able to break that tie-back and store
    /// something the launcher will silently refuse.
    ///
    /// Three options were on the table -- refuse the save, warn and store it
    /// anyway, or derive. **Derive**, because it is the only one that matches
    /// what the user is doing: pointing at `chrome.exe` and then at
    /// `msedge.exe` means "this item is for Edge now", and asking them to
    /// retype the executable name in a second box to confirm it is asking them
    /// to restate what they just said. Refusing the save would also make an
    /// unrelated field (the arguments, the keystroke sequence) un-editable on any item
    /// whose stored path is already odd, which is the class of item most in
    /// need of editing.
    ///
    /// **A path with no file name changes nothing.** `C:\Program Files\` names
    /// a directory; taking an empty `process` off it would produce a match that
    /// can never fire and can never be launched.
    ///
    /// It also clears `hosted`/`title`: a match with an image path chosen off
    /// the file system is not a Microsoft Store app presenting inside a frame,
    /// and leaving the flag set would leave a title that is matched on but no
    /// longer describes anything.
    pub fn set_path(&mut self, path: &str) {
        self.path = path.to_string();
        if let Some(name) = app_identity::file_name_of(path) {
            self.process = name.to_string();
            self.hosted = false;
            self.title = String::new();
        }
    }

    /// Points this binding at a running window, copying everything off that one
    /// row.
    ///
    /// Byte-for-byte the rule `picker_ui::app_match_for` follows, and for its
    /// reasons: all four values come from the SAME row, so the path really is
    /// the image of the process being named, and **the title is recorded only
    /// for a hosted row**. `trigger` and `args` survive here, because they are
    /// the user's settings for this item and not facts about the window -- and
    /// because a Cancel must be able to put them back. Whether the arguments
    /// are ever *saved* is [`Self::to_match`]'s decision, and for a hosted row
    /// they are not.
    pub fn choose_window(&mut self, row: &AppWindowRow) {
        self.process = row.exe_name.clone();
        self.hosted = row.hosted;
        self.title = if row.hosted { row.title.clone() } else { String::new() };
        self.path = row.exe_path.clone();
        self.bound = true;
        self.picking = false;
    }
}

/// What the edit form's Program file row shows, and whether it may be edited.
///
/// A Microsoft Store app has **no image path and no icon**: it presents inside
/// an `ApplicationFrameHost.exe` frame and is matched by its window title (see
/// [`AppMatch::hosted`]). Showing it an empty, editable path box would invite
/// the user to type one in, and a path typed there could never be right.
///
/// **The word "hosted" does not appear**, here or on screen: it is the
/// mechanism. What the user needs is the fact -- this is a Store app -- and
/// that is what the row says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppPathRow {
    /// An editable box holding the path.
    Editable,
    /// A disabled box holding this text, and a disabled Browse button.
    NotApplicable(&'static str),
}

pub const APP_PATH_STORE_APP: &str = "Not applicable \u{2014} Store app";

/// See [`AppPathRow`].
pub fn app_path_row(hosted: bool) -> AppPathRow {
    if hosted {
        AppPathRow::NotApplicable(APP_PATH_STORE_APP)
    } else {
        AppPathRow::Editable
    }
}

/// The warning under the Program file box, or `None` when there is nothing to
/// warn about.
///
/// **A warning, not a refusal.** A path this app would decline to launch is
/// still a perfectly good *match*: matching is done on `process`, which is a
/// name, and it keeps working. Withholding Save would make the arguments and
/// the keystroke sequence un-editable on exactly the items whose path most
/// needs correcting,
/// and would do it over a decision the user can see and fix in the box directly
/// above the message. What is NOT acceptable is storing it silently -- so the
/// message says precisely which rule the path fails to meet.
///
/// Nothing to say when the match is a Store app (there is no path to have an
/// opinion about) or when the path is empty (every match saved before the field
/// existed).
pub fn app_path_warning(m: &AppMatch) -> Option<&'static str> {
    if m.hosted || m.path.is_empty() || m.launchable_path().is_some() {
        return None;
    }
    Some(
        "Deskwarden will still fill this app, but it will not be able to open it: the program \
         file has to be a full path on a drive letter, with no \u{201c}..\u{201d} in it \
         \u{2014} like C:\\Program Files\\App\\App.exe",
    )
}

/// What saving this draft should do to the item's `deskwarden:app-match` field.
///
/// **`Leave` is the important variant.** `EditDraft::apply_to`'s contract is
/// that an edit that changes nothing produces a byte-identical item, and a
/// binding is JSON in a custom field: rewriting it on every save would rewrite
/// the *serializer's* spelling of it over whatever spelling is in the vault --
/// key order, whitespace, and any future key this build does not model. So the
/// field is written only when the value actually differs from what the item
/// already carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppMatchEdit {
    Leave,
    Write(AppMatch),
    Remove,
}

/// See [`AppMatchEdit`].
///
/// `draft` is `None` for every item this form drew no app block for, and that
/// must mean *leave the field alone* rather than *remove it*: an item can carry
/// a binding whose block the form declined to draw, and a save of its name
/// would otherwise silently unbind it.
pub fn app_match_edit(existing: Option<&AppMatch>, draft: Option<&AppMatchDraft>) -> AppMatchEdit {
    let Some(draft) = draft else {
        return AppMatchEdit::Leave;
    };
    if !draft.bound {
        // A Remove against an item that has nothing to remove is `Leave`, not
        // `Remove`: `without_app_match` would be a no-op, but going through it
        // would still count as a change and cost a PUT.
        return match existing {
            Some(_) => AppMatchEdit::Remove,
            None => AppMatchEdit::Leave,
        };
    }
    if draft.is_blank() {
        // The Add button was clicked and nothing was chosen. Writing here would
        // put `{"process":""}` in the user's vault -- a binding that can never
        // match and can never be launched -- on a form the user may have opened
        // to rename the item. `Leave` and not `Remove`, because a blank draft
        // is never one this form read off an item: see `AppMatchDraft::is_blank`
        // for why no control can blank a real binding, and
        // `clicking_add_and_choosing_nothing_writes_no_binding`.
        return AppMatchEdit::Leave;
    }
    let wanted = draft.to_match();
    if existing == Some(&wanted) {
        AppMatchEdit::Leave
    } else {
        AppMatchEdit::Write(wanted)
    }
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
    /// Which kind this draft is for. Set from the item by [`Self::from_item`]
    /// (and then fixed -- an existing item's type cannot be changed), or
    /// chosen by the "+ New" type menu through [`Self::empty_of`] and
    /// [`Self::set_kind`]. [`Self::apply_to`] writes back **only** this kind's
    /// object, and [`Self::to_new_item`] builds **only** this kind's payload.
    ///
    /// **Private, with [`Self::kind`] to read it.** A caller that could assign
    /// this field directly would change the kind while leaving the previous
    /// kind's fields in place, which is how the abandoned kind's data ends up
    /// on the wire under the chosen one -- the same defect class as the
    /// login-grafting bug. `set_kind` is the only way in, and it clears.
    kind: ItemKind,
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
    /// The login's TOTP **seed** -- `otpauth://` URI or bare base32 secret --
    /// which is what `LoginData::totp` holds. Not a code: nothing in this
    /// form generates one, and the read pane's `detail::TotpState` is the
    /// only thing in this crate that does.
    ///
    /// **A secret**, and treated as one everywhere it is treated at all: the
    /// box is `theme::password_field` (masked, with [`Self::reveal_totp`]),
    /// the model side is `Zeroizing` and is written through [`edited_secret`]
    /// exactly as the password is, and nothing here logs it.
    ///
    /// A plain `String` rather than a `Zeroizing<String>` for the same
    /// recorded reason [`Self::password`] and [`CardDraft`]'s two secrets
    /// are: egui's `TextEdit` buffer is a `String` and egui owns copies of it
    /// in its galley cache regardless. This does not widen the zeroize
    /// guarantee -- see `deskwarden/README.md`.
    pub totp: String,
    /// Persistent reveal state for the seed box. See
    /// [`Self::reveal_password`]; the same rule applies, including that a
    /// freshly opened form never starts revealed.
    pub reveal_totp: bool,
    pub card: CardDraft,
    pub identity: IdentityDraft,
    pub ssh_key: SshKeyDraft,
    /// Item-level `notes`, which is where a secure note's entire body lives.
    /// Populated for every kind but written back only for
    /// [`ItemKind::SecureNote`], because that is the only kind this form
    /// offers a notes editor for; on every other kind the item's own notes
    /// ride the clone untouched.
    pub note_body: String,
    /// What the Generate control beside the password box will ask
    /// `bw serve` for. See [`GeneratorDraft`].
    pub generator: GeneratorDraft,
    /// The item's app binding, or `None` when it carries none.
    ///
    /// **Item-level, like `name` and `folder_id`, and not one of the
    /// kind-specific halves.** A `deskwarden:app-match` is a custom field and
    /// can sit on an item of any type, so it does not belong to
    /// [`Self::kind`] and is not cleared by [`Self::set_kind`].
    ///
    /// `None` is load-bearing on the way out as well as in: it is what tells
    /// [`app_match_edit`] to leave the field alone entirely. See
    /// [`AppMatchDraft`].
    pub app: Option<AppMatchDraft>,
    /// The item's custom fields, **all of them, in the item's own order**.
    ///
    /// Item-level, like `name`, `folder_id` and `app`, and not cleared by
    /// [`Self::set_kind`]: a custom field can sit on an item of any type.
    ///
    /// Order is load-bearing. Bitwarden preserves and displays custom-field
    /// order, and `fields` is a JSON *array*, so a draft that reordered them
    /// would show up as a diff on every save -- which is exactly what
    /// `vault_bridge::with_app_match` replaces its field in place to avoid.
    /// This vector is built by walking the item once and written back by
    /// walking itself once; nothing sorts, filters or partitions it.
    ///
    /// It holds the roles this form does not draw as well as the two it does
    /// -- see [`FieldRole`]. That is what makes the walk order-preserving:
    /// a `deskwarden:app-match` in the middle of the user's fields keeps its
    /// slot because it is still in this list.
    pub fields: Vec<FieldDraft>,
}

/// The generator's own form state: which kind of secret to make, and how big.
///
/// **Deliberately not a [`crate::vault_bridge::GenerateRequest`].** That type
/// is the wire shape and holds a `String` separator and eight booleans; this
/// is what two or three widgets edit across frames, and
/// [`EditDraft::generator_request`] is the one conversion between them. Every
/// option the form does not offer is supplied there from
/// `PasswordRecipe::default()`/`PassphraseRecipe::default()`, so adding a
/// control later changes this struct and that function and nothing else.
///
/// `length` and `words` are **both** kept, rather than one number reinterpreted
/// by `passphrase`: 20 characters and 20 words are wildly different requests,
/// and a shared field would silently carry one over as the other when the
/// combo box is flipped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratorDraft {
    /// A word passphrase rather than a character password.
    pub passphrase: bool,
    /// Characters, when generating a password. The route clamps below 5.
    pub length: u32,
    /// Words, when generating a passphrase. The route clamps below 3.
    pub words: u32,
}

impl Default for GeneratorDraft {
    fn default() -> Self {
        Self {
            passphrase: false,
            length: PasswordRecipe::default().length,
            words: PassphraseRecipe::default().words,
        }
    }
}

/// The bounds the generator's number control offers.
///
/// The lower bounds are the route's own clamps (`length` below 5 becomes 5,
/// `words` below 3 becomes 3) stated as UI limits, so the box cannot show a
/// number the backend will silently ignore -- the form would otherwise say 3
/// and produce 5. The upper bounds are this app's, and they are generous
/// rather than derived: nothing in the route caps either.
const MIN_LENGTH: u32 = 5;
const MAX_LENGTH: u32 = 128;
const MIN_WORDS: u32 = 3;
const MAX_WORDS: u32 = 20;

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
            totp: String::new(),
            reveal_totp: false,
            card: CardDraft::default(),
            identity: IdentityDraft::default(),
            ssh_key: SshKeyDraft::default(),
            note_body: String::new(),
            generator: GeneratorDraft::default(),
            app: None,
            fields: Vec::new(),
        }
    }
}

/// The kinds the "+ New" type menu may offer, in the order it should offer
/// them.
///
/// `ItemKind::Unknown(_)` is absent and cannot be added: it means "a type this
/// build does not understand", [`NewItem`] has no variant for it, and there is
/// no form that could fill one in. The menu builds its rows from this array so
/// that the un-creatable kind is not merely rejected on save -- it is never
/// offered. See [`is_creatable`] for the same fact as a predicate.
pub const CREATABLE_KINDS: [ItemKind; 5] = [
    ItemKind::Login,
    ItemKind::SecureNote,
    ItemKind::Card,
    ItemKind::Identity,
    ItemKind::SshKey,
];

/// Whether an item of `kind` can be created by this form.
///
/// Exhaustive with no catch-all, as [`ItemKind`]'s own doc requires: a `_ =>`
/// here would quietly declare whatever Bitwarden ships next to be creatable
/// through a form that has no fields for it.
pub fn is_creatable(kind: ItemKind) -> bool {
    match kind {
        ItemKind::Login
        | ItemKind::SecureNote
        | ItemKind::Card
        | ItemKind::Identity
        | ItemKind::SshKey => true,
        ItemKind::Unknown(_) => false,
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

/// How one draft field becomes one modelled value: given what the item
/// currently holds (`None` when there is no item yet) and what the user typed,
/// what should be written.
///
/// A parameter rather than two copies of the conversion, because a *create*
/// and a *save* answer that question differently while walking exactly the
/// same fields. Two hand-written walks would be two places to add a field to,
/// and the one that got forgotten would fail silently -- a card whose brand
/// the edit form saves and the create form drops.
type FieldRule = fn(Option<&str>, &str) -> Option<String>;

/// The [`FieldRule`] for a **create**: whatever the user typed, verbatim,
/// including a blank.
///
/// There is no `current` to consult -- nothing exists yet -- and blank
/// handling is deliberately not this form's job: [`NewItem::to_payload`]
/// prunes empty values by one shared rule for every kind. Deciding it here as
/// well would put a second opinion in the codebase, and only the model's is on
/// the wire. Contrast [`edited`], which needs `current` precisely because a
/// save can tell "left blank" from "was already an empty string".
fn stated(_current: Option<&str>, draft: &str) -> Option<String> {
    Some(draft.to_string())
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
            totp: drafted(login.and_then(|l| l.totp.as_deref()).map(|t| t.as_str())),
            reveal_totp: false,
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
            // Blank, deliberately, and NOT read out of `item.other["sshKey"]`.
            // `apply_to` cannot write those keys back in this build (see
            // `SshKeyDraft`), so populating them would put a real private key
            // in a box whose contents are discarded on save -- the loudest
            // possible version of the silent-data-loss failure this file keeps
            // guarding against. `form_body` withholds the fields on an edit
            // for the same reason.
            ssh_key: SshKeyDraft::default(),
            note_body: drafted(item.notes.as_deref().map(|n| n.as_str())),
            generator: GeneratorDraft::default(),
            // Read through the SAME reader the detail pane's read-only card
            // uses, so the two panes can never disagree about whether an item
            // is bound or to what.
            app: crate::vault_bridge::extract_app_match(item)
                .map(|m| AppMatchDraft::from_match(&m)),
            // One walk, in the item's order, keeping every element --
            // including the ones this form will not draw. See
            // [`Self::fields`].
            fields: item.fields.iter().map(FieldDraft::from_field).collect(),
        }
    }

    /// A blank draft of the default kind (a login -- see [`Self::default`]).
    pub fn empty() -> Self {
        Self::default()
    }

    /// A blank draft of a **chosen** kind, for the "+ New" type menu.
    ///
    /// Accepts any [`ItemKind`], including `Unknown`, rather than a narrower
    /// "creatable kind" type: the menu offers only [`CREATABLE_KINDS`], and a
    /// draft that somehow held an un-creatable kind is already handled where
    /// it matters -- [`Self::to_new_item`] returns `None` and the form
    /// withholds Save. Adding a second kind enum to prevent a state the UI
    /// cannot reach would buy nothing and would have to be kept in step with
    /// `ItemKind` forever.
    pub fn empty_of(kind: ItemKind) -> Self {
        Self { kind, ..Self::default() }
    }

    /// Which kind this draft is for.
    pub fn kind(&self) -> ItemKind {
        self.kind
    }

    /// Switches the draft to another kind, **keeping the item-level fields**
    /// (name and folder, plus the recorded original folder) and clearing every
    /// kind-specific one.
    ///
    /// Name and folder survive because they belong to the item, not to its
    /// type: a user who has typed a name and picked a folder and then realises
    /// this is a card, not a login, has not changed their mind about either.
    /// Everything else is dropped because it belonged to a kind they are no
    /// longer creating, and carrying it forward is how the abandoned kind's
    /// data reaches the wire under the chosen one. The reveal flags go with
    /// it, so a form the user has just switched into is never already showing
    /// a secret.
    ///
    /// **A no-op when the kind is unchanged**, which is not an optimisation:
    /// an egui type menu re-states its current selection on every frame, so an
    /// unconditional clear would wipe the form as fast as it could be typed
    /// into.
    pub fn set_kind(&mut self, kind: ItemKind) {
        if self.kind == kind {
            return;
        }
        // Spelled out field by field rather than `..Self::empty_of(kind)`,
        // which would also reset `original_folder_id` -- and would keep
        // compiling, silently, if a new item-level field were added that
        // ought to have survived.
        self.kind = kind;
        self.username = String::new();
        self.password = String::new();
        self.reveal_password = false;
        self.totp = String::new();
        self.reveal_totp = false;
        self.card = CardDraft::default();
        self.identity = IdentityDraft::default();
        self.ssh_key = SshKeyDraft::default();
        self.note_body = String::new();
        // `fields` is deliberately NOT cleared, for exactly the reason `app`
        // below is not: a custom field can sit on an item of ANY type, so it
        // is item-level data like the name and the folder, and clearing it
        // here would make flicking the type menu silently delete the user's
        // custom fields.
        //
        // `app` is deliberately NOT cleared either, and for a different
        // reason from `generator`'s: it is not kind-specific data at all. A
        // `deskwarden:app-match` is a custom field on the ITEM, exactly as
        // `name` and `folder_id` are item-level, and clearing it here would
        // make switching the type menu silently unbind an app.
        //
        // `generator` is deliberately NOT cleared. Everything above is the
        // abandoned kind's *data*, and the argument for wiping it is that
        // carrying it forward puts one kind's contents on the wire under
        // another. The generator holds no item data at all -- it is two
        // numbers and a switch describing what the user wants their next
        // password to look like -- so there is nothing to leak, and resetting
        // it would undo a preference the user set moments earlier for a
        // reason unrelated to the kind.
    }

    /// What the Generate control should ask `bw serve` for, built from the
    /// form's own state plus this crate's default recipe for everything the
    /// form does not offer a control for.
    ///
    /// **The one conversion between the form and the wire**, so a control
    /// added to [`GeneratorDraft`] has exactly one place to be honoured and
    /// cannot half-arrive. The unoffered options are taken from
    /// `PasswordRecipe::default()` (all four character classes on, a minimum
    /// of one digit and one symbol, ambiguous characters avoided) and
    /// `PassphraseRecipe::default()` (a `-` separator, capitalised, with a
    /// number) -- see those types for why those defaults are Deskwarden's and
    /// not the CLI's weaker ones.
    ///
    /// The lengths are clamped here as well as in the widget. Not belt and
    /// braces: the route silently raises a too-small `length`/`words` to its
    /// own minimum, so an unclamped 1 would come back as a 5-character
    /// password against a form that said 1 -- a request that "succeeds" and
    /// ignores what was asked. Clamping means the form and the result agree.
    pub fn generator_request(&self) -> GenerateRequest {
        if self.generator.passphrase {
            GenerateRequest::Passphrase(PassphraseRecipe {
                words: self.generator.words.clamp(MIN_WORDS, MAX_WORDS),
                ..PassphraseRecipe::default()
            })
        } else {
            GenerateRequest::Password(PasswordRecipe {
                length: self.generator.length.clamp(MIN_LENGTH, MAX_LENGTH),
                ..PasswordRecipe::default()
            })
        }
    }

    /// Puts a freshly generated secret into the draft's password box.
    ///
    /// A named method rather than `draft.password = generated.to_string()` at
    /// the call site, because the call site is `vault_window/mod.rs` and this
    /// is where the rule about what a generate REPLACES belongs: the whole
    /// password, unconditionally, including one the user had already typed.
    /// (There is no "append" or "only if empty" reading of the button --
    /// generating a password over an empty box and over a typed one are the
    /// same gesture.)
    ///
    /// **The box stays masked.** Bitwarden's own generators reveal what they
    /// produced; this one does not, because `reveal_password` is the user's
    /// toggle and a generate silently flipping it would show a secret the
    /// user never asked to see -- on a form that may be sitting in front of
    /// other people. The Show control is beside the box and is one click.
    /// Stated because it is a deliberate deviation, not an oversight.
    ///
    /// Takes `&str` rather than the bridge's `Zeroizing<String>` so this file
    /// does not need to own one: the caller's `Zeroizing` still wipes on
    /// drop, and the copy that lands in `password` is the same plain `String`
    /// every other box on this form holds (`CardDraft`'s doc records why the
    /// draft's fields are not `Zeroizing` -- egui's `TextEdit` buffer is a
    /// plain `String` regardless).
    pub fn set_generated_password(&mut self, generated: &str) {
        self.password = generated.to_string();
    }

    /// A name is the one thing `bw serve`'s create/edit endpoints reject an
    /// empty item without -- everything else (blank username/password) is
    /// legitimate for e.g. a placeholder entry.
    pub fn is_valid(&self) -> bool {
        !self.name.trim().is_empty()
    }

    /// The keystroke template's refusal, or `None`.
    ///
    /// Separate from [`Self::is_valid`] rather than folded into it, because the
    /// two say different things and the strip says both: "Name is required." is
    /// about a box the user has not filled in, and this is about a string the
    /// user wrote that this build cannot read back. A single bool would have
    /// one caption for two faults.
    pub fn sequence_fault(&self) -> Option<&'static str> {
        self.app.as_ref().and_then(AppMatchDraft::template_fault)
    }

    /// **The save gate.** A name, a creatable kind (the caller's own check),
    /// and a template that parses.
    ///
    /// This is where the design's "a template that won't parse can't be saved"
    /// lives, and it is on the DRAFT rather than in the button, so it is a
    /// decision a test can call. See [`template_fault`] for what "won't parse"
    /// means against a parser that is total by design.
    pub fn is_saveable(&self) -> bool {
        self.is_valid() && self.sequence_fault().is_none()
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
                // Same rule, same helper, and therefore the same guarantee as
                // the password: a seed the form never showed a box for (a
                // CREATE -- see `TOTP_CREATE_NOTICE`) leaves `self.totp`
                // empty, and `edited` on an item that HAS a seed would then
                // read as "cleared". That is why the box is drawn on an edit
                // in both directions -- populated by `from_item`, written
                // back here -- and never half of the pair.
                login.totp = edited_secret(login.totp.as_deref().map(|t| t.as_str()), &self.totp);
                updated.login = Some(login);
            }
            ItemKind::Card => {
                let base = updated.card.take().unwrap_or_default();
                updated.card = Some(self.card_data(base, edited));
            }
            ItemKind::Identity => {
                let base = updated.identity.take().unwrap_or_default();
                updated.identity = Some(self.identity_data(base, edited));
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
            // SSH keys: there is no form that fills an `SshKeyDraft` on an
            // edit -- `form_body` gives an existing key
            // `FormBody::UneditableNotice` and `from_item` leaves the draft
            // blank -- so anything written here would be written from three
            // empty strings, wiping a real private key. The clone preserves
            // the key's data for exactly the reason it always did: *because
            // nothing here touches it*. Do not add an arm that writes it
            // without a form to fill it. This is why `kind_offers_edit` is
            // false for `SshKey` while `is_creatable` is true.
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

        // The custom fields. Item-level, like the name and the folder -- a
        // custom field belongs to no type object -- but written **here**,
        // after the `self.kind != ItemKind::of(item)` gate above, and
        // deliberately so: a draft that disagrees with the item about its
        // type is a draft built from a *different item*, and its field list
        // is that other item's. Writing it would not merely disturb the
        // fields, it would replace them. The gate leaves them alone, which is
        // the same answer the app binding below already gets in that state.
        //
        // **One walk, in the draft's own order.** Nothing sorts, partitions
        // or filters, because `fields` is a JSON array whose order Bitwarden
        // shows the user -- see [`Self::fields`]. The `deskwarden:app-match`
        // element rides through untouched and is then replaced *in place* by
        // `apply_app_match_to`, which is the order that keeps a bound item's
        // own fields where the user put them.
        updated.fields = self.fields.iter().map(FieldDraft::to_field).collect();

        self.apply_app_match_to(item, updated)
    }

    /// The app-binding half of [`Self::apply_to`], split out so the decision
    /// ([`app_match_edit`]) and the write can be pinned separately -- deleting
    /// this call is a mutation `EditDraft`'s own tests catch, and deleting the
    /// call to `app_match_edit` inside it is one `app_match_edit`'s tests
    /// cannot.
    ///
    /// **Applied for every kind**, after the kind's own object and outside the
    /// `self.kind != ItemKind::of(item)` early return above: a binding is a
    /// custom field, not a type object, and an item whose type the draft
    /// disagrees about is still an item whose binding the user just edited.
    ///
    /// Both writes go through `vault_bridge`, which is the one producer and the
    /// one remover of that field -- a hand-rolled `fields.push` here would be a
    /// second one, and would drop the server's extra keys on the field the way
    /// `with_app_match`'s own doc describes.
    fn apply_app_match_to(&self, item: &VaultItem, updated: VaultItem) -> VaultItem {
        match app_match_edit(
            crate::vault_bridge::extract_app_match(item).as_ref(),
            self.app.as_ref(),
        ) {
            AppMatchEdit::Leave => updated,
            AppMatchEdit::Write(m) => crate::vault_bridge::with_app_match(&updated, &m),
            AppMatchEdit::Remove => crate::vault_bridge::without_app_match(&updated),
        }
    }

    /// Puts a browsed-for path into the app block.
    ///
    /// A named method rather than `draft.app.as_mut().unwrap().set_path(..)` at
    /// the call site, for the reason [`Self::set_generated_password`] exists:
    /// the call site is `vault_window/mod.rs`, and the rule about what choosing
    /// a file changes (the path, and `process` derived from it -- see
    /// [`AppMatchDraft::set_path`]) belongs here.
    ///
    /// A no-op when there is no app block, which is the only honest answer: the
    /// dialog cannot have been opened from a form that was not drawing one, and
    /// inventing a binding out of a file choice would create a match the user
    /// never asked for.
    pub fn set_app_path(&mut self, path: &str) {
        if let Some(app) = self.app.as_mut() {
            app.set_path(path);
        }
    }

    /// This draft's card fields as a [`CardData`], built on top of `base`
    /// (what the item already holds; `CardData::default()` when there is no
    /// item yet) by `rule`.
    ///
    /// The **one** draft->`CardData` conversion in this file: both
    /// [`Self::apply_to`] and [`Self::to_new_item`] go through it, differing
    /// only in the [`FieldRule`] they pass. A struct literal rather than
    /// field-by-field mutation, so the compiler -- not a reviewer -- notices a
    /// field added to `CardData` and never wired to the form.
    ///
    /// `base.other` moves through untouched: [`VaultItem`]'s own catch-all
    /// cannot reach inside the card object, so this is the only thing keeping
    /// an unmodelled key Bitwarden puts there.
    fn card_data(&self, base: CardData, rule: FieldRule) -> CardData {
        let d = &self.card;
        CardData {
            cardholder_name: rule(base.cardholder_name.as_deref(), &d.cardholder_name),
            brand: rule(base.brand.as_deref(), &d.brand),
            // Re-wrapped in `Zeroizing` for the reason `edited_secret` exists:
            // the draft's plain `String` has no wipe-on-drop guarantee and the
            // model side keeps one.
            number: rule(base.number.as_deref().map(|n| n.as_str()), &d.number)
                .map(Zeroizing::new),
            exp_month: rule(base.exp_month.as_deref(), &d.exp_month),
            exp_year: rule(base.exp_year.as_deref(), &d.exp_year),
            code: rule(base.code.as_deref().map(|c| c.as_str()), &d.code).map(Zeroizing::new),
            other: base.other,
        }
    }

    /// This draft's identity fields as an [`IdentityData`]. See
    /// [`Self::card_data`] -- same contract, same reasons.
    fn identity_data(&self, base: IdentityData, rule: FieldRule) -> IdentityData {
        let d = &self.identity;
        IdentityData {
            title: rule(base.title.as_deref(), &d.title),
            first_name: rule(base.first_name.as_deref(), &d.first_name),
            middle_name: rule(base.middle_name.as_deref(), &d.middle_name),
            last_name: rule(base.last_name.as_deref(), &d.last_name),
            address1: rule(base.address1.as_deref(), &d.address1),
            address2: rule(base.address2.as_deref(), &d.address2),
            address3: rule(base.address3.as_deref(), &d.address3),
            city: rule(base.city.as_deref(), &d.city),
            state: rule(base.state.as_deref(), &d.state),
            postal_code: rule(base.postal_code.as_deref(), &d.postal_code),
            country: rule(base.country.as_deref(), &d.country),
            company: rule(base.company.as_deref(), &d.company),
            email: rule(base.email.as_deref(), &d.email),
            phone: rule(base.phone.as_deref(), &d.phone),
            ssn: rule(base.ssn.as_deref(), &d.ssn),
            username: rule(base.username.as_deref(), &d.username),
            passport_number: rule(base.passport_number.as_deref(), &d.passport_number),
            license_number: rule(base.license_number.as_deref(), &d.license_number),
            other: base.other,
        }
    }

    /// The create payload for this draft's kind, or `None` for the one kind
    /// that cannot be created.
    ///
    /// Exactly one type object, chosen by [`Self::kind`] and built from that
    /// kind's fields only -- the create-side counterpart of `apply_to`'s
    /// refusal to write more than one object. The draft holds every kind's
    /// fields (it is one struct so that the kind can change without losing the
    /// name and folder), so "build only this kind's" is a property of this
    /// match, and `changing_a_drafts_kind_keeps_the_shared_fields_and_leaks_no_others`
    /// is what holds it to that.
    ///
    /// **`Unknown` returns `None`, and that is the whole handling.** There is
    /// no `NewItem` variant for a type this build does not understand, and
    /// there is no honest substitute: returning a login or a note would create
    /// an item of the wrong type out of a form the user filled in for
    /// something else, silently and irreversibly-ish. The kind is not offered
    /// by the menu ([`CREATABLE_KINDS`]) and Save is withheld for it
    /// ([`is_creatable`]), so this is the third of three doors, not the first.
    ///
    /// Blanks are passed through verbatim -- see [`stated`].
    pub fn to_new_item(&self) -> Option<NewItem> {
        let name = self.name.clone();
        let folder_id = self.folder_id.clone();
        Some(match self.kind {
            ItemKind::Login => {
                NewItem::login(name, self.username.clone(), self.password.clone(), folder_id)
            }
            ItemKind::SecureNote => NewItem::secure_note(name, self.note_body.clone(), folder_id),
            ItemKind::Card => {
                let card = self.card_data(CardData::default(), stated);
                NewItem::card(name, card, folder_id)
            }
            ItemKind::Identity => {
                let identity = self.identity_data(IdentityData::default(), stated);
                NewItem::identity(name, identity, folder_id)
            }
            ItemKind::SshKey => NewItem::ssh_key(
                name,
                self.ssh_key.private_key.clone(),
                self.ssh_key.public_key.clone(),
                self.ssh_key.key_fingerprint.clone(),
                folder_id,
            ),
            ItemKind::Unknown(_) => return None,
        })
    }
}

/// The form's heading. Pure so the wording is asserted directly rather than
/// inferred from a screenshot; the old hardcoded "Edit login" was the
/// login-only era made visible, from when `kind_offers_edit` drew the read
/// pane's button for no other kind. It now draws it for every kind
/// [`EditDraft::apply_to`] writes, so the heading has to name them.
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

/// Which set of fields the form's body shows.
///
/// A pure decision, separate from drawing it, because the interesting case is
/// not "which fields" but *when*: an SSH key's three fields can be **created**
/// and cannot be **edited**, and nothing in this crate can click a widget to
/// check that (`draw_detail_edit` needs an egui context). Deciding it here
/// makes it assertable -- the same device `assignable_folders` uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FormBody {
    Login,
    Card,
    Identity,
    Note,
    SshKey,
    /// "Name and folder only." Shown for a kind whose contents this build
    /// cannot write back.
    UneditableNotice,
}

/// See [`FormBody`]. Exhaustive with no catch-all, as [`ItemKind`] requires.
fn form_body(kind: ItemKind, creating: bool) -> FormBody {
    match kind {
        ItemKind::Login => FormBody::Login,
        ItemKind::Card => FormBody::Card,
        ItemKind::Identity => FormBody::Identity,
        ItemKind::SecureNote => FormBody::Note,
        // The asymmetry, and the reason this function takes `creating`:
        // `NewItem::ssh_key` can POST all three keys, but `VaultItem` has no
        // `sshKey` field in this build, so an existing key's object rides the
        // `other` catch-all and `apply_to` deliberately leaves it alone.
        // Offering the fields on an edit would show three boxes whose
        // contents are silently discarded on save. When the `ssh-key-type`
        // branch lands and `apply_to` grows an arm that writes them, this
        // becomes `FormBody::SshKey` unconditionally -- and not before.
        ItemKind::SshKey if creating => FormBody::SshKey,
        ItemKind::SshKey => FormBody::UneditableNotice,
        // Nothing is known about this type, so there is nothing to offer in
        // either mode; `is_creatable` also withholds Save.
        ItemKind::Unknown(_) => FormBody::UneditableNotice,
    }
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
    /// The Generate control beside the password box was clicked.
    ///
    /// It carries nothing, and the caller is expected to ask
    /// [`EditDraft::generator_request`] what to send and
    /// [`EditDraft::set_generated_password`] where to put the answer. That
    /// shape is deliberate: this form cannot perform the request itself
    /// (`draw_detail_edit` has no backend handle, and it runs on the UI
    /// thread), but the *decisions* -- which recipe, and what a generate
    /// replaces -- belong here rather than being re-made at the call site.
    ///
    /// A failed generate is therefore the caller's to report, and it must be
    /// reported: the box is unchanged on failure, so a silently swallowed
    /// error looks exactly like a button that does nothing.
    GeneratePassword,
    /// The app block's "Browse..." was clicked.
    ///
    /// Carries nothing, exactly as [`Self::GeneratePassword`] does and for the
    /// same reason: the form cannot open the dialog itself. `IFileOpenDialog`
    /// is modal and runs its own message loop, so calling it from inside the
    /// frame closure would re-enter egui's; the caller runs
    /// [`crate::file_picker::pick_executable`] between frames and hands the
    /// answer back through [`EditDraft::set_app_path`].
    ///
    /// A cancelled dialog is `None` and nothing changes -- there is no failure
    /// to report, unlike a failed generate.
    PickAppFile,
    /// **4d.** "Rehearse with fake data" was clicked in the sequence block.
    ///
    /// Carries nothing, for the same reason [`Self::PickAppFile`] does and one
    /// more. The form cannot run it: the scratch window pumps its own message
    /// loop, so it must be opened between frames rather than inside one. And
    /// there is nothing for it to carry -- a rehearsal is built from the
    /// sequence alone by
    /// [`crate::vault_window::rehearsal::sample_plan`], which resolves every
    /// field to a fixed sample, so an item, a password or a one-time code
    /// passed through here would be a value the rehearsal has no use for and
    /// must not have.
    Rehearse,
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

/// The label above the login form's TOTP seed box.
///
/// It says **key**, not "code": the box holds the shared secret the
/// authenticator is built from, and a user who pastes a six-digit code into a
/// box labelled "TOTP" has broken their own two-factor login until they
/// notice.
pub const TOTP_LABEL: &str = "Authenticator key (TOTP)";

/// What the box is for, in the words the user would have been given by the
/// site they are setting up.
pub const TOTP_HINT: &str =
    "The secret behind the 6-digit codes \u{2014} an otpauth:// link, or the base32 key a site \
     shows beside its QR code. Not a code.";

/// What the row says while an item is being **created**.
///
/// `vault_bridge::NewItem::login` has no `totp`, so a seed typed into a
/// create form would be silently discarded on Save. Same reason, same
/// treatment and same wording pattern as [`FIELDS_CREATE_NOTICE`]; offering
/// it needs a `vault_bridge` change.
pub const TOTP_CREATE_NOTICE: &str = "Can be added once this item has been saved.";

// ---------------------------------------------------------------------------
// The edit form's custom-fields block.
//
// Same rule as the app block below: every decision is a pure function or a
// method on `FieldDraft`, and the drawing does nothing but obey it.
// ---------------------------------------------------------------------------

/// The block's heading. Sentence case, like [`APP_BLOCK_HEADING`].
pub const FIELDS_BLOCK_HEADING: &str = "Custom fields";

/// Shown when the item has no field this form draws a row for.
///
/// "no custom fields **of its own**", because an item can be carrying a
/// `deskwarden:app-match` and still show this line -- ours are not the
/// user's, and the app block above is where that one is edited.
pub const FIELDS_NONE_NOTICE: &str =
    "This item has no custom fields of its own yet. Add one to store anything Bitwarden has no \
     box for \u{2014} a PIN, a recovery code, an account number.";

/// The two Add buttons. Two, rather than one button and a type combo box,
/// because the choice is what the field IS and the user makes it once: a
/// combo box that defaults to text is a hidden field one forgotten click away
/// from being a visible one, and this form cannot change a field's type after
/// the fact (see [`FieldRole`]).
pub const FIELD_ADD_TEXT_BUTTON: &str = "Add a field\u{2026}";
pub const FIELD_ADD_HIDDEN_BUTTON: &str = "Add a hidden field\u{2026}";

/// A row's two labels, and its remove.
pub const FIELD_NAME_LABEL: &str = "Field name";
pub const FIELD_VALUE_LABEL: &str = "Value";
/// The hidden row's value label says so, because the masked box alone looks
/// exactly like a text box the user happens not to be able to read.
pub const FIELD_HIDDEN_VALUE_LABEL: &str = "Hidden value";
pub const FIELD_REMOVE_BUTTON: &str = "Remove field";

/// What a [`FieldRole::Preserved`] row says instead of offering boxes.
///
/// It is *listed*, not hidden: a field the form silently omitted would look
/// deleted, and the next thing the user would do is add it back by hand -- as
/// a text field, losing the type this notice exists to protect.
pub const FIELD_PRESERVED_NOTICE: &str =
    "Deskwarden cannot edit this kind of field and leaves it exactly as it is. Change it in the \
     Bitwarden web vault or app.";

/// What the block says while an item is being **created**.
///
/// `vault_bridge::NewItem` has no `fields` payload, so a field typed into a
/// create form would be silently discarded on Save -- the same failure
/// `form_body` withholds an SSH key's boxes on an *edit* to avoid, in the
/// other direction. Saying so is the honest form of not offering it.
pub const FIELDS_CREATE_NOTICE: &str =
    "Custom fields can be added once this item has been saved.";

/// The custom-fields block. Returns nothing: every effect it has is on
/// `fields`, which it owns for the duration of the call.
fn custom_fields_block(ui: &mut egui::Ui, fields: &mut Vec<FieldDraft>, creating: bool) {
    theme::hairline(ui);
    ui.add_space(10.0);
    theme::field_label(ui, FIELDS_BLOCK_HEADING);

    if creating {
        ui.label(RichText::new(FIELDS_CREATE_NOTICE).size(11.0).color(theme::TEXT_FAINT));
        ui.add_space(10.0);
        return;
    }

    // `FieldRole::Internal` rows are drawn by nothing -- see that variant --
    // so "has no fields" is about the rows the user can see, not about the
    // vector's length.
    if !fields.iter().any(|f| f.role != FieldRole::Internal) {
        ui.label(RichText::new(FIELDS_NONE_NOTICE).size(11.0).color(theme::TEXT_FAINT));
        ui.add_space(6.0);
    }

    // Collected, not removed inside the loop: removing while iterating by
    // index is the classic off-by-one.
    //
    // **Deferring is all that claim is worth**, and this comment used to
    // claim more -- that it also stopped "a shifted list handing one row the
    // next row's widget state for a frame". It never did: deferring the
    // removal to the end of the frame protects the iterator and nothing else,
    // and on every FOLLOWING frame each row below the removed one would have
    // answered to its predecessor's id. What actually protects the widget
    // state is that the `id_salt` below is [`FieldDraft::row_id`] and not `i`.
    let mut remove: Option<usize> = None;
    for (i, field) in fields.iter_mut().enumerate() {
        match field.role {
            FieldRole::Internal => continue,
            FieldRole::Preserved => {
                // Named by whatever the field calls itself, so the row is
                // identifiable; a nameless one says so rather than drawing a
                // blank label.
                theme::field_label(
                    ui,
                    if field.name.is_empty() { "(unnamed field)" } else { &field.name },
                );
                ui.label(
                    RichText::new(FIELD_PRESERVED_NOTICE).size(11.0).color(theme::TEXT_FAINT),
                );
                ui.add_space(10.0);
            }
            FieldRole::Text | FieldRole::Hidden => {
                let hidden = field.role == FieldRole::Hidden;
                theme::field_label(ui, FIELD_NAME_LABEL);
                // A scope of its own, because every row draws the same
                // widgets: two boxes and a button. Without one egui gives all
                // of them one id and the focus jumps between rows as the user
                // types.
                //
                // **`UiBuilder::id`, not `push_id`, and that is the whole
                // repair.** `push_id` asks for an id egui derives with
                // `IdSource::Child`, which mixes in the PARENT's
                // `next_auto_id_salt` -- a running count of the widgets the
                // parent has made so far. The heading, the notice and every
                // earlier row's `field_label` are made on that parent, so a
                // removal shifts that counter and every row below the removed
                // one gets a different id anyway, however stable its salt is.
                // Salting with `row_id` alone was measured doing exactly that:
                // the caret left the last row's box when the first row was
                // taken away. `UiBuilder::id` is `IdSource::Explicit`, which
                // egui uses verbatim.
                //
                // See [`FieldDraft::row_id`] and
                // `removing_a_field_row_leaves_the_rows_below_it_holding_their_own_state`.
                ui.scope_builder(
                    egui::UiBuilder::new()
                        .id(egui::Id::new(("custom-field", field.row_id()))),
                    |ui| {
                        theme::text_field(ui, &mut field.name, false);
                        ui.add_space(4.0);
                        theme::field_label(
                            ui,
                            if hidden { FIELD_HIDDEN_VALUE_LABEL } else { FIELD_VALUE_LABEL },
                        );
                        // A hidden field is a secret and gets the password box
                        // -- masked, with the same reveal toggle.
                        // Round-tripping it through a plain box would put it on
                        // screen for anyone standing behind the user, which is
                        // most of what "hidden" buys them.
                        if hidden {
                            theme::password_field(ui, &mut field.value, &mut field.reveal);
                        } else {
                            theme::text_field(ui, &mut field.value, false);
                        }
                        ui.add_space(4.0);
                        if theme::secondary_button(ui, FIELD_REMOVE_BUTTON).clicked() {
                            remove = Some(i);
                        }
                    },
                );
                ui.add_space(10.0);
            }
        }
    }
    if let Some(i) = remove {
        fields.remove(i);
    }

    // **Wrapped, not `horizontal`.** The two captions plus their padding are
    // wider than the card at the app's minimum window size, and an unwrapped
    // row does not shrink to fit -- it pushes the card past the pane and
    // inflates every `available_width()` measured after it. That is
    // `aae9429`'s defect, and the generator row above carries the same fix
    // for the same reason.
    ui.horizontal_wrapped(|ui| {
        if theme::secondary_button(ui, FIELD_ADD_TEXT_BUTTON).clicked() {
            fields.push(FieldDraft::new_of(FieldRole::Text));
        }
        if theme::secondary_button(ui, FIELD_ADD_HIDDEN_BUTTON).clicked() {
            fields.push(FieldDraft::new_of(FieldRole::Hidden));
        }
    });
    ui.add_space(10.0);
}

// ---------------------------------------------------------------------------
// The edit form's app block.
//
// The read pane's `MATCHED APP` card can only *show* a binding and remove it;
// this is where one is changed. Every decision it makes is a pure function
// above (`app_path_row`, `app_path_warning`, `window_row_refusal`,
// `AppMatchDraft::set_path`/`choose_window`, `app_match_edit`) and the drawing
// below does nothing but obey them -- this file's standing reason, stated in
// `detail.rs`: a decision reachable only through an egui closure is a decision
// no test can call.
// ---------------------------------------------------------------------------

/// The block's heading. Sentence case, matching every other `field_label` on
/// this form rather than the read pane's uppercase card headings -- this is a
/// group of fields in a form, not a card.
pub const APP_BLOCK_HEADING: &str = "Matched app";

const APP_PATH_LABEL: &str = "Program file";
const APP_ARGS_LABEL: &str = "Command-line arguments";

/// What the arguments box is for, said in the terms the user asked in.
///
/// Naming the browser-profile case explicitly because it is the case: the
/// same executable, twice, told apart only by this string.
const APP_ARGS_HINT: &str = "Passed to the program when Deskwarden opens it \u{2014} for example \
                             --profile-directory=\"Profile 2\" to pick a browser profile. Saved \
                             exactly as you type it.";

/// The arguments row for a Microsoft Store app. There is no command line to
/// pass: nothing is started by path (see [`AppPathRow`]).
const APP_ARGS_STORE_APP: &str = APP_PATH_STORE_APP;

const APP_WINDOW_LABEL: &str = "Window title";

/// The button that creates a binding on an item that has none.
///
/// **This is the control the form was missing.** Until it existed the app block
/// was drawn only inside `if let Some(app) = draft.app.as_mut()`, so the one
/// place a user goes to change what an item does -- Edit -- could edit a
/// binding and could remove a binding but could not make one. The only way to
/// bind an item was the tray's "Add app..." picker
/// (`picker_ui::run_picker`), a different window that writes straight to the
/// vault, so on the edit form the feature was simply absent.
pub const APP_ADD_BUTTON: &str = "Add an app\u{2026}";

/// What the block says while the item is bound to nothing.
///
/// Says what a binding IS rather than naming the mechanism, the same way
/// [`AppPathRow`]'s Store-app row refuses the word "hosted": the user's
/// question here is "what would this do for me", not "what field is unset".
pub const APP_NONE_NOTICE: &str =
    "Nothing is bound yet. Point this item at a program and Deskwarden can open it and type \
     into it.";

const APP_REMOVED_NOTICE: &str =
    "This app will stop filling from this item when you save. Undo, or Cancel the edit, to keep \
     it.";

// ---------------------------------------------------------------------------
// The keystroke sequence builder.
//
// The user's ask, in their words: "for auto-keystrokes we should show all vars
// available and also keys (Enter, tab...) and Wait N sec - so users can click
// the sequence not google everytime and wonder why not working because of
// typo". So: nothing here is typed from memory. Every field the item really
// has, every key, and a wait are buttons, and what they build is the portable
// string `key_sequence` defines.
//
// EVERY EDIT IS A PURE FUNCTION on `(&str, what was clicked) -> String`, tested
// directly. The drawing below only decides which button was pressed; a decision
// reachable only through an egui closure is a decision no test can call, which
// is this file's standing rule.
// ---------------------------------------------------------------------------

const APP_SEQUENCE_LABEL: &str = "Keystrokes";

/// Why this exists, in the terms of the case that motivates it. Multi-screen
/// sign-ins are named because they are what a plain user-name-Tab-password
/// fill cannot do, and "nothing happens" on such a page is exactly the symptom
/// a user would otherwise be left to guess at.
const APP_SEQUENCE_HINT: &str =
    "What Deskwarden types into this app. Add a wait and an Enter for sign-ins that ask for \
     the address on one screen and the password on the next.";

/// What the block says when the item stores no sequence. **It names the
/// default rather than showing an empty row**: the chips below it are real,
/// they are what would be typed, and a blank space where they sit would read
/// as "nothing will be typed", which is the one thing an empty sequence does
/// not mean (see [`key_sequence::DEFAULT_SEQUENCE`]).
const APP_SEQUENCE_DEFAULT_NOTICE: &str =
    "Default \u{2014} username, Tab, password. Add or remove a step to change it.";

/// The builder's two captions.
const APP_SEQUENCE_OPEN: &str = "Change what it types\u{2026}";
const APP_SEQUENCE_CLOSE: &str = "Done";

/// The shut block's one line: the steps, in words, in order.
///
/// **Named, never elided.** The pane is narrow and the honest way to fit a
/// long sequence into it is to let the label wrap; cutting it off with an
/// ellipsis would hide exactly the step a user came to check (`aae9429`'s
/// lesson, and the reason `assert_visible` compares glyphs with the source).
pub fn sequence_summary(sequence: &str) -> String {
    let view = sequence_view(sequence);
    let steps: Vec<String> = view.tokens.iter().map(|t| t.chip_label()).collect();
    let joined = steps.join(" \u{b7} ");
    if view.is_default {
        format!("{joined}  (default)")
    } else {
        joined
    }
}

/// The eye's two captions. The word says what the click does next, not what
/// the pane is doing now.
const APP_SEQUENCE_REVEAL: &str = "Show what it types";
const APP_SEQUENCE_HIDE: &str = "Hide what it types";

/// The wait box's starting value, in seconds. One second is the shortest wait
/// that is any use against a page navigation and the most common thing typed,
/// so the button works without touching the box at all.
const DEFAULT_WAIT_SECONDS: &str = "1";

/// What the eye shows about the item when the eye is shut. Deliberately not a
/// count of characters or a masked run of dots -- neither is information, and
/// a dotted run invites the reading that the dots are the password's length.
const APP_SEQUENCE_HIDDEN_NOTE: &str = "Values are hidden.";

/// The chips a sequence is drawn as, and whether they are the item's own or
/// the default standing in for a stored value it does not have.
///
/// One type rather than two returns, because the two facts must not be able to
/// disagree: the notice is shown exactly when the tokens came from the
/// default.
#[derive(Debug, Clone, PartialEq)]
pub struct SequenceView {
    pub tokens: Vec<key_sequence::Token>,
    /// The item stores no sequence, so these are [`key_sequence::DEFAULT_SEQUENCE`]'s.
    pub is_default: bool,
}

/// See [`SequenceView`].
pub fn sequence_view(sequence: &str) -> SequenceView {
    SequenceView {
        tokens: key_sequence::effective_tokens(sequence),
        is_default: sequence.is_empty(),
    }
}

/// The stored string after `tokens` replace whatever was there.
///
/// **An empty token list stores the empty string, which means the default
/// again -- not "type nothing".** That is the whole of the reason: this app has
/// no spelling for "type nothing", because that is not a fill anyone wants and
/// because the empty string is already spoken for by every item in every
/// existing vault. So a user who deletes every chip is put back where they
/// started, with the notice saying so, rather than silently given an item that
/// stops filling.
fn store(tokens: &[key_sequence::Token]) -> String {
    if tokens.is_empty() {
        String::new()
    } else {
        key_sequence::render(tokens)
    }
}

/// `sequence` with `token` added on the end.
///
/// Adding to an item that stores nothing **materialises the default first**,
/// so clicking `{TOTP}` on a fresh item gives username-Tab-password-code
/// rather than a sequence that types only the code. The alternative -- start
/// from nothing -- silently deletes the fill the item already had, on a click
/// whose caption said "add".
pub fn sequence_with(sequence: &str, token: key_sequence::Token) -> String {
    let mut tokens = key_sequence::effective_tokens(sequence);
    tokens.push(token);
    store(&tokens)
}

/// `sequence` with the chip at `index` gone. Out of range changes nothing.
pub fn sequence_without(sequence: &str, index: usize) -> String {
    let mut tokens = key_sequence::effective_tokens(sequence);
    if index >= tokens.len() {
        return sequence.to_string();
    }
    tokens.remove(index);
    store(&tokens)
}

/// `sequence` with the chip at `index` swapped with its neighbour.
///
/// A no-op at the ends, and it returns the input **unchanged** there rather
/// than a re-rendered copy of it: a click on a disabled arrow must not be able
/// to rewrite the spelling of a sequence this build merely carries.
pub fn sequence_moved(sequence: &str, index: usize, back: bool) -> String {
    let mut tokens = key_sequence::effective_tokens(sequence);
    let other = if back { index.checked_sub(1) } else { index.checked_add(1) };
    let Some(other) = other else { return sequence.to_string() };
    if index >= tokens.len() || other >= tokens.len() {
        return sequence.to_string();
    }
    tokens.swap(index, other);
    store(&tokens)
}

/// `sequence` with `text` added as literal characters, escaped so it is typed
/// as itself.
///
/// `None` when there is nothing to add, so the caller does not have to decide
/// whether an empty box is an edit -- and so a click on Add with an empty box
/// leaves the stored string byte-identical instead of re-rendering it.
pub fn sequence_with_literal(sequence: &str, text: &str) -> Option<String> {
    if text.is_empty() {
        return None;
    }
    Some(sequence_with(sequence, key_sequence::Token::Literal(text.to_string())))
}

/// `sequence` with a wait added, from the **seconds** the user typed.
///
/// `None` for anything [`key_sequence::wait_ms_from_seconds`] refuses, which
/// is what lets the button be disabled with the box saying why rather than a
/// click that appears to work and adds nothing.
pub fn sequence_with_wait(sequence: &str, seconds: &str) -> Option<String> {
    let ms = key_sequence::wait_ms_from_seconds(seconds)?;
    Some(sequence_with(sequence, key_sequence::Token::Delay(ms)))
}

// ---------------------------------------------------------------------------
// 4a -- the step list
//
// The design's premise: the steps are the editor, and the template string is a
// second view of the same steps. What follows is the step MODEL -- the rows,
// their badges, their payloads and the running tally -- derived purely from the
// stored string so every one of it is callable from a test rather than only
// reachable through a frame.
//
// ROWS ARE ONE PER TOKEN, and that is load-bearing: the up, down and delete
// controls hand their row's index straight to `sequence_moved` and
// `sequence_without`, which index the TOKEN list. A row model that folded two
// tokens into one row (which is what the design's picture does with
// `{DELAY=40}`) would make every index below the fold wrong. So a `{DELAY=n}`
// gets its own row AND is folded into the note of the text rows it governs --
// the design's reading is preserved without the indices lying.
// ---------------------------------------------------------------------------

/// The badge at the head of a step row: what kind of act the step is.
///
/// [`Self::Rate`] and [`Self::Raw`] are the two the design does not draw.
/// They exist because the token list can contain things that are not acts --
/// a typing-rate change, a grouping character, a construct from another
/// password manager -- and a row model with nowhere to put them would have to
/// either drop them (losing the user's string) or mislabel them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepKind {
    Key,
    Text,
    Wait,
    /// `{DELAY=n}` -- not an act, a change to the rate the rows below type at.
    Rate,
    /// Carried, not acted on. See [`SEQUENCE_UNKNOWN_TIP`].
    Raw,
}

impl StepKind {
    /// The badge caption. Short and upper-case so the column reads as a
    /// column, which is the whole of what the design asks of it.
    pub fn badge(self) -> &'static str {
        match self {
            Self::Key => "KEY",
            Self::Text => "TEXT",
            Self::Wait => "WAIT",
            Self::Rate => "RATE",
            Self::Raw => "RAW",
        }
    }
}

/// What a secret step's payload column shows instead of the secret.
///
/// **A FIXED number of dots, not the value's length.** The design draws
/// "•••••••••••• 20 chars", and this build deliberately does not: see
/// [`APP_SEQUENCE_HIDDEN_NOTE`], which settled the same question for the eye.
/// A dotted run whose length is the password's length tells anyone looking at
/// the screen how long the password is, which is the one fact about it that is
/// useful to an attacker and useless to the user.
pub const SECRET_MASK: &str = "\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}";

/// The note on a step whose payload is a secret. Says why the column is empty
/// of information rather than leaving the mask to be read as the value.
pub const SECRET_NOTE: &str = "hidden \u{2014} never shown here";

/// The note on a step that carries no note of its own. An em dash, which is
/// what the design draws in the same cell.
pub const NO_NOTE: &str = "\u{2014}";

/// One row of the step list.
///
/// **`payload` never holds a password.** The only branch that can put a
/// resolved value in it is gated on the field NOT being
/// [`FieldRef::Password`], and the password branch writes [`SECRET_MASK`]
/// unconditionally -- not "when hidden", unconditionally. There is no argument
/// to this function that turns that off, which is the difference between a
/// masked field and a field that happens to be masked right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepRow {
    /// 1-based, as drawn. `number - 1` is the token index the row's controls
    /// act on -- see the module comment above on why that identity holds.
    pub number: usize,
    pub kind: StepKind,
    /// What the step is, in the same words the chip used to say
    /// ([`Token::chip_label`]), so the two views of a step cannot drift.
    pub label: String,
    /// The value column: the mask for a secret, the resolved value for a
    /// revealed non-secret, and empty otherwise.
    pub payload: String,
    /// Whether `payload` is a mask standing in for something that is never
    /// drawn.
    pub secret: bool,
    pub note: String,
    /// Whether this build knows what the step is. `false` draws it faintly and
    /// says [`SEQUENCE_UNKNOWN_TIP`] on hover.
    pub understood: bool,
}

/// The typing-rate note carried by a text row, in the design's words.
fn rate_note(rate: Option<u32>) -> String {
    let ms = rate.unwrap_or_else(|| {
        u32::try_from(crate::injector::sequence::DEFAULT_RATE.as_millis()).unwrap_or(u32::MAX)
    });
    format!("{ms} ms/char")
}

/// The step list for `sequence`, as drawn.
///
/// `reveal` is the eye (`AppMatchDraft::previewing`): with it shut, a field row
/// names its field and shows no value at all. With it open, a non-secret field
/// shows what it would resolve to -- the same thing the preview line already
/// showed, in the row that will type it. **A password shows [`SECRET_MASK`] in
/// both states**, and a one-time code is treated as a secret too: it is a
/// credential, and the design's own rule is that secrets are masked.
pub fn step_rows(sequence: &str, source: &ResolveSource<'_>, reveal: bool) -> Vec<StepRow> {
    let tokens = sequence_view(sequence).tokens;
    let mut rate: Option<u32> = None;
    let mut rows = Vec::with_capacity(tokens.len());
    for (index, token) in tokens.iter().enumerate() {
        let (kind, payload, secret, note) = match token {
            Token::Literal(_) => (StepKind::Text, String::new(), false, rate_note(rate)),
            Token::Field(field) => {
                let secret = matches!(field, FieldRef::Password | FieldRef::Totp);
                let payload = if secret {
                    SECRET_MASK.to_string()
                } else if reveal {
                    resolved_value(field, source)
                } else {
                    String::new()
                };
                let note =
                    if secret { format!("{} \u{b7} {}", rate_note(rate), SECRET_NOTE) } else { rate_note(rate) };
                (StepKind::Text, payload, secret, note)
            }
            Token::Key(_) => (StepKind::Key, String::new(), false, NO_NOTE.to_string()),
            Token::Delay(_) => (StepKind::Wait, String::new(), false, NO_NOTE.to_string()),
            Token::DelayRate(ms) => {
                rate = Some(*ms);
                (StepKind::Rate, String::new(), false, RATE_NOTE.to_string())
            }
            Token::Modifier(_) => (StepKind::Key, String::new(), false, MODIFIER_NOTE.to_string()),
            Token::Grouping(_) | Token::Unknown(_) => {
                (StepKind::Raw, String::new(), false, NO_NOTE.to_string())
            }
        };
        rows.push(StepRow {
            number: index + 1,
            kind,
            label: token.chip_label(),
            payload,
            secret,
            note,
            understood: token.is_understood(),
        });
    }
    rows
}

/// The note on a `{DELAY=n}` row.
pub const RATE_NOTE: &str = "sets the typing speed from here on";

/// The note on a bare modifier, which is held for the key that follows it.
pub const MODIFIER_NOTE: &str = "held for the next key";

/// A non-secret field's value, for the revealed payload column.
///
/// **Never reached for a password**: [`step_rows`]'s only call site is inside
/// the `else` of a `matches!(field, FieldRef::Password | FieldRef::Totp)`. It
/// is written to answer honestly for the fields it does see, and the
/// unresolved cases come back as the same sentence
/// [`key_sequence::resolve_preview`] already uses, so the row and the preview
/// say one thing.
fn resolved_value(field: &FieldRef, source: &ResolveSource<'_>) -> String {
    let parts = key_sequence::resolve_preview(
        std::slice::from_ref(&Token::Field(field.clone())),
        source,
    );
    match parts.first() {
        Some(PreviewPart::Value(v)) => v.clone(),
        Some(PreviewPart::Unresolved(why)) => why.clone(),
        Some(PreviewPart::Pending) => "fetching\u{2026}".to_string(),
        _ => String::new(),
    }
}

// ---------------------------------------------------------------------------
// "N steps . T total" -- the running summary, and the budget
// ---------------------------------------------------------------------------

/// What the sequence adds up to, **asked of the runner's own plan**.
///
/// Not a second projection: [`injector::sequence::Step::projected`] is the one
/// answer to "how long does this take", and it is the same answer
/// [`injector::sequence::MAX_SEQUENCE`] and [`injector::sequence::MAX_BURST`]
/// are checked against at fill time. A count computed here would drift from the
/// thing that actually refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SequenceTally {
    /// The ACTS the sequence performs, which is neither the number of rows nor
    /// [`injector::sequence::Plan::len`].
    ///
    /// Not the rows, because a `{DELAY=n}` is a row and no act, and a run of
    /// literal text either side of a `{USERNAME}` is three rows and one.
    ///
    /// **And deliberately not `Plan::len` either**, which was the first
    /// implementation and was wrong on screen: `plan` splits a text run into
    /// [`injector::sequence::MAX_BURST`]-sized chunks so it can re-check the
    /// foreground between them, so a 21-character password typed at 40 ms/char
    /// is four `Step::Text`s. Those four are one thing the user did and one
    /// row in the list; reporting "9 steps" for the six-row sequence in the
    /// design would be counting an implementation detail of the runner's
    /// safety check. The DURATION still comes from the plan, because there the
    /// chunking makes no difference to the sum.
    pub steps: usize,
    pub total: Duration,
    /// The longest single step, against [`injector::sequence::MAX_BURST`].
    pub burst: Duration,
}

/// [`SequenceTally`] for `sequence`, or `None` when the runner would refuse it
/// outright (in which case [`sequence_warning`] is already saying why).
///
/// The [`Plan`](crate::injector::sequence::Plan) is read and dropped inside
/// this function, and its `Drop` wipes the plaintext it copied -- the same care
/// [`sequence_refusal`] takes, for the same reason.
pub fn sequence_tally(sequence: &str, source: &ResolveSource<'_>) -> Option<SequenceTally> {
    let tokens = sequence_view(sequence).tokens;
    let values = crate::injector::sequence::Resolved {
        username: source.username,
        password: source.password,
        totp: edit_time_totp(source.totp),
        custom: source.custom.clone(),
    };
    let plan = crate::injector::sequence::plan(&tokens, &values).ok()?;
    let tally = SequenceTally {
        steps: acts(&tokens),
        total: plan.projected(),
        burst: plan
            .steps()
            .iter()
            .map(crate::injector::sequence::Step::projected)
            .max()
            .unwrap_or(Duration::ZERO),
    };
    Some(tally)
}

/// How many ACTS `tokens` describe. See [`SequenceTally::steps`].
///
/// A run of adjacent text-producing tokens is one act, a key (with whatever
/// modifiers precede it) is one, a pause is one, and a rate change is none.
/// The same grouping [`injector::sequence::Step`] makes, taken BEFORE the
/// burst chunking rather than after it.
fn acts(tokens: &[Token]) -> usize {
    let mut count = 0;
    let mut typing = false;
    for token in tokens {
        match token {
            Token::Literal(_) | Token::Field(_) => {
                if !typing {
                    count += 1;
                    typing = true;
                }
            }
            Token::Key(_) | Token::Delay(_) => {
                count += 1;
                typing = false;
            }
            // Held for the key that follows, which is the act.
            Token::Modifier(_) => typing = false,
            // Not an act at all -- it changes how the acts below type.
            Token::DelayRate(_) => {}
            Token::Grouping(_) | Token::Unknown(_) => typing = false,
        }
    }
    count
}

/// A duration in the words the design uses: milliseconds under a second, one
/// decimal of a second above it. Never "0 s" for a real pause -- see
/// [`key_sequence::wait_label`], which made the same call for the same reason.
pub fn duration_label(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        format!("{ms} ms")
    } else {
        format!("{}.{} s", ms / 1000, (ms % 1000) / 100)
    }
}

/// The design's "6 steps \u{b7} 2.1 s" line.
pub fn tally_label(tally: &SequenceTally) -> String {
    let unit = if tally.steps == 1 { "step" } else { "steps" };
    format!("{} {unit} \u{b7} {} total", tally.steps, duration_label(tally.total))
}

/// The design's budget pair, **against the crate's real limits** rather than
/// the illustrative 512/64 in the picture.
pub fn budget_label(tally: &SequenceTally) -> String {
    format!(
        "{} of {} total \u{b7} {} of {} in one burst",
        duration_label(tally.total),
        duration_label(crate::injector::sequence::MAX_SEQUENCE),
        duration_label(tally.burst),
        duration_label(crate::injector::sequence::MAX_BURST),
    )
}

/// What the summary says when the runner refuses the sequence outright, so the
/// line is never blank. The reason itself is already above, in
/// [`sequence_warning`]'s words.
pub const TALLY_REFUSED: &str = "won't run \u{2014} see above";

// ---------------------------------------------------------------------------
// 4c -- the template view, and the bridge
// ---------------------------------------------------------------------------

/// The two captions of the Steps/Template toggle.
pub const VIEW_STEPS: &str = "Steps";
pub const VIEW_TEMPLATE: &str = "Template";

/// The insert chips, **in the grammar `key_sequence` really parses**.
///
/// The design's picture writes a pause as `{WAIT=250}`. There is no such
/// construct: [`key_sequence::parse`] reads a pause as `{DELAY 250}` (a space)
/// and a typing rate as `{DELAY=50}` (an equals sign), and a `{WAIT=250}` would
/// come back as [`Token::Unknown`] and be refused at fill time. The template
/// view's entire premise is that the string in the box IS the stored string, so
/// a second spelling here would be a chip that writes a sequence this build
/// cannot run.
///
/// The design's `{SHIFT+TAB}` is wrong the same way and for a different
/// reason: a chord in this grammar is a MODIFIER CHARACTER before a key, so
/// Shift+Tab is `+{TAB}`. (`{CTRL+A}`, which the design's own example opens
/// with, has no spelling at all here -- `^` before a literal letter is
/// [`injector::sequence::Refusal::DanglingModifier`], and there is no `A` in
/// [`key_sequence::KEYS`] -- so it is not offered rather than offered broken.)
pub const TEMPLATE_CHIPS: &[&str] = &[
    "{USERNAME}",
    "{PASSWORD}",
    "{TOTP}",
    "{TAB}",
    "+{TAB}",
    "{ENTER}",
    "{DELAY 250}",
    "{DELAY=50}",
];

/// `template` with `chip` added on the end.
///
/// Inserting into an EMPTY template **materialises the default first**, for
/// exactly the reason [`sequence_with`] does: an empty stored value means
/// [`key_sequence::DEFAULT_SEQUENCE`], and a chip click that silently replaced
/// that whole fill with one token would be a delete wearing an add's caption.
pub fn template_with(template: &str, chip: &str) -> String {
    if template.is_empty() {
        format!("{}{chip}", key_sequence::DEFAULT_SEQUENCE)
    } else {
        format!("{template}{chip}")
    }
}

/// The refusal shown under an unparseable template, and the reason Save is off.
pub const TEMPLATE_UNPARSED: &str =
    "This template has a `{` that is never closed, so the steps below are not what it says. \
     Close the brace, or write `{{}` to type a brace.";

/// Whether `template` is a string this build can hold without changing it.
///
/// **What "will not parse" means against a parser that cannot fail.**
/// [`key_sequence::parse`] is total by design -- an unknown `{WHATEVER}` rides
/// through as [`Token::Unknown`] so a sequence from another password manager is
/// never destroyed by being looked at. That deliberate totality means "parse
/// returned an error" is not available as the test, and inventing one would
/// mean refusing to save exactly the foreign sequences `AppMatch::sequence`
/// exists to carry.
///
/// The honest test is the ROUND TRIP: a template is well formed when
/// `render(parse(t)) == t`, i.e. when the step list under the field really is
/// the string in the field. The one shape that fails it is an unterminated `{`
/// -- which parses as literal text and renders back with the brace escaped, so
/// the string the user is looking at and the string that would be typed differ.
/// That is precisely the case the design says cannot be saved, and it is the
/// only one, so nothing well-formed is caught by it.
pub fn template_fault(template: &str) -> Option<&'static str> {
    if template.is_empty() {
        return None;
    }
    let tokens = key_sequence::parse(template);
    (key_sequence::render(&tokens) != template).then_some(TEMPLATE_UNPARSED)
}

/// The note under an empty template box. Names the default rather than leaving
/// a blank field reading as "types nothing" -- the same correction
/// [`APP_SEQUENCE_DEFAULT_NOTICE`] makes on the steps side.
pub const TEMPLATE_EMPTY_NOTE: &str =
    "Empty means the default: {USERNAME}{TAB}{PASSWORD}. Type a template to change it.";

/// The line above the parsed step list in the template view.
pub const TEMPLATE_READS_AS: &str = "Reads as";

/// The field buttons this form offers, for the item **as it is being edited**.
///
/// The user name and the password come from the DRAFT rather than the item,
/// and that is the point: typing a user name into the form above and then
/// looking for a `{USERNAME}` button should find one, and an item whose
/// password is being cleared should stop offering to type it. The one-time
/// code and the custom fields come from the item, because this form does not
/// edit either -- so the item is the only thing that knows about them, and
/// `{S:PIN}` is discoverable exactly when a field called `PIN` really exists.
///
/// `None` for a create: there is no item yet, so there are no custom fields
/// and no TOTP secret to name, and the two boxes on this very form are the
/// whole of what can be referenced.
pub fn sequence_palette(draft: &EditDraft, item: Option<&VaultItem>) -> Vec<FieldRef> {
    let mut out = Vec::new();
    if !draft.username.is_empty() {
        out.push(FieldRef::Username);
    }
    if !draft.password.is_empty() {
        out.push(FieldRef::Password);
    }
    if let Some(item) = item {
        for field in key_sequence::field_palette(item) {
            match field {
                // The draft has already answered for these two, and it is the
                // more current answer.
                FieldRef::Username | FieldRef::Password => {}
                other if !out.contains(&other) => out.push(other),
                _ => {}
            }
        }
    }
    out
}

/// What the preview resolves against: the draft's own boxes for the two values
/// this form edits, the item for everything else, and the vault window's one
/// [`detail::TotpState`] for the code.
///
/// **Borrowed, for the length of one frame.** Nothing here is copied and
/// nothing is stored; see [`PreviewPart`].
/// The two draft strings are passed separately rather than as `&EditDraft`
/// **so the borrow checker can split them from `draft.app`**, which the block
/// that draws the sequence holds mutably at the same moment. A whole-struct
/// borrow here would make the preview and the builder mutually exclusive.
pub fn sequence_source<'a>(
    username: &'a str,
    password: &'a str,
    item: Option<&'a VaultItem>,
    totp: &'a detail::TotpState,
) -> ResolveSource<'a> {
    ResolveSource {
        username,
        password,
        custom: item.map(key_sequence::custom_pairs).unwrap_or_default(),
        totp,
    }
}

// ---------------------------------------------------------------------------
// Will this sequence run at all?
// ---------------------------------------------------------------------------

/// The one-time code the edit-time check plans against, when the item has a
/// secret at all.
///
/// Six zeroes, never the live code. [`injector::sequence::plan`] reads exactly
/// two things about a code: whether it is EMPTY (which is the refusal) and how
/// many characters it is (which feeds the projected-time bound). Every code
/// this app can fetch is six digits, so a six-character stand-in answers both
/// questions the same way the real one would -- and keeps a secret out of a
/// check that runs on every frame the edit form is drawn.
const TOTP_STAND_IN: &str = "000000";

/// What [`sequence_refusal`] hands [`injector::sequence::plan`] as the code.
///
/// **The whole of the editor's TOTP judgement, and deliberately narrow.**
/// [`detail::TotpState::NoSecret`] is derived from the item every frame -- it
/// means this item has no TOTP secret, so no fill will ever resolve `{TOTP}`,
/// and that is knowable now. The other four states are all about *this
/// moment*: the poll is in flight, the poll reported nothing, the bridge is
/// unavailable, or a code is in hand. None of those is a property of the item
/// the user is editing, so warning about them would be telling the user to fix
/// something that is not broken. They get the stand-in, and no warning.
fn edit_time_totp(totp: &detail::TotpState) -> Option<&'static str> {
    match totp {
        detail::TotpState::NoSecret => None,
        detail::TotpState::Fetching
        | detail::TotpState::NoCodeReported
        | detail::TotpState::Unavailable
        | detail::TotpState::Code { .. } => Some(TOTP_STAND_IN),
    }
}

/// The refusal this sequence would meet at fill time, asked of the runner
/// itself.
///
/// **This calls [`injector::sequence::plan`]. It does not re-derive one rule
/// of it.** That is the whole design: `plan` is the only place that decides
/// whether a sequence can be typed, and a second copy of "an unknown token is
/// refused, a grouping character is refused, a modifier before text is
/// refused" living in the editor would drift from it on the first token this
/// build learns to type. The only thing this function does is *build the
/// values* `plan` needs, which is exactly what the fill path does too -- see
/// [`edit_time_totp`] for the one value that is a stand-in and why.
///
/// The [`Plan`](injector::sequence::Plan) on the success path is dropped
/// immediately, and its `Drop` wipes the plaintext it copied.
pub fn sequence_refusal(
    tokens: &[Token],
    source: &ResolveSource<'_>,
) -> Option<crate::injector::sequence::Refusal> {
    let values = crate::injector::sequence::Resolved {
        username: source.username,
        password: source.password,
        totp: edit_time_totp(source.totp),
        custom: source.custom.clone(),
    };
    crate::injector::sequence::plan(tokens, &values).err()
}

/// The sentence shown over the keystroke block when the sequence cannot run.
///
/// The refusal's own words after the prefix, not a paraphrase: `Refusal`
/// already names the offending construct ("uses {PICKCHARS}, which this build
/// cannot type"), and the editor saying it differently from the notification
/// the user gets at fill time would be two vocabularies for one fact.
const SEQUENCE_REFUSED_PREFIX: &str = "This will not run \u{2014} ";

/// The warning line for `sequence`, or `None` when it would type fine.
///
/// Asked of [`sequence_view`]'s tokens, not of `parse`: an item that stores no
/// sequence is filled with [`key_sequence::DEFAULT_SEQUENCE`], so a login with
/// no user name really would refuse, and the empty string is exactly the case
/// where the user has never opened this block to see why.
pub fn sequence_warning(sequence: &str, source: &ResolveSource<'_>) -> Option<String> {
    let refusal = sequence_refusal(&sequence_view(sequence).tokens, source)?;
    Some(format!("{SEQUENCE_REFUSED_PREFIX}{}", refusal.message()))
}

/// The running-window list, shown under the buttons while `picking`.
///
/// **In the form, not a second window.** The tray's `picker_ui::run_picker`
/// opens its own `eframe` loop on `main`'s thread, and the vault window is a
/// blocking call on that same thread -- which is exactly why the read pane's
/// card only *names* the tray flow instead of routing to it. eframe cannot nest
/// event loops, so raising the tray picker from here would deadlock. What is
/// reusable is the part that matters: `window_list::list_windows`, the one
/// enumeration, reached through [`running_app_rows`]. The list is drawn with
/// this form's own widgets, which costs a scroll area and buys a picker that
/// cannot hang the window.
fn app_window_picker(ui: &mut egui::Ui, app: &mut AppMatchDraft) {
    ui.horizontal(|ui| {
        if theme::secondary_button(ui, "Refresh").clicked() {
            app.windows = running_app_rows();
        }
        if theme::secondary_button(ui, "Close list").clicked() {
            app.picking = false;
        }
    });
    ui.add_space(6.0);
    theme::text_field(ui, &mut app.window_filter, false);
    ui.add_space(4.0);

    let filter = app.window_filter.to_lowercase();
    let mut chosen: Option<usize> = None;
    egui::ScrollArea::vertical()
        .id_salt("edit-app-window-picker")
        .max_height(180.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            // Matches the window title as well as the executable name, the same
            // way the tray picker's search does -- "chrome" and "Google Chrome"
            // should find the same row.
            for index in 0..app.windows.len() {
                let row = &app.windows[index];
                if !filter.is_empty()
                    && !row.title.to_lowercase().contains(&filter)
                    && !row.exe_name.to_lowercase().contains(&filter)
                {
                    continue;
                }
                let refusal = window_row_refusal(row);
                let label = format!("{}  \u{b7}  {}", row.title, row.exe_name);
                let response =
                    ui.add_enabled(refusal.is_none(), egui::Button::new(label).wrap());
                if let Some(why) = refusal {
                    // On the row itself, not only in a tooltip: a disabled row
                    // with no visible reason is the silent no-op again.
                    ui.label(RichText::new(why).size(11.0).color(theme::TEXT_FAINT));
                } else if response.clicked() {
                    chosen = Some(index);
                }
            }
            if app.windows.is_empty() {
                ui.label(
                    RichText::new("No open windows to choose from.")
                        .size(12.0)
                        .color(theme::TEXT_FAINT),
                );
            }
        });
    // Applied after the loop, so the immutable borrow of `app.windows` above is
    // over before the row is copied into the draft.
    if let Some(index) = chosen {
        let row = app.windows[index].clone();
        app.choose_window(&row);
    }
}

/// One chip: the step, and the three controls that move and remove it.
///
/// **In a wrapped row, never a scrolled one.** The pane refuses horizontal
/// scrolling (see `assert_inside`), so a chip row that ran off the right edge
/// would put steps somewhere the user cannot click -- the very defect
/// `aae9429` fixed for the app card. `horizontal_wrapped` is the whole
/// mechanism, and the minimum-size test below is what holds it.
///
/// Returns the edit the click asked for, applied by the caller after the loop
/// so the borrow of the token list is over first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChipEdit {
    Back(usize),
    Forward(usize),
    Remove(usize),
}

/// The badge's ink. One accent hue: the understood kinds wear the blue, and
/// the two that are carried rather than acted on wear the faint ink -- the
/// same distinction the chips drew, said in colour rather than in fill.
fn badge_ink(row: &StepRow) -> egui::Color32 {
    match row.kind {
        _ if !row.understood => theme::TEXT_FAINT,
        StepKind::Rate | StepKind::Raw => theme::TEXT_MUTED,
        _ => theme::BLUE,
    }
}

/// The step list: one card-shaped row per token, with the controls that move
/// and remove it.
///
/// **A row per token and a control set per row**, which is what makes the
/// index a row hands to [`sequence_moved`] the index that function means. The
/// captions are still `<`, `>` and `x` and still sit to the RIGHT of the row's
/// label, because that is the geometry `control_beside` in the tests reads --
/// and because an arrow glyph the app's Latin text face has no coverage for
/// draws as a box (see [`small_chip_button`]).
///
/// `editable` is false in the template view, where the list is the read-out of
/// what the string became rather than the thing being edited.
///
/// Returns the one edit clicked, applied by the caller after the loop so the
/// borrow of the row list is over first.
fn sequence_steps(ui: &mut egui::Ui, rows: &[StepRow], editable: bool) -> Option<ChipEdit> {
    let mut edit = None;
    for row in rows {
        egui::Frame::new()
            .fill(if row.secret { theme::CARD_TINT } else { theme::CARD })
            .stroke(Stroke::new(1.0, theme::HAIRLINE))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(Margin::symmetric(8, 5))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                // **The note gets its own line, and that is a layout fact
                // rather than a taste.** The pane is 298pt at the app's
                // minimum size; a note like "40 ms/char . hidden -- never
                // shown here" is wider than that on its own, so put on the
                // same wrapped row as the index and the badge it wraps to a
                // multi-line galley that egui lays out ACROSS the row's other
                // runs -- `no_two_runs_on_the_tallest_edit_form_overlap`
                // caught exactly that, with the note painted over the index.
                // Line one is the step; line two is what to know about it.
                ui.vertical(|ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(6.0, 3.0);
                    ui.label(
                        RichText::new(row.number.to_string()).size(11.0).color(theme::TEXT_GHOST),
                    );
                    ui.label(theme::semibold(row.kind.badge(), 10.0).color(badge_ink(row)));
                    let label = ui.label(
                        theme::semibold(row.label.clone(), 12.0).color(if row.understood {
                            theme::INK
                        } else {
                            theme::TEXT_FAINT
                        }),
                    );
                    if !row.understood {
                        label.on_hover_text(SEQUENCE_UNKNOWN_TIP);
                    }
                    if !row.payload.is_empty() {
                        // A mask is not a value and must not be drawn like
                        // one: the accent ink is reserved for something the
                        // user can actually read.
                        ui.label(RichText::new(row.payload.clone()).size(12.0).color(
                            if row.secret { theme::TEXT_MUTED } else { theme::BLUE },
                        ));
                    }
                    if editable {
                        let index = row.number - 1;
                        if ui.add_enabled(index > 0, small_chip_button("<")).clicked() {
                            edit = Some(ChipEdit::Back(index));
                        }
                        if ui
                            .add_enabled(row.number < rows.len(), small_chip_button(">"))
                            .clicked()
                        {
                            edit = Some(ChipEdit::Forward(index));
                        }
                        if ui.add(small_chip_button("x")).clicked() {
                            edit = Some(ChipEdit::Remove(index));
                        }
                    }
                });
                ui.label(RichText::new(row.note.clone()).size(11.0).color(theme::TEXT_FAINT));
                });
            });
        ui.add_space(4.0);
    }
    edit
}

/// The Steps / Template toggle. Returns the view asked for, or `None`.
///
/// Two buttons rather than a segmented control, because egui has no segmented
/// control and a painted one would be hit-testing built by hand for no gain --
/// the state is already visible in which of the two is drawn as the current
/// one.
fn view_toggle(ui: &mut egui::Ui, template_view: bool) -> Option<bool> {
    let mut asked = None;
    ui.horizontal(|ui| {
        for (caption, is_template) in [(VIEW_STEPS, false), (VIEW_TEMPLATE, true)] {
            let current = template_view == is_template;
            let button = egui::Button::new(theme::semibold(caption, 11.0).color(if current {
                theme::INK
            } else {
                theme::TEXT_MUTED
            }))
            .fill(if current { theme::BLUE_WASH } else { theme::CARD })
            .stroke(Stroke::new(1.0, if current { theme::BLUE_EDGE } else { theme::BORDER }))
            .corner_radius(CornerRadius::same(7));
            if ui.add(button).clicked() && !current {
                asked = Some(is_template);
            }
        }
    });
    asked
}

/// The tip on a step this build carries but does not understand.
const SEQUENCE_UNKNOWN_TIP: &str =
    "Deskwarden does not know this step. It is kept exactly as it is so another password \
     manager can still read it.";

/// The move/remove controls. ASCII captions on purpose: the app's own font is
/// a Latin text face, and an arrow glyph it has no coverage for is a control
/// that draws as a box.
fn small_chip_button(caption: &str) -> egui::Button<'static> {
    egui::Button::new(theme::semibold(caption.to_string(), 11.0).color(theme::TEXT_FAINT))
        .fill(theme::CARD)
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(CornerRadius::same(5))
}

/// The resolved preview. Draws and drops -- see [`PreviewPart`].
fn sequence_preview(ui: &mut egui::Ui, parts: &[PreviewPart]) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(2.0, 3.0);
        for part in parts {
            let (text, color) = match part {
                PreviewPart::Text(t) => (t.clone(), theme::INK),
                PreviewPart::Value(v) => (v.clone(), theme::BLUE),
                // In brackets, so a key cannot be misread as the characters
                // of its own name sitting in the typed text.
                PreviewPart::Key(symbol) => (format!("[{symbol}]"), theme::TEXT_FAINT),
                PreviewPart::Wait(label) => (format!("[{label}]"), theme::TEXT_FAINT),
                PreviewPart::Unresolved(why) => (why.clone(), theme::ERROR),
                PreviewPart::Pending => ("[fetching code]".to_string(), theme::TEXT_FAINT),
                PreviewPart::Opaque(raw) => (raw.clone(), theme::TEXT_FAINT),
            };
            ui.label(RichText::new(text).size(12.0).color(color));
        }
    });
}

/// The keystroke builder: the chips, the palette, and the eye.
///
/// Every branch here is a call to one of the pure functions above. What is
/// left in this function is which button was pressed.
fn app_sequence_block(
    ui: &mut egui::Ui,
    app: &mut AppMatchDraft,
    palette: &[FieldRef],
    source: &ResolveSource<'_>,
) -> Option<EditAction> {
    theme::field_label(ui, APP_SEQUENCE_LABEL);
    let view = sequence_view(&app.sequence);

    // **Above the fold, and before the block splits.** A sequence that the
    // runner will refuse outright is not a detail of the builder -- it is the
    // fact that this binding does nothing at all, and the builder is SHUT by
    // default. Drawn here it is on screen in both states, so the user who
    // never opens the block still finds out before the fill silently does
    // nothing. `theme::ERROR`, the same ink the form's own "Name is required."
    // wears, because it is the same kind of statement: this will not work, and
    // here is the thing to change.
    if let Some(warning) = sequence_warning(&app.sequence, source) {
        ui.label(RichText::new(warning).size(11.0).color(theme::ERROR));
        ui.add_space(4.0);
    }

    // Shut, this is three lines: what would be typed, said in words, and the
    // way in. See `AppMatchDraft::sequence_open`.
    if !app.sequence_open {
        ui.label(
            RichText::new(sequence_summary(&app.sequence)).size(11.0).color(theme::TEXT_FAINT),
        );
        ui.add_space(4.0);
        if theme::secondary_button(ui, APP_SEQUENCE_OPEN).clicked() {
            app.sequence_open = true;
        }
        ui.add_space(10.0);
        return None;
    }

    ui.label(RichText::new(APP_SEQUENCE_HINT).size(11.0).color(theme::TEXT_FAINT));
    ui.add_space(6.0);

    if view.is_default {
        ui.label(
            RichText::new(APP_SEQUENCE_DEFAULT_NOTICE).size(11.0).color(theme::TEXT_FAINT),
        );
        ui.add_space(4.0);
    }

    // -- the tally, and the view toggle ------------------------------------
    //
    // Both above the list, because both are statements ABOUT the list: how
    // much of it there is, and which of its two spellings is on screen.
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 4.0);
        let summary = match sequence_tally(&app.sequence, source) {
            Some(tally) => tally_label(&tally),
            None => TALLY_REFUSED.to_string(),
        };
        ui.label(theme::semibold(summary, 11.0).color(theme::TEXT_SECONDARY));
        if let Some(wants_template) = view_toggle(ui, app.template_view) {
            // **Seeded verbatim, every time the view is entered.** Not
            // `render`ed, not normalised: the box holds the bytes the item
            // holds, so a user who opens the template view and closes it again
            // has changed nothing at all. See `AppMatchDraft::template_draft`.
            if wants_template {
                app.template_draft = app.sequence.clone();
            }
            app.template_view = wants_template;
        }
    });
    ui.add_space(6.0);

    if app.template_view {
        app_template_view(ui, app, source);
    } else if let Some(edit) = sequence_steps(ui, &step_rows(&app.sequence, source, app.previewing), true)
    {
        app.sequence = match edit {
            ChipEdit::Back(i) => sequence_moved(&app.sequence, i, true),
            ChipEdit::Forward(i) => sequence_moved(&app.sequence, i, false),
            ChipEdit::Remove(i) => sequence_without(&app.sequence, i),
        };
    }
    ui.add_space(8.0);

    if let Some(tally) = sequence_tally(&app.sequence, source) {
        ui.label(RichText::new(budget_label(&tally)).size(11.0).color(theme::TEXT_GHOST));
        ui.add_space(6.0);
    }

    // The palette below belongs to the step list. In the template view the
    // insert chips are the palette, and a second set of Add buttons writing to
    // a string the user is editing by hand would fight the cursor.
    if app.template_view {
        let mut action = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
            if rehearse_button(ui).clicked() {
                action = Some(EditAction::Rehearse);
            }
            if theme::secondary_button(ui, APP_SEQUENCE_CLOSE).clicked() {
                app.sequence_open = false;
                app.previewing = false;
            }
        });
        ui.add_space(10.0);
        return action;
    }

    // -- the palette: nothing here is typed from memory --------------------
    ui.label(RichText::new("Add a value").size(11.0).color(theme::TEXT_FAINT));
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
        for field in palette {
            if ui.add(palette_button(&field.label())).clicked() {
                app.sequence = sequence_with(&app.sequence, Token::Field(field.clone()));
            }
        }
        if palette.is_empty() {
            ui.label(
                RichText::new("This item has no fields to reference yet.")
                    .size(11.0)
                    .color(theme::TEXT_FAINT),
            );
        }
    });
    ui.add_space(6.0);

    ui.label(RichText::new("Add a key").size(11.0).color(theme::TEXT_FAINT));
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
        for key in key_sequence::KEYS.iter().filter(|k| k.palette) {
            if ui.add(palette_button(key.label)).clicked() {
                app.sequence = sequence_with(&app.sequence, Token::Key(key));
            }
        }
    });
    ui.add_space(6.0);

    // -- literal text: `2134{TOTP}` is why this box exists ------------------
    ui.label(RichText::new("Add text").size(11.0).color(theme::TEXT_FAINT));
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
        ui.add(egui::TextEdit::singleline(&mut app.literal_draft).desired_width(120.0));
        let add = theme::secondary_button(ui, "Add text");
        if add.clicked() {
            // Escaping is this app's job, not the user's: they typed a `+`,
            // they get a `+`. See `key_sequence::escape_literal`.
            if let Some(next) = sequence_with_literal(&app.sequence, &app.literal_draft) {
                app.sequence = next;
                app.literal_draft.clear();
            }
        }
    });
    ui.add_space(6.0);

    // -- the wait, in the seconds the user asked for -----------------------
    ui.label(RichText::new("Add a wait").size(11.0).color(theme::TEXT_FAINT));
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
        ui.add(egui::TextEdit::singleline(&mut app.wait_draft).desired_width(48.0));
        ui.label(RichText::new("seconds").size(11.0).color(theme::TEXT_FAINT));
        let addable = key_sequence::wait_ms_from_seconds(&app.wait_draft).is_some();
        if ui.add_enabled(addable, egui::Button::new("Add wait")).clicked() {
            if let Some(next) = sequence_with_wait(&app.sequence, &app.wait_draft) {
                app.sequence = next;
            }
        }
        if !addable {
            ui.label(RichText::new(WAIT_REFUSAL).size(11.0).color(theme::TEXT_FAINT));
        }
    });
    ui.add_space(8.0);

    // -- the rehearsal, and the eye -----------------------------------------
    //
    // 4d's own control, in the one column this pane has rather than in the
    // design's right-hand rail, and FIRST in the row deliberately: rehearsing
    // is the safe way to find out what a sequence does, and the eye beside it
    // is the one that puts a real password on screen.
    let mut action = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
        if rehearse_button(ui).clicked() {
            action = Some(EditAction::Rehearse);
        }
        let caption = if app.previewing { APP_SEQUENCE_HIDE } else { APP_SEQUENCE_REVEAL };
        if theme::secondary_button(ui, caption).clicked() {
            app.previewing = !app.previewing;
        }
        if !view.is_default && theme::secondary_button(ui, "Use the default").clicked() {
            // The empty string, not the default's own spelling: see `store`.
            app.sequence = String::new();
        }
        if theme::secondary_button(ui, APP_SEQUENCE_CLOSE).clicked() {
            app.sequence_open = false;
            // Shutting the builder shuts the eye with it. A reveal that
            // survived being scrolled past and came back open later would be
            // a reveal the user did not ask for the second time.
            app.previewing = false;
        }
    });
    ui.add_space(4.0);
    if app.previewing {
        sequence_preview(ui, &key_sequence::resolve_preview(&view.tokens, source));
    } else {
        ui.label(RichText::new(APP_SEQUENCE_HIDDEN_NOTE).size(11.0).color(theme::TEXT_FAINT));
    }
    ui.add_space(10.0);
    action
}

/// 4d's own caption, and the sentence under it.
///
/// The caption names the fake data because that is the whole promise: this
/// button types, and what it types is not the user's password. A caption that
/// said only "Rehearse" would be a button nobody dares press on an item that
/// matters.
/// **The caption is the whole promise**, and it is on the button rather than in
/// a sentence beside it. 4d puts an explanatory line under the control ("types
/// sample-user and not-a-real-password into a scratch window..."); this pane is
/// one narrow column and the form is already at the height
/// `no_two_runs_on_the_tallest_edit_form_overlap` measures, so a second row of
/// prose pushed the eye and Close below the fold. What could not be dropped is
/// the word that makes the button safe to press on an item that matters, so
/// that is what the caption says.
pub const APP_SEQUENCE_REHEARSE: &str = "Rehearse with fake data";

fn rehearse_button(ui: &mut egui::Ui) -> egui::Response {
    theme::secondary_button(ui, APP_SEQUENCE_REHEARSE)
}

/// **4c -- the template view.** The same sequence as one editable line, with
/// the step list it parses to underneath it.
///
/// The bridge is one assignment: what the user types IS the stored string.
/// There is no render step between the box and `app.sequence`, which is what
/// makes the round trip byte-exact in both directions -- a template that is
/// merely looked at leaves the item untouched, and a template that is edited
/// stores the user's own bytes rather than this build's spelling of them.
fn app_template_view(ui: &mut egui::Ui, app: &mut AppMatchDraft, source: &ResolveSource<'_>) {
    // Multiline, because a sequence with a wait and a rate in it is longer
    // than the pane is wide and the pane refuses horizontal scrolling
    // (`assert_inside`). Wrapped text is readable; a line running off the
    // right edge is a template whose end the user cannot see.
    let response = ui.add(
        egui::TextEdit::multiline(&mut app.template_draft)
            .desired_rows(2)
            .desired_width(f32::INFINITY)
            .font(egui::TextStyle::Monospace),
    );
    if response.changed() {
        app.template_touched = true;
        app.sequence = app.template_draft.clone();
    }
    ui.add_space(4.0);

    if app.template_draft.is_empty() {
        ui.label(RichText::new(TEMPLATE_EMPTY_NOTE).size(11.0).color(theme::TEXT_FAINT));
        ui.add_space(4.0);
    }

    // **The refusal, in the field that caused it.** Save is off while this is
    // on screen -- see `EditDraft::sequence_fault`.
    if let Some(fault) = app.template_fault() {
        ui.label(RichText::new(fault).size(11.0).color(theme::ERROR));
        ui.add_space(4.0);
    }

    ui.label(RichText::new("Insert").size(11.0).color(theme::TEXT_FAINT));
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
        for chip in TEMPLATE_CHIPS {
            if ui.add(palette_button(chip)).clicked() {
                app.template_draft = template_with(&app.template_draft, chip);
                app.template_touched = true;
                app.sequence = app.template_draft.clone();
            }
        }
    });
    ui.add_space(8.0);

    // The parsed steps, under the field, read-only. The design's whole point:
    // the string is never the only thing on screen, so the user always sees
    // what it became.
    ui.label(RichText::new(TEMPLATE_READS_AS).size(11.0).color(theme::TEXT_FAINT));
    ui.add_space(4.0);
    let _ = sequence_steps(ui, &step_rows(&app.sequence, source, false), false);
}

/// Save's caption while the keystroke template will not parse. Names the thing
/// to go and fix, the same way "Save (needs a name)" does.
pub const SAVE_TEMPLATE_BLOCKED: &str = "Save (fix the template)";

/// The refusal under the wait box, said as the rule rather than as "invalid".
const WAIT_REFUSAL: &str = "Type a number of seconds, up to 3600.";

fn palette_button(label: &str) -> egui::Button<'static> {
    egui::Button::new(theme::semibold(label.to_string(), 12.0).color(theme::INK))
        .fill(theme::CARD)
        .stroke(Stroke::new(1.0, theme::BORDER_STRONG))
        .corner_radius(CornerRadius::same(7))
        .wrap()
}

/// The app block's other state: **the item is bound to nothing yet**.
///
/// Returns `true` on the frame the user asks for a binding. The caller then
/// puts an [`AppMatchDraft::unbound`] on the draft and every later frame draws
/// [`app_block`] -- the same block, with the same boxes, the same running-app
/// list and the same Remove, because a binding made here has to be the same
/// thing a binding read off an item is.
///
/// **Three widgets and no more.** The obvious alternative -- open the
/// running-app list immediately, so Add is one click instead of two -- was
/// rejected twice over: it would enumerate every top-level window on the
/// desktop (an `EnumWindows` walk with four Win32 calls per window, see
/// [`AppMatchDraft::windows`]) as a side effect of merely *drawing* a form the
/// user opened to rename something, and it would put a 180pt scrolled list
/// into the middle of a form whose scroll viewport is already the thing three
/// separate commits in this file have pushed a control out of. The user clicks
/// Add, and then chooses.
fn app_add_block(ui: &mut egui::Ui) -> bool {
    theme::hairline(ui);
    ui.add_space(10.0);
    theme::field_label(ui, APP_BLOCK_HEADING);
    ui.label(RichText::new(APP_NONE_NOTICE).size(11.0).color(theme::TEXT_FAINT));
    ui.add_space(6.0);
    let asked = theme::secondary_button(ui, APP_ADD_BUTTON).clicked();
    ui.add_space(10.0);
    asked
}

/// The whole app block. Returns an [`EditAction`] when it needs the caller to
/// do something the form cannot (open the file dialog).
fn app_block(
    ui: &mut egui::Ui,
    app: &mut AppMatchDraft,
    apps: &mut AppIdentityCache,
    palette: &[FieldRef],
    source: &ResolveSource<'_>,
) -> Option<EditAction> {
    let mut action = None;
    theme::hairline(ui);
    ui.add_space(10.0);
    theme::field_label(ui, APP_BLOCK_HEADING);

    if !app.bound {
        ui.label(RichText::new(APP_REMOVED_NOTICE).size(12.0).color(theme::TEXT_FAINT));
        ui.add_space(6.0);
        if theme::secondary_button(ui, "Undo remove").clicked() {
            app.bound = true;
        }
        ui.add_space(10.0);
        return action;
    }

    // Resolved ONCE per path by the cache, off this thread, and copied out
    // here so the borrow of `apps` (and of `app.process`) ends before the
    // boxes below take `app` mutably.
    let (name, icon, pending) = {
        let label = apps.label(ui.ctx(), &app.path, &app.process);
        (label.name.to_string(), label.icon.cloned(), label.pending)
    };
    if pending {
        // A channel is not input, and egui does not repaint for one.
        ui.ctx().request_repaint_after(AppIdentityCache::POLL_INTERVAL);
    }
    ui.horizontal(|ui| {
        if let Some(texture) = &icon {
            ui.add(egui::Image::new(texture).fit_to_exact_size(egui::vec2(18.0, 18.0)));
        }
        // The APP's name -- "Google Chrome", not "chrome.exe". See
        // `app_identity`.
        ui.label(theme::semibold(name, 14.0).color(theme::INK));
    });
    ui.add_space(8.0);

    if app.hosted && !app.title.is_empty() {
        // Read-only, because it is not a setting: it is what the frame was
        // called when the app was captured, and it is the only thing that can
        // identify a suspended Store app. Typing over it would be typing a new
        // identity for something that is not there to check it against.
        theme::disabled_field_label(ui, APP_WINDOW_LABEL);
        theme::disabled_text_field(ui, &app.title);
        ui.add_space(10.0);
    }

    theme::field_label(ui, APP_PATH_LABEL);
    let path_row = app_path_row(app.hosted);
    match path_row {
        AppPathRow::Editable => {
            if theme::text_field(ui, &mut app.path, false).changed() {
                // `process` is re-derived on every keystroke, which is what
                // keeps `launchable_path`'s file-name tie-back satisfiable --
                // see `AppMatchDraft::set_path`.
                let typed = app.path.clone();
                app.set_path(&typed);
            }
        }
        AppPathRow::NotApplicable(text) => {
            theme::disabled_text_field(ui, text);
        }
    }
    ui.add_space(6.0);

    ui.horizontal(|ui| {
        if theme::secondary_button(ui, "Choose a running app\u{2026}").clicked() {
            app.picking = !app.picking;
            if app.picking {
                // Enumerated on OPEN, never per frame.
                app.windows = running_app_rows();
            }
        }
        let browse = egui::Button::new("Browse\u{2026}");
        if ui
            .add_enabled(matches!(path_row, AppPathRow::Editable), browse)
            .clicked()
        {
            action = Some(EditAction::PickAppFile);
        }
    });

    if app.picking {
        ui.add_space(6.0);
        app_window_picker(ui, app);
    }

    if let Some(warning) = app_path_warning(&app.to_match()) {
        ui.add_space(4.0);
        ui.label(RichText::new(warning).size(11.0).color(theme::TEXT_FAINT));
    }
    ui.add_space(10.0);

    theme::field_label(ui, APP_ARGS_LABEL);
    match path_row {
        AppPathRow::Editable => {
            theme::text_field(ui, &mut app.args, false);
        }
        // A Store app is not started by path, so there is no command line to
        // give it. Disabled for the same reason the path box is, and saying so
        // in the same words.
        AppPathRow::NotApplicable(_) => {
            theme::disabled_text_field(ui, APP_ARGS_STORE_APP);
        }
    }
    ui.add_space(4.0);
    ui.label(RichText::new(APP_ARGS_HINT).size(11.0).color(theme::TEXT_FAINT));
    ui.add_space(10.0);

    // **No autofill control here, deliberately.** What a matched foreground
    // window does is one global preference -- `settings::Settings::
    // prompt_on_match` -- and nothing in this build reads `AppMatch::trigger`.
    // A per-item Auto / Prompt / Hotkey choice was a control that wrote a
    // field nothing reads: it persisted, it superseded the item's
    // `revisionDate` to record itself, and it changed nothing the user could
    // observe. The FIELD is still carried through this form untouched (see
    // `AppMatchDraft::trigger`), because v0.5.0 cannot parse an `AppMatch`
    // that lacks it.
    //
    // The sequence block answers what the pills used to sit above: not *when*
    // this item fills, which is no longer a per-item question, but what it
    // types once it does.
    if let Some(asked) = app_sequence_block(ui, app, palette, source) {
        action = Some(asked);
    }

    // Staged, not immediate: unlike the read pane's card -- which writes
    // straight through because there is no Save to wait for -- this is one
    // change among several on a form the user can still Cancel. Nothing is
    // written until Save, and `AppMatchDraft::bound` keeps the fields so the
    // block can still say what is going.
    if theme::secondary_button(ui, "Remove app match").clicked() {
        app.bound = false;
        app.picking = false;
    }
    ui.add_space(10.0);

    action
}

/// The lane reserved down the right-hand edge of the scrolling form for its
/// scroll bar, so the bar is drawn BESIDE the card rather than on top of it.
///
/// 10pt, which is `item_list.rs`'s `LIST_PADDING` -- the same lane width that
/// list already ships, leaving 4pt of clear space between the card and a
/// [`theme::SCROLLBAR_WIDTH`] bar sitting flush to the lane's outer edge,
/// and nothing behind the bar. See `theme::scrollbar_in_gutter` for why
/// that 4pt is the floor and 10 is unreachable while the bar is showing.
///
/// **Where it comes from is the difference from the two siblings.** In
/// `item_list.rs` and `detail.rs` the lane REPLACES a right padding those
/// functions own, so the content keeps the width it always had. This function
/// owns no such padding: the edit pane's horizontal inset is the vault
/// window's central-panel `Margin`, applied outside the `Ui` handed in here.
/// So the lane is taken out of the form's own width instead, and the card is
/// 10pt narrower than it was. That is a deliberate trade -- 10pt of card
/// against a scroll bar the user can see -- and not a width that CHANGES:
/// `AlwaysVisible` below reserves the lane whether or not a bar is painted.
const FORM_SCROLL_GUTTER: f32 = 10.0;

/// The id under which the edit form's "did it overflow last frame?" reading
/// is kept.
fn form_overflow_id() -> egui::Id {
    egui::Id::new("detail-edit-form-overflow")
}

/// Whether the edit form's content was taller than its viewport the last time
/// it was drawn -- i.e. whether the scroll bar has anything to say.
///
/// Read back from the last frame rather than predicted: this form's height is
/// the sum of however many fields its kind draws, plus a conditional app
/// block and a wrapping hint, and there is no row-count-times-pitch to
/// compute it from the way `item_list.rs` has. The one frame of lag can only
/// show a bar for a frame on a form that turns out to fit -- never hide one
/// that is needed for longer than that.
///
/// Absent (the first frame this pane is ever drawn) answers TRUE, for the
/// same reason as `detail.rs`'s read-side twin: a bar shown on a form that
/// fits is gone next frame, whereas a bar hidden on a form that really does
/// scroll tells the user there is nothing below -- which is the report.
fn form_overflowed(ctx: &egui::Context) -> bool {
    ctx.data(|data| data.get_temp::<bool>(form_overflow_id())).unwrap_or(true)
}

/// Records this frame's reading for [`form_overflowed`] to use on the next.
fn note_form_overflow(ctx: &egui::Context, overflowed: bool) {
    ctx.data_mut(|data| data.insert_temp(form_overflow_id(), overflowed));
}

pub fn draw_detail_edit(
    ui: &mut egui::Ui,
    draft: &mut EditDraft,
    folders: &[Folder],
    creating: bool,
    apps: &mut AppIdentityCache,
    // `item` is the item being edited, or `None` while one is being created.
    // It is read for exactly two things -- the custom fields the keystroke
    // palette offers, and the ones its preview resolves -- because this form
    // does not edit either and the item is the only thing that knows them.
    // Everything else on screen still comes from the draft.
    item: Option<&VaultItem>,
    // The vault window's ONE `detail::TotpState`, so the keystroke preview can
    // show what `{TOTP}` would type. Not a second poll: this form cannot fetch
    // a code and does not try.
    totp: &detail::TotpState,
) -> EditAction {
    let mut action = EditAction::None;
    // Read before the closure borrows `draft` mutably.
    let may_unfile = draft.may_unfile();

    ui.label(theme::bold(form_title(draft.kind, creating), 19.0).color(theme::INK));
    ui.add_space(12.0);

    // A create of a kind `NewItem` cannot express has no payload at all (see
    // `EditDraft::to_new_item`), so Save is withheld rather than left to
    // produce nothing when clicked. Unreachable through the "+ New" menu,
    // which offers only `CREATABLE_KINDS` -- this is the backstop, and it says
    // why instead of doing nothing quietly. An *edit* of such an item is
    // fine: the name and folder still save.
    let creatable = !creating || is_creatable(draft.kind);

    // The action strip is drawn BEFORE the form, as a bottom panel, and that
    // order is the whole fix. This form was one plain `Ui` with no scroll area
    // at any level, so on a window shorter than the form -- which commit
    // `4b05adb`'s app block made an ordinary login on an ordinary window --
    // Save and Cancel were laid out past the bottom of the pane, painted
    // nowhere, and reachable by nothing. There was no way to save an edit.
    //
    // `Panel::bottom` rather than the alternatives, because it is the
    // only one that measures what it holds:
    //
    //   * `ui.available_height() - <a button's height>` is the obvious fix and
    //     the trap. The strip's height is not constant -- the two error labels
    //     below are conditional, the row wraps on a narrow pane, and a third
    //     button one day would add another line. Any constant is wrong in some
    //     state, and wrong here means the buttons go back off-screen.
    //   * Wrapping the whole function in a `ScrollArea` scrolls the buttons
    //     away with everything else, which is the bug restated.
    //   * `Layout::bottom_up` works, but the strip has to be written in
    //     reverse -- buttons first, then the errors that belong above them --
    //     and a later edit that appends a label in source order would silently
    //     put it under the buttons.
    //
    // The panel takes its natural height off the bottom whatever that height
    // is, and the `ScrollArea` below then gets exactly the rest. The title
    // stays outside both, so it does not scroll away either -- see
    // `edit_pane_layout_tests`, which pins all three facts as geometry.
    egui::Panel::bottom("detail-edit-actions")
        // The strip is part of the pane, not a docked tool window: the pane's
        // own card already carries the only edge this form draws.
        .show_separator_line(false)
        .frame(
            egui::Frame::new()
                .fill(theme::CANVAS)
                // Replaces the `ui.add_space(12.0)` that used to separate the
                // strip from the card; the card's own bottom edge is now the
                // scrolled content's, and this is the gap above the buttons.
                .inner_margin(Margin { top: 12, ..Margin::ZERO }),
        )
        .show(ui, |ui| {
            if !creatable {
                ui.label(
                    RichText::new(
                        "Deskwarden does not know this item type and cannot create one. Create \
                         it in the Bitwarden web vault or app.",
                    )
                    .size(12.0)
                    .color(theme::ERROR),
                );
                ui.add_space(6.0);
            }
            if !draft.is_valid() {
                ui.label(RichText::new("Name is required.").size(12.0).color(theme::ERROR));
                ui.add_space(6.0);
            }
            // Repeated here, beside the button it disables. The same sentence
            // is also drawn under the template box that caused it (see
            // `app_template_view`), because the box can be scrolled off the
            // top of a long form and a Save that is off for no visible reason
            // is the silent no-op this file keeps refusing to ship.
            if let Some(fault) = draft.sequence_fault() {
                ui.label(RichText::new(fault).size(12.0).color(theme::ERROR));
                ui.add_space(6.0);
            }

            ui.horizontal(|ui| {
                // `min_size`, and the SAME height Cancel beside it gets from
                // `theme::secondary_button`. Measured before this line
                // existed: Save 26pt tall, Cancel 32, both starting at the
                // strip's top, so Save's bottom edge stopped 6pt short of its
                // neighbour's -- the "save button is off" the user reported.
                // `theme::BUTTON_HEIGHT` and not 32, because it is the same
                // height for the same reason as Cancel's; a literal here is
                // one the next design change moves only half of.
                //
                // Only the height. Save is NOT run through
                // `theme::secondary_button`: that helper is `ui.add`, and
                // this button is `add_enabled` (it is disabled while the name
                // is empty, or the kind is not creatable). Its filled default
                // look is also what distinguishes the primary action from the
                // outlined Cancel, and egui's `add_enabled` is what greys it
                // -- see `the_disabled_save_button_does_not_look_enabled`.
                // The zero x floor leaves the width to the label, which
                // changes with the validity ("Save" / "Save (needs a name)").
                let save = egui::Button::new(if !draft.is_valid() {
                    "Save (needs a name)"
                } else if draft.sequence_fault().is_some() {
                    SAVE_TEMPLATE_BLOCKED
                } else {
                    "Save"
                })
                .min_size(egui::Vec2::new(0.0, theme::BUTTON_HEIGHT));
                if ui.add_enabled(draft.is_saveable() && creatable, save).clicked() {
                    action = EditAction::Save;
                }
                if theme::secondary_button(ui, "Cancel").clicked() {
                    action = EditAction::Cancel;
                }
            });
        });

    // `auto_shrink([false; 2])`: the area must take the full width the pane
    // gives it (the card inside sets its own width from `available_width`) and
    // the full height left over, so a short form does not leave the strip
    // floating in the middle of the pane.
    //
    // **A `scope`, not this `ui` directly.** The two calls below configure the
    // scroll bar by mutating the style of the `Ui` they are given, and this
    // `Ui` belongs to the caller in `vault_window/mod.rs`. A child's style is
    // its own clone (`Ui::spacing_mut` goes through `Arc::make_mut`), so
    // scoping keeps the settings from outliving the form -- the property
    // `item_list.rs` and `detail.rs` both get for free by already drawing into
    // a child.
    let scrolled = ui
        .scope(|ui| {
            // **The fix.** The area shown here was egui's default FLOATING
            // bar with no gutter: a 1.2pt sliver at the pane's extreme right,
            // painted ON TOP of the form card, which only widens once the
            // pointer is already on it. Commit `68f86cb` made the form
            // scrollable but left nothing on screen to say so, which is half
            // of the report it answered ("cannot scroll"). Same lane, and the
            // same reason, as `item_list.rs`'s list and `detail.rs`'s body -- and,
            // since the rule moved into the helper, the same PLACEMENT: the bar takes
            // the outermost `theme::SCROLLBAR_WIDTH` of the lane rather than sitting
            // centred in it, so all 4pt of the 10pt lane's slack falls between the bar
            // and the card instead of 2pt there and 2pt behind the bar.
            theme::scrollbar_in_gutter(ui, FORM_SCROLL_GUTTER);
            // ... and the bar is hidden outright when there is nothing to
            // scroll: a full-height 6pt bar down a form that cannot move is
            // an affordance that lies. The lane stays reserved either way --
            // that is what `AlwaysVisible` below is for, and why the card
            // keeps ONE width whether or not the bar is showing. `092da70`
            // measured a real 10pt jump when that reservation was left
            // conditional.
            if !form_overflowed(ui.ctx()) {
                theme::hide_scrollbar(ui);
            }
            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                // Required by `scrollbar_in_gutter`: the lane is only reserved
                // for a bar egui is actually showing, so anything conditional
                // here puts the width jump back.
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
                .show(ui, |ui| {
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
            match form_body(draft.kind, creating) {
                FormBody::Login => {
                    theme::field_label(ui, "Username");
                    theme::text_field(ui, &mut draft.username, false);
                    ui.add_space(10.0);

                    theme::field_label(ui, "Password");
                    theme::password_field(ui, &mut draft.password, &mut draft.reveal_password);
                    ui.add_space(6.0);
                    // The generator, in this form's own idiom rather than
                    // design block 3d's. 3d is the OVERLAY's generator -- a
                    // full panel with its own chrome, character-class
                    // switches and a re-roll -- and it lives in a file this
                    // work does not own. Porting its chrome into a stacked
                    // label/field form would look like a foreign panel
                    // dropped into the middle of it. So: one control row
                    // under the box it fills, built from the same widgets
                    // every other row here uses. **The overlay's generator is
                    // a separate, still-outstanding task.**
                    //
                    // **Wrapped, not `horizontal`.** This row is the one place
                    // in the whole form whose content has a floor rather than
                    // a share: "Generate", a 110pt combo box and a
                    // `DragValue` wide enough for " chars" come to 279.4pt of
                    // content, and the card at the app's MINIMUM window size
                    // offers 264. An unwrapped row does not shrink to fit --
                    // it pushes the card out to 307pt inside a 298pt pane,
                    // and every `available_width()` measured after it answers
                    // with the inflated number. That is `aae9429`'s defect
                    // exactly, and `horizontal_wrapped` is the same mechanism
                    // that already holds the keystroke builder's chip row
                    // (see `every_chip_and_button_is_reachable_at_the_apps_minimum_width`).
                    ui.horizontal_wrapped(|ui| {
                        // **Why the row sets `interact_size.y`, and why it is
                        // `BUTTON_HEIGHT` rather than a number.** The three
                        // controls here are the only place in this form where
                        // widgets of three different KINDS stand side by side,
                        // and left to themselves they came out three different
                        // heights sitting at three different tops: the button
                        // 32pt (its `min_size`, from `theme::secondary_button`),
                        // the `DragValue` and the combo 26pt each -- and the
                        // combo lower still than the spinner, because
                        // `ComboBox` wraps itself in a nested `ui.horizontal`
                        // whose `button_frame` starts from
                        // `available_rect_before_wrap`, so it is not centred in
                        // the row the way a directly-added widget is. That is
                        // the "the dropdown sits lower than the buttons" the
                        // user reported.
                        //
                        // `interact_size.y` is the one dial all three read:
                        // `Button` takes it as a floor, `DragValue` passes it
                        // to `min_size`, and `ComboBox::button_frame` raises
                        // its outer rect to `at_least(interact_size.y)`. Set it
                        // to the button's own height and the three agree by
                        // CONSTRUCTION -- there is no second literal to drift.
                        // `interact_size.x` is deliberately left alone: it is
                        // what the widths are built from, and the 279.4pt
                        // content floor this row wraps at must not move.
                        //
                        // Scoped to this row: `ui` here is the wrapped row's
                        // own child, and `spacing_mut` clones its style
                        // (`Arc::make_mut`), so nothing below the row sees it
                        // -- the same property the scroll-area `scope` below
                        // relies on.
                        ui.spacing_mut().interact_size.y = theme::BUTTON_HEIGHT;
                        if theme::secondary_button(ui, "Generate").clicked() {
                            action = EditAction::GeneratePassword;
                        }
                        egui::ComboBox::from_id_salt("generator-kind")
                            .selected_text(if draft.generator.passphrase {
                                "Passphrase"
                            } else {
                                "Password"
                            })
                            .width(110.0)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut draft.generator.passphrase,
                                    false,
                                    "Password",
                                );
                                ui.selectable_value(
                                    &mut draft.generator.passphrase,
                                    true,
                                    "Passphrase",
                                );
                            });
                        // One control, two meanings, because a passphrase's
                        // "4" and a password's "20" are not the same quantity
                        // -- so they are separate fields on the draft (see
                        // `GeneratorDraft`) and the suffix says which is on
                        // screen. The ranges are the route's own clamps; a
                        // box that offered 1 would come back as 5.
                        if draft.generator.passphrase {
                            ui.add(
                                egui::DragValue::new(&mut draft.generator.words)
                                    .range(MIN_WORDS..=MAX_WORDS)
                                    .suffix(" words"),
                            );
                        } else {
                            ui.add(
                                egui::DragValue::new(&mut draft.generator.length)
                                    .range(MIN_LENGTH..=MAX_LENGTH)
                                    .suffix(" chars"),
                            );
                        }
                    });
                    ui.add_space(10.0);

                    // Below the password and its generator, because that is
                    // the order the user meets them in when setting an
                    // account up, and because the seed is the one field on
                    // this body the generator has nothing to do with.
                    theme::field_label(ui, TOTP_LABEL);
                    if creating {
                        theme::disabled_text_field(ui, TOTP_CREATE_NOTICE);
                    } else {
                        // Masked, like the password above it and for the same
                        // reason: it is a secret, and this form may be open
                        // in front of other people. `password_field` is the
                        // crate's one masked box -- reaching for a plain
                        // `text_field` here is the mutation
                        // `the_totp_seed_is_masked_and_never_painted_in_the_clear`
                        // exists to catch.
                        theme::password_field(ui, &mut draft.totp, &mut draft.reveal_totp);
                    }
                    ui.add_space(4.0);
                    ui.label(RichText::new(TOTP_HINT).size(11.0).color(theme::TEXT_FAINT));
                    ui.add_space(10.0);
                }
                FormBody::Card => {
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
                FormBody::Identity => {
                    for (label, value) in identity_rows(&mut draft.identity) {
                        theme::field_label(ui, label);
                        theme::text_field(ui, value, false);
                        ui.add_space(10.0);
                    }
                }
                FormBody::Note => {
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
                FormBody::SshKey => {
                    let ssh = &mut draft.ssh_key;
                    // Wire keys `privateKey`, `publicKey`, `keyFingerprint`,
                    // captured from a real type-5 item -- see `SshKeyDraft`.
                    theme::field_label(ui, "Private key");
                    // The one secret of the three, so the one with a reveal.
                    // Multiline would suit a PEM block better, but
                    // `theme` has no masked multiline box and an unmasked one
                    // would show the key by default.
                    theme::password_field(ui, &mut ssh.private_key, &mut ssh.reveal_private_key);
                    ui.add_space(10.0);

                    theme::field_label(ui, "Public key");
                    theme::text_field(ui, &mut ssh.public_key, false);
                    ui.add_space(10.0);

                    theme::field_label(ui, "Fingerprint");
                    theme::text_field(ui, &mut ssh.key_fingerprint, false);
                    ui.add_space(10.0);
                }
                FormBody::UneditableNotice => {
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

            // Between the kind's own boxes and the app block: a custom field
            // is the user's own extra data about the item, so it belongs with
            // the item's contents and above the section that is about what
            // Deskwarden does with the item.
            custom_fields_block(ui, &mut draft.fields, creating);

            // Between the kind's own fields and the folder, because a
            // binding is neither: it is about what Deskwarden does with this
            // item, which is the same argument that puts the read pane's
            // `MATCHED APP` card last among the body cards.
            //
            // **Drawn in both states.** An item that HAS a binding gets the
            // block that edits it; an item that has none gets the heading, a
            // line saying so, and the button that makes one -- because a form
            // that can only edit what is already there cannot answer "add an
            // app", which is what this form is for. The read pane is the other
            // way round and deliberately so: it shows the MATCHED APP card only
            // when there is one, since there is nothing to add from there.
            // Built here, immediately before the block that reads them, and
            // dropped immediately after: `source` borrows the draft's own
            // user-name and password boxes (never a copy of either), and
            // splitting them from `draft.app` is only possible field by field
            // like this. See `sequence_source`.
            let palette = sequence_palette(draft, item);
            let source = sequence_source(&draft.username, &draft.password, item, totp);
            match draft.app.as_mut() {
                Some(app) => {
                    if let Some(requested) = app_block(ui, app, apps, &palette, &source) {
                        action = requested;
                    }
                }
                // One assignment, and the block above draws it from the next
                // frame on. Nothing is written to the vault by this click: the
                // draft is blank until the user picks a program, and
                // `app_match_edit` leaves a blank draft alone.
                None => {
                    if app_add_block(ui) {
                        draft.app = Some(AppMatchDraft::unbound());
                    }
                }
            }
            ui.add_space(4.0);

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
                })
        })
        .inner;
    note_form_overflow(ui.ctx(), scrolled.content_size.y > scrolled.inner_rect.height());

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
            ssh_key: None,
            notes: None,
            item_type: Some(1),
            folder_id: Some("f1".into()),
            favorite: true,
            other: item_other,
        }
    }

    // -- the app block ------------------------------------------------------
    //
    // The decisions first, then `apply_to` end to end. `app_block_ui_tests`
    // (below, its own module) drives the widgets.

    /// The binding on the user's motivating item: a browser, plus the switch
    /// that says which profile.
    fn chrome_match() -> AppMatch {
        AppMatch {
            process: "chrome.exe".to_string(),
            title: String::new(),
            hosted: false,
            path: r"C:\Program Files\Google\Chrome\Application\chrome.exe".to_string(),
            args: r#"--profile-directory="Profile 2""#.to_string(),
            sequence: String::new(),
            trigger: TriggerMode::Prompt,
        }
    }

    /// A Microsoft Store binding: a title, no path, and nothing to launch.
    fn store_match() -> AppMatch {
        AppMatch {
            process: "Speedtest.exe".to_string(),
            title: "Speedtest".to_string(),
            hosted: true,
            path: String::new(),
            args: String::new(),
            sequence: String::new(),
            trigger: TriggerMode::Hotkey,
        }
    }

    fn bound_item(m: &AppMatch) -> VaultItem {
        crate::vault_bridge::with_app_match(&item(), m)
    }

    fn window_row(exe: &str, hosted: bool) -> AppWindowRow {
        AppWindowRow {
            title: "Ledgerline - Invoices".to_string(),
            exe_name: exe.to_string(),
            exe_path: format!(r"C:\Apps\{exe}"),
            hosted,
            pid: 42,
            hwnd: 4242,
        }
    }

    #[test]
    fn from_item_reads_the_binding_the_item_carries() {
        let draft = EditDraft::from_item(&bound_item(&chrome_match()));
        let app = draft.app.expect("a bound item must give the form a block to draw");
        assert!(app.bound);
        assert_eq!(app.process, "chrome.exe");
        assert_eq!(app.path, chrome_match().path);
        assert_eq!(app.args, r#"--profile-directory="Profile 2""#);
        assert_eq!(app.trigger, TriggerMode::Prompt);
    }

    #[test]
    fn an_item_with_no_binding_gives_the_draft_no_binding_either() {
        // The positive control for the test above: `app` must not be `Some` for
        // every item, which is what a `from_item` inventing a draft would give.
        //
        // **This is about the DRAFT, not about what is drawn.** The form does
        // draw an app section for an item with no binding -- the heading, the
        // notice and the Add button (see
        // `a_form_with_no_binding_offers_the_control_that_makes_one`) -- and
        // the draft staying `None` until the user asks is exactly what keeps
        // `app_match_edit` from touching the field on a save that did not.
        assert!(EditDraft::from_item(&item()).app.is_none());
    }

    #[test]
    fn the_draft_carries_the_arguments_back_out_untouched() {
        for args in [
            r#"--profile-directory="Profile 2""#,
            "  spaced  ",
            r"--user-data-dir=C:\Users\me\Chrome Beta",
        ] {
            let m = AppMatch { args: args.to_string(), ..chrome_match() };
            let round = AppMatchDraft::from_match(&m).to_match();
            assert_eq!(round.args, args, "args: {args:?}");
            assert_eq!(round, m, "args: {args:?}");
        }
    }

    #[test]
    fn a_draft_never_carries_a_title_onto_an_unhosted_match() {
        // Review 31's Important 1 as an invariant of the draft: an unhosted title
        // is never matched on, so storing one is storing a value that looks live
        // and is not. Deleting the `if self.hosted` in `to_match` gives
        //     left: "Ledgerline - Invoices"  right: ""
        let mut app = AppMatchDraft::from_match(&store_match());
        app.hosted = false;
        assert_eq!(app.to_match().title, "");
        // Positive control: a hosted one keeps it, so the assertion above is not
        // satisfied by a `to_match` that drops every title.
        assert_eq!(AppMatchDraft::from_match(&store_match()).to_match().title, "Speedtest");
    }

    #[test]
    fn a_draft_never_carries_arguments_onto_a_store_match() {
        // The mirror of the rule above, and the same defect: the form draws the
        // arguments row for a hosted binding as a DISABLED box saying "not
        // applicable", so an `args` string in the draft -- typed while the
        // binding pointed at a real executable, then left behind by a re-point
        // at a Store app, or written by an older build -- is invisible,
        // unclearable, and written on every save.
        let mut app = AppMatchDraft::from_match(&chrome_match());
        app.choose_window(&window_row("Speedtest.exe", true));
        assert_eq!(
            app.args,
            r#"--profile-directory="Profile 2""#,
            "the draft keeps them, so a Cancel -- or a re-point at a real .exe -- puts them back"
        );
        assert_eq!(app.to_match().args, "", "but a Store binding must not SAVE them");

        // Positive control: the arguments of an unhosted binding are the user's
        // and are saved verbatim, so the assertion above is not satisfied by a
        // `to_match` that drops every argument string.
        assert_eq!(
            AppMatchDraft::from_match(&chrome_match()).to_match().args,
            r#"--profile-directory="Profile 2""#
        );
    }

    #[test]
    fn choosing_a_path_derives_the_process_from_it() {
        // The whole of the path-validation decision: the file-name tie-back
        // `AppMatch::launchable_path` insists on is satisfied by construction
        // rather than checked and refused. Deleting the derivation leaves
        // `process` at "chrome.exe" against a path ending in "msedge.exe", and
        // `launchable_path` then answers `None` for a path the user just chose.
        let mut app = AppMatchDraft::from_match(&chrome_match());
        app.set_path(r"C:\Program Files\Microsoft\Edge\Application\msedge.exe");
        assert_eq!(app.process, "msedge.exe");
        assert!(
            app.to_match().launchable_path().is_some(),
            "a path chosen through the form must be one the launcher would accept"
        );
        // And the arguments and the trigger are the user's, not the file's.
        assert_eq!(app.args, r#"--profile-directory="Profile 2""#);
        assert_eq!(app.trigger, TriggerMode::Prompt);
    }

    #[test]
    fn a_path_that_names_no_file_leaves_the_process_alone() {
        // Half-typed, on the way to a real path. Taking an empty `process` off it
        // would produce a match that can never fire again.
        let mut app = AppMatchDraft::from_match(&chrome_match());
        app.set_path(r"C:\Program Files\");
        assert_eq!(app.process, "chrome.exe");
        assert_eq!(app.path, r"C:\Program Files\", "the box still shows what was typed");
    }

    #[test]
    fn giving_a_store_binding_a_real_path_stops_it_being_a_store_binding() {
        let mut app = AppMatchDraft::from_match(&store_match());
        app.set_path(r"C:\Apps\Ledgerline.exe");
        assert!(!app.hosted, "a file on disk is not an app inside a frame");
        assert_eq!(app.title, "", "and its old frame title identifies nothing now");
    }

    #[test]
    fn choosing_a_running_window_copies_that_one_rows_four_values() {
        let mut app = AppMatchDraft::from_match(&chrome_match());
        app.picking = true;
        app.choose_window(&window_row("Ledgerline.exe", false));
        assert_eq!(app.process, "Ledgerline.exe");
        assert_eq!(app.path, r"C:\Apps\Ledgerline.exe");
        assert_eq!(app.title, "", "an unhosted row records no title");
        assert!(!app.hosted);
        assert!(!app.picking, "choosing a row closes the list");
        // The user's own settings survive a change of app.
        assert_eq!(app.args, r#"--profile-directory="Profile 2""#);
        assert_eq!(app.trigger, TriggerMode::Prompt);
    }

    #[test]
    fn choosing_a_hosted_window_is_the_one_case_that_records_a_title() {
        // The positive control for the assertion above, and the same rule
        // `picker_ui::app_match_for` follows.
        let mut app = AppMatchDraft::from_match(&chrome_match());
        app.choose_window(&window_row("Speedtest.exe", true));
        assert!(app.hosted);
        assert_eq!(app.title, "Ledgerline - Invoices");
    }

    #[test]
    fn a_row_that_names_the_window_host_cannot_be_chosen() {
        let refusal = window_row_refusal(&window_row("ApplicationFrameHost.exe", false));
        assert!(refusal.is_some(), "matching the host fills this item into every Store app");
        // Positive control: an ordinary row is offered.
        assert_eq!(window_row_refusal(&window_row("Ledgerline.exe", false)), None);
    }

    #[test]
    fn a_store_binding_offers_no_path_box_and_says_why() {
        assert_eq!(app_path_row(true), AppPathRow::NotApplicable(APP_PATH_STORE_APP));
        assert_eq!(app_path_row(false), AppPathRow::Editable);
        assert!(
            !APP_PATH_STORE_APP.to_lowercase().contains("hosted"),
            "the mechanism must not reach the screen: {APP_PATH_STORE_APP:?}"
        );
    }

    #[test]
    fn a_path_the_launcher_would_refuse_is_warned_about_and_not_refused() {
        let bad = AppMatch { path: r"\\attacker\share\chrome.exe".to_string(), ..chrome_match() };
        assert!(app_path_warning(&bad).is_some());
        // Nothing to warn about for a good path, an empty one, or a Store app --
        // the positive controls that stop the warning being permanent furniture.
        assert_eq!(app_path_warning(&chrome_match()), None);
        assert_eq!(app_path_warning(&store_match()), None);
        assert_eq!(
            app_path_warning(&AppMatch { path: String::new(), ..chrome_match() }),
            None
        );
    }

    #[test]
    fn the_store_warning_never_says_hosted() {
        for text in [
            app_path_warning(&AppMatch { path: r"..\x\chrome.exe".to_string(), ..chrome_match() })
                .unwrap(),
            APP_ARGS_HINT,
            APP_REMOVED_NOTICE,
            APP_ARGS_STORE_APP,
        ] {
            assert!(!text.to_lowercase().contains("hosted"), "{text:?}");
            assert!(
                !text.contains("ApplicationFrameHost"),
                "the mechanism is not the fact: {text:?}"
            );
        }
    }

    // -- making a binding from nothing --------------------------------------

    #[test]
    fn clicking_add_and_choosing_nothing_writes_no_binding() {
        // The whole risk the Add button introduces: a draft exists now where
        // there used to be `None`, and `app_match_edit`'s old reading of
        // `bound: true` was "write it". That would put `{"process":""}` --
        // a binding that can never match -- onto an item whose name the user
        // came to fix.
        let fresh = AppMatchDraft::unbound();
        assert!(fresh.is_blank(), "a fresh draft is supposed to name nothing: {fresh:?}");
        assert_eq!(app_match_edit(None, Some(&fresh)), AppMatchEdit::Leave);
        // ... and it does not silently unbind an item that DID have one
        // either, which `Remove` would.
        assert_eq!(app_match_edit(Some(&chrome_match()), Some(&fresh)), AppMatchEdit::Leave);
    }

    #[test]
    fn a_binding_made_from_an_empty_draft_writes_the_moment_it_names_an_app() {
        // The positive control for the test above. Without it, an
        // `app_match_edit` that answered `Leave` for every draft born unbound
        // would pass -- and the Add button would be a control that changes
        // nothing, which is this crate's standing defect stated exactly.
        let mut fresh = AppMatchDraft::unbound();
        fresh.choose_window(&window_row("chrome.exe", false));
        assert!(!fresh.is_blank());
        assert_eq!(
            app_match_edit(None, Some(&fresh)),
            AppMatchEdit::Write(AppMatch {
                process: "chrome.exe".to_string(),
                title: String::new(),
                hosted: false,
                path: r"C:\Apps\chrome.exe".to_string(),
                args: String::new(),
                sequence: String::new(),
                trigger: NEW_BINDING_TRIGGER,
            })
        );
    }

    #[test]
    fn binding_an_app_from_an_empty_draft_produces_the_same_shape_as_editing_one() {
        // The two paths must converge on ONE shape, because there is one app
        // block and one `to_match`. The mutation this exists for is a second,
        // parallel construction for the add case -- one that forgets to derive
        // `process` from the row, or that drops the title on a Store app, or
        // that invents a different `trigger`. Every such divergence is
        // invisible to the widget tests, which only look at what is painted.
        //
        // The item being re-pointed carries NOTHING of its own beyond the app
        // it names, deliberately: a fixture whose old binding already held the
        // new one's arguments or sequence could not tell a shape that was
        // rebuilt from a shape that was merely left alone.
        let previous = AppMatch::for_process("olditem.exe", NEW_BINDING_TRIGGER);
        let mut checked = 0;
        for row in [
            window_row("chrome.exe", false),
            window_row("Speedtest.exe", true),
        ] {
            checked += 1;

            let mut made = AppMatchDraft::unbound();
            made.choose_window(&row);

            let mut edited = AppMatchDraft::from_match(&previous);
            edited.choose_window(&row);

            assert_eq!(
                made.to_match(),
                edited.to_match(),
                "binding {row:?} from an empty draft and re-pointing an existing binding at \
                 the same row produced two different bindings -- the add path is not the \
                 edit path"
            );
            // ... and the shape really is the row's, not an empty default that
            // both paths agree on by producing nothing.
            assert_eq!(made.to_match().process, row.exe_name);
            assert_eq!(made.to_match().hosted, row.hosted);
        }
        assert_eq!(checked, 2, "the loop visited no rows, so it asserted nothing");
    }

    #[test]
    fn a_binding_this_form_makes_carries_the_key_v0_5_0_cannot_parse_without() {
        // `AppMatch::trigger` has no `#[serde(default)]`. A binding created
        // here that omitted it, or that invented a mode the other creation
        // path does not write, would be a rollback the user cannot read --
        // see `NEW_BINDING_TRIGGER`.
        let mut fresh = AppMatchDraft::unbound();
        fresh.choose_window(&window_row("chrome.exe", false));
        let json = fresh.to_match().to_field_value();
        assert!(json.contains("\"trigger\""), "a new binding serializes without a trigger: {json}");
        assert_eq!(fresh.to_match().trigger, TriggerMode::Prompt);
    }

    // -- what a save does ---------------------------------------------------

    #[test]
    fn a_save_that_changed_nothing_leaves_the_binding_alone() {
        assert_eq!(
            app_match_edit(
                Some(&chrome_match()),
                Some(&AppMatchDraft::from_match(&chrome_match()))
            ),
            AppMatchEdit::Leave
        );
    }

    #[test]
    fn changing_the_arguments_writes_the_binding() {
        // The positive control for the test above: `Leave` must not be the answer
        // to everything, which is what an `app_match_edit` returning `Leave`
        // unconditionally would give -- and that mutation makes the arguments box
        // inert while every pure test of the draft keeps passing.
        let mut draft = AppMatchDraft::from_match(&chrome_match());
        draft.args = "--profile-directory=Personal".to_string();
        assert_eq!(
            app_match_edit(Some(&chrome_match()), Some(&draft)),
            AppMatchEdit::Write(AppMatch {
                args: "--profile-directory=Personal".to_string(),
                ..chrome_match()
            })
        );
    }

    #[test]
    fn removing_the_binding_removes_the_field_and_only_when_there_is_one() {
        let mut draft = AppMatchDraft::from_match(&chrome_match());
        draft.bound = false;
        assert_eq!(app_match_edit(Some(&chrome_match()), Some(&draft)), AppMatchEdit::Remove);
        // Nothing to remove: a `Remove` here would cost a PUT that changes nothing.
        assert_eq!(app_match_edit(None, Some(&draft)), AppMatchEdit::Leave);
    }

    #[test]
    fn a_form_that_drew_no_block_never_touches_the_field() {
        // The failure this prevents: saving an item's NAME silently unbinding its
        // app, on any item whose block the form declined to draw.
        assert_eq!(app_match_edit(Some(&chrome_match()), None), AppMatchEdit::Leave);
        assert_eq!(app_match_edit(None, None), AppMatchEdit::Leave);
    }

    #[test]
    fn saving_an_edited_argument_string_reaches_the_item() {
        // **The wiring pin for the write.** Deleting the `apply_app_match_to` call
        // at the end of `apply_to` leaves every `app_match_edit` test above green
        // and the arguments box permanently inert. This is what fails:
        //     the arguments never reached the item
        let item = bound_item(&chrome_match());
        let mut draft = EditDraft::from_item(&item);
        draft.app.as_mut().unwrap().args = "--profile-directory=Personal".to_string();

        let saved = draft.apply_to(&item);
        let stored = crate::vault_bridge::extract_app_match(&saved)
            .expect("the arguments never reached the item");
        assert_eq!(stored.args, "--profile-directory=Personal");
        // And nothing else about the binding moved.
        assert_eq!(stored.process, "chrome.exe");
        assert_eq!(stored.path, chrome_match().path);
        assert_eq!(stored.trigger, TriggerMode::Prompt);
    }

    #[test]
    fn saving_an_edited_path_reaches_the_item_with_its_process_derived() {
        let item = bound_item(&chrome_match());
        let mut draft = EditDraft::from_item(&item);
        draft.set_app_path(r"C:\Program Files\Microsoft\Edge\Application\msedge.exe");

        let stored = crate::vault_bridge::extract_app_match(&draft.apply_to(&item)).unwrap();
        assert_eq!(stored.process, "msedge.exe");
        assert_eq!(stored.launchable_path(), Some(stored.path.as_str()));
    }

    /// **`AppMatch::trigger` survives an edit untouched.**
    ///
    /// Replaces `saving_a_changed_trigger_reaches_the_item`, which asserted
    /// that a trigger the user picked in this form reached the vault. There is
    /// no such choice any more -- nothing in this build reads the field -- but
    /// the field itself is not this pass's to drop or to re-default: v0.5.0's
    /// `trigger` has no `#[serde(default)]`, so an item written without it, or
    /// with a value this build invented, is a binding a rolled-back build
    /// reads wrong or not at all. See `app_match::AppMatch::trigger`.
    #[test]
    fn an_edit_carries_the_stored_trigger_through_untouched() {
        // The fixture's mode is deliberately NOT the one a new binding is
        // written with: a `Prompt` fixture could not tell "carried through"
        // from "quietly reset to the default".
        let original = store_match();
        assert_ne!(
            original.trigger,
            TriggerMode::Prompt,
            "the premise: the fixture's stored mode differs from the default a new binding gets, \
             so `carried through` and `reset` give different answers here"
        );
        let item = bound_item(&original);
        let mut draft = EditDraft::from_item(&item);
        draft.name = "Renamed".to_string();
        let saved = draft.apply_to(&item);
        // The premise again, from the other end: the edit really happened, so
        // the assertion below is not about an item nothing touched.
        assert_eq!(saved.name, "Renamed");
        let stored = crate::vault_bridge::extract_app_match(&saved).unwrap();
        assert_eq!(
            stored.trigger, original.trigger,
            "an edit rewrote the stored trigger; v0.5.0 reads this key and this build must \
             neither drop it nor invent a value for it"
        );
    }

    #[test]
    fn saving_a_removed_binding_takes_the_field_off_the_item() {
        let item = bound_item(&chrome_match());
        let mut draft = EditDraft::from_item(&item);
        draft.app.as_mut().unwrap().bound = false;
        let saved = draft.apply_to(&item);
        assert!(crate::vault_bridge::extract_app_match(&saved).is_none());
        // The rest of the item is untouched -- a Remove is one field, not a reset.
        assert_eq!(saved.name, item.name);
        assert_eq!(saved.favorite, item.favorite);
    }

    /// **The contract this feature could most easily have broken.**
    ///
    /// `an_edit_that_changes_nothing_produces_a_byte_identical_item` above proves
    /// it for an item with no binding. This proves it for one WITH a binding,
    /// whose field is JSON: a save that rewrote the field unconditionally would
    /// replace whatever spelling is in the user's vault with the serializer's,
    /// and this is what would notice.
    #[test]
    fn an_edit_that_changes_nothing_leaves_a_bound_item_byte_identical() {
        let item = bound_item(&chrome_match());
        let draft = EditDraft::from_item(&item);
        assert_eq!(
            serde_json::to_string(&draft.apply_to(&item)).unwrap(),
            serde_json::to_string(&item).unwrap()
        );
    }

    /// The same, for the shape a **previous build** wrote: five keys, no `args`.
    /// Opening such an item in the editor and saving it must not grow the field.
    #[test]
    fn saving_an_untouched_binding_from_an_older_build_does_not_rewrite_it() {
        let stored = r#"{"process":"Speedtest.exe","title":"Speedtest","hosted":true,"path":"C:\\Apps\\Speedtest.exe","trigger":"prompt"}"#;
        let mut item = item();
        item.fields.push(crate::vault_bridge::VaultField {
            name: Some(crate::app_match::APP_MATCH_FIELD_NAME.to_string()),
            value: Some(Zeroizing::new(stored.to_string())),
            other: serde_json::Map::new(),
        });
        let draft = EditDraft::from_item(&item);
        let saved = draft.apply_to(&item);
        assert_eq!(
            saved.fields[0].value.as_ref().map(|v| v.as_str()),
            Some(stored),
            "an untouched binding was rewritten on save"
        );
    }

    /// **The value the SAVE path builds does not reach the allocator in the
    /// clear.**
    ///
    /// Copy **F** of the trace in [`crate::vault_bridge::VaultField::value`]'s
    /// doc, and the other half of the pair the type change actually closes.
    /// `FieldDraft::to_field` is where a secret the user typed into a hidden
    /// custom field becomes a modelled value, and it is why `edited_secret`
    /// exists next to `edited`: `edited` returns a bare `String`, which goes
    /// back to the allocator holding the plaintext when the saved item drops.
    /// Swap `edited_secret` for `edited` in `to_field` and this fails.
    ///
    /// **The draft is built outside the armed window on purpose.** A
    /// `FieldDraft`'s own `value` is a plain `String` -- egui's text-edit
    /// buffer, copy B of the trace, a recorded exception -- so building the
    /// draft inside the window would report a leak this change never claimed
    /// to fix and the assertion would be about the wrong copy.
    #[test]
    fn a_saved_custom_field_value_does_not_reach_the_allocator_in_the_clear() {
        use crate::login_ui::password_lifetime_tests::{plaintext_reached_the_allocator, PROBE};

        // The instrument is awake and can see an unwiped plaintext go past.
        // Without it, `!leaked` below is satisfied by a probe that reports
        // nothing about anything.
        let bare = String::from_utf8(PROBE.as_bytes().to_vec()).expect("PROBE is UTF-8");
        assert!(
            plaintext_reached_the_allocator(move || drop(bare)),
            "the probe cannot see an unwiped value, so this test proves nothing"
        );

        // A hidden (`type: 1`) field -- the role that made this a secret.
        let mut hidden = serde_json::Map::new();
        hidden.insert("type".to_string(), serde_json::json!(1));
        let mut item = item();
        item.fields.push(crate::vault_bridge::VaultField {
            name: Some("Recovery code".to_string()),
            value: Some(Zeroizing::new("old".to_string())),
            other: hidden,
        });

        let mut draft = EditDraft::from_item(&item);
        let row = draft
            .fields
            .iter_mut()
            .find(|f| f.name == "Recovery code")
            .expect("the hidden field has no draft row");
        assert_eq!(row.role, FieldRole::Hidden, "the fixture row is not a hidden field");
        // The user types the secret. This plain `String` is copy B and stays
        // plain; it lives past the armed window and is dropped after it.
        row.value = String::from_utf8(PROBE.as_bytes().to_vec()).expect("PROBE is UTF-8");

        let mut carried = false;
        let leaked = plaintext_reached_the_allocator(|| {
            let saved = draft.apply_to(&item);
            carried = saved
                .fields
                .iter()
                .any(|f| f.value.as_ref().map(|v| v.as_str()) == Some(PROBE));
            drop(saved);
        });

        // Positive control: the save really did build the secret into a
        // `VaultField`, so `!leaked` cannot mean "nothing was produced".
        assert!(carried, "the save never wrote the typed secret -- nothing was watched");
        assert!(!leaked, "the save path freed the typed custom-field value in the clear");

        drop(draft);
    }

    /// Changing the item's TYPE in the create form must not throw away a binding
    /// -- `app` is item-level, like the name and the folder.
    #[test]
    fn switching_kind_keeps_the_app_binding() {
        let mut draft = EditDraft::from_item(&bound_item(&chrome_match()));
        draft.set_kind(ItemKind::Card);
        assert!(draft.app.is_some(), "changing the type menu unbound the app");
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
        // A source-text guard over this file (the same device as
        // `settings.rs`'s `the_config_path_still_matches_the_one_main_resolves`):
        // the pure function above is only worth anything if the dropdown is
        // the thing calling it.
        //
        // **Both needles are assembled rather than written out**, and the
        // positive one is the reason this comment exists. `include_str!`
        // pulls in this test module too, so a needle written as one literal
        // matches *its own declaration* and `contains` is unconditionally
        // true -- the guard passes forever and guards nothing. It was written
        // that way, and a reviewer proved it: replacing the call below with
        // `folders.to_vec()` restored the exact regression this test names
        // and the whole suite stayed green. The negative needle was already
        // assembled for this reason; the positive one was missed, which is
        // why it is spelled out here rather than left to be re-derived.
        let source = include_str!("detail_edit.rs");
        let call = concat!("let assignable = ", "assignable_folders(folders);");
        assert!(
            source.contains(call),
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
        // **Built from the item**, not from `EditDraft::empty()`, and the
        // difference is the subject of this test rather than a detail of it:
        // this form now HAS boxes for the TOTP seed and the custom fields, so
        // a draft that was never populated from the item says "the user
        // cleared them", which is a different claim from "the form never
        // touched them". The three overridden fields below are the edit; the
        // assertions are about everything else.
        let item = item();
        let draft = EditDraft {
            name: "Renamed".into(),
            username: "new@b.com".into(),
            password: "np".into(),
            folder_id: None,
            // ...and the recorded original with it, so this stays the
            // un-file case it has always been: `may_unfile` refuses a clear
            // on an item that arrived filed, and `from_item` records that
            // `item()` did.
            original_folder_id: None,
            ..EditDraft::from_item(&item)
        };
        let updated = draft.apply_to(&item);

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

    /// Types a recognisable new value into every box the form would offer for
    /// `kind` -- and, for the kinds the form offers nothing for, into *every*
    /// box there is, so "nothing was written" is a claim about `apply_to`'s
    /// gate rather than about a draft that was never edited.
    fn edit_every_box(draft: &mut EditDraft, kind: ItemKind) {
        let offered = detail::kind_offers_edit(kind);
        if !offered || kind == ItemKind::Login {
            draft.username = "edited-user".into();
            draft.password = "edited-pass".into();
            draft.totp = "EDITEDSEED".into();
        }
        if !offered || kind == ItemKind::Card {
            draft.card.cardholder_name = "Edited Holder".into();
            draft.card.number = "4111111111111111".into();
        }
        if !offered || kind == ItemKind::Identity {
            draft.identity.first_name = "Grace".into();
            draft.identity.city = "Edited City".into();
        }
        if !offered || kind == ItemKind::SecureNote {
            draft.note_body = "edited body".into();
        }
        if !offered {
            draft.ssh_key.private_key = "EDITED-PRIV".into();
            draft.ssh_key.public_key = "EDITED-PUB".into();
            draft.ssh_key.key_fingerprint = "SHA256:EDITED".into();
        }
    }

    /// **The guard that stops the two facts drifting.** `kind_offers_edit`
    /// draws the Edit button; `apply_to` is what a Save through that button
    /// does. This asserts they agree *behaviourally*, for every kind: a real
    /// `EditDraft` built from a real `VaultItem`, every box the form has
    /// filled with a new value, saved -- and then the item itself is asked
    /// whether anything changed.
    ///
    /// A button offered for a kind `apply_to` does not write is a form that
    /// silently discards what the user typed (widen the predicate to include
    /// `SshKey` and this reds); an unoffered kind that `apply_to` does write
    /// is a feature nobody can reach.
    #[test]
    fn edit_is_offered_for_exactly_the_kinds_apply_to_writes() {
        let mut offered_count = 0;
        for raw in EVERY_KIND {
            let item = parse(raw);
            let kind = ItemKind::of(&item);
            let mut draft = EditDraft::from_item(&item);
            edit_every_box(&mut draft, kind);
            let saved = draft.apply_to(&item);

            // Whole-item comparison, not a per-kind field lookup: the name and
            // folder are untouched above, so any difference at all is the
            // kind's own object (or `notes`) having been written.
            let before = serde_json::to_value(&item).unwrap();
            let after = serde_json::to_value(&saved).unwrap();
            let wrote_something = before != after;

            assert_eq!(
                wrote_something,
                detail::kind_offers_edit(kind),
                "{kind:?}: kind_offers_edit says {} but a save through the form {} \
                 the item.\nbefore: {before}\nafter:  {after}",
                detail::kind_offers_edit(kind),
                if wrote_something { "changed" } else { "did not change" }
            );
            if detail::kind_offers_edit(kind) {
                offered_count += 1;
            }
        }
        assert_eq!(offered_count, 4, "the fixture set no longer covers four editable kinds");
    }

    /// The user's reported bug, end to end: a card can be edited, the change
    /// lands, and the item does **not** gain a `login` object on the way.
    ///
    /// That second half is the assertion that proves enabling the button was
    /// safe -- an empty `login` grafted onto a card is exactly what the
    /// login-only gate existed to prevent.
    #[test]
    fn a_card_round_trips_through_the_form_without_growing_a_login() {
        let item = parse(CARD_WITH_EXTRAS);
        assert!(detail::kind_offers_edit(ItemKind::of(&item)), "a card must offer Edit");

        let mut draft = EditDraft::from_item(&item);
        // `from_item` populated the card draft -- the user sees their card,
        // not an empty form.
        assert_eq!(draft.card.cardholder_name, "John Doe");
        assert_eq!(draft.card.number, "4242424242424242");
        assert_eq!(draft.card.exp_year, "2028");

        draft.card.number = "4111111111111111".into();
        let saved = draft.apply_to(&item);

        let card = saved.card.as_ref().expect("the save dropped the card object");
        assert_eq!(card.number.as_deref().map(|n| n.as_str()), Some("4111111111111111"));
        assert_eq!(card.cardholder_name.as_deref(), Some("John Doe"));
        assert_eq!(card.exp_year.as_deref(), Some("2028"));

        assert!(saved.login.is_none(), "editing a card grew a login object");
        let json = serde_json::to_value(&saved).unwrap();
        assert!(
            json.as_object().unwrap().get("login").is_none(),
            "editing a card put a `login` key on the wire: {json}"
        );
    }

    #[test]
    fn an_identity_round_trips_through_the_form_without_growing_a_login() {
        let item = parse(IDENTITY_WITH_EXTRAS);
        assert!(detail::kind_offers_edit(ItemKind::of(&item)), "an identity must offer Edit");

        let mut draft = EditDraft::from_item(&item);
        assert_eq!(draft.identity.first_name, "Ada");
        assert_eq!(draft.identity.last_name, "Lovelace");

        draft.identity.first_name = "Grace".into();
        let saved = draft.apply_to(&item);

        let identity = saved.identity.as_ref().expect("the save dropped the identity object");
        assert_eq!(identity.first_name.as_deref(), Some("Grace"));
        assert_eq!(identity.last_name.as_deref(), Some("Lovelace"));

        assert!(saved.login.is_none(), "editing an identity grew a login object");
        let json = serde_json::to_value(&saved).unwrap();
        assert!(
            json.as_object().unwrap().get("login").is_none(),
            "editing an identity put a `login` key on the wire: {json}"
        );
    }

    #[test]
    fn a_secure_note_round_trips_through_the_form_without_growing_a_login() {
        let item = parse(NOTE_WITH_EXTRAS);
        assert!(detail::kind_offers_edit(ItemKind::of(&item)), "a secure note must offer Edit");

        let mut draft = EditDraft::from_item(&item);
        assert_eq!(draft.note_body, "the passphrase");

        draft.note_body = "the new passphrase".into();
        let saved = draft.apply_to(&item);

        assert_eq!(
            saved.notes.as_deref().map(|n| n.as_str()),
            Some("the new passphrase"),
            "the note body did not round-trip"
        );
        // Its `{"type": 0}` discriminator rides `other` and must survive.
        assert_eq!(saved.other.get("secureNote"), Some(&serde_json::json!({"type": 0})));

        assert!(saved.login.is_none(), "editing a secure note grew a login object");
        let json = serde_json::to_value(&saved).unwrap();
        assert!(
            json.as_object().unwrap().get("login").is_none(),
            "editing a secure note put a `login` key on the wire: {json}"
        );
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

    /// Every custom-field shape this form has to survive, on one item:
    /// text, hidden, one of ours, boolean, linked (whose `value` really is
    /// `null` and whose payload is `linkedId`), and one with **no `type` key
    /// at all** -- the shape a field this app created wears until `bw`
    /// normalises it.
    ///
    /// Six elements, in an order nothing may change.
    const LOGIN_WITH_EVERY_FIELD_TYPE: &str = r#"{
        "object":"item","id":"77777777-7777-7777-7777-777777777777","name":"Everything",
        "type":1,"favorite":false,"reprompt":0,"key":"ITEMKEY",
        "creationDate":"2026-01-02T03:04:05.000Z","revisionDate":"2026-02-03T04:05:06.000Z",
        "attachments":null,"collectionIds":[],"passwordHistory":null,
        "fields":[
            {"name":"Account number","value":"12345","type":0,"linkedId":null},
            {"name":"Recovery code","value":"s3cret","type":1,"linkedId":null},
            {"name":"deskwarden:reserved","value":"ours","type":0,"linkedId":null},
            {"name":"Paid up","value":"true","type":2,"linkedId":null},
            {"name":"Linked user","value":null,"type":3,"linkedId":100},
            {"name":"Legacy","value":"no type key"}
        ],
        "login":{"username":"u","password":"p"}
    }"#;

    fn field_names(item: &VaultItem) -> Vec<Option<&str>> {
        item.fields.iter().map(|f| f.name.as_deref()).collect()
    }

    /// **The regression this whole feature most risks.** Before it, `apply_to`
    /// never touched `fields` at all and preservation was free; now the draft
    /// owns them, so an untouched save is a full rewrite that has to land on
    /// the same bytes.
    #[test]
    fn an_item_whose_fields_are_untouched_writes_them_back_unchanged() {
        let item = parse(LOGIN_WITH_EVERY_FIELD_TYPE);
        // The premise, asserted rather than assumed: an item with no fields
        // cannot demonstrate that no field was lost, and this test would pass
        // green on one.
        assert_eq!(item.fields.len(), 6, "the fixture has nothing to preserve");
        assert!(
            item.fields.iter().any(|f| f.other.get("type") == Some(&serde_json::json!(1))),
            "the fixture carries no hidden field"
        );

        let draft = EditDraft::from_item(&item);
        assert_eq!(draft.fields.len(), 6, "from_item dropped a field on the way in");
        let saved = draft.apply_to(&item);

        let before: serde_json::Value = serde_json::from_str(LOGIN_WITH_EVERY_FIELD_TYPE).unwrap();
        assert_eq!(
            before,
            serde_json::to_value(&saved).unwrap(),
            "an untouched save rewrote the item's custom fields"
        );
        // ...and the array's ORDER specifically, which the whole-value
        // comparison above does cover but does not name. Bitwarden shows the
        // user this order.
        assert_eq!(
            field_names(&saved),
            vec![
                Some("Account number"),
                Some("Recovery code"),
                Some("deskwarden:reserved"),
                Some("Paid up"),
                Some("Linked user"),
                Some("Legacy"),
            ]
        );
    }

    /// The same, with a real `deskwarden:app-match` **in the middle** -- so
    /// the field-list rewrite and `with_app_match`'s replace-in-place have to
    /// agree, not merely each be right alone.
    #[test]
    fn saving_a_bound_item_does_not_reshuffle_the_users_own_fields() {
        let mut item = parse(LOGIN_WITH_EVERY_FIELD_TYPE);
        let m = chrome_match();
        item.fields.insert(
            2,
            crate::vault_bridge::VaultField {
                name: Some(crate::app_match::APP_MATCH_FIELD_NAME.to_string()),
                value: Some(Zeroizing::new(m.to_field_value())),
                other: serde_json::Map::new(),
            },
        );
        assert!(
            crate::vault_bridge::extract_app_match(&item).is_some(),
            "the fixture's binding does not parse, so this proves nothing about a BOUND item"
        );

        let draft = EditDraft::from_item(&item);
        assert!(draft.app.is_some(), "from_item did not read the binding");
        let saved = draft.apply_to(&item);

        assert_eq!(
            serde_json::to_value(&saved).unwrap(),
            serde_json::to_value(&item).unwrap(),
            "saving a bound item disturbed something"
        );
        assert_eq!(
            field_names(&saved)[2],
            Some(crate::app_match::APP_MATCH_FIELD_NAME),
            "the app-match field moved out of its slot"
        );
    }

    /// **The type-preservation claim.** A hidden field is a secret; an edit
    /// that wrote it back as `type: 0` would publish it to every Bitwarden
    /// client, silently, which is the failure `VaultField`'s own doc records
    /// happening on a real vault.
    #[test]
    fn a_hidden_field_stays_hidden_through_an_edit() {
        let item = parse(LOGIN_WITH_EVERY_FIELD_TYPE);
        let mut draft = EditDraft::from_item(&item);

        // The row really is drafted as a hidden one, and by its `type` -- not
        // by its name or its position.
        let hidden = draft.fields.iter().position(|f| f.name == "Recovery code").unwrap();
        assert_eq!(draft.fields[hidden].role(), FieldRole::Hidden);
        // And the neighbouring text field is NOT, so the assertion above is
        // about the type and not about `from_field` answering `Hidden` for
        // everything.
        let text = draft.fields.iter().position(|f| f.name == "Account number").unwrap();
        assert_eq!(draft.fields[text].role(), FieldRole::Text);

        // Edit it, which is the case a preserved-untouched implementation
        // would still pass: this changes the value and must keep the type.
        draft.fields[hidden].value = "n3wsecret".into();
        draft.name = "Everything (renamed)".into();
        let saved = draft.apply_to(&item);

        let value = serde_json::to_value(&saved).unwrap();
        let field = value["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["name"] == serde_json::json!("Recovery code"))
            .expect("the hidden field vanished");
        assert_eq!(
            field,
            &serde_json::json!({
                "name":"Recovery code","value":"n3wsecret","type":1,"linkedId":null
            }),
            "an edited hidden field did not come back as a hidden field"
        );
    }

    /// The two types this form has no editor for are listed, refused, and
    /// written back byte for byte -- **not flattened into text boxes**.
    #[test]
    fn a_field_type_the_form_cannot_edit_is_preserved_and_never_offered() {
        let item = parse(LOGIN_WITH_EVERY_FIELD_TYPE);
        let draft = EditDraft::from_item(&item);

        let roles: Vec<(&str, FieldRole)> =
            draft.fields.iter().map(|f| (f.name.as_str(), f.role())).collect();
        assert_eq!(
            roles,
            vec![
                ("Account number", FieldRole::Text),
                ("Recovery code", FieldRole::Hidden),
                ("deskwarden:reserved", FieldRole::Internal),
                ("Paid up", FieldRole::Preserved),
                ("Linked user", FieldRole::Preserved),
                // No `type` key at all reads as text, which is what `bw` does
                // with the field this app writes.
                ("Legacy", FieldRole::Text),
            ]
        );

        // A `Preserved` row's write ignores whatever is in its draft strings,
        // which is what makes it safe to list it at all.
        let mut vandal = draft.clone();
        for field in vandal.fields.iter_mut().filter(|f| f.role() == FieldRole::Preserved) {
            field.name = "clobbered".into();
            field.value = "clobbered".into();
        }
        let saved = vandal.apply_to(&item);
        assert_eq!(
            serde_json::to_value(&saved).unwrap()["fields"],
            serde_json::from_str::<serde_json::Value>(LOGIN_WITH_EVERY_FIELD_TYPE).unwrap()
                ["fields"],
            "a field this form cannot edit was rewritten anyway"
        );
    }

    /// A field added through the form reaches the item, carries the type the
    /// button promised, and changes **nothing else**.
    ///
    /// "Byte identically" is the whole item either side of the one addition:
    /// the before-value with the new element pushed onto its `fields` array
    /// must be exactly what is written.
    #[test]
    fn a_custom_field_added_in_the_form_round_trips_byte_identically() {
        for (role, wire_type) in [(FieldRole::Text, 0), (FieldRole::Hidden, 1)] {
            let item = parse(LOGIN_WITH_EVERY_FIELD_TYPE);
            let mut draft = EditDraft::from_item(&item);
            // What the Add button does, and nothing else -- see
            // `custom_fields_block`.
            draft.fields.push(FieldDraft::new_of(role));
            draft.fields.last_mut().unwrap().name = "PIN".into();
            draft.fields.last_mut().unwrap().value = "4321".into();

            let saved = draft.apply_to(&item);
            let mut expected: serde_json::Value =
                serde_json::from_str(LOGIN_WITH_EVERY_FIELD_TYPE).unwrap();
            expected["fields"].as_array_mut().unwrap().push(serde_json::json!({
                "name":"PIN","value":"4321","type":wire_type
            }));
            assert_eq!(
                expected,
                serde_json::to_value(&saved).unwrap(),
                "adding a {role:?} field changed more than the field it added"
            );
        }
    }

    /// A field the user added and then left blank is **still a field**.
    ///
    /// Its `value` goes out as `null`, which is not an omission but the shape
    /// `VaultField` deliberately produces (that struct's doc records why the
    /// two modelled keys carry no `skip_serializing_if`), and it is the same
    /// shape a real linked field arrives in. The blank rule [`edited`] answers
    /// `None` here because there was no previous value -- what must not
    /// happen is the whole *element* going missing.
    #[test]
    fn a_new_field_left_empty_still_writes_a_field() {
        let item = parse(LOGIN_WITH_EVERY_FIELD_TYPE);
        let mut draft = EditDraft::from_item(&item);
        draft.fields.push(FieldDraft::new_of(FieldRole::Text));
        draft.fields.last_mut().unwrap().name = "Empty".into();
        let saved = draft.apply_to(&item);
        assert_eq!(saved.fields.len(), 7, "the new field was dropped");
        assert_eq!(
            serde_json::to_value(&saved).unwrap()["fields"][6],
            serde_json::json!({"name":"Empty","value":null,"type":0}),
            "an empty new field wrote a shape nobody asked for"
        );
    }

    /// Removing a row removes **that** row and leaves the rest in order.
    #[test]
    fn removing_a_field_removes_exactly_that_field() {
        let item = parse(LOGIN_WITH_EVERY_FIELD_TYPE);
        let mut draft = EditDraft::from_item(&item);
        let at = draft.fields.iter().position(|f| f.name == "Account number").unwrap();
        draft.fields.remove(at);
        let saved = draft.apply_to(&item);
        assert_eq!(
            field_names(&saved),
            vec![
                Some("Recovery code"),
                Some("deskwarden:reserved"),
                Some("Paid up"),
                Some("Linked user"),
                Some("Legacy"),
            ]
        );
    }

    /// The editor and the keystroke palette hide the same prefix.
    ///
    /// Two lists of what counts as ours is how one of them starts offering
    /// `deskwarden:app-match` as a field to type -- or, here, as a raw text
    /// box beside the app block that already edits it.
    #[test]
    fn the_editor_hides_the_same_prefix_the_palette_hides() {
        let item = parse(LOGIN_WITH_EVERY_FIELD_TYPE);
        let draft = EditDraft::from_item(&item);
        let hidden_from_editor: Vec<&str> = draft
            .fields
            .iter()
            .filter(|f| f.role() == FieldRole::Internal)
            .map(|f| f.name.as_str())
            .collect();
        assert_eq!(hidden_from_editor, vec!["deskwarden:reserved"]);

        let offered: Vec<String> = crate::key_sequence::field_palette(&item)
            .into_iter()
            .filter_map(|f| match f {
                FieldRef::Custom(name) => Some(name),
                _ => None,
            })
            .collect();
        for name in &hidden_from_editor {
            assert!(
                !offered.contains(&name.to_string()),
                "{name} is hidden from the editor but offered by the palette"
            );
        }
        // The control: a field that is NOT ours is offered by both.
        assert!(offered.contains(&"Account number".to_string()), "{offered:?}");
        assert!(
            draft.fields.iter().any(|f| f.name == "Account number" && f.is_editable()),
            "an ordinary custom field is not editable"
        );
        assert_eq!(
            OURS_PREFIX, "deskwarden:",
            "the prefix moved; `key_sequence::field_palette` has its own copy"
        );
    }

    /// The seed is read out of the item, edited, and written back -- the
    /// wiring pin for a box that was previously preserved and never offered.
    #[test]
    fn the_totp_seed_is_read_edited_and_written_back() {
        let item = parse(LOGIN_WITH_EXTRAS);
        let mut draft = EditDraft::from_item(&item);
        assert_eq!(draft.totp, "SEED", "from_item did not read the seed into the box");

        draft.totp = "otpauth://totp/Ledgerline?secret=NEWSEED".into();
        let saved = draft.apply_to(&item);
        assert_eq!(
            saved.login.as_ref().unwrap().totp.as_deref().map(|t| t.as_str()),
            Some("otpauth://totp/Ledgerline?secret=NEWSEED"),
            "the edited seed never reached the item"
        );
        // And nothing else in the login moved.
        assert_eq!(saved.login.as_ref().unwrap().username.as_deref(), Some("a@b.com"));
        assert_eq!(saved.login.as_ref().unwrap().uris.len(), 1);
    }

    /// Clearing the box removes the key, by the same blank rule every other
    /// box on this form follows -- and an item that never had one does not
    /// gain a `"totp": null`.
    #[test]
    fn clearing_the_totp_box_removes_the_seed_and_an_absent_one_is_not_invented() {
        let item = parse(LOGIN_WITH_EXTRAS);
        let mut draft = EditDraft::from_item(&item);
        draft.totp = String::new();
        let saved = draft.apply_to(&item);
        assert!(saved.login.as_ref().unwrap().totp.is_none(), "clearing the box did nothing");

        // The other half: an item with no seed, saved untouched, must not
        // grow the key. `LoginData::totp` carries `skip_serializing_if`, and
        // this is what holds it to that through the new box.
        let bare = parse(
            r#"{"object":"item","id":"9","name":"Bare","type":1,"favorite":false,
                "fields":[],"login":{"username":"u","password":"p"}}"#,
        );
        let saved = EditDraft::from_item(&bare).apply_to(&bare);
        let value = serde_json::to_value(&saved).unwrap();
        assert_eq!(
            value["login"].as_object().unwrap().keys().collect::<Vec<_>>(),
            vec!["password", "username"],
            "an untouched login with no seed gained a totp key"
        );
    }

    /// Custom fields are item-level, like the name, the folder and the app
    /// binding -- flicking the type menu must not delete them.
    #[test]
    fn switching_kind_keeps_the_custom_fields() {
        let mut draft = EditDraft::from_item(&parse(LOGIN_WITH_EVERY_FIELD_TYPE));
        draft.set_kind(ItemKind::Card);
        assert_eq!(draft.fields.len(), 6, "changing the type menu deleted the custom fields");
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

    /// A draft of `kind` with every kind's fields filled in with a
    /// recognisable value, so a payload that carries the wrong kind's data --
    /// the login-grafting defect class -- shows up as a value that does not
    /// belong rather than as an absence.
    fn stuffed_draft(kind: ItemKind) -> EditDraft {
        let mut draft = EditDraft::empty_of(kind);
        draft.name = "Everything".into();
        draft.folder_id = Some("f2".into());
        draft.username = "login-user".into();
        draft.password = "login-pass".into();
        draft.note_body = "note-body".into();
        draft.card.cardholder_name = "card-holder".into();
        draft.card.brand = "card-brand".into();
        draft.card.number = "card-number".into();
        draft.card.exp_month = "04".into();
        draft.card.exp_year = "2028".into();
        draft.card.code = "card-code".into();
        for (i, (_, value)) in identity_rows(&mut draft.identity).into_iter().enumerate() {
            *value = format!("identity-{i}");
        }
        draft.ssh_key.private_key = "ssh-private".into();
        draft.ssh_key.public_key = "ssh-public".into();
        draft.ssh_key.key_fingerprint = "ssh-fingerprint".into();
        draft
    }

    /// The type object of a create payload, as a map -- **never** by indexing.
    /// `Value["missing"]` is `Null`, so `assert_eq!(body["x"], Value::Null)`
    /// passes whether the key is absent or explicitly null, and an assertion
    /// that cannot fail is worse than none.
    fn type_object<'a>(
        payload: &'a serde_json::Value,
        key: &str,
    ) -> &'a serde_json::Map<String, serde_json::Value> {
        payload
            .as_object()
            .expect("a create payload is an object")
            .get(key)
            .unwrap_or_else(|| panic!("the payload has no `{key}` object"))
            .as_object()
            .expect("a type object is an object")
    }

    #[test]
    fn a_draft_can_start_at_a_chosen_kind() {
        for kind in CREATABLE_KINDS {
            assert_eq!(EditDraft::empty_of(kind).kind(), kind);
        }
        // The no-argument constructor keeps its documented default.
        assert_eq!(EditDraft::empty().kind(), ItemKind::Login);
    }

    #[test]
    fn the_five_creatable_kinds_are_offered_and_unknown_is_not() {
        assert_eq!(
            CREATABLE_KINDS.to_vec(),
            vec![
                ItemKind::Login,
                ItemKind::SecureNote,
                ItemKind::Card,
                ItemKind::Identity,
                ItemKind::SshKey,
            ]
        );
        assert!(!CREATABLE_KINDS.iter().any(|k| matches!(k, ItemKind::Unknown(_))));
    }

    #[test]
    fn an_unknown_kind_has_no_create_payload() {
        // `NewItem` has no variant for a type this build does not understand,
        // and inventing one (a login, a note) would create an item of the
        // WRONG TYPE from a form the user filled in for something else.
        let mut draft = stuffed_draft(ItemKind::Login);
        draft.set_kind(ItemKind::Unknown(9));
        assert!(draft.to_new_item().is_none());
        assert!(!is_creatable(ItemKind::Unknown(9)));
        for kind in CREATABLE_KINDS {
            assert!(is_creatable(kind), "{kind:?} is offered by the menu but cannot be created");
            assert!(
                stuffed_draft(kind).to_new_item().is_some(),
                "{kind:?} is creatable but produced no payload"
            );
        }
    }

    #[test]
    fn a_login_draft_creates_a_login() {
        let payload = stuffed_draft(ItemKind::Login).to_new_item().unwrap().to_payload();
        assert_eq!(payload["type"], serde_json::json!(1));
        assert_eq!(payload["name"], serde_json::json!("Everything"));
        assert_eq!(payload["folderId"], serde_json::json!("f2"));
        let login = type_object(&payload, "login");
        assert_eq!(login.get("username"), Some(&serde_json::json!("login-user")));
        assert_eq!(login.get("password"), Some(&serde_json::json!("login-pass")));
        let keys: Vec<&str> = payload.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, vec!["folderId", "login", "name", "type"]);
    }

    #[test]
    fn a_secure_note_draft_creates_a_note_whose_body_is_item_level() {
        let payload = stuffed_draft(ItemKind::SecureNote).to_new_item().unwrap().to_payload();
        assert_eq!(payload["type"], serde_json::json!(2));
        assert_eq!(
            payload.as_object().unwrap().get("notes"),
            Some(&serde_json::json!("note-body")),
            "a note's body is item-level `notes`, not a field of its type object"
        );
        assert_eq!(type_object(&payload, "secureNote").get("type"), Some(&serde_json::json!(0)));
        assert!(
            payload.as_object().unwrap().get("login").is_none(),
            "a note draft carried the login fields the form also holds"
        );
    }

    #[test]
    fn a_card_draft_creates_a_card_with_the_six_captured_keys() {
        let payload = stuffed_draft(ItemKind::Card).to_new_item().unwrap().to_payload();
        assert_eq!(payload["type"], serde_json::json!(3));
        let card = type_object(&payload, "card");
        assert_eq!(card.get("cardholderName"), Some(&serde_json::json!("card-holder")));
        assert_eq!(card.get("brand"), Some(&serde_json::json!("card-brand")));
        assert_eq!(card.get("number"), Some(&serde_json::json!("card-number")));
        // Zero-padded and still a string: `item.card`'s captured template
        // sends `expMonth: "04"`, so a create must not turn it into 4.
        assert_eq!(card.get("expMonth"), Some(&serde_json::json!("04")));
        assert_eq!(card.get("expYear"), Some(&serde_json::json!("2028")));
        assert_eq!(card.get("code"), Some(&serde_json::json!("card-code")));
        assert_eq!(card.len(), 6);
    }

    #[test]
    fn an_identity_draft_creates_an_identity_with_every_modelled_key() {
        let payload = stuffed_draft(ItemKind::Identity).to_new_item().unwrap().to_payload();
        assert_eq!(payload["type"], serde_json::json!(4));
        let identity = type_object(&payload, "identity");
        // The row order `identity_rows` renders, so a mis-wired row shows up
        // as a value in the wrong key rather than as a missing one.
        for (key, value) in [
            ("title", "identity-0"),
            ("firstName", "identity-1"),
            ("middleName", "identity-2"),
            ("lastName", "identity-3"),
            ("email", "identity-4"),
            ("phone", "identity-5"),
            ("username", "identity-6"),
            ("company", "identity-7"),
            ("address1", "identity-8"),
            ("address2", "identity-9"),
            ("address3", "identity-10"),
            ("city", "identity-11"),
            ("state", "identity-12"),
            ("postalCode", "identity-13"),
            ("country", "identity-14"),
            ("ssn", "identity-15"),
            ("passportNumber", "identity-16"),
            ("licenseNumber", "identity-17"),
        ] {
            assert_eq!(
                identity.get(key),
                Some(&serde_json::json!(value)),
                "`{key}` did not carry the value its form row holds"
            );
        }
        assert_eq!(identity.len(), 18);
    }

    #[test]
    fn an_ssh_draft_creates_the_three_captured_ssh_keys() {
        // Key names from `.superpowers/sdd/item-shapes-capture.md`'s
        // 2026-08-01 capture of a real type-5 item, not from memory.
        let payload = stuffed_draft(ItemKind::SshKey).to_new_item().unwrap().to_payload();
        assert_eq!(payload["type"], serde_json::json!(5));
        let ssh = type_object(&payload, "sshKey");
        assert_eq!(ssh.get("privateKey"), Some(&serde_json::json!("ssh-private")));
        assert_eq!(ssh.get("publicKey"), Some(&serde_json::json!("ssh-public")));
        assert_eq!(ssh.get("keyFingerprint"), Some(&serde_json::json!("ssh-fingerprint")));
        assert_eq!(ssh.len(), 3);
    }

    #[test]
    fn a_blank_field_reaches_the_model_blank_and_the_model_prunes_it() {
        // Blank handling belongs to `NewItem::to_payload` (one rule, applied
        // to every kind). The form must not second-guess it by dropping or
        // defaulting a field on the way in -- if the two ever disagree about
        // what blank means, only the model's answer is on the wire.
        for (kind, type_key) in [
            (ItemKind::Login, "login"),
            (ItemKind::Card, "card"),
            (ItemKind::Identity, "identity"),
            (ItemKind::SshKey, "sshKey"),
        ] {
            let mut draft = EditDraft::empty_of(kind);
            draft.name = "Only a name".into();
            let payload = draft.to_new_item().unwrap().to_payload();
            assert!(
                type_object(&payload, type_key).is_empty(),
                "{kind:?}'s create payload invented a value for a field the user left blank"
            );
            assert_eq!(payload["name"], serde_json::json!("Only a name"));
            assert_eq!(payload["folderId"], serde_json::Value::Null);
        }
        // A blank note keeps its discriminator (0 is a real value) and gains
        // no `notes` key.
        let mut note = EditDraft::empty_of(ItemKind::SecureNote);
        note.name = "Only a name".into();
        let payload = note.to_new_item().unwrap().to_payload();
        assert_eq!(type_object(&payload, "secureNote").get("type"), Some(&serde_json::json!(0)));
        assert_eq!(payload.as_object().unwrap().get("notes"), None);
    }

    #[test]
    fn changing_a_drafts_kind_keeps_the_shared_fields_and_leaks_no_others() {
        // The same defect class as the login-grafting bug: a form that
        // remembers the kind the user abandoned puts that kind's data on the
        // wire under the kind they chose. Name and folder are item-level and
        // must survive; everything else must not follow.
        for kind in CREATABLE_KINDS {
            let mut draft = stuffed_draft(ItemKind::Login);
            draft.set_kind(kind);

            assert_eq!(draft.kind(), kind);
            assert_eq!(draft.name, "Everything", "{kind:?} lost the name the user typed");
            assert_eq!(
                draft.folder_id.as_deref(),
                Some("f2"),
                "{kind:?} lost the folder the user chose"
            );

            let payload = draft.to_new_item().unwrap().to_payload();
            if kind == ItemKind::Login {
                // Switching to the kind it already is changes nothing; that
                // is `re_selecting_the_kind_a_draft_already_has_changes_nothing`.
                continue;
            }

            // Nothing survives the switch, so the new kind's object is empty
            // even though the draft arrived with every kind's fields filled
            // in. Written as "empty", not "does not contain the login's two
            // values": a card that kept `card-holder` from before the switch
            // is the same defect and would slip past a string search for the
            // login's values.
            let type_key = match kind {
                ItemKind::SecureNote => "secureNote",
                ItemKind::Card => "card",
                ItemKind::Identity => "identity",
                ItemKind::SshKey => "sshKey",
                ItemKind::Login | ItemKind::Unknown(_) => unreachable!("handled above"),
            };
            let body = type_object(&payload, type_key);
            if kind == ItemKind::SecureNote {
                // Its one key is the `{"type": 0}` discriminator, which is
                // not user data.
                assert_eq!(body.get("type"), Some(&serde_json::json!(0)));
                assert_eq!(body.len(), 1);
            } else {
                assert!(
                    body.is_empty(),
                    "a {kind:?} create payload carried {body:?} over the switch"
                );
            }

            let serialised = serde_json::to_string(&payload).unwrap();
            for leaked in ["login-user", "login-pass", "note-body", "card-number", "identity-0"] {
                assert!(
                    !serialised.contains(leaked),
                    "a {kind:?} create payload carried `{leaked}`: {serialised}"
                );
            }
            assert!(
                payload.as_object().unwrap().get("login").is_none(),
                "a {kind:?} create payload carried a login object"
            );
        }

        // And the other direction: leaving a kind wipes the fields it owned,
        // so switching back does not resurrect them.
        let mut draft = stuffed_draft(ItemKind::Card);
        draft.set_kind(ItemKind::Login);
        draft.set_kind(ItemKind::Card);
        assert!(type_object(&draft.to_new_item().unwrap().to_payload(), "card").is_empty());
    }

    #[test]
    fn re_selecting_the_kind_a_draft_already_has_changes_nothing() {
        // The trap this pins: the type menu re-states its selection on every
        // frame. If `set_kind` cleared unconditionally, the create form would
        // erase itself as fast as the user could type into it.
        let mut draft = stuffed_draft(ItemKind::Card);
        for _ in 0..3 {
            draft.set_kind(ItemKind::Card);
        }
        let card = type_object(&draft.to_new_item().unwrap().to_payload(), "card").clone();
        assert_eq!(card.get("number"), Some(&serde_json::json!("card-number")));
        assert_eq!(card.len(), 6);
    }

    #[test]
    fn a_new_kinds_secret_starts_masked() {
        // Same rule as `reveal_password`: the toggle must be persistent draft
        // state, and a form the user has just switched into must not be
        // showing the previous kind's reveal decision.
        let mut draft = EditDraft::empty_of(ItemKind::SshKey);
        assert!(!draft.ssh_key.reveal_private_key);
        draft.ssh_key.reveal_private_key = true;
        draft.set_kind(ItemKind::Card);
        draft.set_kind(ItemKind::SshKey);
        assert!(!draft.ssh_key.reveal_private_key, "a reveal survived a trip through another kind");
    }

    #[test]
    fn the_ssh_form_offers_its_fields_only_where_they_can_be_saved() {
        // Creating an SSH key posts all three keys (`NewItem::ssh_key`), but
        // EDITING one cannot touch them: `VaultItem` has no `sshKey` field in
        // this build, so the object rides the `other` catch-all and
        // `apply_to` deliberately leaves it alone. Offering the fields in
        // edit mode would show boxes whose contents are silently discarded.
        assert_eq!(form_body(ItemKind::SshKey, true), FormBody::SshKey);
        assert_eq!(form_body(ItemKind::SshKey, false), FormBody::UneditableNotice);
        // An unknown type has no form either way, and cannot be created.
        assert_eq!(form_body(ItemKind::Unknown(9), true), FormBody::UneditableNotice);
        assert_eq!(form_body(ItemKind::Unknown(9), false), FormBody::UneditableNotice);
        for kind in [ItemKind::Login, ItemKind::SecureNote, ItemKind::Card, ItemKind::Identity] {
            assert_eq!(
                form_body(kind, true),
                form_body(kind, false),
                "{kind:?}'s form must not depend on whether the item exists yet"
            );
        }
    }

    #[test]
    fn creating_an_ssh_key_does_not_disturb_editing_one() {
        // The SSH draft fields are new; `apply_to` must still be the no-op it
        // was for a type-5 item, whatever they contain.
        let item = parse(SSH_WITH_EXTRAS);
        let mut draft = EditDraft::from_item(&item);
        draft.ssh_key.private_key = "leak".into();
        draft.ssh_key.public_key = "leak".into();
        draft.ssh_key.key_fingerprint = "leak".into();
        let before: serde_json::Value = serde_json::from_str(SSH_WITH_EXTRAS).unwrap();
        assert_eq!(before, serde_json::to_value(draft.apply_to(&item)).unwrap());
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
        // `.expect` is the only edit this test needed when `to_new_item`
        // became fallible: a login IS creatable, so `None` here is itself a
        // failure. Every assertion below is unchanged.
        let payload = draft.to_new_item().expect("a login draft has a create payload").to_payload();
        assert_eq!(payload["name"], serde_json::json!("New"));
        assert_eq!(payload["folderId"], serde_json::json!("f2"));
        assert_eq!(payload["type"], serde_json::json!(1));
    }

    // -----------------------------------------------------------------
    // The password generator
    // -----------------------------------------------------------------

    #[test]
    fn a_fresh_draft_generates_a_password_not_a_passphrase() {
        // The default matters: it is what the button does before anyone
        // touches the combo box.
        let draft = EditDraft::empty();
        assert!(matches!(
            draft.generator_request(),
            GenerateRequest::Password(_)
        ));
    }

    #[test]
    fn the_form_state_reaches_the_request_for_both_kinds() {
        let mut draft = EditDraft::empty();
        draft.generator.length = 32;
        match draft.generator_request() {
            GenerateRequest::Password(p) => assert_eq!(p.length, 32),
            other => panic!("expected a password request: {other:?}"),
        }

        draft.generator.passphrase = true;
        draft.generator.words = 7;
        match draft.generator_request() {
            GenerateRequest::Passphrase(p) => assert_eq!(p.words, 7),
            other => panic!("expected a passphrase request: {other:?}"),
        }
    }

    #[test]
    fn a_length_the_route_would_silently_raise_is_clamped_before_it_is_sent() {
        // THE "SUCCEEDS AND IGNORES YOU" GUARD. `bw serve` raises a `length`
        // below 5 to 5 and `words` below 3 to 3, answers 200, and says
        // nothing. A form showing 1 would therefore appear to work while
        // producing something else. Clamping here means the number on screen
        // and the secret that comes back agree.
        let mut draft = EditDraft::empty();
        draft.generator.length = 1;
        match draft.generator_request() {
            GenerateRequest::Password(p) => assert_eq!(p.length, MIN_LENGTH),
            other => panic!("expected a password request: {other:?}"),
        }

        draft.generator.passphrase = true;
        draft.generator.words = 1;
        match draft.generator_request() {
            GenerateRequest::Passphrase(p) => assert_eq!(p.words, MIN_WORDS),
            other => panic!("expected a passphrase request: {other:?}"),
        }
    }

    #[test]
    fn a_passphrase_and_a_password_do_not_share_one_number() {
        // `GeneratorDraft` keeps `length` and `words` apart on purpose. With
        // one shared field, setting 32 characters and then flipping the combo
        // box would ask for 32 WORDS -- a request that succeeds and produces
        // something absurd.
        let mut draft = EditDraft::empty();
        draft.generator.length = 32;
        draft.generator.passphrase = true;
        match draft.generator_request() {
            GenerateRequest::Passphrase(p) => assert_eq!(
                p.words,
                PassphraseRecipe::default().words,
                "flipping to a passphrase carried the character length over as a word count"
            ),
            other => panic!("expected a passphrase request: {other:?}"),
        }
    }

    #[test]
    fn generating_replaces_a_password_the_user_had_already_typed() {
        // The rule about what a generate REPLACES lives on the draft, not at
        // the call site -- "only if empty" and "append" are both readings of
        // the button that this pins out.
        let mut draft = EditDraft::empty();
        draft.password = "typed-by-hand".into();
        draft.set_generated_password("Fresh-Generated-1");
        assert_eq!(draft.password, "Fresh-Generated-1");
    }

    #[test]
    fn a_generated_password_does_not_unmask_the_box_by_itself() {
        // A deliberate deviation from Bitwarden's own generators, which show
        // what they produced. `reveal_password` is the user's toggle; a
        // generate flipping it would put a secret on screen that nobody asked
        // to see.
        let mut draft = EditDraft::empty();
        assert!(!draft.reveal_password, "the premise");
        draft.set_generated_password("Fresh-Generated-1");
        assert!(
            !draft.reveal_password,
            "generating a password unmasked the box on the user's behalf"
        );
    }

    #[test]
    fn switching_kind_keeps_the_generator_settings() {
        // `set_kind` clears every kind-specific field because carrying one
        // kind's DATA into another is how it reaches the wire under the wrong
        // type. The generator holds no item data -- two numbers and a switch
        // -- so it is exempt, and this pins that the exemption is deliberate
        // rather than a field someone forgot to add to `set_kind`.
        let mut draft = EditDraft::empty();
        draft.generator.length = 32;
        draft.generator.passphrase = true;
        draft.set_kind(ItemKind::Card);
        draft.set_kind(ItemKind::Login);
        assert_eq!(draft.generator.length, 32);
        assert!(draft.generator.passphrase);
    }

    #[test]
    fn the_generators_unoffered_options_come_from_the_crates_own_default_recipe() {
        // The form offers a kind and a size and nothing else, so everything
        // that makes the result STRONG -- all four character classes, a
        // minimum digit and symbol, ambiguous characters avoided -- is
        // supplied by `generator_request`. Nothing else in the app would
        // notice if it quietly stopped being.
        match EditDraft::empty().generator_request() {
            GenerateRequest::Password(p) => {
                assert!(p.uppercase && p.lowercase && p.number && p.special, "{p:?}");
                assert!(p.min_number >= 1 && p.min_special >= 1, "{p:?}");
                assert!(p.avoid_ambiguous, "{p:?}");
            }
            other => panic!("expected a password request: {other:?}"),
        }
    }
}

/// The generator row's **widget bindings**, which none of the tests above can
/// see.
///
/// `generator_request` and `set_generated_password` are pure, directly tested,
/// and were never the risk. What was untested is what the three widgets beside
/// the password box hand them, and a reviewer proved the gap with three
/// mutations that left the whole suite green: deleting the Generate button
/// outright (the feature ships inert, and `EditAction::GeneratePassword` being
/// `pub` means not even a dead-code warning), binding the combo's "Password"
/// entry to `true` (picking Password yields a passphrase), and pointing the
/// "N words" spinner at `generator.length` (setting "8 words" edits the
/// character count instead).
///
/// These are **behavioural**, not source-text guards. `draw_detail_edit` takes
/// a `&mut Ui` and returns its action, so a headless `egui::Context` really can
/// press these widgets -- the same harness `prefs_ui`, `detail.rs` and
/// `item_list.rs` already run. The file's one source guard,
/// `the_folder_dropdown_offers_exactly_the_assignable_folders`, says "no test
/// in this crate can click that combo box"; that was true of the loop it
/// guards, whose input is assembled outside the closure, but it is not true of
/// a click, and a guard that watches the pixels is worth more than one that
/// watches the spelling.
///
/// Every assertion is paired with a positive control, because each of these
/// tests has a way to pass while seeing nothing: a click that misses every
/// widget also leaves the action `None`, and a combo that never opened also
/// paints no entry to click.
#[cfg(test)]
mod generator_row_tests {
    use super::*;
    use eframe::egui::{Pos2, Rect};

    /// Wide enough that the generator row is not wrapped and tall enough that
    /// the combo's popup opens *downward*, which `popup_entry` relies on.
    ///
    /// **The height is a harness size, not a claim about the app.** These
    /// tests ask what the form *says*; the form scrolls, so a control below
    /// the fold is painted only if the harness pane is tall enough to hold the
    /// whole form. Raised from 900 when the app block gained the keystroke
    /// summary, and from 1100 when the login body gained the custom-fields
    /// block and the TOTP seed row. What the app can really be resized to is
    /// asserted separately and deliberately, against
    /// `MIN_PANE_WIDTH`/`MIN_PANE_HEIGHT` in `min_size_tests` -- those are
    /// the geometry pins and this is not one.
    const BODY: egui::Vec2 = egui::vec2(560.0, 1400.0);

    #[derive(Default)]
    struct Painted {
        texts: Vec<(String, Rect)>,
    }

    impl Painted {
        fn strings(&self) -> Vec<&str> {
            self.texts.iter().map(|(t, _)| t.as_str()).collect()
        }

        fn rects_of(&self, label: &str) -> Vec<Rect> {
            self.texts.iter().filter(|(t, _)| t == label).map(|(_, r)| *r).collect()
        }

        /// The one rect painting `label`, or a failure naming everything that
        /// *was* painted -- which is what turns "the button is gone" into a
        /// readable message instead of a silent no-op click.
        fn rect_of(&self, label: &str) -> Rect {
            let found = self.rects_of(label);
            assert_eq!(
                found.len(),
                1,
                "expected exactly one {label:?} in the edit form, found {}; painted: {:?}",
                found.len(),
                self.strings()
            );
            found[0]
        }

        /// The combo BUTTON showing `label`: the last rect painting it, since
        /// this form paints the field label "Password" above the row and the
        /// combo's own `selected_text` may spell the same word.
        fn combo_button(&self, label: &str) -> Rect {
            *self.rects_of(label).last().unwrap_or_else(|| {
                panic!(
                    "the generator combo is not showing {label:?}; painted: {:?}",
                    self.strings()
                )
            })
        }

        /// The combo popup's entry for `label`: the one painted BELOW the
        /// closed combo button, so it cannot be confused with the button's own
        /// `selected_text` (which spells one of the same two words).
        fn popup_entry(&self, label: &str, button: Rect) -> Rect {
            let found: Vec<Rect> = self
                .rects_of(label)
                .into_iter()
                .filter(|r| r.center().y > button.bottom())
                .collect();
            assert_eq!(
                found.len(),
                1,
                "expected exactly one {label:?} row in the open generator combo, found {}; \
                 painted: {:?}",
                found.len(),
                self.strings()
            );
            found[0]
        }
    }

    fn walk(shape: &egui::Shape, painted: &mut Painted) {
        match shape {
            egui::Shape::Text(text) => painted.texts.push((
                text.galley.text().to_string(),
                Rect::from_min_size(text.pos, text.galley.size()),
            )),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, painted);
                }
            }
            _ => {}
        }
    }

    fn raw_input(events: &[egui::Event]) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, BODY)),
            events: events.to_vec(),
            ..Default::default()
        }
    }

    /// A context with `theme::apply`'s fonts live. The two throwaway frames are
    /// the ones every other harness in this crate runs: a font set registered
    /// during a frame is only usable from the start of the next one.
    fn styled_context() -> egui::Context {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(raw_input(&[]), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(raw_input(&[]), |_ui| {});
        ctx
    }

    fn frame(
        ctx: &egui::Context,
        draft: &mut EditDraft,
        events: &[egui::Event],
    ) -> (EditAction, Painted) {
        let mut apps = AppIdentityCache::default();
        let mut action = EditAction::None;
        let output = ctx.run_ui(raw_input(events), |ui| {
            action = draw_detail_edit(ui, draft, &[], false, &mut apps, None, &detail::TotpState::NoSecret);
        });
        let mut painted = Painted::default();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut painted);
        }
        (action, painted)
    }

    /// A full press-and-release, which is what egui needs before it will report
    /// `Response::clicked` -- a press alone is not a click.
    fn click(pos: Pos2) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    /// A login draft with both sizes set to numbers neither default nor equal,
    /// so a spinner wired to the wrong field paints a visibly wrong number
    /// rather than a coincidentally right one.
    fn login_draft(passphrase: bool) -> EditDraft {
        let mut draft = EditDraft::empty();
        draft.generator = GeneratorDraft { passphrase, length: 33, words: 7 };
        draft
    }

    // -- the Generate button -----------------------------------------------

    #[test]
    fn clicking_generate_is_what_asks_the_caller_to_generate() {
        // Mutation this catches: delete the button. Nothing else in the crate
        // notices -- `EditAction::GeneratePassword` is `pub`, so its remaining
        // producers being zero is not even a warning, and every test of the
        // conversion functions keeps passing while the feature is inert.
        let ctx = styled_context();
        let mut draft = login_draft(false);

        let (idle, first) = frame(&ctx, &mut draft, &[]);
        assert_eq!(
            idle,
            EditAction::None,
            "the form reported an action on a frame with no input at all"
        );
        let button = first.rect_of("Generate");

        let (action, _) = frame(&ctx, &mut draft, &click(button.center()));
        assert_eq!(
            action,
            EditAction::GeneratePassword,
            "clicking Generate did not ask for a password; the button is decoration"
        );
    }

    #[test]
    fn a_click_that_misses_the_generate_button_generates_nothing() {
        // The positive control for the test above: if a click anywhere in the
        // form produced `GeneratePassword`, that test would pass with the
        // button deleted and Cancel under the cursor.
        let ctx = styled_context();
        let mut draft = login_draft(false);
        let (_, first) = frame(&ctx, &mut draft, &[]);
        let button = first.rect_of("Generate");

        let miss = Pos2::new(button.center().x, button.top() - 40.0);
        let (action, _) = frame(&ctx, &mut draft, &click(miss));
        assert_eq!(action, EditAction::None, "a click that hit nothing still generated");
    }

    // -- the kind combo ----------------------------------------------------

    /// Opens the combo and clicks the entry named `entry`, returning the draft
    /// afterwards. Four frames, and the shape is egui's: the popup only PAINTS
    /// on the frame after the button that opened it was clicked, so the frame
    /// that locates a row and the frame that clicks the button cannot be the
    /// same one.
    fn pick_generator_kind(start: bool, entry: &str) -> GeneratorDraft {
        let ctx = styled_context();
        let mut draft = login_draft(start);

        let (_, closed) = frame(&ctx, &mut draft, &[]);
        let button = closed.combo_button(if start { "Passphrase" } else { "Password" });

        let _ = frame(&ctx, &mut draft, &click(button.center()));
        let (_, open) = frame(&ctx, &mut draft, &[]);
        let row = open.popup_entry(entry, button);

        let _ = frame(&ctx, &mut draft, &click(row.center()));
        draft.generator.clone()
    }

    #[test]
    fn picking_password_in_the_combo_asks_for_a_password() {
        // Mutation this catches: `selectable_value(&mut draft.generator
        // .passphrase, true, "Password")`. The conversion test
        // `a_passphrase_and_a_password_do_not_share_one_number` keeps passing
        // through that -- it guards the conversion, and this is the binding.
        let generator = pick_generator_kind(true, "Password");
        assert!(
            !generator.passphrase,
            "the combo's \"Password\" row does not select a password"
        );
    }

    #[test]
    fn picking_passphrase_in_the_combo_asks_for_a_passphrase() {
        // The other direction, and the positive control for the one above: a
        // combo whose rows were both bound to `false` would satisfy that test
        // alone, as would one whose rows were inert with the draft already
        // holding the expected value.
        let generator = pick_generator_kind(false, "Passphrase");
        assert!(
            generator.passphrase,
            "the combo's \"Passphrase\" row does not select a passphrase"
        );
    }

    // -- the size spinner --------------------------------------------------

    /// The number a `DragValue` is showing, and the rect to grab it by.
    ///
    /// egui paints the value and the suffix as two separate galleys ("7" and
    /// " words"), so neither half alone identifies the widget and no assertion
    /// can match the joined string. This pairs them by adjacency: the text
    /// ending immediately left of the suffix, on the suffix's own line.
    fn spinner(painted: &Painted, suffix: &str) -> (String, Rect) {
        let suffix_rect = painted.rect_of(suffix);
        let mut left: Vec<&(String, Rect)> = painted
            .texts
            .iter()
            .filter(|(_, r)| {
                r.max.x <= suffix_rect.min.x + 1.0
                    && r.center().y > suffix_rect.top()
                    && r.center().y < suffix_rect.bottom()
            })
            .collect();
        left.sort_by(|a, b| a.1.max.x.total_cmp(&b.1.max.x));
        let (value, rect) = left.last().unwrap_or_else(|| {
            panic!(
                "nothing is painted left of the {suffix:?} suffix, so that spinner is showing \
                 no number at all; painted: {:?}",
                painted.strings()
            )
        });
        (value.clone(), rect.union(suffix_rect))
    }

    #[test]
    fn the_words_spinner_shows_and_edits_the_word_count() {
        // Mutation this catches: `DragValue::new(&mut draft.generator.length)`
        // under the passphrase arm. The suffix still reads " words" while the
        // number is the character count, so setting "8 words" silently retunes
        // the password length and the word count stays pinned at its default.
        let ctx = styled_context();
        let mut draft = login_draft(true);

        let (_, painted) = frame(&ctx, &mut draft, &[]);
        let (shown, rect) = spinner(&painted, " words");
        assert_eq!(
            shown, "7",
            "the words spinner is showing {shown:?}, not `generator.words` (7) -- it is bound \
             to the wrong number"
        );

        drag(&ctx, &mut draft, rect.center());
        assert_ne!(draft.generator.words, 7, "dragging the words spinner changed no word count");
        assert_eq!(
            draft.generator.length, 33,
            "dragging the words spinner moved the password's character count"
        );
    }

    #[test]
    fn the_chars_spinner_shows_and_edits_the_character_count() {
        // The mirror, and the positive control: without it, both spinners
        // could be bound to `words` and the test above would still pass.
        let ctx = styled_context();
        let mut draft = login_draft(false);

        let (_, painted) = frame(&ctx, &mut draft, &[]);
        let (shown, rect) = spinner(&painted, " chars");
        assert_eq!(
            shown, "33",
            "the chars spinner is showing {shown:?}, not `generator.length` (33) -- it is bound \
             to the wrong number"
        );

        drag(&ctx, &mut draft, rect.center());
        assert_ne!(
            draft.generator.length, 33,
            "dragging the chars spinner changed no character count"
        );
        assert_eq!(
            draft.generator.words, 7,
            "dragging the chars spinner moved the passphrase's word count"
        );
    }

    /// Press on a `DragValue` and pull it right across three frames. The
    /// distance is far more than one step at any drag speed egui picks, and
    /// the tests assert only *which* field moved, never by how much.
    fn drag(ctx: &egui::Context, draft: &mut EditDraft, from: Pos2) {
        let _ = frame(
            ctx,
            draft,
            &[
                egui::Event::PointerMoved(from),
                egui::Event::PointerButton {
                    pos: from,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        let to = from + egui::vec2(120.0, 0.0);
        let _ = frame(ctx, draft, &[egui::Event::PointerMoved(to)]);
        let _ = frame(
            ctx,
            draft,
            &[egui::Event::PointerButton {
                pos: to,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
    }

    // -- the app block ------------------------------------------------------
    //
    // These drive the WIDGETS. Every decision they exercise has a pure test in
    // `tests` above; what only these can see is whether the form calls it --
    // this crate's standing defect being a change that is correct in isolation
    // and never reaches the behaviour it claims. Each names the mutation it
    // catches.

    /// A draft with a binding, and no item behind it: the form reads
    /// `draft.app`, so a `VaultItem` would only be a longer way to set it.
    fn app_draft(m: &AppMatch) -> EditDraft {
        let mut draft = EditDraft::empty();
        draft.name = "Ledgerline".to_string();
        draft.app = Some(AppMatchDraft::from_match(m));
        draft
    }

    fn chrome() -> AppMatch {
        AppMatch {
            process: "chrome.exe".to_string(),
            title: String::new(),
            hosted: false,
            // Deliberately a path that does not exist, so the identity lookup
            // resolves the same way on every machine (to the file name) and no
            // assertion here depends on what is installed.
            //
            // And deliberately a file name that is NOT `process`. Chrome really
            // does ship a `chrome_proxy.exe` beside `chrome.exe`, so this is a
            // real shape rather than a contrivance -- but the reason it is here
            // is that the two inputs to `AppIdentityCache::label` are a path and
            // a process name, and a fixture where those agree cannot tell which
            // one the form passed. Measured: with both reading `chrome.exe`,
            // `label(ctx, &app.process, &app.process)` -- which probes a bare
            // relative name, so no description ever resolves and there is never
            // an icon -- passed every test in this file.
            path: r"C:\Deskwarden Test\Chrome\chrome_proxy.exe".to_string(),
            args: "--profile-directory=WorkProfile".to_string(),
            sequence: String::new(),
            trigger: TriggerMode::Prompt,
        }
    }

    fn store() -> AppMatch {
        AppMatch {
            process: "Speedtest.exe".to_string(),
            // Not "Speedtest": the block paints a name and a title, and a title
            // that is a prefix of the process name is a title that cannot say
            // which of the two was drawn. Real Store frames are titled like
            // this anyway.
            title: "Speedtest by Ookla".to_string(),
            hosted: true,
            path: String::new(),
            // A Store binding that carries arguments: they are unreachable in
            // the form (the row is disabled and says so) and they must not be
            // saved. See `to_match`.
            args: "--headless".to_string(),
            sequence: String::new(),
            trigger: TriggerMode::Hotkey,
        }
    }

    /// Click `pos`, then type `text` into whatever took focus. Two frames,
    /// because egui gives a widget focus on the frame the click lands and reads
    /// text events on the next.
    fn click_and_type(ctx: &egui::Context, draft: &mut EditDraft, pos: Pos2, text: &str) {
        let _ = frame(ctx, draft, &click(pos));
        let _ = frame(ctx, draft, &[egui::Event::Text(text.to_string())]);
    }

    #[test]
    fn the_form_draws_an_app_block_for_a_bound_item() {
        // Mutation this catches: delete the `app_block(ui, app, apps)` call in
        // `draw_detail_edit`. Every pure test of the draft, of `app_match_edit`
        // and of `apply_to` keeps passing while the whole block is invisible --
        // which is exactly the shape of defect this crate keeps shipping.
        let ctx = styled_context();
        let mut draft = app_draft(&chrome());
        let (_, painted) = frame(&ctx, &mut draft, &[]);
        let strings = painted.strings();
        for needle in [
            APP_BLOCK_HEADING,
            APP_PATH_LABEL,
            APP_ARGS_LABEL,
            "Browse\u{2026}",
            "Choose a running app\u{2026}",
            "Remove app match",
        ] {
            assert!(strings.contains(&needle), "the app block is missing {needle:?}: {strings:?}");
        }
        // The app's NAME, not the raw path, is the heading of the block; the
        // lookup has not answered yet on the frame after the first, so this is
        // the file name -- which is what `app_identity` promises as its
        // placeholder and its last fallback both.
        //
        // **The PATH's file name, and that is the assertion.** The mutation this
        // catches is `apps.label(ui.ctx(), &app.process, &app.process)`: the
        // identity lookup fed the match's process name where the image path
        // belongs. It probes a bare relative name, so `FileDescription` never
        // resolves and `SHGetFileInfoW` never finds an icon -- the block shows
        // `chrome.exe` for ever and is never able to say "Google Chrome". Every
        // test in this file passed under it while the fixture's path and
        // process agreed.
        assert!(
            strings.contains(&"chrome_proxy.exe"),
            "the app block is not named after the program file it is bound to: {strings:?}"
        );
        assert!(
            !strings.contains(&"chrome.exe"),
            "the app block is named after `process`, not after the path: {strings:?}"
        );
    }

    /// **Replaces `a_form_with_no_binding_draws_no_app_block`, which had come
    /// to describe the bug.** That test asserted the form drew NO app section
    /// at all for an unbound item, which is exactly why the edit form could
    /// not create a binding: the only way to get one was the tray's separate
    /// "Add app..." picker. It was written as the positive control for
    /// `the_form_draws_an_app_block_for_a_bound_item` -- so that a block drawn
    /// unconditionally could not satisfy that test -- and this keeps that job
    /// by asserting the two states are DIFFERENT, rather than that one of them
    /// is empty.
    #[test]
    fn a_form_with_no_binding_offers_the_control_that_makes_one() {
        let ctx = styled_context();
        let mut draft = EditDraft::empty();
        draft.name = "Ledgerline".to_string();
        // The premise, first: this fixture really has no binding, so
        // everything below is about the empty case and not about a draft that
        // quietly carried one.
        assert!(draft.app.is_none(), "the fixture is not an unbound draft");

        let (_, painted) = frame(&ctx, &mut draft, &[]);
        let strings = painted.strings();
        let mut checked = 0;
        for needle in [APP_BLOCK_HEADING, APP_NONE_NOTICE, APP_ADD_BUTTON] {
            checked += 1;
            assert!(
                strings.contains(&needle),
                "an item with no binding is not offered {needle:?}, so there is no way to \
                 make one from the edit form: {strings:?}"
            );
        }
        assert_eq!(checked, 3, "the loop visited nothing, so it asserted nothing");

        // ... and it is the ADD state, not the edit block drawn over an empty
        // draft. This is what keeps `the_form_draws_an_app_block_for_a_bound_item`
        // meaningful: a block drawn unconditionally fails here.
        for absent in [APP_PATH_LABEL, APP_ARGS_LABEL, "Remove app match", "Browse\u{2026}"] {
            assert!(
                !strings.contains(&absent),
                "the editing control {absent:?} is drawn for an item bound to nothing: \
                 {strings:?}"
            );
        }
    }

    #[test]
    fn clicking_add_an_app_opens_the_same_block_an_existing_binding_gets() {
        // Mutation this catches: `app_add_block` drawn but wired to nothing --
        // a button that paints, highlights on hover, and leaves `draft.app`
        // `None` for ever. Every pure test of `unbound`, `is_blank` and
        // `app_match_edit` keeps passing while the control does nothing, which
        // is this crate's standing defect.
        let ctx = styled_context();
        let mut draft = EditDraft::empty();
        draft.name = "Ledgerline".to_string();
        let (_, before) = frame(&ctx, &mut draft, &[]);
        let button = before.rect_of(APP_ADD_BUTTON);

        // Two frames: the click lands during the first, which has already
        // painted the add control by the time the button reports it. The
        // block it opened is what the SECOND frame draws.
        let _ = frame(&ctx, &mut draft, &click(button.center()));
        let (_, after) = frame(&ctx, &mut draft, &[]);
        let app = draft.app.as_ref().expect("clicking Add did not create a draft binding");
        // Blank, so a click and a Save writes nothing -- the pure half of that
        // is `clicking_add_and_choosing_nothing_writes_no_binding`.
        assert!(app.is_blank(), "clicking Add invented a binding: {app:?}");
        // ...and the running-app list was NOT enumerated as a side effect of
        // the click. See `app_add_block`'s doc: that is an `EnumWindows` walk.
        assert!(!app.picking, "Add opened the window picker by itself");
        assert!(app.windows.is_empty(), "Add enumerated the desktop's windows");

        let strings = after.strings();
        let mut checked = 0;
        for needle in [
            APP_BLOCK_HEADING,
            APP_PATH_LABEL,
            APP_ARGS_LABEL,
            "Browse\u{2026}",
            "Choose a running app\u{2026}",
            "Remove app match",
        ] {
            checked += 1;
            assert!(
                strings.contains(&needle),
                "after clicking Add the form does not offer {needle:?} -- the add path is \
                 not the edit block: {strings:?}"
            );
        }
        assert_eq!(checked, 6, "the loop visited nothing, so it asserted nothing");
        assert!(
            !strings.contains(&APP_ADD_BUTTON),
            "the Add button is still drawn beside the block it opened: {strings:?}"
        );
    }

    #[test]
    fn a_click_that_misses_add_an_app_binds_nothing() {
        // The positive control for the test above: without it, a form that
        // created a binding on EVERY frame -- or on any click anywhere --
        // would satisfy it.
        let ctx = styled_context();
        let mut draft = EditDraft::empty();
        draft.name = "Ledgerline".to_string();
        let (_, painted) = frame(&ctx, &mut draft, &[]);
        let button = painted.rect_of(APP_ADD_BUTTON);
        let miss = Pos2::new(button.center().x, button.bottom() + 60.0);
        assert!(
            !button.contains(miss),
            "the miss is inside the button, so this test asserts nothing"
        );
        let _ = frame(&ctx, &mut draft, &click(miss));
        assert!(draft.app.is_none(), "a click that missed Add bound the item anyway");
    }

    #[test]
    fn the_trigger_modes_are_still_gone() {
        // A guard, not a feature test. The three Auto / Prompt / Hotkey pills
        // were deliberately removed from this form (nothing in this build
        // reads `AppMatch::trigger`; the behaviour is the one global
        // "Prompt on match" setting), and the risk a NEW app control brings is
        // that they come back with it -- a fresh binding is the one case where
        // "which mode should this be?" looks like a question worth asking.
        //
        // Both states, because `the_edit_form_offers_no_autofill_trigger_control`
        // above only ever looks at a form drawn from an existing binding: the
        // add path and the block it opens are two more places a pill could be
        // painted.
        let ctx = styled_context();
        let mut unbound = EditDraft::empty();
        unbound.name = "Ledgerline".to_string();
        let (_, before_add) = frame(&ctx, &mut unbound, &[]);
        let button = before_add.rect_of(APP_ADD_BUTTON);
        // Two frames -- the block the click opened is drawn by the second.
        let _ = frame(&ctx, &mut unbound, &click(button.center()));
        let (_, after_add) = frame(&ctx, &mut unbound, &[]);
        assert!(
            unbound.app.is_some(),
            "Add did not open the block, so the second state below is the first one again"
        );
        // ...and the state below really is the block, not the add control
        // again, which would make this a second copy of the first state.
        assert!(
            after_add.strings().contains(&"Remove app match"),
            "the second state is not the opened block: {:?}",
            after_add.strings()
        );

        let mut states = 0;
        for (what, painted) in [("the add control", &before_add), ("a new binding", &after_add)] {
            states += 1;
            let strings = painted.strings();
            // The premise: the app section really is on screen in this state,
            // so every absence below is about the pills.
            assert!(
                strings.contains(&APP_BLOCK_HEADING),
                "{what} draws no app section at all, so this state asserts nothing: \
                 {strings:?}"
            );
            assert!(
                !strings.contains(&"Autofill"),
                "{what} draws the row the pills sat in: {strings:?}"
            );
            let mut modes = 0;
            for mode in detail::TRIGGER_ORDER {
                modes += 1;
                assert!(
                    !strings.contains(&detail::trigger_label(mode)),
                    "{what} draws the {mode:?} pill: {strings:?}"
                );
                assert!(
                    !strings.contains(&detail::trigger_caption(mode)),
                    "{what} draws the {mode:?} caption: {strings:?}"
                );
            }
            assert_eq!(modes, 3, "the mode loop visited nothing for {what}");
        }
        assert_eq!(states, 2, "the state loop visited nothing, so it asserted nothing");
    }

    #[test]
    fn typing_in_the_arguments_box_edits_the_arguments() {
        // Mutation this catches: `theme::text_field(ui, &mut app.path, false)`
        // under the arguments label. The box still appears, the label still says
        // "Command-line arguments", and every keystroke silently retunes the
        // program path instead -- which the pure tests cannot see, because they
        // set the field directly.
        let ctx = styled_context();
        let mut draft = app_draft(&chrome());
        let (_, painted) = frame(&ctx, &mut draft, &[]);
        let box_rect = painted.rect_of("--profile-directory=WorkProfile");

        click_and_type(&ctx, &mut draft, box_rect.center(), "Z");
        let app = draft.app.as_ref().unwrap();
        assert!(
            app.args.contains('Z'),
            "typing in the arguments box changed no arguments: {:?}",
            app.args
        );
        assert_eq!(
            app.path,
            chrome().path,
            "typing in the arguments box moved the program path"
        );
    }

    #[test]
    fn typing_in_the_path_box_edits_the_path() {
        // The mirror, and the positive control: without it, both boxes could be
        // bound to `args` and the test above would still pass.
        let ctx = styled_context();
        let mut draft = app_draft(&chrome());
        let (_, painted) = frame(&ctx, &mut draft, &[]);
        let box_rect = painted.rect_of(chrome().path.as_str());

        click_and_type(&ctx, &mut draft, box_rect.center(), "Z");
        let app = draft.app.as_ref().unwrap();
        assert_ne!(app.path, chrome().path, "typing in the path box changed no path");
        assert_eq!(
            app.args,
            chrome().args,
            "typing in the path box moved the command-line arguments"
        );
    }

    /// **Hand-typing a new program file re-points the match.**
    ///
    /// The mutation this catches is the whole of the path row's body reduced to
    /// `app.path = typed`, and it survived every test in this file: the pure
    /// test calls `set_path` directly, the browse test goes through
    /// `EditDraft::set_app_path`, and `typing_in_the_path_box_edits_the_path`
    /// looks only at `path` and `args`. Nothing looked at `process`.
    ///
    /// What that costs the user: they retype Program file from Chrome's
    /// executable to Edge's, Save, and the binding still keys on `chrome.exe`.
    /// The item goes on filling Chrome, and Open refuses -- `launchable_path`
    /// requires the path's file name to BE `process`, and it no longer is.
    ///
    /// So this drives the box far enough that the typed file name really
    /// differs from the process it started on, and asserts the derivation
    /// followed.
    #[test]
    fn typing_a_new_program_file_re_points_the_match() {
        let ctx = styled_context();
        let mut draft = app_draft(&chrome());
        assert_eq!(draft.app.as_ref().unwrap().process, "chrome.exe");

        let (_, painted) = frame(&ctx, &mut draft, &[]);
        let box_rect = painted.rect_of(chrome().path.as_str());

        // Click in, select the whole path, and type a different one over it --
        // a different DIRECTORY and a different file name, which is what
        // re-pointing an item at another browser looks like.
        let _ = frame(&ctx, &mut draft, &click(box_rect.center()));
        let typed = r"C:\Deskwarden Test\Edge\msedge.exe";
        let _ = frame(
            &ctx,
            &mut draft,
            &[
                egui::Event::Key {
                    key: egui::Key::A,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::COMMAND,
                },
                egui::Event::Text(typed.to_string()),
            ],
        );

        let app = draft.app.as_ref().unwrap();
        assert_eq!(app.path, typed, "the box did not take the typed path");
        assert_eq!(
            app.process, "msedge.exe",
            "the typed path did not re-derive `process`: this item still fills chrome.exe and \
             cannot open the program it now names"
        );
        assert!(
            app.to_match().launchable_path().is_some(),
            "a hand-typed path must be one the launcher would accept, which is the whole reason \
             `process` is derived rather than checked"
        );
    }

    #[test]
    fn clicking_browse_asks_the_caller_to_open_the_file_dialog() {
        // Mutation this catches: delete the button, or drop the
        // `action = Some(EditAction::PickAppFile)`. `EditAction::PickAppFile` is
        // `pub`, so having no producers left is not even a warning.
        let ctx = styled_context();
        let mut draft = app_draft(&chrome());
        let (idle, painted) = frame(&ctx, &mut draft, &[]);
        assert_eq!(idle, EditAction::None, "the form reported an action with no input");

        let (action, _) = frame(&ctx, &mut draft, &click(painted.rect_of("Browse\u{2026}").center()));
        assert_eq!(action, EditAction::PickAppFile);
    }

    #[test]
    fn a_click_that_misses_browse_asks_for_nothing() {
        // The positive control: if any click in the form reported `PickAppFile`,
        // the test above would pass with the button deleted.
        let ctx = styled_context();
        let mut draft = app_draft(&chrome());
        let (_, painted) = frame(&ctx, &mut draft, &[]);
        let button = painted.rect_of("Browse\u{2026}");
        let miss = Pos2::new(button.center().x, button.top() - 60.0);
        let (action, _) = frame(&ctx, &mut draft, &click(miss));
        assert_ne!(action, EditAction::PickAppFile, "a click that hit nothing opened the dialog");
    }

    #[test]
    fn clicking_remove_stages_the_removal_rather_than_performing_it() {
        // Mutation this catches: delete `app.bound = false`. The button stays,
        // clicking it does nothing, and `app_match_edit`'s `Remove` arm becomes
        // unreachable while its own test keeps passing.
        let ctx = styled_context();
        let mut draft = app_draft(&chrome());
        let (_, painted) = frame(&ctx, &mut draft, &[]);

        let _ = frame(&ctx, &mut draft, &click(painted.rect_of("Remove app match").center()));
        assert!(!draft.app.as_ref().unwrap().bound, "Remove did not stage a removal");
        // A frame LATER: the block above was laid out before the click was
        // processed, so the frame that carries the click still paints the
        // pre-click state.
        let (_, after) = frame(&ctx, &mut draft, &[]);
        // Nothing has been written: the fields are still there, and the block
        // says what is about to happen and offers the way back.
        assert_eq!(draft.app.as_ref().unwrap().args, chrome().args);
        assert!(
            after.strings().contains(&"Undo remove"),
            "a staged removal offers no way back: {:?}",
            after.strings()
        );
    }

    /// **The edit form offers no autofill trigger control.**
    ///
    /// `d5d5d64` made what a matched foreground window does one global
    /// preference (`crate::settings::Settings::prompt_on_match`) and stopped
    /// anything reading [`AppMatch::trigger`]. The read-only card's pills went
    /// with it; this form's did not, so the user could still pick Auto /
    /// Prompt / Hotkey here, have the choice persisted, supersede the item's
    /// `revisionDate` to record it, and observe no difference at all.
    ///
    /// **The stored field is still written**, untouched -- see
    /// `an_edit_carries_the_stored_trigger_through_untouched`. It is the
    /// CHOICE that is retired, not the byte on disk, which v0.5.0 requires.
    ///
    /// Driven at the form on a live binding -- exactly where the pills were
    /// painted -- rather than asserted on `app_block`'s source, so a control
    /// re-added anywhere in the block fails this.
    #[test]
    fn the_edit_form_offers_no_autofill_trigger_control() {
        let ctx = styled_context();
        let mut draft = app_draft(&chrome());
        let (_, painted) = frame(&ctx, &mut draft, &[]);
        let strings = painted.strings();

        // The premise: the block IS drawn, so every absence below is about the
        // control and not about a block that failed to appear at all.
        assert!(
            strings.contains(&APP_BLOCK_HEADING),
            "the app block is not drawn, so this test asserts nothing: {strings:?}"
        );
        assert!(
            strings.contains(&"Remove app match"),
            "the app block is not drawn, so this test asserts nothing: {strings:?}"
        );

        assert!(
            !strings.contains(&"Autofill"),
            "the row the pills sat in is still drawn, so the form still presents a per-item                  autofill setting: {strings:?}"
        );
        let mut checked = 0;
        for mode in detail::TRIGGER_ORDER {
            checked += 1;
            assert!(
                !strings.contains(&detail::trigger_label(mode)),
                "the {mode:?} pill is still drawn in the edit form: {strings:?}"
            );
            assert!(
                !strings.contains(&detail::trigger_caption(mode)),
                "the {mode:?} caption is still drawn in the edit form: {strings:?}"
            );
        }
        assert_eq!(checked, 3, "the loop visited no modes, so it asserted nothing");
    }

    #[test]
    fn a_store_app_gets_a_read_only_path_row_and_keeps_its_remove() {
        // The user's own choice for this case: state the reason, disable the
        // file picker, leave the rest of the block working.
        let ctx = styled_context();
        let mut draft = app_draft(&store());
        let (_, painted) = frame(&ctx, &mut draft, &[]);
        let strings = painted.strings();

        assert!(
            strings.contains(&APP_PATH_STORE_APP),
            "a Store binding does not say why it has no path: {strings:?}"
        );
        assert!(
            strings.contains(&"Speedtest by Ookla"),
            "the window title is not shown: {strings:?}"
        );
        // The arguments row is drawn, says why it is disabled, and does NOT
        // show the string the draft is still carrying -- which is exactly why
        // that string must not be saved. See the `to_match` assertion below.
        assert!(
            strings.contains(&APP_ARGS_STORE_APP),
            "a Store binding does not say why it has no arguments: {strings:?}"
        );
        assert!(
            !strings.contains(&"--headless"),
            "arguments that cannot be edited were nonetheless shown: {strings:?}"
        );
        assert_eq!(
            draft.app.as_ref().unwrap().to_match().args,
            "",
            "a Store binding saved arguments the user could neither see nor clear"
        );
        assert!(strings.contains(&"Remove app match"), "Remove is gone: {strings:?}");
        // The word the user must never see.
        assert!(
            !strings.iter().any(|s| s.to_lowercase().contains("hosted")),
            "the mechanism reached the screen: {strings:?}"
        );

        // Browse is drawn but refuses: clicking it asks for nothing.
        let (action, _) = frame(&ctx, &mut draft, &click(painted.rect_of("Browse\u{2026}").center()));
        assert_ne!(
            action,
            EditAction::PickAppFile,
            "a Store binding offered a file dialog whose answer it could not use"
        );

        // ...and Remove still works, which is the half of the requirement a
        // wholesale `add_enabled(false)` around the block would have broken.
        // The trigger pills held this half until they were retired; Remove is
        // the live control that is left, and it dies to exactly that mutation.
        let (_, painted) = frame(&ctx, &mut draft, &[]);
        assert!(draft.app.as_ref().unwrap().bound, "the premise: it starts bound");
        let remove = painted.rect_of("Remove app match");
        let _ = frame(&ctx, &mut draft, &click(remove.center()));
        assert!(
            !draft.app.as_ref().unwrap().bound,
            "a Store binding's Remove is inert, so the block really was disabled wholesale"
        );
    }

    #[test]
    fn opening_the_process_picker_is_one_click_and_lists_what_windows_there_are() {
        // The enumeration is the real desktop's, so nothing here asserts a row.
        // What it asserts is that the list opens at all, that it is NOT populated
        // before it is opened (the per-frame-I/O mutation), and that it closes
        // again.
        let ctx = styled_context();
        let mut draft = app_draft(&chrome());
        let (_, painted) = frame(&ctx, &mut draft, &[]);
        assert!(!draft.app.as_ref().unwrap().picking);
        assert!(
            draft.app.as_ref().unwrap().windows.is_empty(),
            "the desktop was enumerated before anyone asked"
        );

        let open = painted.rect_of("Choose a running app\u{2026}");
        let (_, listed) = frame(&ctx, &mut draft, &click(open.center()));
        assert!(draft.app.as_ref().unwrap().picking, "the picker did not open");
        assert!(
            listed.strings().contains(&"Refresh") && listed.strings().contains(&"Close list"),
            "the open picker has no controls: {:?}",
            listed.strings()
        );

        let (_, after) = frame(&ctx, &mut draft, &[]);
        let _ = frame(&ctx, &mut draft, &click(after.rect_of("Close list").center()));
        assert!(!draft.app.as_ref().unwrap().picking, "the picker did not close");
    }

    /// A row for the running-app picker, injected rather than enumerated.
    ///
    /// The enumeration is the real desktop's, which is why the picker's click
    /// wiring had no test at all -- there was no row anyone could be sure of
    /// clicking. But the list the picker draws is `AppMatchDraft::windows`, a
    /// plain `pub Vec` filled on the button click and read on every frame after
    /// it, so a test can simply put a row there and open the list. Nothing in
    /// production is bent to allow it: this is the same field the button writes.
    fn picker_row(title: &str, exe: &str) -> AppWindowRow {
        AppWindowRow {
            title: title.to_string(),
            exe_name: exe.to_string(),
            // A directory that is NOT the exe name and NOT the fixture's, so
            // "the path came from this row" cannot be satisfied by any other
            // string in the form.
            exe_path: format!(r"C:\Deskwarden Test\Picked\{exe}"),
            hosted: false,
            pid: 4242,
            hwnd: 909,
        }
    }

    /// The label `app_window_picker` paints for a row, spelled here in the
    /// pieces the test supplies rather than by calling the production
    /// formatter, so a row that stops being drawn cannot be found by this test
    /// agreeing with itself.
    fn row_label(title: &str, exe: &str) -> String {
        format!("{title}  \u{b7}  {exe}")
    }

    #[test]
    fn clicking_a_row_in_the_running_app_picker_binds_the_item_to_that_row() {
        // Mutation this catches: dropping the `app.choose_window(&row)` after
        // the loop (or the `chosen = Some(index)` inside it). The list still
        // opens, the rows still draw, every click is a silent no-op, and
        // `choose_window`'s own pure test keeps passing.
        let ctx = styled_context();
        let mut draft = app_draft(&chrome());
        {
            let app = draft.app.as_mut().unwrap();
            app.picking = true;
            app.windows = vec![picker_row("Ledgerline - Invoices", "Ledgerline.exe")];
        }

        let (_, listed) = frame(&ctx, &mut draft, &[]);
        let row = listed.rect_of(row_label("Ledgerline - Invoices", "Ledgerline.exe").as_str());
        let _ = frame(&ctx, &mut draft, &click(row.center()));

        let app = draft.app.as_ref().unwrap();
        assert_eq!(app.process, "Ledgerline.exe", "clicking a row bound nothing");
        assert_eq!(app.path, r"C:\Deskwarden Test\Picked\Ledgerline.exe");
        assert_eq!(app.args, chrome().args, "choosing an app threw away the user's arguments");
        assert!(!app.picking, "choosing a row left the list open");
    }

    #[test]
    fn a_click_that_misses_every_row_binds_nothing() {
        // The positive control: without it, a `choose_window` called
        // unconditionally at the end of the picker -- on whatever row happened
        // to be first -- would satisfy the test above.
        let ctx = styled_context();
        let mut draft = app_draft(&chrome());
        {
            let app = draft.app.as_mut().unwrap();
            app.picking = true;
            app.windows = vec![picker_row("Ledgerline - Invoices", "Ledgerline.exe")];
        }

        let (_, listed) = frame(&ctx, &mut draft, &[]);
        let row = listed.rect_of(row_label("Ledgerline - Invoices", "Ledgerline.exe").as_str());
        let miss = Pos2::new(row.center().x, row.top() - 200.0);
        let _ = frame(&ctx, &mut draft, &click(miss));

        let app = draft.app.as_ref().unwrap();
        assert_eq!(app.process, "chrome.exe", "a click that hit no row re-pointed the item");
        assert!(app.picking, "a click that hit no row closed the list");
    }

    #[test]
    fn a_row_that_names_the_window_host_cannot_be_clicked() {
        // The refusal has a pure test; what only this can see is whether the
        // form honours it. Mutation this catches: `ui.add_enabled(true, ..)`.
        let ctx = styled_context();
        let mut draft = app_draft(&chrome());
        {
            let app = draft.app.as_mut().unwrap();
            app.picking = true;
            app.windows = vec![picker_row("Speedtest by Ookla", "ApplicationFrameHost.exe")];
        }

        let (_, listed) = frame(&ctx, &mut draft, &[]);
        let row =
            listed.rect_of(row_label("Speedtest by Ookla", "ApplicationFrameHost.exe").as_str());
        let _ = frame(&ctx, &mut draft, &click(row.center()));

        assert_eq!(
            draft.app.as_ref().unwrap().process,
            "chrome.exe",
            "a row Windows could not attribute bound this item to the frame host, which would \
             fill it into every Store app on the machine"
        );
    }

    /// **The `icon_probe` tripwire, answered.**
    ///
    /// `theme::icon_probe` identifies every drawn icon in this app by vertex
    /// count alone, and `theme::no_two_drawn_icons_share_a_vertex_count` is a
    /// live tripwire against two of them colliding. The app block paints
    /// something none of them are -- a bitmap, from an executable's shell icon
    /// -- so the question is whether that bitmap can be mistaken for a drawn
    /// one.
    ///
    /// It cannot, and this measures it rather than asserting it: an
    /// `egui::Image` paints a `Shape::Mesh`, and every probe matches
    /// `Shape::Path` (or `Shape::Circle`, for the kebab). A mesh has no
    /// `points` for a vertex count to be taken off. Two textures are drawn, at
    /// two sizes, so the answer cannot be "this one happened not to collide".
    #[test]
    fn an_app_icon_bitmap_is_not_mistaken_for_any_drawn_icon() {
        let ctx = styled_context();
        let pixels = egui::ColorImage::from_rgba_unmultiplied([4, 4], &[200u8; 4 * 4 * 4]);
        let texture =
            ctx.load_texture("app-icon-probe-test", pixels, egui::TextureOptions::default());

        let output = ctx.run_ui(raw_input(&[]), |ui| {
            ui.add(egui::Image::new(&texture).fit_to_exact_size(egui::vec2(18.0, 18.0)));
            ui.add(egui::Image::new(&texture).fit_to_exact_size(egui::vec2(24.0, 24.0)));
        });
        let all = egui::Shape::Vec(output.shapes.iter().map(|c| c.shape.clone()).collect());

        assert!(theme::icon_probe::stars(&all).is_empty(), "a bitmap was read as a star");
        assert!(theme::icon_probe::eyes(&all).is_empty(), "a bitmap was read as an eye");
        assert!(
            theme::icon_probe::tune_icons(&all).is_empty(),
            "a bitmap was read as a Preferences tune icon"
        );
        assert!(
            theme::icon_probe::kebab_dots(&all).is_empty(),
            "a bitmap was read as a kebab dot"
        );
        assert!(
            theme::icon_probe::chevrons(&all).is_empty(),
            "a bitmap was read as a chevron"
        );

        // Positive control: the probes are not simply blind. A real drawn icon
        // in the same tree IS found, so "found nothing" above is a fact about
        // the bitmap and not about the walker.
        let output = ctx.run_ui(raw_input(&[]), |ui| {
            theme::eye_toggle(ui, false);
        });
        let drawn = egui::Shape::Vec(output.shapes.iter().map(|c| c.shape.clone()).collect());
        assert!(
            !theme::icon_probe::eyes(&drawn).is_empty(),
            "control: the eye probe finds no eye even when one is drawn"
        );
    }
}

/// The keystroke builder: its decisions, its wiring, and its geometry.
///
/// Every decision the block makes is one of the pure functions above, and each
/// is called here directly rather than through a frame. The three tests that
/// DO run a frame are the wiring pins -- they exist to fail when the call that
/// draws the block, or the assignment that saves it, is deleted.
#[cfg(test)]
mod sequence_builder_tests {
    use super::*;
    use eframe::egui::{Pos2, Rect, Vec2};

    // -- fixtures ----------------------------------------------------------
    //
    // Every value differs from every other one, deliberately: a fixture whose
    // password equalled its user name would let a `{PASSWORD}` that resolved
    // the user name pass every assertion below.

    const USERNAME: &str = "ada@contoso.test";
    const PASSWORD: &str = "correct-horse-battery";
    const PIN: &str = "8421";
    const TOTP_CODE: &str = "776699";

    fn vault_field(name: &str, value: &str) -> crate::vault_bridge::VaultField {
        crate::vault_bridge::VaultField {
            name: Some(name.to_string()),
            value: Some(Zeroizing::new(value.to_string())),
            other: serde_json::Map::new(),
        }
    }

    /// The user's own case: a Microsoft-365-shaped login with a PIN field, so
    /// `{S:PIN}` is discoverable and resolvable.
    fn item() -> VaultItem {
        VaultItem {
            id: "item-1".to_string(),
            name: "Contoso 365".to_string(),
            fields: vec![vault_field("PIN", PIN)],
            login: Some(LoginData {
                username: Some(USERNAME.to_string()),
                password: Some(PASSWORD.to_string().into()),
                totp: Some("JBSWY3DPEHPK3PXP".to_string().into()),
                uris: Vec::new(),
                other: serde_json::Map::new(),
            }),
            card: None,
            identity: None,
            ssh_key: None,
            notes: None,
            item_type: Some(1),
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        }
    }

    fn draft_for(item: &VaultItem, sequence: &str) -> EditDraft {
        let mut draft = EditDraft::from_item(item);
        draft.app = Some(AppMatchDraft::from_match(&AppMatch {
            process: "msedge.exe".to_string(),
            title: String::new(),
            hosted: false,
            path: r"C:\Deskwarden Test\Edge\msedge.exe".to_string(),
            args: String::new(),
            sequence: sequence.to_string(),
            trigger: TriggerMode::Prompt,
        }));
        draft
    }

    fn live_code() -> detail::TotpState {
        detail::TotpState::Code { code: TOTP_CODE.to_string(), seconds_left: 21 }
    }

    // -- the view and the default ------------------------------------------

    #[test]
    fn an_item_with_no_sequence_shows_the_default_and_says_so() {
        let view = sequence_view("");
        assert!(view.is_default);
        assert_eq!(
            view.tokens,
            key_sequence::parse(key_sequence::DEFAULT_SEQUENCE),
            "an empty sequence must show what would really be typed, not nothing"
        );
        assert_eq!(sequence_summary(""), "Username \u{b7} Tab \u{b7} Password  (default)");
    }

    #[test]
    fn an_item_with_a_sequence_shows_its_own_and_is_not_marked_default() {
        let view = sequence_view("{USERNAME}{ENTER}{DELAY 2000}{PASSWORD}");
        assert!(!view.is_default);
        assert_eq!(
            sequence_summary("{USERNAME}{ENTER}{DELAY 2000}{PASSWORD}"),
            "Username \u{b7} Enter \u{b7} Wait 2s \u{b7} Password"
        );
    }

    // -- the edits ---------------------------------------------------------

    #[test]
    fn adding_to_an_item_that_stores_nothing_keeps_the_fill_it_already_had() {
        // The trap this closes: starting from an empty token list would make
        // one click on `{TOTP}` silently delete the user name and password
        // this item has always typed.
        let next = sequence_with(&String::new(), Token::Field(FieldRef::Totp));
        assert_eq!(next, "{USERNAME}{TAB}{PASSWORD}{TOTP}");
    }

    #[test]
    fn adding_appends_to_a_sequence_the_item_already_stores() {
        assert_eq!(
            sequence_with("{USERNAME}", Token::Key(key_sequence::key_named("ENTER").unwrap())),
            "{USERNAME}{ENTER}"
        );
    }

    #[test]
    fn removing_a_step_removes_that_step_and_no_other() {
        let stored = "{USERNAME}{TAB}{PASSWORD}{ENTER}";
        assert_eq!(sequence_without(stored, 1), "{USERNAME}{PASSWORD}{ENTER}");
        assert_eq!(sequence_without(stored, 0), "{TAB}{PASSWORD}{ENTER}");
        assert_eq!(sequence_without(stored, 3), "{USERNAME}{TAB}{PASSWORD}");
        // Out of range changes nothing, byte for byte.
        assert_eq!(sequence_without(stored, 4), stored);
    }

    #[test]
    fn removing_every_step_gives_the_default_back_rather_than_an_item_that_types_nothing() {
        let mut sequence = "{TAB}{ENTER}".to_string();
        sequence = sequence_without(&sequence, 0);
        sequence = sequence_without(&sequence, 0);
        assert_eq!(sequence, "", "an emptied sequence must store the empty string");
        let view = sequence_view(&sequence);
        assert!(view.is_default);
        assert!(
            !view.tokens.is_empty(),
            "emptying the builder left an item that would type nothing"
        );
    }

    #[test]
    fn moving_a_step_swaps_it_with_its_neighbour() {
        let stored = "{USERNAME}{TAB}{PASSWORD}";
        assert_eq!(sequence_moved(stored, 2, true), "{USERNAME}{PASSWORD}{TAB}");
        assert_eq!(sequence_moved(stored, 0, false), "{TAB}{USERNAME}{PASSWORD}");
    }

    /// **A click that does nothing must change nothing, byte for byte.**
    ///
    /// The fixtures are chosen so the assertion can actually fail. A sequence
    /// that happens to round-trip through this build's own renderer cannot
    /// tell "returned the input" from "re-rendered the input" -- the first
    /// version of this test used `{PICKCHARS}{tab}{DELAY  9}`, which
    /// round-trips exactly, and a mutant that re-rendered on every disabled
    /// arrow SURVIVED it. These three do not round-trip to themselves:
    ///
    ///  * the empty string, which re-renders as the whole default -- so a
    ///    disabled arrow would silently write a sequence onto an item that
    ///    stored none;
    ///  * an unterminated brace, which re-renders escaped;
    ///  * a lower-case key beside them, so the value is one this build only
    ///    carries.
    #[test]
    fn a_move_that_does_nothing_leaves_the_stored_bytes_exactly_as_they_were() {
        for stored in ["", "{USERNAME", "{tab}{PICKCHARS"] {
            for (index, back) in [(0, true), (0, false), (9, true), (9, false)] {
                // Only the no-op directions: index 0 forward on a one-token
                // sequence is a no-op too, and index 9 is past the end of all
                // three.
                if index == 0 && !back && key_sequence::effective_tokens(stored).len() > 1 {
                    continue;
                }
                assert_eq!(
                    sequence_moved(stored, index, back),
                    stored,
                    "moving {index} (back = {back}) rewrote {stored:?}"
                );
            }
            assert_eq!(
                sequence_without(stored, 9),
                stored,
                "removing a step that is not there rewrote {stored:?}"
            );
        }
        // The control: these fixtures really would change if re-rendered, so
        // the assertions above are not satisfied by a value that is its own
        // rendering.
        for stored in ["", "{USERNAME", "{tab}{PICKCHARS"] {
            assert_ne!(
                key_sequence::render(&key_sequence::effective_tokens(stored)),
                stored,
                "{stored:?} is its own rendering, so this test cannot tell a no-op from a rewrite"
            );
        }
        // ...and a move that IS a move still moves.
        assert_eq!(sequence_moved("{TAB}{ENTER}", 1, true), "{ENTER}{TAB}");
    }

    #[test]
    fn text_the_user_types_is_escaped_for_them() {
        // "user can decide whether they want ... to put literals" -- and the
        // user is not expected to know that `+` and `{` are special.
        let stored = sequence_with_literal("", "100%+{x}").unwrap();
        assert_eq!(stored, "{USERNAME}{TAB}{PASSWORD}100{%}{+}{{}x{}}");
        // ...and it comes back as the very characters they typed.
        let tokens = key_sequence::parse(&stored);
        assert_eq!(tokens.last(), Some(&Token::Literal("100%+{x}".to_string())));
    }

    #[test]
    fn an_empty_text_box_is_not_an_edit() {
        assert_eq!(sequence_with_literal("{TAB}", ""), None);
    }

    #[test]
    fn the_users_own_example_is_expressible_by_clicking() {
        // "2134{TOTP} - which will return as 2134776699"
        let mut sequence = String::new();
        // Start from the default, strip it back, then build the literal.
        sequence = sequence_without(&sequence, 0);
        sequence = sequence_without(&sequence, 0);
        sequence = sequence_without(&sequence, 0);
        sequence = sequence_with_literal(&sequence, "2134").unwrap();
        // The default came back when the list emptied, so drop it again and
        // keep only what was typed.
        let index_of_literal = key_sequence::parse(&sequence)
            .iter()
            .position(|t| matches!(t, Token::Literal(text) if text == "2134"))
            .expect("the literal was added");
        for _ in 0..index_of_literal {
            sequence = sequence_without(&sequence, 0);
        }
        sequence = sequence_with(&sequence, Token::Field(FieldRef::Totp));
        assert_eq!(sequence, "2134{TOTP}");

        let item = item();
        let totp = live_code();
        let draft = draft_for(&item, &sequence);
        let source = sequence_source(&draft.username, &draft.password, Some(&item), &totp);
        let parts = key_sequence::resolve_preview(&key_sequence::parse(&sequence), &source);
        assert_eq!(
            parts,
            vec![
                PreviewPart::Text("2134".to_string()),
                PreviewPart::Value(TOTP_CODE.to_string()),
            ],
            "the user's own example does not resolve to 2134{TOTP_CODE}"
        );
    }

    #[test]
    fn a_wait_is_added_in_seconds_and_stored_in_milliseconds() {
        assert_eq!(sequence_with_wait("{TAB}", "1.5"), Some("{TAB}{DELAY 1500}".to_string()));
        assert_eq!(sequence_with_wait("{TAB}", "2"), Some("{TAB}{DELAY 2000}".to_string()));
        // Refused rather than added as nothing, which is what lets the button
        // be disabled with the rule shown beside it.
        for typed in ["", "soon", "-1", "9999"] {
            assert_eq!(sequence_with_wait("{TAB}", typed), None, "{typed:?}");
        }
    }

    // -- the palette -------------------------------------------------------

    #[test]
    fn the_palette_lists_this_items_own_fields_and_not_a_fixed_list() {
        let item = item();
        let draft = draft_for(&item, "");
        assert_eq!(
            sequence_palette(&draft, Some(&item)),
            vec![
                FieldRef::Username,
                FieldRef::Password,
                FieldRef::Totp,
                FieldRef::Custom("PIN".to_string()),
            ],
            "`{{S:PIN}}` is discoverable only if the palette comes from the item"
        );
        // The control: a different item offers different buttons.
        let mut other = item.clone();
        other.fields = vec![vault_field("Security answer", "Fido")];
        other.login.as_mut().unwrap().totp = None;
        let other_draft = draft_for(&other, "");
        let palette = sequence_palette(&other_draft, Some(&other));
        assert!(!palette.contains(&FieldRef::Totp), "{palette:?}");
        assert!(palette.contains(&FieldRef::Custom("Security answer".to_string())), "{palette:?}");
        assert!(!palette.contains(&FieldRef::Custom("PIN".to_string())), "{palette:?}");
    }

    #[test]
    fn the_palette_follows_the_boxes_on_this_very_form() {
        let item = item();
        let mut draft = draft_for(&item, "");
        draft.username.clear();
        let palette = sequence_palette(&draft, Some(&item));
        assert!(
            !palette.contains(&FieldRef::Username),
            "a user name cleared on the form is still offered: {palette:?}"
        );
        assert!(palette.contains(&FieldRef::Password), "{palette:?}");
    }

    #[test]
    fn a_create_offers_only_the_two_boxes_it_has() {
        let mut draft = EditDraft::empty();
        draft.username = "someone@example.test".to_string();
        assert_eq!(sequence_palette(&draft, None), vec![FieldRef::Username]);
    }

    // -- the preview -------------------------------------------------------

    #[test]
    fn the_preview_resolves_against_the_boxes_on_the_form_not_the_saved_item() {
        let item = item();
        let mut draft = draft_for(&item, "{USERNAME}{TAB}{PASSWORD}");
        draft.password = "just-typed-this".to_string();
        let totp = live_code();
        let source = sequence_source(&draft.username, &draft.password, Some(&item), &totp);
        let parts =
            key_sequence::resolve_preview(&key_sequence::parse(&draft.app.as_ref().unwrap().sequence), &source);
        assert_eq!(
            parts,
            vec![
                PreviewPart::Value(USERNAME.to_string()),
                PreviewPart::Key("\u{21e5}"),
                PreviewPart::Value("just-typed-this".to_string()),
            ]
        );
        assert_ne!("just-typed-this", PASSWORD, "the fixture's two passwords must differ");
    }

    #[test]
    fn a_reference_to_a_field_that_is_not_there_is_shown_as_unresolved() {
        let item = item();
        let draft = draft_for(&item, "{S:Missing}");
        let totp = live_code();
        let source = sequence_source(&draft.username, &draft.password, Some(&item), &totp);
        let parts = key_sequence::resolve_preview(&key_sequence::parse("{S:Missing}"), &source);
        assert!(
            matches!(parts.as_slice(), [PreviewPart::Unresolved(_)]),
            "{parts:?}"
        );
    }

    // -- the draft carries it, verbatim, in both directions -----------------

    #[test]
    fn the_draft_carries_the_stored_sequence_verbatim_in_both_directions() {
        for sequence in [
            "",
            "{USERNAME}{TAB}{PASSWORD}",
            "{PICKCHARS}{tab}{DELAY  9}",
            "2134{TOTP}",
        ] {
            let m = AppMatch {
                sequence: sequence.to_string(),
                ..AppMatch::for_process("a.exe", TriggerMode::Auto)
            };
            let draft = AppMatchDraft::from_match(&m);
            assert_eq!(draft.sequence, sequence, "into the draft: {sequence:?}");
            assert_eq!(draft.to_match().sequence, sequence, "out of it: {sequence:?}");
            assert_eq!(draft.to_match(), m, "{sequence:?}");
        }
    }

    /// Unlike the arguments beside it, the sequence is **kept** for a Store
    /// app: nothing is started by path, but a Store window is typed into like
    /// any other. Making `to_match` zero it the way it zeroes `args` fails
    /// here.
    #[test]
    fn a_store_binding_keeps_its_sequence() {
        let m = AppMatch {
            process: "Speedtest.exe".to_string(),
            title: "Speedtest by Ookla".to_string(),
            hosted: true,
            path: String::new(),
            args: "--headless".to_string(),
            sequence: "{USERNAME}{ENTER}".to_string(),
            trigger: TriggerMode::Hotkey,
        };
        let round = AppMatchDraft::from_match(&m).to_match();
        assert_eq!(round.sequence, "{USERNAME}{ENTER}");
        // The control: the arguments beside it really are dropped, so the
        // assertion above is about the sequence and not about a `to_match`
        // that drops nothing.
        assert_eq!(round.args, "");
    }

    /// **The byte-identity promise, end to end.** An edit that does not touch
    /// the sequence must not rewrite it -- not even into a spelling this build
    /// finds tidier. `{tab}` and `{DELAY  9}` are exactly the constructs this
    /// build would re-spell if it ever round-tripped the string through its
    /// own renderer.
    #[test]
    fn an_edit_that_does_not_touch_the_sequence_writes_nothing_at_all() {
        let stored = "{USERNAME}{tab}{DELAY  9}{PICKCHARS}{PASSWORD}";
        let existing =
            AppMatch { sequence: stored.to_string(), ..AppMatch::for_process("a.exe", TriggerMode::Auto) };
        let draft = AppMatchDraft::from_match(&existing);
        assert_eq!(
            app_match_edit(Some(&existing), Some(&draft)),
            AppMatchEdit::Leave,
            "renaming an item rewrote a keystroke sequence this build merely carries"
        );
        // The control: an edit that DOES touch it is written.
        let mut edited = draft.clone();
        edited.sequence = sequence_with(&edited.sequence, Token::Field(FieldRef::Totp));
        assert!(matches!(
            app_match_edit(Some(&existing), Some(&edited)),
            AppMatchEdit::Write(m) if m.sequence.starts_with(stored)
        ));
    }

    // -- the wiring, pinned separately from the decisions --------------------

    /// Tall enough to hold the whole form with the builder OPEN, so a control
    /// below the fold is painted and can be asserted about. A harness size,
    /// not a claim about the app: the form scrolls, and what the app can
    /// really be resized to is asserted against `MIN_PANE_WIDTH` below and in
    /// `edit_pane_layout_tests`.
    const PANE: Vec2 = egui::vec2(560.0, 1700.0);

    /// The narrowest the detail pane can be -- the same derivation, and the
    /// same reason, as `edit_pane_layout_tests`'s.
    const MIN_PANE_WIDTH: f32 = crate::settings::MIN_VAULT_WINDOW_SIZE.0 as f32
        - crate::vault_window::SIDEBAR_WIDTH
        - crate::vault_window::LIST_WIDTH;

    #[derive(Default)]
    struct Painted {
        texts: Vec<(String, Rect)>,
        rendered: Vec<(String, String, Rect)>,
    }

    impl Painted {
        fn strings(&self) -> Vec<&str> {
            self.texts.iter().map(|(t, _)| t.as_str()).collect()
        }

        fn rects_of(&self, label: &str) -> Vec<Rect> {
            self.texts.iter().filter(|(t, _)| t == label).map(|(_, r)| *r).collect()
        }

        fn rect_of(&self, label: &str) -> Rect {
            let found = self.rects_of(label);
            assert_eq!(
                found.len(),
                1,
                "expected exactly one {label:?}, found {}; painted: {:?}",
                found.len(),
                self.strings()
            );
            found[0]
        }
    }

    fn walk(shape: &egui::Shape, painted: &mut Painted) {
        match shape {
            egui::Shape::Text(text) => {
                let rect = Rect::from_min_size(text.pos, text.galley.size());
                let rendered: String = text
                    .galley
                    .rows
                    .iter()
                    .flat_map(|row| row.glyphs.iter().map(|glyph| glyph.chr))
                    .collect();
                painted.texts.push((text.galley.text().to_string(), rect));
                painted.rendered.push((text.galley.text().to_string(), rendered, rect));
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, painted);
                }
            }
            _ => {}
        }
    }

    fn raw_input(pane: Vec2, events: &[egui::Event]) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, pane)),
            events: events.to_vec(),
            ..Default::default()
        }
    }

    fn styled_context(pane: Vec2) -> egui::Context {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(raw_input(pane, &[]), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(raw_input(pane, &[]), |_ui| {});
        ctx
    }

    fn frame(
        ctx: &egui::Context,
        pane: Vec2,
        draft: &mut EditDraft,
        item: &VaultItem,
        totp: &detail::TotpState,
        events: &[egui::Event],
    ) -> Painted {
        let mut apps = AppIdentityCache::default();
        let output = ctx.run_ui(raw_input(pane, events), |ui| {
            let _ = draw_detail_edit(ui, draft, &[], false, &mut apps, Some(item), totp);
        });
        let mut painted = Painted::default();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut painted);
        }
        painted
    }

    /// [`frame`], keeping the action the form reported.
    ///
    /// A second helper rather than a wider `frame`, because every other test in
    /// this module is about what is painted and would have to thread an answer
    /// it does not read.
    fn frame_action(
        ctx: &egui::Context,
        pane: Vec2,
        draft: &mut EditDraft,
        item: &VaultItem,
        totp: &detail::TotpState,
        events: &[egui::Event],
    ) -> (EditAction, Painted) {
        let mut apps = AppIdentityCache::default();
        let mut action = EditAction::None;
        let output = ctx.run_ui(raw_input(pane, events), |ui| {
            action = draw_detail_edit(ui, draft, &[], false, &mut apps, Some(item), totp);
        });
        let mut painted = Painted::default();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut painted);
        }
        (action, painted)
    }

    fn click(pos: Pos2) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            },
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            },
        ]
    }

    /// Opens the builder, and answers with the frame drawn after it opened.
    fn open_builder(
        ctx: &egui::Context,
        pane: Vec2,
        draft: &mut EditDraft,
        item: &VaultItem,
        totp: &detail::TotpState,
    ) -> Painted {
        let shut = frame(ctx, pane, draft, item, totp, &[]);
        let at = shut.rect_of(APP_SEQUENCE_OPEN).center();
        let _ = frame(ctx, pane, draft, item, totp, &click(at));
        assert!(draft.app.as_ref().unwrap().sequence_open, "the builder did not open");
        frame(ctx, pane, draft, item, totp, &[])
    }

    /// **4d's way in.** Mutation this catches: delete the button, or drop the
    /// `action = Some(EditAction::Rehearse)`. `EditAction::Rehearse` is `pub`,
    /// so having no producers left is not even a warning -- which is exactly
    /// how a control that reports nothing ships.
    #[test]
    fn clicking_rehearse_asks_the_caller_to_run_one() {
        let item = item();
        let ctx = styled_context(PANE);
        let mut draft = draft_for(&item, "");
        let totp = detail::TotpState::NoSecret;
        // The button lives in the keystroke builder, which is shut by default.
        draft.app.as_mut().unwrap().sequence_open = true;

        let (idle, painted) = frame_action(&ctx, PANE, &mut draft, &item, &totp, &[]);
        assert_eq!(idle, EditAction::None, "the form reported an action with no input");
        let button = painted.rect_of(APP_SEQUENCE_REHEARSE);

        let (action, _) =
            frame_action(&ctx, PANE, &mut draft, &item, &totp, &click(button.center()));
        assert_eq!(action, EditAction::Rehearse);

        // The positive control: were any click in the form reporting
        // `Rehearse`, the assertion above would pass with the button deleted.
        let miss = Pos2::new(button.center().x, button.top() - 60.0);
        let (missed, _) = frame_action(&ctx, PANE, &mut draft, &item, &totp, &click(miss));
        assert_ne!(missed, EditAction::Rehearse, "a click that hit nothing started a rehearsal");
    }

    /// The same control in the template view, which draws its own row and so is
    /// a second place for it to go missing.
    #[test]
    fn the_template_view_can_rehearse_too() {
        let item = item();
        let ctx = styled_context(PANE);
        let mut draft = draft_for(&item, "");
        let totp = detail::TotpState::NoSecret;
        {
            let app = draft.app.as_mut().unwrap();
            app.sequence_open = true;
            app.template_view = true;
        }
        let (_, painted) = frame_action(&ctx, PANE, &mut draft, &item, &totp, &[]);
        let at = painted.rect_of(APP_SEQUENCE_REHEARSE).center();
        let (action, _) = frame_action(&ctx, PANE, &mut draft, &item, &totp, &click(at));
        assert_eq!(action, EditAction::Rehearse);
    }

    /// **The wiring pin for the drawing.** Deleting the
    /// `app_sequence_block(...)` call inside `app_block` fails here: the form
    /// stops saying anything about keystrokes at all.
    #[test]
    fn the_form_draws_the_keystroke_block_for_a_bound_item() {
        let item = item();
        let ctx = styled_context(PANE);
        let mut draft = draft_for(&item, "");
        let painted = frame(&ctx, PANE, &mut draft, &item, &detail::TotpState::NoSecret, &[]);
        let strings = painted.strings();
        assert!(strings.contains(&APP_SEQUENCE_LABEL), "{strings:?}");
        assert!(
            strings.iter().any(|s| *s == sequence_summary("")),
            "the block does not say what would be typed: {strings:?}"
        );
        assert!(strings.contains(&APP_SEQUENCE_OPEN), "there is no way in: {strings:?}");
    }

    /// The palette really is on screen and really is the item's own: every
    /// field button, every key button, and the two boxes.
    #[test]
    fn opening_the_builder_shows_every_field_key_and_wait_the_user_can_click() {
        let item = item();
        let ctx = styled_context(PANE);
        let mut draft = draft_for(&item, "");
        let painted = open_builder(&ctx, PANE, &mut draft, &item, &live_code());
        let strings = painted.strings();
        for field in sequence_palette(&draft, Some(&item)) {
            assert!(
                strings.contains(&field.label().as_str()),
                "{:?} is not a button: {strings:?}",
                field.label()
            );
        }
        for key in key_sequence::KEYS.iter().filter(|k| k.palette) {
            assert!(strings.contains(&key.label), "{} is not a button: {strings:?}", key.label);
        }
        assert!(strings.contains(&"Add text"), "{strings:?}");
        assert!(strings.contains(&"Add wait"), "{strings:?}");
        assert!(strings.contains(&APP_SEQUENCE_REVEAL), "the eye is missing: {strings:?}");
    }

    /// **The wiring pin for the write.** A click on a palette button reaches
    /// the draft, and the draft reaches the saved item. Deleting
    /// `sequence: self.sequence.clone()` from `AppMatchDraft::to_match` fails
    /// on the second half.
    #[test]
    fn clicking_a_field_adds_it_to_the_sequence_and_the_save_carries_it() {
        let item = item();
        let ctx = styled_context(PANE);
        let mut draft = draft_for(&item, "");
        let painted = open_builder(&ctx, PANE, &mut draft, &item, &live_code());
        let at = painted.rect_of(&FieldRef::Totp.label()).center();
        let _ = frame(&ctx, PANE, &mut draft, &item, &live_code(), &click(at));
        assert_eq!(
            draft.app.as_ref().unwrap().sequence,
            "{USERNAME}{TAB}{PASSWORD}{TOTP}",
            "the click did not reach the draft"
        );
        let saved = draft.apply_to(&item);
        let stored = crate::vault_bridge::extract_app_match(&saved).expect("the binding is saved");
        assert_eq!(
            stored.sequence, "{USERNAME}{TAB}{PASSWORD}{TOTP}",
            "the sequence the user built was not written to the item"
        );
    }

    /// The eye is what makes this parser have a reader. Shut, the values are
    /// not on screen; open, they are.
    #[test]
    fn the_eye_reveals_what_the_sequence_would_actually_type() {
        let item = item();
        let ctx = styled_context(PANE);
        let mut draft = draft_for(&item, "2134{TOTP}");
        let shut = open_builder(&ctx, PANE, &mut draft, &item, &live_code());
        assert!(
            !shut.strings().iter().any(|s| s.contains(TOTP_CODE)),
            "the code is on screen with the eye shut: {:?}",
            shut.strings()
        );
        let at = shut.rect_of(APP_SEQUENCE_REVEAL).center();
        let _ = frame(&ctx, PANE, &mut draft, &item, &live_code(), &click(at));
        let open = frame(&ctx, PANE, &mut draft, &item, &live_code(), &[]);
        let strings = open.strings();
        assert!(
            strings.contains(&TOTP_CODE),
            "the eye is open and the code is not shown: {strings:?}"
        );
        assert!(strings.contains(&"2134"), "the literal is not shown: {strings:?}");
    }

    /// Every chip, and every palette button, is inside the pane at the app's
    /// MINIMUM width -- and drawn with the glyphs of its own label, not an
    /// elision of it.
    ///
    /// This is `aae9429`'s defect stated for a new row: the edit form's card
    /// is already 309.3pt wide on a 298pt pane, and a chip row that ran off
    /// the right edge would put steps somewhere with no horizontal scroll to
    /// reach them. `horizontal_wrapped` is what holds it.
    #[test]
    fn every_chip_and_button_is_reachable_at_the_apps_minimum_width() {
        let pane = egui::vec2(MIN_PANE_WIDTH, 2200.0);
        let item = item();
        let ctx = styled_context(pane);
        // A deliberately long sequence: more steps than fit on one row, so
        // the wrap is really exercised rather than merely available.
        let mut draft =
            draft_for(&item, "{USERNAME}{TAB}{PASSWORD}{ENTER}{DELAY 2000}{S:PIN}{TAB}{TOTP}{ENTER}");
        let painted = open_builder(&ctx, pane, &mut draft, &item, &live_code());
        let bounds = Rect::from_min_size(Pos2::ZERO, pane);

        let mut checked = 0;
        for token in sequence_view(&draft.app.as_ref().unwrap().sequence).tokens {
            let label = token.chip_label();
            for rect in painted.rects_of(&label) {
                assert!(
                    bounds.contains_rect(rect),
                    "the chip {label:?} is painted at {rect:?}, off a {}x{}pt pane -- this pane \
                     does not scroll horizontally",
                    pane.x,
                    pane.y
                );
                checked += 1;
            }
            // ...and the label is the whole label, not an elision of it.
            let rendered: Vec<&String> = painted
                .rendered
                .iter()
                .filter(|(source, _, _)| *source == label)
                .map(|(_, drawn, _)| drawn)
                .collect();
            assert!(!rendered.is_empty(), "the chip {label:?} was not painted at all");
            for drawn in rendered {
                assert_eq!(
                    *drawn, label,
                    "the chip {label:?} DREW {drawn:?} -- it is on the pane and unreadable"
                );
            }
        }
        assert!(checked >= 8, "only {checked} chips were checked; the row was not drawn");

        // The palette buttons too: a key the user cannot reach is a key the
        // user has to type from memory, which is the whole thing they asked
        // not to have to do.
        for key in key_sequence::KEYS.iter().filter(|k| k.palette) {
            for rect in painted.rects_of(key.label) {
                assert!(
                    bounds.contains_rect(rect),
                    "the {} button is painted at {rect:?}, off a {}pt-wide pane",
                    key.label,
                    pane.x
                );
            }
        }

        // The control on this assertion: it is not vacuous because the chips
        // really did need more than one row.
        let rows: std::collections::BTreeSet<i64> = sequence_view(&draft.app.as_ref().unwrap().sequence)
            .tokens
            .iter()
            .flat_map(|t| painted.rects_of(&t.chip_label()))
            .map(|r| r.top() as i64)
            .collect();
        assert!(
            rows.len() > 1,
            "every chip landed on one row, so nothing here proves the row wraps"
        );
    }
    // -- the wiring at the real call sites, not the pure functions ----------
    //
    // EVERY test above this line calls `sequence_source`, `sequence_without`
    // or `sequence_moved` ITSELF, with arguments of its own. That leaves the
    // calls `draw_detail_edit` really makes unexercised, which is how an
    // argument at one of them can be swapped, constant-ified or nulled with
    // the whole suite green. These three drive the drawn form and read what
    // it PAINTED.

    /// The one row of text painted strictly below `below`, left to right.
    ///
    /// Scoped deliberately. The form's own user-name and password boxes carry
    /// the very strings the preview does and are painted ABOVE the eye, so a
    /// read over the whole frame is satisfied by those boxes and says nothing
    /// at all about the preview -- the first attempt at the test below was
    /// vacuous for exactly that reason. Reading the row under the eye button
    /// is reading the preview and nothing else.
    fn row_below(painted: &Painted, below: Rect) -> Vec<String> {
        let mut under: Vec<&(String, Rect)> =
            painted.texts.iter().filter(|(_, r)| r.top() > below.bottom()).collect();
        let top = under.iter().map(|(_, r)| r.top()).fold(f32::INFINITY, f32::min);
        under.retain(|(_, r)| r.top() - top < 4.0);
        under.sort_by(|(_, a), (_, b)| a.left().total_cmp(&b.left()));
        under.into_iter().map(|(text, _)| text.clone()).collect()
    }

    /// Opens the builder AND the eye, and answers with the frame drawn with
    /// both open.
    fn reveal(
        ctx: &egui::Context,
        pane: Vec2,
        draft: &mut EditDraft,
        item: &VaultItem,
        totp: &detail::TotpState,
    ) -> Painted {
        let shut = open_builder(ctx, pane, draft, item, totp);
        let at = shut.rect_of(APP_SEQUENCE_REVEAL).center();
        let _ = frame(ctx, pane, draft, item, totp, &click(at));
        assert!(draft.app.as_ref().unwrap().previewing, "the eye did not open");
        frame(ctx, pane, draft, item, totp, &[])
    }

    /// **Which value the eye draws under which placeholder.**
    ///
    /// This is the whole job of the one component that has it, and it was
    /// unpinned: swapping the two arguments at the form's own
    /// `sequence_source(&draft.username, &draft.password, item, totp)` drew
    /// the plaintext PASSWORD where `{USERNAME}` sits and the address where
    /// `{PASSWORD}` sits, and the entire suite stayed green -- the other
    /// preview tests call `sequence_source` themselves, and the only test
    /// that reached the real call site used the fixture `"2134{TOTP}"`, which
    /// contains neither placeholder.
    ///
    /// So: both placeholders, on a fixture whose user name and password are
    /// nothing like each other, read from the paint.
    #[test]
    fn the_eye_draws_each_value_under_the_placeholder_that_asked_for_it() {
        let item = item();
        let ctx = styled_context(PANE);
        let mut draft = draft_for(&item, "{USERNAME}{TAB}{PASSWORD}");
        // The control on the fixture: two values that agreed would let a swap
        // satisfy every assertion below.
        assert_ne!(USERNAME, PASSWORD, "the fixture cannot tell a swap from a fill");

        let open = reveal(&ctx, PANE, &mut draft, &item, &live_code());
        let eye = open.rect_of(APP_SEQUENCE_HIDE);
        assert_eq!(
            row_below(&open, eye),
            vec![USERNAME.to_string(), "[\u{21e5}]".to_string(), PASSWORD.to_string()],
            "the row under the eye is not user name, tab, password: the one component whose \
             job is saying which value goes where is saying the wrong thing"
        );
        // ...and both are drawn with their own glyphs, not an elision of
        // them: `Galley::text()` answers with the SOURCE string, so the
        // assertion above is blind to truncation on its own.
        for label in [USERNAME, PASSWORD] {
            let drawn: Vec<&String> = open
                .rendered
                .iter()
                .filter(|(source, _, rect)| source == label && rect.top() > eye.bottom())
                .map(|(_, drawn, _)| drawn)
                .collect();
            assert!(!drawn.is_empty(), "{label:?} is not painted below the eye at all");
            for drawn in drawn {
                assert_eq!(drawn, label, "the preview DREW {drawn:?} for {label:?}");
            }
        }
    }

    /// The `<`, `>` or `x` painted beside `chip`, on `chip`'s own row.
    ///
    /// By GEOMETRY rather than by paint order, because paint order is exactly
    /// what a constant or an off-by-one at the binding would still get right.
    /// The question is "which control is next to the chip the user is looking
    /// at", and only the rects can answer it.
    fn control_beside(painted: &Painted, chip: Rect, caption: &str) -> Pos2 {
        let mut found: Vec<Rect> = painted
            .rects_of(caption)
            .into_iter()
            .filter(|r| (r.center().y - chip.center().y).abs() < 8.0 && r.left() >= chip.right())
            .collect();
        found.sort_by(|a, b| a.left().total_cmp(&b.left()));
        assert!(!found.is_empty(), "no {caption:?} is painted beside the chip at {chip:?}");
        found[0].center()
    }

    /// The one chip carrying `label`, found in the CHIP ROW rather than
    /// anywhere on the form.
    ///
    /// Necessary, not fastidious: "Username" and "Password" are also field
    /// labels above, and "Tab" and "Enter" are also palette buttons below, so
    /// a bare `rect_of` finds two of everything. The chip row is the band
    /// between the block's hint and the "Add a value" heading.
    fn chip_rect(painted: &Painted, label: &str) -> Rect {
        let floor = painted.rect_of(APP_SEQUENCE_HINT).bottom();
        let ceiling = painted.rect_of("Add a value").top();
        let found: Vec<Rect> = painted
            .rects_of(label)
            .into_iter()
            .filter(|r| r.top() > floor && r.top() < ceiling)
            .collect();
        assert_eq!(found.len(), 1, "expected one {label:?} chip, found {}", found.len());
        found[0]
    }

    /// **The index a chip's own control hands to the pure function.**
    ///
    /// `sequence_without` and `sequence_moved` are exhaustively tested as
    /// pure functions and nothing tested the BINDING: `ChipEdit::Remove(index)`
    /// -> `Remove(0)` deleted the first step whichever `x` was clicked, and
    /// saved that, with every test passing. There was no chip-click test at
    /// all -- only palette-click and eye-click ones.
    ///
    /// The THIRD of four steps, so it is neither first nor last: a constant
    /// and an off-by-one in either direction are all three visible. All three
    /// controls, because they share the shape by construction.
    #[test]
    fn each_chips_own_controls_act_on_that_chip_and_not_another() {
        const SEQUENCE: &str = "{USERNAME}{TAB}{PASSWORD}{ENTER}";
        // Read from what the row is drawn from, so the label under test
        // cannot drift from the chip under test.
        let labels: Vec<String> =
            sequence_view(SEQUENCE).tokens.iter().map(|t| t.chip_label()).collect();
        assert_eq!(labels.len(), 4, "the fixture is not four steps long: {labels:?}");
        let target = labels[2].clone();

        for (caption, expected) in [
            ("x", "{USERNAME}{TAB}{ENTER}"),
            ("<", "{USERNAME}{PASSWORD}{TAB}{ENTER}"),
            (">", "{USERNAME}{TAB}{ENTER}{PASSWORD}"),
        ] {
            // The control on the expectation: each of these differs from what
            // the SAME edit applied to step 0 or to a neighbour would produce,
            // so none of them can be satisfied by acting on the wrong step.
            assert_ne!(expected, sequence_without(SEQUENCE, 0), "{caption}");
            assert_ne!(expected, SEQUENCE, "{caption}");

            let item = item();
            let ctx = styled_context(PANE);
            let mut draft = draft_for(&item, SEQUENCE);
            let open = open_builder(&ctx, PANE, &mut draft, &item, &live_code());
            let chip = chip_rect(&open, &target);
            let at = control_beside(&open, chip, caption);
            let _ = frame(&ctx, PANE, &mut draft, &item, &live_code(), &click(at));
            assert_eq!(
                draft.app.as_ref().unwrap().sequence,
                expected,
                "clicking {caption:?} beside the {target:?} chip -- step 3 of 4 -- acted on a \
                 different step"
            );
        }
    }

    /// **Shutting the builder shuts the eye.** Deleting `app.previewing =
    /// false;` from the Done arm leaves a reveal armed that the user never
    /// asked for a second time: scroll the block shut, open it again later,
    /// and the plaintext is straight back on screen.
    #[test]
    fn closing_the_builder_closes_the_eye_so_reopening_reveals_nothing() {
        let item = item();
        let ctx = styled_context(PANE);
        let mut draft = draft_for(&item, "{USERNAME}{TAB}{PASSWORD}");
        let open = reveal(&ctx, PANE, &mut draft, &item, &live_code());
        // The control: the eye really was open, and really was showing it.
        assert!(open.strings().contains(&PASSWORD), "{:?}", open.strings());

        let at = open.rect_of(APP_SEQUENCE_CLOSE).center();
        let _ = frame(&ctx, PANE, &mut draft, &item, &live_code(), &click(at));
        let app = draft.app.as_ref().unwrap();
        assert!(!app.sequence_open, "the builder did not close");
        assert!(!app.previewing, "the builder closed with the eye left open");

        let reopened = open_builder(&ctx, PANE, &mut draft, &item, &live_code());
        assert!(
            !reopened.strings().contains(&PASSWORD),
            "reopening the builder put the password back on screen without being asked: {:?}",
            reopened.strings()
        );
    }

    // -- will it run at all? the decisions ---------------------------------
    //
    // The editor used to show a sequence that the runner refuses BEFORE the
    // first keystroke as perfectly writable, and the user found out when a
    // fill silently did nothing. These pin `sequence_refusal`, which answers
    // that question by asking `injector::sequence::plan` -- the runner's own
    // function -- rather than by re-deriving one of its rules.

    use crate::injector::sequence::Refusal;

    /// A `ResolveSource` over [`item`], with the TOTP state the caller names.
    fn source_for<'a>(item: &'a VaultItem, totp: &'a detail::TotpState) -> ResolveSource<'a> {
        let login = item.login.as_ref().unwrap();
        sequence_source(
            login.username.as_deref().unwrap_or(""),
            login.password.as_deref().map_or("", |v| v.as_str()),
            Some(item),
            totp,
        )
    }

    /// **Every refusal the editor claims to know about, and the SAME variant
    /// the runner would raise.**
    ///
    /// Not "some warning appears": the variant, compared against
    /// [`Refusal`]'s own discriminants. A check that only asserted "is
    /// `Some`" would pass on an editor that answered `Nothing` to every
    /// question, which is a warning that names the wrong thing to go and fix.
    #[test]
    fn the_editor_knows_the_refusals_that_are_facts_about_the_item() {
        let item = item();
        let totp = detail::TotpState::NoSecret;
        let source = source_for(&item, &totp);

        // A field this item does not have. `PIN` it HAS -- see `item()` --
        // so the fixture's two halves deliberately differ and a check that
        // matched any `{S:...}` at all would be caught by the control below.
        assert_eq!(
            sequence_refusal(&key_sequence::parse("{S:Missing}"), &source),
            Some(Refusal::Unresolved("a field called Missing".to_string()))
        );
        // A token a KeePass-authored sequence may carry and this build cannot
        // type.
        assert_eq!(
            sequence_refusal(&key_sequence::parse("{PICKCHARS}"), &source),
            Some(Refusal::Unsupported("{PICKCHARS}".to_string()))
        );
        assert_eq!(
            sequence_refusal(&key_sequence::parse("{CLEARFIELD}"), &source),
            Some(Refusal::Unsupported("{CLEARFIELD}".to_string()))
        );
        // A grouping character, which the parser carries faithfully and the
        // runner refuses faithfully.
        assert!(matches!(
            sequence_refusal(&key_sequence::parse("(ab)"), &source),
            Some(Refusal::Unsupported(_))
        ));
        // A modifier with no key after it -- values-independent, so knowable.
        assert_eq!(
            sequence_refusal(&key_sequence::parse("+{S:PIN}"), &source),
            Some(Refusal::DanglingModifier("Shift".to_string()))
        );
        // `{TOTP}` on an item with no secret at all.
        assert_eq!(
            sequence_refusal(&key_sequence::parse("{TOTP}"), &source),
            Some(Refusal::Unresolved("a one-time code".to_string()))
        );

        // **The control.** A sequence built out of what this item really has
        // is not warned about, so none of the above is an editor that simply
        // refuses everything.
        assert_eq!(
            sequence_refusal(&key_sequence::parse("{USERNAME}{TAB}{PASSWORD}{ENTER}"), &source),
            None
        );
        assert_eq!(sequence_refusal(&key_sequence::parse("{S:PIN}"), &source), None);
    }

    /// **The refusals the editor must NOT claim**, which is the other half of
    /// being honest about what is knowable.
    ///
    /// A `{TOTP}` is refusable at edit time only when the item has no secret.
    /// The other four [`detail::TotpState`]s are about THIS MOMENT -- the poll
    /// is in flight, the poll reported nothing, the bridge is unavailable, a
    /// code is in hand -- and none of them is a property of the item the user
    /// is editing. Warning about those would send the user to fix something
    /// that is not broken, and the warning would come and go on its own while
    /// they looked at it.
    #[test]
    fn the_editor_does_not_warn_about_a_one_time_code_that_is_merely_late() {
        let item = item();
        for state in [
            detail::TotpState::Fetching,
            detail::TotpState::NoCodeReported,
            detail::TotpState::Unavailable,
            live_code(),
        ] {
            let source = source_for(&item, &state);
            assert_eq!(
                sequence_refusal(&key_sequence::parse("{TOTP}"), &source),
                None,
                "the editor warned about {{TOTP}} in the {state:?} state, which is not a \
                 fact about the item"
            );
        }
        // The control on the loop: the ONE state that is a fact about the
        // item still warns, so the four above are not passing because the
        // check was switched off.
        let none = detail::TotpState::NoSecret;
        assert_eq!(
            sequence_refusal(&key_sequence::parse("{TOTP}"), &source_for(&item, &none)),
            Some(Refusal::Unresolved("a one-time code".to_string()))
        );
    }

    /// **The stand-in code is six characters, and that is load-bearing.**
    ///
    /// `plan` reads two things about a one-time code: whether it is empty, and
    /// how long it is (which feeds the projected-time bound). A stand-in of
    /// the wrong length would make the editor and the runner disagree about
    /// `MAX_SEQUENCE` on a sequence full of `{TOTP}`s. Pinned against the
    /// runner's real code length rather than against the literal.
    #[test]
    fn the_stand_in_code_is_the_length_a_real_one_is() {
        assert_eq!(
            TOTP_STAND_IN.chars().count(),
            TOTP_CODE.chars().count(),
            "the stand-in and a real one-time code are different lengths, so the editor and \
             the runner will disagree about how long a sequence takes"
        );
        assert!(
            !TOTP_STAND_IN.is_empty(),
            "an empty stand-in would make every {{TOTP}} refuse, in every state"
        );
        // And it is NOT the real code: this check runs every frame and must
        // not be a second home for a secret.
        assert_ne!(TOTP_STAND_IN, TOTP_CODE);
    }

    /// **The warning says what the runner says.**
    ///
    /// Compared against `Refusal::message()` computed in this test, so a
    /// paraphrase in the editor -- "this sequence is invalid" -- fails here.
    /// That is the drift this project has a defect class from: the notification
    /// the user gets at fill time and the warning they get at edit time have to
    /// be one vocabulary.
    #[test]
    fn the_warning_is_the_runners_own_sentence() {
        let item = item();
        let totp = detail::TotpState::NoSecret;
        let source = source_for(&item, &totp);

        for sequence in ["{PICKCHARS}", "{S:Missing}", "{TOTP}", "+{S:PIN}"] {
            let refusal = sequence_refusal(&key_sequence::parse(sequence), &source)
                .unwrap_or_else(|| panic!("{sequence} was not refused at all"));
            let warning = sequence_warning(sequence, &source)
                .unwrap_or_else(|| panic!("{sequence} produced no warning"));
            assert_eq!(warning, format!("{SEQUENCE_REFUSED_PREFIX}{}", refusal.message()));
            // ...and the sentence really does NAME the offending construct,
            // which is the whole point of showing it.
            assert!(
                warning.len() > SEQUENCE_REFUSED_PREFIX.len() + 10,
                "the warning for {sequence} is {warning:?}, which names nothing"
            );
        }
        assert_eq!(sequence_warning("{USERNAME}{TAB}{PASSWORD}", &source), None);
    }

    /// **The empty sequence is judged as the DEFAULT, because that is what
    /// would be typed.**
    ///
    /// An item that stores no sequence is filled with
    /// `key_sequence::DEFAULT_SEQUENCE`, so a login with no user name really
    /// does refuse -- and the empty string is exactly the case where the user
    /// has never opened the block and has no other way to find out. Asked of
    /// `sequence_view`'s tokens rather than of `parse`, and this is what says
    /// so.
    #[test]
    fn a_stored_sequence_of_nothing_is_judged_as_the_default() {
        let mut item = item();
        item.login.as_mut().unwrap().username = None;
        let totp = detail::TotpState::NoSecret;
        // The draft's own boxes are what get saved, so an empty user name box
        // is what the editor judges -- see `sequence_source`.
        let source = sequence_source("", "correct-horse-battery", Some(&item), &totp);

        // **Asked of `sequence_warning`, not of `sequence_refusal`.** The
        // expansion is `sequence_warning`'s own argument, and a version of it
        // that called `parse` instead of `sequence_view` -- judging the empty
        // string as "types nothing" -- survives every assertion that hands
        // `sequence_refusal` tokens the TEST expanded. It was caught only by
        // driving the function that does the expanding, which is the shape
        // this crate's reviews keep finding.
        assert_eq!(
            sequence_warning("", &source).as_deref(),
            Some(
                format!(
                    "{SEQUENCE_REFUSED_PREFIX}{}",
                    Refusal::Unresolved("a username".to_string()).message()
                )
                .as_str()
            ),
            "the empty sequence was judged as typing nothing rather than as the default"
        );
        // The pure function underneath agrees, given the same tokens.
        assert_eq!(
            sequence_refusal(&sequence_view("").tokens, &source),
            Some(Refusal::Unresolved("a username".to_string()))
        );

        // The control: with a user name in the box, the same empty sequence is
        // fine. So the warning above is about the value and not about the
        // sequence being empty -- and a `parse`-based expansion, which would
        // answer `Nothing` here, fails this line too.
        let filled =
            sequence_source("ada@contoso.test", "correct-horse-battery", Some(&item), &totp);
        assert_eq!(sequence_warning("", &filled), None);
        assert_eq!(sequence_refusal(&sequence_view("").tokens, &filled), None);
    }

    // -- ...and the wiring, at the call the drawn form really makes ---------

    /// **The warning is on screen with the builder SHUT.**
    ///
    /// This is the requirement, not a nicety: the block is shut by default and
    /// a user may never open it, so a warning only visible inside it is a
    /// warning the user hit the bug without ever seeing. Deleting the
    /// `sequence_warning(...)` call in `app_sequence_block` fails here.
    #[test]
    fn a_sequence_that_cannot_run_says_so_from_the_closed_state() {
        let item = item();
        let ctx = styled_context(PANE);
        let mut draft = draft_for(&item, "{PICKCHARS}");
        let painted = frame(&ctx, PANE, &mut draft, &item, &live_code(), &[]);

        // The premise: the block really is shut.
        assert!(!draft.app.as_ref().unwrap().sequence_open, "the builder started open");
        assert!(
            painted.strings().contains(&APP_SEQUENCE_OPEN),
            "the way into the builder is not on screen, so this is not the closed state"
        );

        let expected = sequence_warning("{PICKCHARS}", &source_for(&item, &live_code())).unwrap();
        assert!(
            painted.strings().contains(&expected.as_str()),
            "the closed block does not say the sequence will not run. Wanted {expected:?}, \
             painted: {:?}",
            painted.strings()
        );
    }

    /// ...and it is still there once the builder is open, where the user has
    /// gone to do something about it.
    #[test]
    fn the_warning_survives_opening_the_builder() {
        let item = item();
        let ctx = styled_context(PANE);
        let mut draft = draft_for(&item, "{PICKCHARS}");
        let painted = open_builder(&ctx, PANE, &mut draft, &item, &live_code());
        let expected = sequence_warning("{PICKCHARS}", &source_for(&item, &live_code())).unwrap();
        assert!(
            painted.strings().contains(&expected.as_str()),
            "the open builder dropped the warning: {:?}",
            painted.strings()
        );
    }

    /// **A sequence that runs is not warned about**, in either state.
    ///
    /// Without this, an `app_sequence_block` that painted the warning
    /// unconditionally -- or one that computed it from a constant -- would
    /// pass both tests above.
    #[test]
    fn a_sequence_that_runs_is_not_warned_about() {
        let item = item();
        let ctx = styled_context(PANE);
        let mut draft = draft_for(&item, "{USERNAME}{TAB}{PASSWORD}{ENTER}");
        for painted in [
            frame(&ctx, PANE, &mut draft.clone(), &item, &live_code(), &[]),
            open_builder(&ctx, PANE, &mut draft, &item, &live_code()),
        ] {
            let shouted: Vec<&str> = painted
                .strings()
                .into_iter()
                .filter(|s| s.starts_with(SEQUENCE_REFUSED_PREFIX))
                .collect();
            assert!(
                shouted.is_empty(),
                "a sequence this item can type was warned about: {shouted:?}"
            );
        }
    }

    /// **The call site's ARGUMENTS, not just the call.**
    ///
    /// Both arguments are substituted and the drawn form must change:
    ///
    /// * the SEQUENCE -- the same item, two different drafts, must produce two
    ///   different warnings. A call site that passed a constant sequence, or
    ///   the item's stored one instead of the draft's, passes every test above
    ///   and fails here.
    /// * the SOURCE -- the same draft, two different TOTP states, must produce
    ///   a warning in one and none in the other. A call site that built its own
    ///   `ResolveSource` out of nothing, or passed a defaulted one, fails here.
    ///
    /// This is the shape the crate's reviews keep finding: an argument nulled
    /// at a call site under a pure function that is exhaustively tested.
    #[test]
    fn the_drawn_warning_follows_both_of_its_arguments() {
        let item = item();
        let ctx = styled_context(PANE);

        // -- the sequence argument -----------------------------------------
        let mut missing_field = draft_for(&item, "{S:Missing}");
        let mut unknown_token = draft_for(&item, "{PICKCHARS}");
        let a = frame(&ctx, PANE, &mut missing_field, &item, &live_code(), &[]);
        let b = frame(&ctx, PANE, &mut unknown_token, &item, &live_code(), &[]);
        let said = |p: &Painted| -> String {
            p.strings()
                .into_iter()
                .find(|s| s.starts_with(SEQUENCE_REFUSED_PREFIX))
                .unwrap_or_else(|| panic!("no warning was painted; painted: {:?}", p.strings()))
                .to_string()
        };
        let (said_a, said_b) = (said(&a), said(&b));
        assert_ne!(
            said_a, said_b,
            "two different broken sequences produced the same warning, so the drawn warning \
             is not reading the draft's own sequence"
        );
        assert!(said_a.contains("Missing"), "{said_a:?} does not name the missing field");
        assert!(said_b.contains("PICKCHARS"), "{said_b:?} does not name the unknown token");

        // -- the source argument -------------------------------------------
        // One draft, drawn twice, differing ONLY in the TOTP state handed to
        // the form.
        let mut totp_draft = draft_for(&item, "{TOTP}");
        let with_secret = frame(&ctx, PANE, &mut totp_draft.clone(), &item, &live_code(), &[]);
        let no_secret =
            frame(&ctx, PANE, &mut totp_draft, &item, &detail::TotpState::NoSecret, &[]);
        assert!(
            !with_secret.strings().iter().any(|s| s.starts_with(SEQUENCE_REFUSED_PREFIX)),
            "a {{TOTP}} with a live code was warned about: {:?}",
            with_secret.strings()
        );
        assert!(
            no_secret.strings().iter().any(|s| s.starts_with(SEQUENCE_REFUSED_PREFIX)),
            "a {{TOTP}} on an item with no secret was NOT warned about, so the drawn warning \
             is not reading the TOTP state it was handed: {:?}",
            no_secret.strings()
        );
    }

    // -- 4a: the step list -------------------------------------------------
    //
    // The rows are a pure function of the stored string, so every claim the
    // list makes is asked of `step_rows` directly. The three frame tests below
    // are the wiring pins.

    /// A `ResolveSource` over [`item`], for the row tests.
    fn rows_source<'a>(item: &'a VaultItem, totp: &'a detail::TotpState) -> ResolveSource<'a> {
        let login = item.login.as_ref().unwrap();
        sequence_source(
            login.username.as_deref().unwrap_or(""),
            login.password.as_deref().map_or("", |v| v.as_str()),
            Some(item),
            totp,
        )
    }

    /// **The row index IS the token index, and the whole edit path depends on
    /// it.** `<`, `>` and `x` hand `number - 1` to `sequence_moved` and
    /// `sequence_without`, which index the token list -- so a row model that
    /// dropped, merged or reordered a token would silently make every control
    /// below the fold act on a different step.
    #[test]
    fn every_token_gets_exactly_one_row_at_its_own_index() {
        const SEQUENCE: &str = "{ESC}{USERNAME}{TAB}{DELAY 250}{DELAY=40}{PASSWORD}{ENTER}";
        let item = item();
        let totp = live_code();
        let tokens = sequence_view(SEQUENCE).tokens;
        let rows = step_rows(SEQUENCE, &rows_source(&item, &totp), false);

        assert_eq!(rows.len(), tokens.len(), "rows: {rows:?}");
        for (index, (row, token)) in rows.iter().zip(&tokens).enumerate() {
            assert_eq!(row.number, index + 1, "row {index} is numbered {}", row.number);
            // The label is the chip's, so the two views of one step cannot
            // drift into two vocabularies.
            assert_eq!(row.label, token.chip_label(), "row {index}");
        }
    }

    /// The badges the design asks for, on the tokens that earn them -- and the
    /// two it does not draw, on the tokens that are not acts.
    #[test]
    fn each_kind_of_token_wears_the_badge_that_names_what_it_does() {
        const SEQUENCE: &str = "{ESC}{USERNAME}{TAB}{DELAY 250}{DELAY=40}{PASSWORD}{PICKCHARS}";
        let item = item();
        let totp = live_code();
        let badges: Vec<&str> = step_rows(SEQUENCE, &rows_source(&item, &totp), false)
            .iter()
            .map(|r| r.kind.badge())
            .collect();
        assert_eq!(
            badges,
            vec!["KEY", "TEXT", "KEY", "WAIT", "RATE", "TEXT", "RAW"],
            "the badges do not say what each step actually does"
        );
    }

    /// **A password is never in the row, in either state of the eye.**
    ///
    /// Both states, because "masked" must be a property of the FIELD and not
    /// of a flag that is off right now: the reveal argument is passed both
    /// ways and the assertion is the same both times. The one-time code is
    /// held to the same rule -- it is a credential too.
    #[test]
    fn a_secret_step_shows_a_mask_and_never_its_value() {
        const SEQUENCE: &str = "{USERNAME}{TAB}{PASSWORD}{TOTP}";
        let item = item();
        let totp = live_code();
        let source = rows_source(&item, &totp);

        for reveal in [false, true] {
            let rows = step_rows(SEQUENCE, &source, reveal);
            let secrets: Vec<&StepRow> = rows.iter().filter(|r| r.secret).collect();
            assert_eq!(secrets.len(), 2, "reveal={reveal}: {rows:?}");
            for row in &secrets {
                assert_eq!(row.payload, SECRET_MASK, "reveal={reveal}");
            }
            for row in &rows {
                for cell in [&row.label, &row.payload, &row.note] {
                    assert!(
                        !cell.contains(PASSWORD),
                        "reveal={reveal}: a row cell {cell:?} carries the password"
                    );
                    assert!(
                        !cell.contains(TOTP_CODE),
                        "reveal={reveal}: a row cell {cell:?} carries the one-time code"
                    );
                }
            }
        }
    }

    /// The positive control on the test above: the eye really does reach the
    /// row, for the values it is allowed to reach. Without this, an
    /// implementation that put NOTHING in any payload would satisfy the
    /// masking test while showing the user nothing at all.
    #[test]
    fn the_eye_fills_a_non_secret_rows_payload_and_leaves_it_empty_when_shut() {
        const SEQUENCE: &str = "{USERNAME}";
        let item = item();
        let totp = live_code();
        let source = rows_source(&item, &totp);
        assert_eq!(step_rows(SEQUENCE, &source, true)[0].payload, USERNAME);
        assert_eq!(step_rows(SEQUENCE, &source, false)[0].payload, "");
    }

    /// The rate a `{DELAY=n}` sets is carried into the notes of the rows BELOW
    /// it and not the ones above -- which is the design's picture, drawn
    /// without folding two tokens into one row.
    #[test]
    fn a_rate_change_notes_itself_on_the_steps_it_governs_and_not_the_earlier_ones() {
        const SEQUENCE: &str = "{USERNAME}{DELAY=40}{PASSWORD}";
        let item = item();
        let totp = live_code();
        let rows = step_rows(SEQUENCE, &rows_source(&item, &totp), false);
        assert!(!rows[0].note.contains("40 ms/char"), "the earlier row took a later rate: {:?}", rows[0].note);
        assert!(rows[2].note.contains("40 ms/char"), "the later row missed the rate: {:?}", rows[2].note);
    }

    // -- 4a: the tally -----------------------------------------------------

    /// **The count and the time come from the runner's own plan.** Steps are
    /// not rows: a `{DELAY=n}` is a row and no step, and the whole point of
    /// asking `plan` is that the figure on screen is the figure
    /// `MAX_SEQUENCE` is checked against.
    #[test]
    fn the_tally_counts_the_runners_steps_rather_than_the_rows() {
        const SEQUENCE: &str = "{ESC}{USERNAME}{TAB}{DELAY 250}{DELAY=40}{PASSWORD}{ENTER}";
        let item = item();
        let totp = live_code();
        let source = rows_source(&item, &totp);

        let rows = step_rows(SEQUENCE, &source, false);
        let tally = sequence_tally(SEQUENCE, &source).expect("a runnable sequence has a tally");
        assert_eq!(rows.len(), 7, "the fixture changed shape");
        assert_eq!(
            tally.steps, 6,
            "the tally is not counting the acts the user can see -- either the {{DELAY=40}} row              was counted, or the runner's burst chunking was"
        );
        // **The control on that assertion**, and the reason the fixture types
        // at 40 ms/char: the runner really does split this password into more
        // `Step`s than there are acts, so `tally.steps == 6` cannot be
        // satisfied by handing back `Plan::len()`.
        let chunks = crate::injector::sequence::plan(
            &sequence_view(SEQUENCE).tokens,
            &crate::injector::sequence::Resolved {
                username: source.username,
                password: source.password,
                totp: edit_time_totp(source.totp),
                custom: source.custom.clone(),
            },
        )
        .expect("the fixture must plan")
        .len();
        assert!(
            chunks > tally.steps,
            "the fixture no longer distinguishes the acts ({}) from the runner's chunks ({chunks})",
            tally.steps
        );
        assert!(
            tally.total >= Duration::from_millis(250),
            "the 250 ms wait is not in the total: {:?}",
            tally.total
        );
        assert!(
            tally.burst <= tally.total && tally.burst > Duration::ZERO,
            "the burst figure is not a step of this plan: {tally:?}"
        );
        assert_eq!(tally_label(&tally), format!("6 steps \u{b7} {} total", duration_label(tally.total)));
    }

    /// A sequence the runner refuses has no tally, and the block says so
    /// rather than showing a zero -- "0 steps" would read as a sequence that
    /// is merely empty.
    #[test]
    fn a_refused_sequence_has_no_tally() {
        let item = item();
        let totp = live_code();
        let source = rows_source(&item, &totp);
        assert!(sequence_refusal(&sequence_view("{PICKCHARS}").tokens, &source).is_some());
        assert_eq!(sequence_tally("{PICKCHARS}", &source), None);
    }

    /// The budget line quotes the CRATE's limits, not the design's
    /// illustrative ones.
    #[test]
    fn the_budget_line_is_measured_against_the_runners_own_limits() {
        let item = item();
        let totp = live_code();
        let tally = sequence_tally("{USERNAME}", &rows_source(&item, &totp)).unwrap();
        let line = budget_label(&tally);
        assert!(
            line.contains(&duration_label(crate::injector::sequence::MAX_SEQUENCE)),
            "the budget does not quote MAX_SEQUENCE: {line:?}"
        );
        assert!(
            line.contains(&duration_label(crate::injector::sequence::MAX_BURST)),
            "the budget does not quote MAX_BURST: {line:?}"
        );
    }

    #[test]
    fn a_duration_reads_in_milliseconds_below_a_second_and_in_tenths_above_it() {
        assert_eq!(duration_label(Duration::from_millis(250)), "250 ms");
        assert_eq!(duration_label(Duration::from_millis(2100)), "2.1 s");
        assert_eq!(duration_label(Duration::from_secs(60)), "60.0 s");
    }

    // -- 4c: the template view ---------------------------------------------

    /// **The chips write the grammar `key_sequence` really parses.**
    ///
    /// The design's picture spells a pause `{WAIT=250}`, which does not exist:
    /// it would come back as `Token::Unknown` and be refused at fill time. A
    /// chip that inserted one would be a button that breaks the sequence it
    /// was clicked to build, so every chip is parsed here and required to be
    /// something this build understands.
    #[test]
    fn every_insert_chip_parses_into_a_step_this_build_understands() {
        for chip in TEMPLATE_CHIPS {
            let tokens = key_sequence::parse(chip);
            assert!(!tokens.is_empty(), "the chip {chip:?} inserts nothing");
            for token in &tokens {
                assert!(
                    token.is_understood(),
                    "the chip {chip:?} inserts a step this build cannot type: {token:?}"
                );
            }
            assert_eq!(
                key_sequence::render(&tokens),
                *chip,
                "the chip {chip:?} is not spelled the way this build spells it"
            );
            assert_eq!(template_fault(chip), None, "the chip {chip:?} is not a saveable template");
        }
        // The design's own spelling, held here so the departure is a test
        // rather than a comment: it does NOT parse to anything typable.
        for spelling in ["{WAIT=250}", "{SHIFT+TAB}", "{CTRL+A}"] {
            let tokens = key_sequence::parse(spelling);
            assert!(
                tokens.iter().any(|t| !t.is_understood()),
                "the design's {spelling:?} became typable; the chips can be revisited"
            );
        }
    }

    /// Inserting into an empty template materialises the default first -- the
    /// same correction `sequence_with` makes, for the same reason.
    #[test]
    fn inserting_into_an_empty_template_keeps_the_default_it_stood_for() {
        assert_eq!(
            template_with("", "{ENTER}"),
            format!("{}{{ENTER}}", key_sequence::DEFAULT_SEQUENCE)
        );
        assert_eq!(template_with("{TAB}", "{ENTER}"), "{TAB}{ENTER}");
    }

    /// **What "will not parse" means, and what it must NOT catch.**
    ///
    /// Everything well formed passes, including the foreign constructs
    /// `AppMatch::sequence` exists to carry -- refusing those would make this
    /// build unable to save an item another password manager wrote. The one
    /// shape refused is the one where the field and the step list under it
    /// disagree: an unterminated brace.
    #[test]
    fn a_template_is_faulted_exactly_when_it_will_not_read_back_as_itself() {
        for good in [
            "",
            "{USERNAME}{TAB}{PASSWORD}",
            "{ESC}{USERNAME}{TAB}{DELAY 250}{DELAY=40}{PASSWORD}{ENTER}",
            // Carried, not understood -- and still saveable.
            "{PICKCHARS}{USERNAME}",
            "{APPACTIVATE Foo}",
            "{WAIT=250}",
            // An escaped brace is well formed and means a typed brace.
            "a{{}b",
            "{S:PIN}",
        ] {
            assert_eq!(template_fault(good), None, "the well-formed {good:?} was refused");
        }
        for bad in ["{", "{USERNAME}{TAB", "a{FOO", "{{"] {
            assert_eq!(
                template_fault(bad),
                Some(TEMPLATE_UNPARSED),
                "the malformed {bad:?} was accepted"
            );
        }
    }

    /// The fault is a property of what the USER wrote, never of what they
    /// inherited: a vault already holding an unreadable sequence must still be
    /// renameable.
    #[test]
    fn an_inherited_bad_sequence_does_not_block_a_save_but_an_edited_one_does() {
        let item = item();
        let mut draft = draft_for(&item, "{USERNAME}{TAB");
        assert_eq!(template_fault(&draft.app.as_ref().unwrap().sequence), Some(TEMPLATE_UNPARSED));
        assert!(draft.is_saveable(), "an inherited unreadable sequence blocked the save");
        assert_eq!(draft.sequence_fault(), None);

        draft.app.as_mut().unwrap().template_touched = true;
        assert_eq!(draft.sequence_fault(), Some(TEMPLATE_UNPARSED));
        assert!(!draft.is_saveable(), "an edited unparseable template was still saveable");
        // ...and it is the TEMPLATE and not the name that is wrong, so the two
        // refusals stay distinguishable.
        assert!(draft.is_valid(), "the fixture lost its name");
    }

    // -- 4c: the bridge ----------------------------------------------------

    /// **The round trip, byte for byte.** Opening the template view, looking
    /// at it and closing it again must leave the stored string exactly as it
    /// arrived -- including a spelling this build would never produce. The
    /// fixture is deliberately one `render` would rewrite (a lower-case
    /// `{S:pin}` sitting beside a construct this build does not model), so a
    /// seed that went through the renderer fails here.
    #[test]
    fn opening_and_closing_the_template_view_does_not_re_spell_the_stored_string() {
        const STORED: &str = "{PICKCHARS}{USERNAME}{DELAY 250}{S:pin}{APPACTIVATE Foo}";
        let item = item();
        let ctx = styled_context(PANE);
        let mut draft = draft_for(&item, STORED);
        let open = open_builder(&ctx, PANE, &mut draft, &item, &live_code());

        let to_template = open.rect_of(VIEW_TEMPLATE).center();
        let after = frame(&ctx, PANE, &mut draft, &item, &live_code(), &click(to_template));
        assert!(draft.app.as_ref().unwrap().template_view, "the Template toggle did not switch");
        assert_eq!(
            draft.app.as_ref().unwrap().template_draft,
            STORED,
            "the template box was seeded with something other than the stored bytes"
        );

        let to_steps = after.rect_of(VIEW_STEPS).center();
        let _ = frame(&ctx, PANE, &mut draft, &item, &live_code(), &click(to_steps));
        assert!(!draft.app.as_ref().unwrap().template_view, "the Steps toggle did not switch back");
        assert_eq!(
            draft.app.as_ref().unwrap().sequence,
            STORED,
            "a round trip through the template view re-spelled the user's string"
        );
        assert!(
            !draft.app.as_ref().unwrap().template_touched,
            "merely looking at the template counted as an edit"
        );
        assert!(draft.is_saveable(), "an untouched round trip made the item unsaveable");
    }

    /// **The wiring pin for 4c.** Typing into the template box writes the
    /// user's own bytes onto the draft, and the step list under it re-reads
    /// them -- which is the whole of the bridge. Deleting the
    /// `app.sequence = app.template_draft.clone();` assignment fails here.
    #[test]
    fn typing_a_template_stores_those_bytes_and_re_parses_them_underneath() {
        let item = item();
        let ctx = styled_context(PANE);
        let mut draft = draft_for(&item, "{USERNAME}");
        let open = open_builder(&ctx, PANE, &mut draft, &item, &live_code());
        let at = open.rect_of(VIEW_TEMPLATE).center();
        let _ = frame(&ctx, PANE, &mut draft, &item, &live_code(), &click(at));

        // **Really typed, not assigned.** Click into the box -- it is the one
        // widget between the toggle and the Insert row -- and send the
        // characters as input events, so the assignment that copies the box
        // onto the draft is on the path under test.
        let template = frame(&ctx, PANE, &mut draft, &item, &live_code(), &[]);
        let top = template.rect_of(VIEW_TEMPLATE).bottom();
        let bottom = template.rect_of("Insert").top();
        let field = template
            .texts
            .iter()
            .find(|(text, rect)| text == "{USERNAME}" && rect.top() > top && rect.top() < bottom)
            .map(|(_, rect)| *rect)
            .expect("the template box is not drawn between the toggle and the Insert row");

        // Two frames: egui grants focus at the END of the frame the click
        // arrives in, so text sent in that same frame goes nowhere.
        let _ = frame(&ctx, PANE, &mut draft, &item, &live_code(), &click(field.center()));
        let mut events: Vec<egui::Event> = Vec::new();
        events.push(egui::Event::Key {
            key: egui::Key::End,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        });
        events.push(egui::Event::Text("{TAB}{PASSWORD}".to_string()));
        let after = frame(&ctx, PANE, &mut draft, &item, &live_code(), &events);
        assert_eq!(draft.app.as_ref().unwrap().sequence, "{USERNAME}{TAB}{PASSWORD}");
        // The parsed list is on screen under the field, so the string is never
        // the only thing the user can see.
        for step in ["Username", "Tab", "Password"] {
            assert!(
                after.strings().contains(&step),
                "the parsed step {step:?} is not drawn under the template field: {:?}",
                after.strings()
            );
        }
        assert!(after.strings().contains(&TEMPLATE_READS_AS), "the read-out lost its heading");
    }

    /// **The refusal reaches the button.** An unparseable template turns Save
    /// off and re-captions it, and the reason is on screen in the strip as
    /// well as under the field.
    #[test]
    fn an_unparseable_template_turns_save_off_and_says_why() {
        let item = item();
        let ctx = styled_context(PANE);
        let mut draft = draft_for(&item, "{USERNAME}");
        let _ = open_builder(&ctx, PANE, &mut draft, &item, &live_code());
        {
            let app = draft.app.as_mut().unwrap();
            app.template_view = true;
            app.template_draft = "{USERNAME}{TAB".to_string();
            app.template_touched = true;
            app.sequence = app.template_draft.clone();
        }
        assert!(!draft.is_saveable(), "the pure gate let an unparseable template through");

        // Two frames: `egui::Panel::bottom` lays its content out against the
        // height it measured LAST frame, and the refusal line above the
        // buttons is new this one, so the strip is a frame behind.
        let _ = frame(&ctx, PANE, &mut draft, &item, &live_code(), &[]);
        let painted = frame(&ctx, PANE, &mut draft, &item, &live_code(), &[]);
        assert!(
            painted.strings().contains(&SAVE_TEMPLATE_BLOCKED),
            "Save does not say the template is what is wrong: {:?}",
            painted.strings()
        );
        assert!(
            painted.strings().contains(&TEMPLATE_UNPARSED),
            "the refusal itself is not on screen: {:?}",
            painted.strings()
        );

        // The control: fix it, and both go away.
        {
            let app = draft.app.as_mut().unwrap();
            app.template_draft = "{USERNAME}{TAB}".to_string();
            app.sequence = app.template_draft.clone();
        }
        assert!(draft.is_saveable(), "a fixed template did not re-enable the save");
        let _ = frame(&ctx, PANE, &mut draft, &item, &live_code(), &[]);
        let fixed = frame(&ctx, PANE, &mut draft, &item, &live_code(), &[]);
        assert!(!fixed.strings().contains(&TEMPLATE_UNPARSED));
        assert!(!fixed.strings().contains(&SAVE_TEMPLATE_BLOCKED));
    }

    /// **The wiring pin for 4a's summary.** The tally is drawn, and it moves
    /// when the sequence does. A constant would pass "is it on screen".
    #[test]
    fn the_step_count_on_screen_follows_the_sequence() {
        let item = item();
        let ctx = styled_context(PANE);
        let mut draft = draft_for(&item, "{USERNAME}");
        let one = open_builder(&ctx, PANE, &mut draft, &item, &live_code());
        let totp = live_code();
        let source = rows_source(&item, &totp);
        let expected_one = tally_label(&sequence_tally("{USERNAME}", &source).unwrap());
        assert!(
            one.strings().contains(&expected_one.as_str()),
            "the tally {expected_one:?} is not on screen: {:?}",
            one.strings()
        );

        draft.app.as_mut().unwrap().sequence = "{USERNAME}{TAB}{PASSWORD}{ENTER}".to_string();
        let more = frame(&ctx, PANE, &mut draft, &item, &live_code(), &[]);
        let expected_more =
            tally_label(&sequence_tally("{USERNAME}{TAB}{PASSWORD}{ENTER}", &source).unwrap());
        assert_ne!(expected_one, expected_more, "the fixture cannot tell the two apart");
        assert!(
            more.strings().contains(&expected_more.as_str()),
            "the tally did not follow the sequence: {:?}",
            more.strings()
        );
    }

    /// **The password is not painted in the step list**, with the eye shut and
    /// with it open. Asked of the band the rows occupy rather than of the
    /// whole form, because the form's own password box is above it and the
    /// eye's preview -- which is a reveal the user asked for by name -- is
    /// below.
    #[test]
    fn no_row_in_the_step_list_paints_the_password() {
        const SEQUENCE: &str = "{USERNAME}{TAB}{PASSWORD}{ENTER}";
        let item = item();
        let ctx = styled_context(PANE);
        let mut draft = draft_for(&item, SEQUENCE);
        let open = open_builder(&ctx, PANE, &mut draft, &item, &live_code());

        for previewing in [false, true] {
            draft.app.as_mut().unwrap().previewing = previewing;
            let painted = frame(&ctx, PANE, &mut draft, &item, &live_code(), &[]);
            let floor = painted.rect_of(APP_SEQUENCE_HINT).bottom();
            let ceiling = painted.rect_of("Add a value").top();
            for (source, drawn, rect) in &painted.rendered {
                if rect.top() <= floor || rect.top() >= ceiling {
                    continue;
                }
                assert!(
                    !source.contains(PASSWORD) && !drawn.contains(PASSWORD),
                    "previewing={previewing}: the step list painted the password ({source:?} / \
                     {drawn:?})"
                );
            }
            // The positive control: the masked row IS there, so the loop above
            // is not passing over an empty band.
            assert!(
                painted.strings().contains(&SECRET_MASK),
                "previewing={previewing}: the masked password row is not drawn at all"
            );
        }
        let _ = open;
    }
}

/// The edit form's SHAPE, on a pane too short to hold it.
///
/// The bug these exist for: `draw_detail_edit` drew the title, the whole form
/// and the Save/Cancel row into one plain `Ui` with no scroll area anywhere,
/// so on a window shorter than the form the buttons were painted BELOW the
/// pane and the user could neither reach them nor scroll to them -- an edit
/// that could not be saved or cancelled. Commit `4b05adb`'s app block is what
/// pushed a routine login form past a routine window's height.
///
/// These are geometry assertions on the rects egui really painted, not on the
/// presence of a galley: a galley exists whether or not it is on screen, and
/// `Galley::text()` answers with the SOURCE string, so "the button is there"
/// is exactly the claim that stayed true all through the bug. Each test
/// carries a positive control saying what it would look like to be blind.
#[cfg(test)]
mod edit_pane_layout_tests {
    use super::*;
    // One copy of "what counts as ink", shared with `detail.rs`'s read-pane
    // suite. See [`detail::shape_ink`] for why it lives there.
    use detail::shape_ink::{glyph_ink, ink_of};
    use eframe::egui::{Pos2, Rect, Vec2};

    /// The narrowest the detail pane can be, derived from the three constants
    /// that produce it (900 - 212 - 390 = 298pt) rather than written out --
    /// same derivation, and same reason, as `detail.rs`'s `MIN_PANE`.
    const MIN_PANE_WIDTH: f32 = crate::settings::MIN_VAULT_WINDOW_SIZE.0 as f32
        - crate::vault_window::SIDEBAR_WIDTH
        - crate::vault_window::LIST_WIDTH;

    /// The pane's height at the app's minimum window height. 600 is the whole
    /// WINDOW; the detail pane gets what is left after the toolbar and the
    /// central panel's 18pt margins, so this over-states the room the form
    /// really has. That is the safe direction: a form that will not fit here
    /// cannot fit in the app either.
    const MIN_PANE_HEIGHT: f32 = crate::settings::MIN_VAULT_WINDOW_SIZE.1 as f32;

    /// Shorter than anything the app can be resized to, and deliberately so:
    /// it is the case where nearly every field is off-screen, which is where a
    /// layout that merely *shrinks* the form instead of scrolling it gets
    /// caught.
    const TINY_PANE_HEIGHT: f32 = 300.0;

    #[derive(Default)]
    struct Painted {
        texts: Vec<(String, Rect)>,
        /// Every string the frame painted with **the characters egui really
        /// laid glyphs for**, which is not the same list as [`texts`].
        ///
        /// `Galley::text()` answers with the layout job's SOURCE string, so a
        /// run egui elided down to `"Save (needs\u{2026}"` -- or all the way
        /// to one `"\u{2026}"` -- still reports the full label it was handed,
        /// off a rect that is honestly small but a name that is not. Every
        /// assertion in this module used to be blind in exactly that way,
        /// which is the same class of defect as the header title this crate
        /// already shipped a vacuous test for. Borrowed from `detail.rs`'s
        /// `Frame::rendered`, which exists for that reason.
        rendered: Vec<(String, String, Rect)>,
        /// Every filled rectangle, so the scroll bar -- which paints no
        /// string at all -- can be found by its geometry.
        rects: Vec<(Rect, egui::Color32)>,
        /// The box the run's GLYPHS really cover, which is not
        /// [`Painted::texts`]'s box.
        ///
        /// `Galley::size()` is the box the LAYOUT was given, and inside a
        /// `horizontal_wrapped` row that is the whole wrap width: the word
        /// "seconds" reports a 93.7pt box for 40pt of ink, and it therefore
        /// appears to sit on top of the wait field beside it. Every one of
        /// this crate's earlier geometry blindnesses has been a galley
        /// answering about the layout job rather than about the pixels
        /// (`Galley::text()` for the characters, this for the box), so the
        /// overlap assertion is asked of the glyph positions egui really
        /// placed -- the same source `Painted::rendered` reads.
        glyphs: Vec<(String, Rect)>,
        /// Every OTHER shape the frame drew ink with, named and boxed:
        /// carets, icons, circles, lines, curves, meshes.
        ///
        /// This field exists because the two above it are not a partition.
        /// `walk` used to end `_ => {}`, so a shape that was neither a
        /// galley nor a filled rect was DISCARDED, and every assertion in
        /// this module was silent about it. Instrumenting the discard on the
        /// tallest form found three `Shape::Path` -- the combo boxes' carets,
        /// one of them at x = 188.8..198.6, in the very row the 309.4pt card
        /// defect lived in. A later layout could push a caret past the pane's
        /// edge without moving a single rect, and nothing here would say so.
        /// That is the same blindness one level up from the one the rects
        /// field was added to close, which was itself one level up from the
        /// texts field. See [`ink_of`] for what counts as drawn.
        ///
        /// `Rect` is NOT in here -- it has [`Painted::rect_ink`], so that the
        /// non-vacuity assertion on this field still says what it says: that
        /// the tallest form really paints carets.
        marks: Vec<(&'static str, Rect)>,
        /// The ink each filled rect really lays down, which is not
        /// [`Painted::rects`]'s box.
        ///
        /// Kept apart from `rects` rather than replacing it, because the two
        /// answer different questions and `bar_rects` needs the first: the
        /// scroll bar's WIDTH is `RectShape::rect`'s width, and a bar measured
        /// against its blurred bounds would report a width it does not have.
        /// What the assertions need this for is the other question -- whether
        /// any of the ink is painted where the reader cannot reach it. A rect
        /// recorded at 280..298 on a 298pt pane covers 274..304 with a 6pt
        /// outside stroke, 270..308 with a 20pt blur, and 275.9..302.1 rotated
        /// 0.6 rad, and until this field existed every one of those was in
        /// bounds as far as this module could tell. See [`ink_of`].
        rect_ink: Vec<(Rect, egui::Color32)>,
    }

    impl Painted {
        fn strings(&self) -> Vec<&str> {
            self.texts.iter().map(|(t, _)| t.as_str()).collect()
        }

        fn rects_of(&self, label: &str) -> Vec<Rect> {
            self.texts.iter().filter(|(t, _)| t == label).map(|(_, r)| *r).collect()
        }

        /// The one rect painting `label`, or a failure naming everything that
        /// was painted.
        fn rect_of(&self, label: &str) -> Rect {
            let found = self.rects_of(label);
            assert_eq!(
                found.len(),
                1,
                "expected exactly one {label:?} in the edit form, found {}; painted: {:?}",
                found.len(),
                self.strings()
            );
            found[0]
        }

        /// The smallest painted rectangle enclosing `inner`: the FRAME a
        /// widget drew around its own text.
        ///
        /// This is what the eye reads as a control's height -- the button's
        /// outline, the combo's box, the spinner's background -- and none of
        /// it is a `Shape::Text`, so `rect_of` cannot see any of it. Painted
        /// ink, not a requested size: `Response::rect` would restate what the
        /// code asked for, which is the thing under test. Smallest, because
        /// the card, the pane and the window all enclose it too.
        fn frame_around(&self, inner: Rect) -> Rect {
            let mut found: Vec<Rect> = self
                .rects
                .iter()
                .map(|(r, _)| *r)
                .filter(|f| {
                    f.min.x <= inner.min.x + 1.0
                        && f.min.y <= inner.min.y + 1.0
                        && f.max.x >= inner.max.x - 1.0
                        && f.max.y >= inner.max.y - 1.0
                })
                .collect();
            found.sort_by(|a, b| (a.width() * a.height()).total_cmp(&(b.width() * b.height())));
            *found.first().unwrap_or_else(|| {
                panic!(
                    "nothing is painted around {inner:?}, so that control drew no frame at \
                     all -- which is what a zero-sized or undrawn widget looks like"
                )
            })
        }

        /// What was actually DRAWN for the run whose source text is `label`
        /// -- glyphs, not the string the layout job was handed. See
        /// [`Painted::rendered`].
        fn rendered_glyphs(&self, label: &str) -> String {
            let found: Vec<&String> = self
                .rendered
                .iter()
                .filter(|(source, _, _)| source == label)
                .map(|(_, rendered, _)| rendered)
                .collect();
            assert_eq!(
                found.len(),
                1,
                "expected exactly one run laid out from {label:?}, found {}; painted: {:?}",
                found.len(),
                self.strings()
            );
            found[0].clone()
        }
    }

    fn walk(shape: &egui::Shape, painted: &mut Painted) {
        match shape {
            egui::Shape::Text(text) => {
                let rect = Rect::from_min_size(text.pos, text.galley.size());
                // One entry per glyph egui really placed, so an elided run
                // reports the prefix it drew and the ellipsis it drew instead
                // of the label it was asked for.
                let rendered: String = text
                    .galley
                    .rows
                    .iter()
                    .flat_map(|row| row.glyphs.iter().map(|glyph| glyph.chr))
                    .collect();
                painted.texts.push((text.galley.text().to_string(), rect));
                painted.rendered.push((text.galley.text().to_string(), rendered, rect));
                // ... and the box the ink really covers. See `Painted::glyphs`.
                if let Some(ink) = glyph_ink(text) {
                    painted.glyphs.push((text.galley.text().to_string(), ink));
                }
            }
            egui::Shape::Rect(rect) => {
                painted.rects.push((rect.rect, rect.fill));
                // ... and, separately, the ink that box really lays down,
                // which is NOT `rect.rect`. See [`Painted::rect_ink`].
                if let Some((_, ink)) = ink_of(shape) {
                    painted.rect_ink.push((ink, rect.fill));
                }
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, painted);
                }
            }
            // NOT `_ => {}`. Everything else that draws ink is recorded by
            // its visual bounds, so the assertions below can see a caret, an
            // icon, a circle or a line leave the pane. See [`Painted::marks`].
            other => {
                if let Some(mark) = ink_of(other) {
                    painted.marks.push(mark);
                }
            }
        }
    }


    fn raw_input(pane: Vec2, events: &[egui::Event]) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, pane)),
            events: events.to_vec(),
            ..Default::default()
        }
    }

    /// A context with `theme::apply`'s fonts live, sized to `pane`. The two
    /// throwaway frames are this crate's standing harness: a font set
    /// registered during a frame is only usable from the next one.
    fn styled_context(pane: Vec2) -> egui::Context {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(raw_input(pane, &[]), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(raw_input(pane, &[]), |_ui| {});
        ctx
    }

    fn frame(
        ctx: &egui::Context,
        pane: Vec2,
        draft: &mut EditDraft,
        creating: bool,
        events: &[egui::Event],
    ) -> Painted {
        frame_for(ctx, pane, draft, creating, events, None, &detail::TotpState::NoSecret)
    }

    /// [`frame`], against a real item and a real TOTP state -- what the
    /// keystroke palette and its preview need and nothing else on this form
    /// reads.
    fn frame_for(
        ctx: &egui::Context,
        pane: Vec2,
        draft: &mut EditDraft,
        creating: bool,
        events: &[egui::Event],
        item: Option<&VaultItem>,
        totp: &detail::TotpState,
    ) -> Painted {
        acting_frame_for(ctx, pane, draft, creating, events, item, totp).1
    }

    /// [`frame_for`], keeping the action the form reported.
    ///
    /// Every layout test here throws the action away, which is right for a
    /// test about pixels -- but it left the Save button's validity gate
    /// measured by nothing but a fill colour. See
    /// [`clicking_save_on_an_invalid_draft_saves_nothing`].
    fn acting_frame_for(
        ctx: &egui::Context,
        pane: Vec2,
        draft: &mut EditDraft,
        creating: bool,
        events: &[egui::Event],
        item: Option<&VaultItem>,
        totp: &detail::TotpState,
    ) -> (EditAction, Painted) {
        let mut apps = AppIdentityCache::default();
        let mut action = EditAction::None;
        let output = ctx.run_ui(raw_input(pane, events), |ui| {
            action = draw_detail_edit(ui, draft, &[], creating, &mut apps, item, totp);
        });
        let mut painted = Painted::default();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut painted);
        }
        (action, painted)
    }

    /// The tallest form the app can actually put on screen: a Login (the only
    /// body with a generator row), being CREATED (which is when the generator
    /// is offered), with an app binding drawn in full (commit `4b05adb`'s
    /// block, the height that made this bug reachable), and with the name
    /// still empty so the conditional "Name is required." error is up too.
    /// This is the case the user hit.
    fn tallest_draft() -> EditDraft {
        let mut draft = EditDraft::empty();
        draft.generator = GeneratorDraft { passphrase: false, length: 33, words: 7 };
        draft.app = Some(AppMatchDraft::from_match(&AppMatch {
            process: "chrome.exe".to_string(),
            title: String::new(),
            hosted: false,
            // A path that exists nowhere, so the identity lookup resolves the
            // same way on every machine and no assertion here depends on what
            // happens to be installed.
            path: r"C:\Deskwarden Test\Chrome\chrome.exe".to_string(),
            args: r#"--profile-directory="Profile 2""#.to_string(),
            sequence: String::new(),
            trigger: TriggerMode::Prompt,
        }));
        assert!(!draft.is_valid(), "the tall case wants the name error showing");
        draft
    }

    /// The label the disabled Save wears while the name is empty.
    const SAVE: &str = "Save (needs a name)";

    /// The suffix the generator's size spinner wears in each of the row's two
    /// states. One place, so a test that measures one state cannot silently
    /// be measuring the other's widget.
    fn spinner_suffix(passphrase: bool) -> &'static str {
        if passphrase { " words" } else { " chars" }
    }

    /// `label` is painted inside the pane **and is still legible as itself**.
    ///
    /// Both halves, because either alone is satisfiable by a broken pane:
    ///
    /// * The rect check is `contains_rect` on BOTH axes -- deliberately
    ///   stronger than `detail.rs`'s vertical-only `assert_visible`, because
    ///   a control the user has to click is not reachable by being merely at
    ///   the right height.
    /// * The glyph check is `rendered_glyphs`, because the rect above is
    ///   taken off a galley whose `text()` is the SOURCE string. A Save
    ///   button squeezed down to `"Save (needs\u{2026}"` paints a small,
    ///   entirely in-bounds box and reports the full label, so the rect half
    ///   passes and says nothing. Equality with the label, not merely
    ///   `!= "\u{2026}"`: these are two- and three-word button captions with
    ///   no room to lose a word, and a check for the ellipsis alone would
    ///   wave `"Save (needs\u{2026}"` through.
    fn assert_inside(what: &str, label: &str, pane: Vec2, painted: &Painted) {
        let rect = painted.rect_of(label);
        let bounds = Rect::from_min_size(Pos2::ZERO, pane);
        assert!(
            bounds.contains_rect(rect),
            "{what} is painted at {rect:?}, outside the {}x{} pane -- the user cannot \
             click it. Painted: {:?}",
            pane.x,
            pane.y,
            painted.strings()
        );
        let rendered = painted.rendered_glyphs(label);
        assert_eq!(
            rendered, label,
            "{what} was laid out from {label:?} but DREW {rendered:?} on a {}x{} pane -- \
             the control is on screen and unreadable",
            pane.x, pane.y
        );
    }

    /// A pane wide enough that the generator row does NOT wrap, and tall
    /// enough to hold the whole form. Wrapping is asserted separately, at
    /// `MIN_PANE_WIDTH`, by
    /// [`the_generator_rows_controls_are_all_reachable_at_the_minimum_width`].
    const ROOMY_PANE: Vec2 = egui::vec2(560.0, 1400.0);

    /// The three frames the generator row paints, in the order the user
    /// reads them: Generate, the kind combo, the size spinner --
    /// **in whichever of the row's two states `passphrase` names.**
    ///
    /// **Both states, and why that is not a detail.** The row's third control
    /// is a `DragValue` on either branch -- `2cb45fc`'s report claimed the
    /// widget itself changes with the kind, and that is simply false; what
    /// changes is which draft field it is bound to and which suffix it wears
    /// (`" words"` against `" chars"`), and the combo's `selected_text` with
    /// it. But this helper used to key on `" chars"` unconditionally and
    /// every caller passed a draft with `passphrase == false`, so the
    /// passphrase branch was measured by NOTHING: an `interact_size.y = 26.0`
    /// planted inside it left the row misaligned in exactly the way the fix
    /// was written to stop, and the whole suite stayed green. Taking the
    /// state as a parameter is what makes the alignment a fact about the ROW
    /// rather than about one of its two spellings.
    ///
    /// **The count assertion is the point.** egui culls a shape lying
    /// entirely outside the screen rect, so a control pushed off the pane
    /// comes back as NOTHING -- and every "they all agree" assertion is
    /// vacuously true of two controls, or of one. `frame_around` panics
    /// rather than skipping when a control drew no frame, which is what a
    /// zero-sized widget looks like: on screen to a presence check, invisible
    /// to the eye.
    fn generator_row_frames(painted: &Painted, passphrase: bool) -> [Rect; 3] {
        let generate = painted.rect_of("Generate");
        // The LAST match: on the password branch the form paints the field
        // label "Password" above the row and the combo's `selected_text`
        // spells the same word, so the combo is the lower of the two.
        let kind = if passphrase { "Passphrase" } else { "Password" };
        let combo_text = *painted.rects_of(kind).last().unwrap_or_else(|| {
            panic!(
                "the generator combo is showing no kind at all; painted: {:?}",
                painted.strings()
            )
        });
        let spinner = painted.rect_of(spinner_suffix(passphrase));
        assert!(
            combo_text.top() > generate.top() - 40.0 && combo_text.top() < generate.top() + 40.0,
            "the {combo_text:?} taken for the generator combo is nowhere near the Generate \
             button at {generate:?} -- this helper picked up the wrong galley"
        );
        [
            painted.frame_around(generate),
            painted.frame_around(combo_text),
            painted.frame_around(spinner),
        ]
    }

    /// **The generator row's three controls are one row of one height.**
    ///
    /// The reported defect: "Password dropdown on Edit screen looks off since
    /// it is lower than the rest of buttons". Measured on the pre-fix layout,
    /// on this exact pane -- Generate `top=261.94 h=32`, the combo
    /// `top=268.94 h=26`, the spinner `top=265.44 h=26`. Three heights' worth
    /// of disagreement and three different tops, with the combo the lowest of
    /// the three, exactly as reported.
    ///
    /// Asserted on PAINTED FRAMES, not on `Response::rect` and not on the
    /// source: a widget's requested size is the thing under test, so reading
    /// it back would only restate the code. See [`Painted::frame_around`].
    ///
    /// **Run in BOTH of the row's states.** The row is drawn from a branch on
    /// `draft.generator.passphrase`, and until this test was parametrised
    /// every caller of [`generator_row_frames`] handed it a password draft --
    /// so the passphrase arm, a separate `ui.add` on a separate line of
    /// source, was measured by nothing at all. A stray `interact_size` set
    /// inside that arm alone survived the entire suite.
    #[test]
    fn the_generator_row_controls_share_one_baseline() {
        for passphrase in [false, true] {
            let ctx = styled_context(ROOMY_PANE);
            let mut draft = tallest_draft();
            draft.generator.passphrase = passphrase;
            let _ = frame(&ctx, ROOMY_PANE, &mut draft, true, &[]);
            let painted = frame(&ctx, ROOMY_PANE, &mut draft, true, &[]);

            // The state really is the one asked for: the two arms differ only
            // by suffix and by the combo's caption, so a row drawn from the
            // wrong arm would otherwise be measured happily and reported as
            // the other one.
            assert!(
                painted.strings().iter().any(|s| *s == spinner_suffix(passphrase)),
                "asked for passphrase={passphrase} and the row is not showing \
                 {:?}; painted: {:?}",
                spinner_suffix(passphrase),
                painted.strings()
            );

            let [generate, combo, spinner] = generator_row_frames(&painted, passphrase);
            let names = ["Generate", "the kind combo", "the size spinner"];
            for (name, rect) in names.iter().zip([generate, combo, spinner]) {
                assert!(
                    rect.width() > 1.0 && rect.height() > 1.0,
                    "{name} painted a {rect:?} in the passphrase={passphrase} state -- a \
                     control that size is not a control"
                );
            }
            // One row, so one top and one height; the bottoms then follow.
            // Half a point of slack for the sub-pixel positions egui lays
            // rows out at, and no more: the defect was 7pt of it.
            for (name, rect) in names[1..].iter().zip([combo, spinner]) {
                assert!(
                    (rect.top() - generate.top()).abs() <= 0.5,
                    "{name} is painted at top {} while the Generate button beside it starts \
                     at {} -- the row does not sit on one line in the \
                     passphrase={passphrase} state",
                    rect.top(),
                    generate.top()
                );
                assert!(
                    (rect.height() - generate.height()).abs() <= 0.5,
                    "{name} is {}pt tall against the Generate button's {}pt in the \
                     passphrase={passphrase} state -- the row's controls are different sizes",
                    rect.height(),
                    generate.height()
                );
            }
            // And the height is the button height by construction, not
            // whatever the three happened to agree on: a row where all three
            // collapsed to egui's 26pt default would satisfy everything above.
            assert!(
                (generate.height() - theme::BUTTON_HEIGHT).abs() <= 0.5,
                "the generator row is {}pt tall in the passphrase={passphrase} state, not \
                 theme::BUTTON_HEIGHT ({})",
                generate.height(),
                theme::BUTTON_HEIGHT
            );
        }
    }

    /// The positive control for the test above: the row's controls really can
    /// be measured as DISAGREEING, so a green run there is a fact about the
    /// row and not about the measurement.
    ///
    /// The folder combo further down the form is a control of the same kind,
    /// drawn on a line of its own with no `interact_size` set -- so it is
    /// egui's untreated default height, the height the generator combo had
    /// before this fix. If `frame_around` were returning some shared
    /// container for everything, this would come back equal to the generator
    /// row's and the assertion above would be vacuous.
    #[test]
    fn the_alignment_assertion_can_tell_two_different_heights_apart() {
        let ctx = styled_context(ROOMY_PANE);
        let mut draft = tallest_draft();
        let _ = frame(&ctx, ROOMY_PANE, &mut draft, true, &[]);
        let painted = frame(&ctx, ROOMY_PANE, &mut draft, true, &[]);

        let [generate, _, _] = generator_row_frames(&painted, false);
        let folder = painted.frame_around(painted.rect_of("No folder"));
        assert!(
            (folder.height() - generate.height()).abs() > 0.5,
            "the untreated folder combo measures {}pt and the treated generator row {}pt -- \
             if those are the same number, `frame_around` is reporting one shared box for \
             both and the baseline assertion proves nothing",
            folder.height(),
            generate.height()
        );
    }

    /// **All three generator controls are reachable at the app's minimum
    /// width** -- the row still wraps, and wrapping is still what keeps it
    /// inside the pane.
    ///
    /// The row's content has a floor of 279.4pt against the 264pt card at
    /// `MIN_PANE_WIDTH`, so an unwrapped `ui.horizontal` pushes the card out
    /// past the pane and inflates every `available_width()` measured after it
    /// -- `aae9429`'s defect. This asserts the outcome rather than the call:
    /// each control's painted frame is inside the pane on both axes, and the
    /// spinner's glyphs are still the glyphs (a control elided to nothing
    /// paints an honest little box and reports the label it was handed).
    #[test]
    fn the_generator_rows_controls_are_all_reachable_at_the_minimum_width() {
        for passphrase in [false, true] {
            let pane = egui::vec2(MIN_PANE_WIDTH, 1400.0);
            let ctx = styled_context(pane);
            let mut draft = tallest_draft();
            draft.generator.passphrase = passphrase;
            let _ = frame(&ctx, pane, &mut draft, true, &[]);
            let painted = frame(&ctx, pane, &mut draft, true, &[]);

            let bounds = Rect::from_min_size(Pos2::ZERO, pane);
            let names = ["Generate", "the kind combo", "the size spinner"];
            for (name, rect) in names.iter().zip(generator_row_frames(&painted, passphrase)) {
                assert!(
                    rect.width() > 1.0 && rect.height() > 1.0,
                    "{name} painted a {rect:?} at the minimum width in the \
                     passphrase={passphrase} state -- a control drawn at no size passes \
                     every in-pane assertion and cannot be clicked"
                );
                assert!(
                    bounds.contains_rect(rect),
                    "{name} is painted at {rect:?}, outside the {}x{} pane -- the user cannot \
                     reach it. Painted: {:?}",
                    pane.x,
                    pane.y,
                    painted.strings()
                );
            }
            assert_inside("the generator's Generate button", "Generate", pane, &painted);
            let suffix = spinner_suffix(passphrase);
            assert_eq!(
                painted.rendered_glyphs(suffix),
                suffix,
                "the size spinner's suffix was squeezed away at the minimum width"
            );
        }
    }

    /// **Save and Cancel are one strip of one height.**
    ///
    /// The second report on this screen: "Also save button is off". Measured
    /// on the pre-fix layout -- Save `top=1368 h=26 bot=1394`, Cancel
    /// `top=1368 h=32 bot=1400`. Save was a bare `egui::Button` and Cancel
    /// goes through `theme::secondary_button`, so Save's bottom edge stopped
    /// 6pt above its neighbour's. Both label widths are checked, because the
    /// caption changes with the draft's validity.
    #[test]
    fn the_button_strips_controls_share_one_baseline() {
        for (what, name) in [(true, "Save"), (false, SAVE)] {
            let ctx = styled_context(ROOMY_PANE);
            let mut draft = tallest_draft();
            if what {
                draft.name = "Ledgerline".to_string();
                assert!(draft.is_valid(), "the valid case wants Save's short caption");
            }
            let _ = frame(&ctx, ROOMY_PANE, &mut draft, true, &[]);
            let painted = frame(&ctx, ROOMY_PANE, &mut draft, true, &[]);

            let save = painted.frame_around(painted.rect_of(name));
            let cancel = painted.frame_around(painted.rect_of("Cancel"));
            assert!(
                (save.top() - cancel.top()).abs() <= 0.5
                    && (save.height() - cancel.height()).abs() <= 0.5,
                "{name} is painted {save:?} beside Cancel's {cancel:?} -- the strip's two \
                 buttons are different sizes"
            );
            assert!(
                (save.height() - theme::BUTTON_HEIGHT).abs() <= 0.5,
                "the strip is {}pt tall, not theme::BUTTON_HEIGHT ({})",
                save.height(),
                theme::BUTTON_HEIGHT
            );
        }
    }

    /// **A Save that cannot be pressed does not look like one that can.**
    ///
    /// The trap in giving Save the height its neighbour has: a button styled
    /// into agreement with Cancel can lose the one signal that says the form
    /// is not saveable yet. `add_enabled` greys the fill, and this asserts
    /// that on the painted rect's colour -- the frame at the identical
    /// position and size, so nothing but the fill can be carrying it.
    #[test]
    fn the_disabled_save_button_does_not_look_enabled() {
        let fill_of = |draft: &mut EditDraft, label: &str| -> (Rect, egui::Color32) {
            let ctx = styled_context(ROOMY_PANE);
            let _ = frame(&ctx, ROOMY_PANE, draft, true, &[]);
            let painted = frame(&ctx, ROOMY_PANE, draft, true, &[]);
            let text = painted.rect_of(label);
            let frame_rect = painted.frame_around(text);
            let fill = painted
                .rects
                .iter()
                .find(|(r, _)| *r == frame_rect)
                .map(|(_, c)| *c)
                .expect("the frame just found by `frame_around` is not in `rects`");
            (frame_rect, fill)
        };

        let mut enabled = tallest_draft();
        enabled.name = "Ledgerline".to_string();
        let (_, on) = fill_of(&mut enabled, "Save");

        let mut disabled = tallest_draft();
        assert!(!disabled.is_valid(), "the disabled case wants an invalid draft");
        let (_, off) = fill_of(&mut disabled, SAVE);

        assert_ne!(
            on, off,
            "the Save button paints the same fill whether it can be pressed or not -- the \
             form gives the user no sign that it will not save"
        );
    }

    /// **A Save that cannot be pressed does not save**, asserted on the
    /// action the form returns and not on how the button looks.
    ///
    /// The gap this closes, stated exactly. Until this test existed, the only
    /// thing between the user and "Save writes an item with no name" was
    /// [`the_disabled_save_button_does_not_look_enabled`] -- a comparison of
    /// two fill colours. Two mutations prove it: `add_enabled(valid, save)`
    /// reduced to `add(save)` with the validity still checked on the click
    /// (behaviour preserved, appearance broken) failed that one test, and the
    /// SAME reduction with **no guard at all** -- an invalid draft genuinely
    /// saved -- failed that same one test and nothing else. A colour was
    /// carrying a correctness property.
    ///
    /// Both halves are here because either alone is worthless. The invalid
    /// half can pass on a click that lands nowhere, on a form that reports no
    /// action at all, or on a Save button that has been deleted; the valid
    /// half is what says the click really reaches the control and really
    /// produces `EditAction::Save` when it should. The visual test stays --
    /// it covers a different property, which is that the user can SEE the
    /// refusal before spending a click on it.
    #[test]
    fn clicking_save_on_an_invalid_draft_saves_nothing() {
        let press = |draft: &mut EditDraft, label: &str| -> (EditAction, EditAction) {
            let ctx = styled_context(ROOMY_PANE);
            let no_input: &[egui::Event] = &[];
            let _ = frame(&ctx, ROOMY_PANE, draft, true, no_input);
            let (idle, painted) = acting_frame_for(
                &ctx,
                ROOMY_PANE,
                draft,
                true,
                no_input,
                None,
                &detail::TotpState::NoSecret,
            );
            let at = painted.rect_of(label).center();
            let (clicked, _) = acting_frame_for(
                &ctx,
                ROOMY_PANE,
                draft,
                true,
                &click(at),
                None,
                &detail::TotpState::NoSecret,
            );
            (idle, clicked)
        };

        // The control first, so a green run below is a fact about the gate
        // rather than about a harness that cannot click anything.
        let mut valid = tallest_draft();
        valid.name = "Ledgerline".to_string();
        assert!(valid.is_valid(), "the control case wants a saveable draft");
        let (idle, saved) = press(&mut valid, "Save");
        assert_eq!(
            idle,
            EditAction::None,
            "control: the form reported an action on a frame with no input at all, so the \
             click below proves nothing"
        );
        assert_eq!(
            saved,
            EditAction::Save,
            "control: clicking Save on a VALID draft did not save, so this harness cannot \
             tell a working gate from a Save button it never hit"
        );

        let mut invalid = tallest_draft();
        assert!(!invalid.is_valid(), "the case under test wants an unsaveable draft");
        let (_, refused) = press(&mut invalid, SAVE);
        assert_eq!(
            refused,
            EditAction::None,
            "clicking Save on a draft with no name asked the caller to SAVE it. The form's \
             only remaining defence would be the fill colour the disabled button wears"
        );
    }

    /// The bug, stated as geometry: on a pane the app can really be resized
    /// to, holding the form the user really had, Save and Cancel are on
    /// screen.
    ///
    /// Positive control: the same assertion FAILS on the pre-fix layout (one
    /// plain `Ui`, no scroll area) -- verified by running it against that
    /// layout before the fix, where Save landed several hundred points below
    /// the pane. It cannot pass by accident, because a form drawn top-down
    /// with no scrolling puts its last widget at the form's full height.
    #[test]
    fn save_and_cancel_are_on_screen_at_the_minimum_window_size() {
        for height in [MIN_PANE_HEIGHT, TINY_PANE_HEIGHT] {
            let pane = egui::vec2(MIN_PANE_WIDTH, height);
            let ctx = styled_context(pane);
            let mut draft = tallest_draft();
            // Two frames: a `ScrollArea` needs one to learn its content size,
            // and the app draws this form continuously anyway.
            let _ = frame(&ctx, pane, &mut draft, true, &[]);
            let painted = frame(&ctx, pane, &mut draft, true, &[]);

            assert_inside("Save", SAVE, pane, &painted);
            assert_inside("Cancel", "Cancel", pane, &painted);
            // The error label belongs to the buttons: it is the reason Save is
            // disabled, and it is useless where it cannot be read.
            assert_inside("the name error", "Name is required.", pane, &painted);

            // On screen is not enough: the strip has to be at the BOTTOM of
            // the pane, below the form, which is where a form's actions
            // belong and where the user looks for them. Without this a strip
            // pinned to the TOP -- directly under the title, above every
            // field it acts on -- passes everything above. The slack is the
            // button's own padding under its glyphs plus the strip's bottom
            // edge, not room for a whole widget.
            let save = painted.rect_of(SAVE);
            assert!(
                pane.y - save.bottom() <= 30.0,
                "Save's glyphs end at y = {} on a pane {} tall -- the action strip is not \
                 pinned to the bottom of the pane",
                save.bottom(),
                pane.y
            );
        }
    }

    /// The other half of the fix: the fields the buttons were pinned away from
    /// are reachable. Scrolling brings the LAST thing in the form ("Folder")
    /// on screen, and it does not drag the buttons off.
    #[test]
    fn the_form_scrolls_to_its_last_field_while_the_buttons_stay_put() {
        let pane = egui::vec2(MIN_PANE_WIDTH, MIN_PANE_HEIGHT);
        let ctx = styled_context(pane);
        let mut draft = tallest_draft();

        let _ = frame(&ctx, pane, &mut draft, true, &[]);
        let before = frame(&ctx, pane, &mut draft, true, &[]);
        let bounds = Rect::from_min_size(Pos2::ZERO, pane);
        // Control: without this the test could pass on a pane that never
        // needed scrolling at all, which would make the scroll below a no-op
        // and this test blind.
        assert!(
            // "Not visible" is checked with `rects_of`, not `rect_of`: an
            // unscrolled field is not merely painted out of bounds -- egui
            // culls it and paints NOTHING, which is exactly why the user saw
            // no Save button at all rather than a Save button off the edge.
            !before.rects_of("Folder").iter().any(|r| bounds.contains_rect(*r)),
            "the tall form already fits in a {}x{} pane, so this test is not \
             exercising scrolling at all",
            pane.x,
            pane.y
        );
        let save_before = before.rect_of(SAVE);
        let cancel_before = before.rect_of("Cancel");

        // A wheel over the middle of the form. `MouseWheel` in points is what
        // egui's scroll areas consume; the pointer has to be over the area
        // first or the event goes nowhere.
        let middle = Pos2::new(pane.x / 2.0, pane.y / 2.0);
        let scroll = vec![
            egui::Event::PointerMoved(middle),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -4000.0),
                modifiers: egui::Modifiers::NONE,
                phase: egui::TouchPhase::Move,
            },
        ];
        let _ = frame(&ctx, pane, &mut draft, true, &scroll);
        // egui smooths a wheel step out over several frames, so the scroll is
        // not finished when the event frame returns; these are the frames the
        // app would draw while the user's flick settles.
        for _ in 0..12 {
            let _ = frame(&ctx, pane, &mut draft, true, &[]);
        }
        let after = frame(&ctx, pane, &mut draft, true, &[]);

        assert_inside("the last field's label (Folder)", "Folder", pane, &after);
        assert_eq!(
            after.rect_of(SAVE),
            save_before,
            "scrolling the form moved Save -- the buttons are not pinned"
        );
        assert_eq!(
            after.rect_of("Cancel"),
            cancel_before,
            "scrolling the form moved Cancel -- the buttons are not pinned"
        );
    }

    /// The title is the other thing a naive "wrap the whole function in a
    /// ScrollArea" fix loses -- and this crate has already shipped a layout
    /// test that certified a pane whose title had been annihilated. It stays
    /// on screen, above the form, at the minimum size.
    #[test]
    fn the_form_title_survives_the_short_pane() {
        let pane = egui::vec2(MIN_PANE_WIDTH, MIN_PANE_HEIGHT);
        let ctx = styled_context(pane);
        let mut draft = tallest_draft();
        let _ = frame(&ctx, pane, &mut draft, true, &[]);
        let painted = frame(&ctx, pane, &mut draft, true, &[]);

        let heading = form_title(draft.kind, true);
        let title = painted.rect_of(&heading);
        assert_inside("the form title", &heading, pane, &painted);
        assert!(
            title.bottom() < painted.rect_of(SAVE).top(),
            "the title is not above the buttons: title {title:?}, Save {:?}",
            painted.rect_of(SAVE)
        );
    }

    /// A pane tall enough that the form has nothing to scroll.
    const ROOMY_PANE_HEIGHT: f32 = 2000.0;

    /// A pane WIDE enough that the form card is not already overflowing it
    /// before this fix has any say -- the width at which "the bar is clear of
    /// the card" is a question about the lane rather than about that
    /// overflow.
    ///
    /// **The overflow is pre-existing and is not this fix's to answer.**
    /// egui's `TextEdit` has a default `desired_width` of 280pt which it
    /// treats as a MINIMUM, so the card is never narrower than 280 plus its
    /// own 14pt margins and stroke -- about 309pt, measured identically on
    /// `68f86cb^` and on the emptiest possible draft. At the 298pt minimum
    /// pane the card therefore spills sideways whatever the scroll bar does,
    /// which is why the lane calculation here does NOT assume the card fits
    /// and why the two edge tests below are measured at a width where it
    /// does. 420pt is comfortably past 309 and well inside the window sizes
    /// the app actually opens at.
    const CARD_FITS_PANE_WIDTH: f32 = 420.0;

    /// The tallest form with the app binding dropped: the block draws a full
    /// program path, and this is the fixture for tests that must not have
    /// their measurements moved by the identity lookup's wrapped text.
    fn tall_draft_that_fits_sideways() -> EditDraft {
        let mut draft = tallest_draft();
        draft.app = None;
        draft
    }

    /// The left edge of the lane [`FORM_SCROLL_GUTTER`] reserves.
    fn lane_left(pane: Vec2) -> f32 {
        pane.x - FORM_SCROLL_GUTTER
    }

    /// Every coloured rectangle that STARTS inside the reserved lane and is
    /// narrow enough to be a scroll bar rather than a card spilling into it.
    ///
    /// The search and the assertions it feeds are for two different
    /// questions, and the filter is written so they cannot answer each
    /// other's.
    ///
    /// * **Is this a bar?** -- the width ceiling is the LANE's width, not the
    ///   bar's, on purpose: the defect this test exists for painted a 1.2pt
    ///   sliver, and a filter set at the bar's own width would have quietly
    ///   accepted it. What the sliver fails is the assertion, not the search.
    /// * **Is the bar where it belongs?** -- not asked here. The filter used
    ///   to require `rect.right() <= pane.x + 0.5`, which made the identical
    ///   clause in the assertion below unfailable, and made a bar that
    ///   OVERHANGS the pane disappear from the search rather than fail it.
    ///   Verified: with the bar pushed to 296..302 the two callers reported
    ///   "no scroll bar was painted, so this test is vacuous" and "a form that
    ///   overflows a 300pt pane paints no scroll bar at all" -- both the
    ///   opposite of what had happened. An overhanging bar is now FOUND here
    ///   and REJECTED there, by a message that says it overhangs.
    fn bar_rects(painted: &Painted, pane: Vec2) -> Vec<Rect> {
        let lane = lane_left(pane);
        painted
            .rects
            .iter()
            .filter(|(rect, fill)| {
                fill.a() > 0
                    && rect.left() >= lane - 0.5
                    && rect.width() > 0.0
                    && rect.width() <= FORM_SCROLL_GUTTER + 0.5
                    && rect.height() > FORM_SCROLL_GUTTER
            })
            .map(|(rect, _)| *rect)
            .collect()
    }

    /// The white form card: every [`theme::CARD`] fill that contains the
    /// "Name" label, unioned. A `Frame` with a corner radius paints itself as
    /// more than one rectangle, so the union rather than one of them.
    fn card_rect(painted: &Painted) -> Rect {
        let name = painted.rect_of("Name");
        painted
            .rects
            .iter()
            .filter(|(rect, fill)| *fill == theme::CARD && rect.contains_rect(name))
            .map(|(rect, _)| *rect)
            .reduce(Rect::union)
            .unwrap_or_else(|| panic!("the form card has no white surface; painted: {:?}", painted.strings()))
    }

    /// Two frames with the pointer parked in the middle of the form, which is
    /// what the app is doing whenever the user is looking at this pane.
    ///
    /// **Both halves matter.** Two frames, because `form_overflowed` decides
    /// the second frame's bar from a reading taken on the first. Pointer
    /// INSIDE, because egui's floating bar is fully transparent while the
    /// pointer is away from the area -- a test that read a dormant bar would
    /// certify the placement of something that was not drawn, which is one of
    /// the vacuous tests this crate has already shipped.
    fn settled(ctx: &egui::Context, pane: Vec2, draft: &mut EditDraft) -> Painted {
        let over = [egui::Event::PointerMoved(Pos2::new(pane.x / 2.0, pane.y / 2.0))];
        let _ = frame(ctx, pane, draft, true, &over);
        frame(ctx, pane, draft, true, &over)
    }

    /// **Finding 1, stated as geometry.** The bar the form scrolls with is a
    /// [`theme::SCROLLBAR_WIDTH`] bar filling the OUTER end of a reserved lane
    /// -- not egui's floating default, which is a 1.2pt sliver pinned to the
    /// pane's extreme right and only widened once the pointer is already on it.
    ///
    /// **This assertion was CENTRING and is now flush.** Finding 1 was about
    /// the bar's WIDTH and about it being in a lane of its own at all; the
    /// centring was the placement `theme::scrollbar_in_gutter` happened to ship
    /// with, and it left 2pt of the 10pt lane behind the bar, against the pane's
    /// own edge, where nothing is being compared to it. The item list measured
    /// that as a real report ("the right padding feels smaller") and went flush;
    /// the rule now lives in the helper, so this form follows. The assertion is
    /// not weaker for it -- it still pins the bar to an ABSOLUTE x, one that is
    /// still inside the lane and still clear of the card, and the same 0.5pt
    /// tolerance still catches the same 1.2pt sliver.
    ///
    /// **Both of the two assertions below are load-bearing, and this is
    /// written down so that neither is later removed as redundant.** An
    /// earlier version of this note claimed the WIDTH check was the one that
    /// rejects the sliver "now that the position no longer does". That is
    /// false, and it was checked both ways. egui pins its floating sliver to
    /// the pane's right edge, so on a 298pt pane the sliver is 296.8..298:
    /// the width check fires first (`1.2000122pt wide, not 6pt`), but with
    /// the width check neutered the FLUSH check fires on its own
    /// (`the scroll bar spans x = 296.8..298 but the outermost 6pt ... is
    /// 292..298`) -- because flushness is measured from the bar's LEFT edge,
    /// and a narrow bar that shares the right edge does not share the left
    /// one. Neither check is a spare.
    ///
    /// Commit `68f86cb` made this form scrollable and left nothing on screen
    /// to say so; the user who reported "cannot scroll" got no affordance.
    /// Both sibling panes (`item_list.rs`, `detail.rs`) already draw the bar
    /// this way.
    #[test]
    fn the_form_scroll_bar_is_a_full_bar_at_the_outer_edge_of_its_own_lane() {
        let pane = egui::vec2(MIN_PANE_WIDTH, MIN_PANE_HEIGHT);
        let ctx = styled_context(pane);
        let mut draft = tallest_draft();
        let painted = settled(&ctx, pane, &mut draft);

        let bars = bar_rects(&painted, pane);
        assert!(
            !bars.is_empty(),
            "a form that overflows a {}x{} pane paints nothing at all in its scroll lane, \
             so nothing tells the user there is more below",
            pane.x,
            pane.y
        );
        // The lane's outermost `SCROLLBAR_WIDTH`, in absolute pane coordinates.
        let bar_left = pane.x - theme::SCROLLBAR_WIDTH;
        for bar in &bars {
            assert!(
                (bar.width() - theme::SCROLLBAR_WIDTH).abs() <= 0.5,
                "the scroll bar is {}pt wide, not {}pt -- this is egui's floating default \
                 painted over the card, not a bar in the reserved lane. Bars: {bars:?}",
                bar.width(),
                theme::SCROLLBAR_WIDTH
            );
            // Two separate assertions, because they fail for two different
            // reasons and a reader who sees one must not be told the other's
            // story. `bar_rects` no longer filters overhanging bars out, so
            // this first one can actually fail.
            assert!(
                bar.right() <= pane.x + 0.5,
                "the scroll bar spans x = {}..{} and so hangs {}pt off the right edge of \
                 the {}pt pane it scrolls -- that ink is painted outside the panel \
                 altogether, where the pane clips it. Bars: {bars:?}",
                bar.left(),
                bar.right(),
                bar.right() - pane.x,
                pane.x
            );
            assert!(
                (bar.left() - bar_left).abs() <= 0.5,
                "the scroll bar spans x = {}..{} but the outermost \
                 {}pt of the {FORM_SCROLL_GUTTER}pt lane is {bar_left}..{} -- the bar \
                 is not flush to the pane's outer edge, so it is spending the \
                 reader's padding on a gap behind itself. Bars: {bars:?}",
                bar.left(),
                bar.right(),
                theme::SCROLLBAR_WIDTH,
                pane.x
            );
        }
    }

    /// The other half of Finding 1: the bar is BESIDE the card, not on it.
    ///
    /// Measured at [`CARD_FITS_PANE_WIDTH`], not at the 298pt minimum: at the
    /// minimum the card overflows the pane on its own account whatever the
    /// bar does, so the comparison there would be a statement about that
    /// pre-existing defect instead of about the lane.
    #[test]
    fn the_form_scroll_bar_does_not_paint_over_the_card() {
        let pane = egui::vec2(CARD_FITS_PANE_WIDTH, TINY_PANE_HEIGHT);
        let ctx = styled_context(pane);
        let mut draft = tall_draft_that_fits_sideways();
        let painted = settled(&ctx, pane, &mut draft);

        let card = card_rect(&painted);
        // Control: if this draft's card overflowed the pane too, the
        // comparison below would be about the card and not about the lane.
        assert!(
            card.right() <= pane.x + 0.5,
            "the fixture's card already runs off the {}pt pane at {} -- this test is \
             measuring the horizontal overflow, not the scroll lane",
            pane.x,
            card.right()
        );

        let bars = bar_rects(&painted, pane);
        assert!(!bars.is_empty(), "no scroll bar was painted, so this test is vacuous");
        for bar in &bars {
            assert!(
                bar.left() >= card.right() - 0.5,
                "the scroll bar starts at x = {} but the card runs to {} -- the bar is \
                 painted ON TOP of the form's content",
                bar.left(),
                card.right()
            );
        }
    }

    /// No bar when there is nothing to scroll: an always-visible bar down a
    /// form that cannot move is an affordance that lies, and it is 6pt of the
    /// 10pt lane's clear space besides.
    ///
    /// The first-ever frame is checked separately and the other way round:
    /// `form_overflowed` has no reading to go on there and answers TRUE, so
    /// the bar is SHOWN. Ties go to showing it -- a bar on a form that turns
    /// out to fit is gone next frame, a missing bar on a form that really
    /// scrolls is the report.
    #[test]
    fn the_form_bar_is_painted_only_when_there_is_something_to_scroll() {
        let ink_in_the_lane = |height: f32| {
            let pane = egui::vec2(MIN_PANE_WIDTH, height);
            let ctx = styled_context(pane);
            let mut draft = tall_draft_that_fits_sideways();
            let painted = settled(&ctx, pane, &mut draft);
            bar_rects(&painted, pane).len()
        };

        assert!(
            ink_in_the_lane(TINY_PANE_HEIGHT) > 0,
            "a form that overflows a {TINY_PANE_HEIGHT}pt pane paints no scroll bar at all"
        );
        assert_eq!(
            ink_in_the_lane(ROOMY_PANE_HEIGHT),
            0,
            "a form with nothing to scroll still paints a bar down its right margin"
        );

        let pane = egui::vec2(MIN_PANE_WIDTH, TINY_PANE_HEIGHT);
        let ctx = styled_context(pane);
        let mut draft = tall_draft_that_fits_sideways();
        let first = frame(
            &ctx,
            pane,
            &mut draft,
            true,
            &[egui::Event::PointerMoved(Pos2::new(pane.x / 2.0, pane.y / 2.0))],
        );
        assert!(
            !bar_rects(&first, pane).is_empty(),
            "the very first frame this form is ever drawn paints no bar at all"
        );
    }

    /// The lane and the hiding are configured by MUTATING a `Ui`'s style, and
    /// the `Ui` this function is handed belongs to `vault_window/mod.rs`. So
    /// the settings must not survive the call: the next thing that pane draws
    /// -- today the read pane, tomorrow anything -- would otherwise inherit a
    /// 10pt reserved gutter and, on a form that fits, six zeroed opacities
    /// that make its OWN scroll bar invisible.
    ///
    /// The `scope` in `draw_detail_edit` is what buys this (a child `Ui`'s
    /// style is its own clone), and this is the assertion that says so.
    /// Checked in the FITS case, because that is the one that also calls
    /// `theme::hide_scrollbar` -- the settings that would be worst to leak.
    #[test]
    fn the_form_leaves_the_callers_scroll_style_alone() {
        let pane = egui::vec2(CARD_FITS_PANE_WIDTH, ROOMY_PANE_HEIGHT);
        let ctx = styled_context(pane);
        let mut draft = tall_draft_that_fits_sideways();
        let mut apps = AppIdentityCache::default();
        // The four numbers `theme::scrollbar_in_gutter` sets and one of the
        // six `theme::hide_scrollbar` zeroes, before and after.
        let sample = |ui: &egui::Ui| {
            let s = &ui.spacing().scroll;
            (
                s.floating_allocated_width,
                s.bar_width,
                s.floating_width,
                s.bar_outer_margin,
                s.active_handle_opacity,
            )
        };
        let mut seen = None;
        for _ in 0..2 {
            let _ = ctx.run_ui(raw_input(pane, &[]), |ui| {
                let before = sample(ui);
                let _ = draw_detail_edit(ui, &mut draft, &[], true, &mut apps, None, &detail::TotpState::NoSecret);
                seen = Some((before, sample(ui)));
            });
        }
        let (before, after) = seen.expect("the frame closure never ran");
        // Control: a default style whose values were already the ones the
        // helpers set would make the equality below say nothing.
        assert_ne!(
            before.0, FORM_SCROLL_GUTTER,
            "the caller's style already reserves a {FORM_SCROLL_GUTTER}pt lane, so this \
             test cannot see a leak"
        );
        assert_eq!(
            before, after,
            "drawing the edit form changed the CALLER's scroll style from {before:?} to \
             {after:?} -- the bar's settings have escaped the form"
        );
    }

    /// The card keeps ONE width whether or not the bar is showing.
    ///
    /// This is the trap `092da70` measured on the item list: under egui's
    /// default `VisibleWhenNeeded` the lane is reserved only while the bar is
    /// shown, so the content's right edge jumps by the lane's width as the
    /// content crosses the overflow threshold. `AlwaysVisible` plus
    /// `theme::scrollbar_in_gutter` makes the reservation unconditional and
    /// `theme::hide_scrollbar` merely stops painting, so nothing moves.
    ///
    /// The same DRAFT on two pane heights is the comparison -- one that
    /// overflows and one that does not. Two different drafts would have
    /// measured the drafts.
    #[test]
    fn the_form_bar_does_not_change_the_card_width() {
        let edges = |height: f32| {
            let pane = egui::vec2(CARD_FITS_PANE_WIDTH, height);
            let ctx = styled_context(pane);
            let mut draft = tall_draft_that_fits_sideways();
            let card = card_rect(&settled(&ctx, pane, &mut draft));
            (card.left(), card.right())
        };

        let scrolls = edges(TINY_PANE_HEIGHT);
        let fits = edges(ROOMY_PANE_HEIGHT);

        // **The absolute half**: the lane is really reserved, on both. A
        // fix that centred the bar without reserving anything would keep the
        // two equal and still paint over the card.
        let pane = egui::vec2(CARD_FITS_PANE_WIDTH, TINY_PANE_HEIGHT);
        for (what, (_, right)) in [("that scrolls", scrolls), ("that fits", fits)] {
            assert!(
                right <= lane_left(pane) + 0.5,
                "on a pane {what} the card runs to x = {right}, into the \
                 {FORM_SCROLL_GUTTER}pt lane that ends at {}",
                lane_left(pane)
            );
        }
        assert_eq!(
            scrolls, fits,
            "the form card spans {scrolls:?} on a pane that scrolls and {fits:?} on one that \
             does not -- the bar's lane is being reserved conditionally"
        );
    }

    // -- the whole form, at the size the app can really be made ------------

    /// The item behind the tallest form: a login with a user name, a password,
    /// a TOTP secret and a custom `PIN`, so the keystroke builder's palette is
    /// as WIDE as this app can make it. Every value differs from every other:
    /// a fixture whose password equalled its user name would let a row that
    /// drew the wrong one pass.
    fn palette_item() -> VaultItem {
        VaultItem {
            id: "layout-1".to_string(),
            name: "Contoso 365".to_string(),
            fields: vec![crate::vault_bridge::VaultField {
                name: Some("PIN".to_string()),
                value: Some(Zeroizing::new("8421".to_string())),
                other: serde_json::Map::new(),
            }],
            login: Some(LoginData {
                username: Some("ada@contoso.test".to_string()),
                password: Some("correct-horse-battery".to_string().into()),
                totp: Some("JBSWY3DPEHPK3PXP".to_string().into()),
                uris: Vec::new(),
                other: serde_json::Map::new(),
            }),
            card: None,
            identity: None,
            ssh_key: None,
            notes: None,
            item_type: Some(1),
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        }
    }

    /// **The tallest and widest thing this form can be**, drawn on `pane`.
    ///
    /// [`tallest_draft`]'s conditions -- creating a Login, the generator row
    /// showing, an app binding present, the name error up -- plus the one this
    /// suite could not reach before: the keystroke builder OPEN, which adds
    /// the chip row, three palettes, two boxes and the eye. Opened by CLICKING
    /// the button the user clicks, not by setting the flag, so the form drawn
    /// here is one the app can really be in.
    ///
    /// The pane is deliberately tall. Height is
    /// `save_and_cancel_are_on_screen_at_the_minimum_window_size`'s subject
    /// and is answered by scrolling; WIDTH is this one's, and a form that
    /// overflows horizontally cannot be scrolled to on this pane at all. Every
    /// field must therefore be drawn rather than culled below the fold, or the
    /// widths of the ones underneath would go unmeasured.
    fn tallest_form_with_the_builder_open(
        ctx: &egui::Context,
        pane: Vec2,
        draft: &mut EditDraft,
        item: &VaultItem,
        totp: &detail::TotpState,
    ) -> Painted {
        let shut = frame_for(ctx, pane, draft, true, &[], Some(item), totp);
        let at = shut.rect_of(APP_SEQUENCE_OPEN).center();
        let click = vec![
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ];
        let _ = frame_for(ctx, pane, draft, true, &click, Some(item), totp);
        assert!(
            draft.app.as_ref().unwrap().sequence_open,
            "the builder did not open, so this is not the tallest form"
        );
        frame_for(ctx, pane, draft, true, &[], Some(item), totp)
    }

    /// A pane as tall as the whole form needs, so nothing is culled. Not a
    /// claim about the app -- the WIDTH is, and that is `MIN_PANE_WIDTH`.
    const UNCULLED_PANE_HEIGHT: f32 = 4000.0;

    /// The live one-time code the tall form is drawn against. Different from
    /// every other value in `palette_item`, deliberately.
    fn live_code() -> detail::TotpState {
        detail::TotpState::Code { code: "776699".to_string(), seconds_left: 21 }
    }

    /// **Nothing the tallest form paints is outside the pane it is painted
    /// on** -- not a glyph, and not a box either.
    ///
    /// This is the recorded defect, measured: the form's card came out
    /// **309.4pt wide inside a 298pt pane**. Not one widget was out of bounds,
    /// which is exactly why it survived every assertion this suite already
    /// had -- the card overflowed to the right and its contents, laid inside
    /// it, still fell under 298. What overflowed was the card itself, and with
    /// it every `available_width()` measured after it, which is the mechanism
    /// `aae9429` documented when an unwrapped path inflated the read pane's
    /// app card to 467.8 and pushed `Open` 87% off the edge.
    ///
    /// So the rects are asserted about and not only the text. The cause was
    /// one row: "Generate" plus a 110pt combo box plus a `DragValue` is
    /// 279.4pt of content in a 264pt card, and `ui.horizontal` does not wrap.
    ///
    /// **Positive control**: verified to FAIL with that row back at
    /// `ui.horizontal` -- two boxes at x = 0..309.4 -- and to pass with it
    /// wrapped. It cannot pass by accident.
    #[test]
    fn nothing_on_the_tallest_edit_form_is_painted_outside_the_minimum_pane() {
        let pane = egui::vec2(MIN_PANE_WIDTH, UNCULLED_PANE_HEIGHT);
        let ctx = styled_context(pane);
        let mut draft = tallest_draft();
        let item = palette_item();
        let painted =
            tallest_form_with_the_builder_open(&ctx, pane, &mut draft, &item, &live_code());

        // The premise: this really is the form with the builder open. Without
        // it, a regression that stopped drawing the builder would make every
        // assertion below trivially true.
        assert!(
            painted.strings().contains(&APP_SEQUENCE_REVEAL),
            "the builder's eye is not on screen, so this is not the tallest form: {:?}",
            painted.strings()
        );

        for (label, rect) in &painted.glyphs {
            assert!(
                within_pane(*rect, pane),
                "the run {label:?} draws ink at x = {}..{} on a {}pt-wide pane -- this pane \
                 does not scroll horizontally, so that ink is unreachable",
                rect.left(),
                rect.right(),
                pane.x
            );
        }
        for (rect, fill) in &painted.rects {
            assert!(
                within_pane(*rect, pane),
                "a {fill:?} box spans x = {}..{} on a {}pt-wide pane. If that is the form's \
                 card, then every available_width() measured inside it is the box's width \
                 and not the pane's",
                rect.left(),
                rect.right(),
                pane.x
            );
        }

        // ... and what those boxes really cover, which the loop above is
        // blind to: a stroke, a shadow's blur and a rotation all put ink
        // outside `RectShape::rect`, and this pane clips every point of it.
        assert!(
            !painted.rect_ink.is_empty(),
            "the tallest form paints no visible box at all, so the loop below asserts \
             about nothing"
        );
        for (ink, fill) in &painted.rect_ink {
            assert!(
                within_pane(*ink, pane),
                "a {fill:?} box lays ink over x = {}..{} on a {}pt-wide pane. Its recorded \
                 rect may well be inside; its stroke, its blur or its rotation is not, and \
                 this pane does not scroll horizontally",
                ink.left(),
                ink.right(),
                pane.x
            );
        }

        // ... and everything that is neither. A caret, an icon, a line or a
        // curve can leave the pane without moving one rect or one glyph, and
        // until [`Painted::marks`] existed this loop had nothing to read:
        // `walk` threw those shapes away. The tallest form really paints
        // three of them -- the combo boxes' carets.
        assert!(
            !painted.marks.is_empty(),
            "the tallest form paints no shape that is neither text nor a filled box, so \
             the loop below asserts about nothing -- either the combo boxes' carets have \
             stopped being drawn, or `ink_of` has stopped recognising them"
        );
        for (kind, rect) in &painted.marks {
            assert!(
                within_pane(*rect, pane),
                "{kind} draws ink at x = {}..{} on a {}pt-wide pane -- it is neither a \
                 glyph nor a filled box, so the two loops above are silent about it, and \
                 this pane does not scroll horizontally, so that ink is unreachable",
                rect.left(),
                rect.right(),
                pane.x
            );
        }
    }

    /// Every control an **empty** draft of `kind` must be offered.
    ///
    /// Built from the same sources the form draws from where there is one
    /// (`identity_rows`, the block's own consts), so a field that is modelled
    /// and drafted and then never offered cannot pass by both lists being
    /// edited to agree.
    fn expected_controls(kind: ItemKind, creating: bool) -> Vec<&'static str> {
        // What every kind gets in both states: its name, the custom-fields
        // section, the app section -- the two gaps this list exists for --
        // and its folder.
        let mut expected =
            vec!["Name", FIELDS_BLOCK_HEADING, APP_BLOCK_HEADING, APP_ADD_BUTTON, "Folder"];
        if creating {
            // `NewItem` has no `fields` payload, so the block says so instead
            // of offering boxes whose contents Save would discard. The notice
            // is asserted, not merely the absence of the buttons: "it says
            // why" is the behaviour, and a block that drew nothing at all
            // would pass an absence check.
            expected.push(FIELDS_CREATE_NOTICE);
        } else {
            expected.extend([FIELD_ADD_TEXT_BUTTON, FIELD_ADD_HIDDEN_BUTTON]);
        }
        // Keyed off the form's OWN decision function rather than off `kind`,
        // so an SSH key -- whose boxes exist on a create and are withheld on
        // an edit -- is expected to offer exactly what `form_body` says and
        // this list cannot drift from it.
        match form_body(kind, creating) {
            FormBody::Login => {
                expected.extend(["Username", "Password", "Generate", TOTP_LABEL]);
                // The seed box, like the custom-field boxes, cannot exist on
                // a create (`NewItem::login` has no `totp`), so the row says
                // why instead of taking input Save would drop.
                expected.push(if creating { TOTP_CREATE_NOTICE } else { TOTP_HINT });
            }
            FormBody::Card => expected.extend([
                "Cardholder name",
                "Brand",
                "Number",
                "Expiry month",
                "Expiry year",
                "Security code",
            ]),
            FormBody::Identity => {
                // The form's own row list, not a copy of it.
                expected.extend(identity_rows(&mut IdentityDraft::default()).into_iter().map(
                    |(label, _)| label,
                ));
            }
            FormBody::Note => expected.push("Note"),
            FormBody::SshKey => expected.extend(["Private key", "Public key", "Fingerprint"]),
            // Name and folder only, both already in the list above.
            FormBody::UneditableNotice => {}
        }
        expected
    }

    /// **The user's ask, as a test: on a form with nothing filled in, every
    /// control the kind supports is there.**
    ///
    /// The recorded gap was the app block, drawn only inside
    /// `if let Some(app) = draft.app.as_mut()` -- so an item bound to nothing
    /// had no way to bind anything, and the edit form could edit and remove a
    /// binding it could not create. This asserts the whole list rather than
    /// that one control, because the question the user asked was about the
    /// form and not about the app.
    ///
    /// Three things make it bite rather than pass green:
    ///
    /// * the fixture is asserted EMPTY first -- a draft that arrived carrying
    ///   an app, or a name, would prove the populated case again;
    /// * the loop's visit count is asserted, and so is each kind's control
    ///   count, because egui CULLS a shape entirely outside the screen rect --
    ///   a control pushed out of the form comes back as *nothing*, and a loop
    ///   over what was painted would simply not visit it;
    /// * every rect found is checked with [`within_pane`], because a control
    ///   that is painted and unreachable is the same defect as one that is
    ///   missing. The pane is `MIN_PANE_WIDTH` for that reason -- the width is
    ///   the axis this form cannot scroll.
    #[test]
    fn every_control_the_kind_supports_is_offered_even_on_an_empty_draft() {
        let pane = egui::vec2(MIN_PANE_WIDTH, UNCULLED_PANE_HEIGHT);
        let ctx = styled_context(pane);
        let mut kinds = 0;
        let mut states = 0;
        // **Both states.** `creating: true` is the mode in which an SSH key's
        // boxes are offered at all (see `form_body`); `creating: false` is
        // the mode the user's ask names -- "On Edit" -- and it is the only
        // one in which the custom-field controls exist, because
        // `vault_bridge::NewItem` has no `fields` payload to carry them.
        // Running one state only was how the second half of that ask stayed
        // unnoticed.
        for creating in [true, false] {
            states += 1;
            for kind in CREATABLE_KINDS {
                kinds += 1;
                let mut draft = EditDraft::empty_of(kind);

                // The fixture really is empty. Without this the test proves
                // nothing about the empty case at all.
                assert!(draft.app.is_none(), "{kind:?}'s fixture already carries a binding");
                assert!(draft.name.is_empty(), "{kind:?}'s fixture already carries a name");
                assert!(
                    draft.fields.is_empty(),
                    "{kind:?}'s fixture already carries custom fields, so the block below is \
                     being asked about a populated form"
                );
                assert_eq!(draft.kind(), kind);

                let _ = frame(&ctx, pane, &mut draft, creating, &[]);
                let painted = frame(&ctx, pane, &mut draft, creating, &[]);
                let strings = painted.strings();

                let expected = expected_controls(kind, creating);
                assert!(
                    expected.len() >= 6,
                    "{kind:?} expects only {} controls -- the expectation itself is empty",
                    expected.len()
                );
                let mut controls = 0;
                for label in &expected {
                    controls += 1;
                    assert!(
                        strings.contains(label),
                        "a {kind:?} (creating: {creating}) is not offered {label:?}. egui culls \
                         a shape outside the screen rect entirely, so a control pushed out of \
                         the form is painted as NOTHING and reads exactly like this. Painted: \
                         {strings:?}"
                    );
                    let rects = painted.rects_of(label);
                    assert!(!rects.is_empty(), "{label:?} was found as a string but has no rect");
                    for rect in rects {
                        assert!(
                            within_pane(rect, pane),
                            "{kind:?}'s {label:?} is painted at x = {}..{} on a {}pt-wide pane \
                             -- this pane does not scroll horizontally, so the control is on \
                             screen and out of reach",
                            rect.left(),
                            rect.right(),
                            pane.x
                        );
                    }
                }
                assert_eq!(
                    controls,
                    expected.len(),
                    "the control loop for {kind:?} did not visit every expected control"
                );
            }
        }
        assert_eq!(states, 2, "the state loop visited {states} states");
        assert_eq!(
            kinds,
            CREATABLE_KINDS.len() * 2,
            "the kind loop visited {kinds} kinds, so it did not assert about the form"
        );
        assert_eq!(
            kinds, 10,
            "CREATABLE_KINDS changed size -- this test's expectations have not"
        );
    }

    /// **The Add buttons are wired to the draft, not merely painted.**
    ///
    /// `9dcee36` recorded the exact shape of the defect this catches: a
    /// control drawn and connected to nothing passes every presence and
    /// in-pane assertion above. This clicks each button on a real frame and
    /// reads the draft.
    #[test]
    fn the_add_field_buttons_put_a_field_of_the_promised_type_on_the_draft() {
        let pane = egui::vec2(MIN_PANE_WIDTH, UNCULLED_PANE_HEIGHT);
        let mut cases = 0;
        for (button, expected) in
            [(FIELD_ADD_TEXT_BUTTON, FieldRole::Text), (FIELD_ADD_HIDDEN_BUTTON, FieldRole::Hidden)]
        {
            cases += 1;
            let ctx = styled_context(pane);
            let mut draft = EditDraft::empty_of(ItemKind::Login);
            assert!(draft.fields.is_empty(), "the fixture already carries a field");

            let _ = frame(&ctx, pane, &mut draft, false, &[]);
            let painted = frame(&ctx, pane, &mut draft, false, &[]);
            let at = painted.rect_of(button).center();
            let _ = frame(&ctx, pane, &mut draft, false, &click(at));

            assert_eq!(
                draft.fields.len(),
                1,
                "clicking {button:?} did not reach the draft -- the control is painted and \
                 connected to nothing"
            );
            assert_eq!(
                draft.fields[0].role(),
                expected,
                "{button:?} made a field of the wrong type"
            );
        }
        assert_eq!(cases, 2, "the button loop asserted about nothing");
    }

    /// **The remove control is wired too, and removes the row it is on.**
    ///
    /// Two rows, because a remove that always takes the last one -- or the
    /// first -- passes a one-row test.
    #[test]
    fn removing_a_field_row_takes_that_row_and_no_other() {
        let pane = egui::vec2(MIN_PANE_WIDTH, UNCULLED_PANE_HEIGHT);
        let ctx = styled_context(pane);
        let mut draft = EditDraft::empty_of(ItemKind::Login);
        for name in ["first", "second", "third"] {
            let mut field = FieldDraft::new_of(FieldRole::Text);
            field.name = name.to_string();
            draft.fields.push(field);
        }

        let _ = frame(&ctx, pane, &mut draft, false, &[]);
        let painted = frame(&ctx, pane, &mut draft, false, &[]);
        let removes = painted.rects_of(FIELD_REMOVE_BUTTON);
        assert_eq!(removes.len(), 3, "three rows must draw three removes, found {removes:?}");
        // The MIDDLE one, which is the only index a hardcoded first-or-last
        // remove cannot get right by accident.
        let at = removes[1].center();
        let _ = frame(&ctx, pane, &mut draft, false, &click(at));

        let names: Vec<&str> = draft.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["first", "third"], "the wrong row was removed");
    }

    /// **A removal does not hand the rows below it the removed row's widget
    /// state.**
    ///
    /// A row's `id_salt` used to be its index in `EditDraft::fields`, and the
    /// deferred remove was documented as protecting against exactly this. It
    /// did not: deferring guards the iterator for one frame and says nothing
    /// about the next, on which every row below the removed one answers to its
    /// predecessor's id and inherits its caret, its selection and its undo
    /// buffer. See [`FieldDraft::row_id`].
    ///
    /// Stated as focus, which is the cheapest visible consequence of a
    /// widget's identity: put the caret in the LAST row's name box, take the
    /// FIRST row away, and the caret is still in the same box. Under
    /// index-salted ids the id that box answered to belongs to nothing after
    /// the shift, egui drops the focus at the end of the frame, and the answer
    /// is `None`.
    ///
    /// **The premise is measured on the ids egui was handed, not on
    /// `row_id`.** A `custom_fields_block` that gave every row ONE shared id
    /// -- no identity at all, as against a shifted one -- leaves the focus
    /// comparison below trivially green, because a shared id has nothing to
    /// shift into. Comparing the drafts' `row_id`s could not have seen that:
    /// those are distinct whatever the block does with them, or ignores. So
    /// the three identities are read back out of egui, by focusing each row's
    /// name box in turn and keeping the id egui reports.
    ///
    /// **And the identity has to survive the user typing.** A row id taken
    /// from something the row *shows* -- its name, say -- passes everything
    /// above: the three fixture names differ, so the three ids differ, and
    /// none of them changes when a different row is removed. It is still
    /// wrong, and visibly so: the id changes under the caret on the first
    /// character the user types into the name box, and two rows named the
    /// same would share one state outright. That mutant survived the whole
    /// suite until the last block of this test was added.
    ///
    /// The row is removed off the draft directly rather than by clicking
    /// Remove, because a click is itself an interaction with the focus system
    /// and would leave the reading ambiguous. It is the same mutation of
    /// `fields` that `custom_fields_block` performs -- the click path is
    /// already pinned by `removing_a_field_row_takes_that_row_and_no_other`.
    #[test]
    fn removing_a_field_row_leaves_the_rows_below_it_holding_their_own_state() {
        let pane = egui::vec2(MIN_PANE_WIDTH, UNCULLED_PANE_HEIGHT);
        let ctx = styled_context(pane);
        let mut draft = EditDraft::empty_of(ItemKind::Login);
        for name in ["first", "second", "third"] {
            let mut field = FieldDraft::new_of(FieldRole::Text);
            field.name = name.to_string();
            draft.fields.push(field);
        }
        let _ = frame(&ctx, pane, &mut draft, false, &[]);
        let painted = frame(&ctx, pane, &mut draft, false, &[]);
        assert_eq!(
            painted.rects_of(FIELD_NAME_LABEL).len(),
            3,
            "three rows did not draw three name boxes: {:?}",
            painted.strings()
        );

        // **The premise, and it is read back off egui, not off the drafts.**
        // The three rows must have three different identities *as widgets*.
        // Asserting that the three [`FieldDraft::row_id`]s differ would be an
        // assertion about the struct instead: a `custom_fields_block` that
        // ignored `row_id` and handed every row ONE explicit id would leave
        // that green -- and would then leave the comparison at the bottom of
        // this test green too, trivially, because an id every row shares has
        // nothing to shift into when a row goes. So each row's name box is
        // focused in turn and the id egui itself reports is what is compared.
        let mut row_focus: Vec<Option<egui::Id>> = Vec::new();
        for name in ["first", "second", "third"] {
            let at = painted.rect_of(name).center();
            let _ = frame(&ctx, pane, &mut draft, false, &click(at));
            let id = ctx.memory(|m| m.focused());
            assert!(
                id.is_some(),
                "clicking the {name:?} row's name box did not focus it, so there is no \
                 state for a shift to lose and this test proves nothing"
            );
            row_focus.push(id);
        }
        // A set, not a sort: `egui::Id` is `Hash + Eq` and is deliberately
        // not `Ord` -- it is a hash, and an ordering over it would mean
        // nothing.
        let distinct: std::collections::HashSet<Option<egui::Id>> =
            row_focus.iter().copied().collect();
        assert_eq!(
            distinct.len(),
            3,
            "the three rows' name boxes answered to fewer than three widget ids \
             ({row_focus:?}) -- the rows are not told apart at all, so the removal below \
             could not shift anything and everything past this point would pass on a form \
             with no per-row identity"
        );

        // The caret is left in the LAST row, which is the one every shift
        // moves -- that was the final click of the loop above.
        let focused = row_focus[2];

        draft.fields.remove(0);
        let after = frame(&ctx, pane, &mut draft, false, &[]);

        // The control: the removal really happened and the pane really redrew.
        assert_eq!(
            after.rects_of(FIELD_NAME_LABEL).len(),
            2,
            "the frame after the removal drew {} name boxes, not 2: {:?}",
            after.rects_of(FIELD_NAME_LABEL).len(),
            after.strings()
        );
        assert!(
            !after.strings().iter().any(|s| *s == "first"),
            "the first row is still on screen, so nothing was removed"
        );
        assert!(
            after.strings().iter().any(|s| *s == "third"),
            "the third row stopped being drawn: {:?}",
            after.strings()
        );

        assert_eq!(
            ctx.memory(|m| m.focused()),
            focused,
            "the caret left the third row's name box when the FIRST row was removed --              the rows are still identified by their position"
        );

        // And it survives the user typing into that same box. A row
        // identified by what it displays -- its name -- would change id under
        // the caret on the first character, so the box the user is typing in
        // would stop being the box that holds the focus.
        let typed = frame(
            &ctx,
            pane,
            &mut draft,
            false,
            &[egui::Event::Text("x".to_string())],
        );
        assert_eq!(
            typed.rects_of(FIELD_NAME_LABEL).len(),
            2,
            "the form stopped drawing two rows while a character was typed: {:?}",
            typed.strings()
        );
        // The control: the keystroke really reached the third row's name box,
        // so the assertion after it is about a row whose name has changed.
        let third = draft.fields.last().expect("the third row is gone").name.clone();
        assert!(
            third != "third" && third.len() == 6 && third.contains('x'),
            "the typed character did not land in the third row's name box -- it reads \
             {third:?}"
        );
        // One more frame, and it is load-bearing. The frame that carries the
        // keystroke draws the row under its OLD name, so a name-salted id is
        // still the id the focus is on when it ends; it is the NEXT frame
        // that asks for the row under a new id and leaves the old one unseen,
        // which is when egui drops the focus. Without this frame the
        // assertion below is green under exactly the mutant it exists for --
        // measured, not supposed.
        let settled = frame(&ctx, pane, &mut draft, false, &[]);
        assert_eq!(
            settled.rects_of(FIELD_NAME_LABEL).len(),
            2,
            "the form stopped drawing two rows on the frame after the keystroke: {:?}",
            settled.strings()
        );
        assert_eq!(
            ctx.memory(|m| m.focused()),
            focused,
            "the caret left the third row's name box as soon as the row was RENAMED -- the \
             row is identified by something it displays, which changes under the user"
        );
    }

    /// Scrolls the form down in 20pt steps until `label` is drawn **wholly
    /// inside** the pane, and answers with that frame.
    ///
    /// Steps rather than one large wheel event, because a control in the
    /// MIDDLE of the form is passed by a scroll to the bottom -- and a
    /// control that has been scrolled past is culled and paints nothing,
    /// which reads exactly like a control that was never drawn. Twenty
    /// points, so the frame returned is one where the label has only just
    /// come into view and everything a row or two above it is in view too.
    ///
    /// Fails naming what WAS painted, rather than looping forever, if the
    /// label never arrives -- which is the report when a control really has
    /// been pushed out of reach.
    fn scroll_to_reveal(
        ctx: &egui::Context,
        pane: Vec2,
        draft: &mut EditDraft,
        label: &str,
    ) -> Painted {
        let bounds = Rect::from_min_size(Pos2::ZERO, pane);
        let middle = Pos2::new(pane.x / 2.0, pane.y / 2.0);
        // Enough steps to cross a form far taller than any this app draws:
        // 300 * 20pt = 6000pt.
        for _ in 0..300 {
            let painted = frame(ctx, pane, draft, false, &[]);
            if painted.rects_of(label).iter().any(|r| bounds.contains_rect(*r)) {
                return painted;
            }
            let _ = frame(
                ctx,
                pane,
                draft,
                false,
                &[
                    egui::Event::PointerMoved(middle),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0.0, -20.0),
                        modifiers: egui::Modifiers::NONE,
                        phase: egui::TouchPhase::Move,
                    },
                ],
            );
        }
        let painted = frame(ctx, pane, draft, false, &[]);
        panic!(
            "{label:?} never came into a {}x{} pane however far the form was scrolled -- it is \
             out of reach. Painted at the bottom: {:?}",
            pane.x,
            pane.y,
            painted.strings()
        );
    }

    /// A full press-and-release, which is what egui needs before it will
    /// report `Response::clicked` -- a press alone is not a click.
    fn click(pos: Pos2) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    /// **The custom-field controls can be reached and clicked at the app's
    /// minimum window size, and stay reachable as rows are added.**
    ///
    /// The same shape as `the_add_an_app_button_is_reachable_at_the_minimum_window_size`
    /// and for the same recorded reason -- three times a layout change in
    /// this file has pushed a control out of a scrolled pane. The second half
    /// is what this block adds to that risk: the number of rows is not fixed,
    /// so the Add buttons move down the form every time the user uses one.
    ///
    /// The short case asserts the buttons start CULLED, so the scroll below
    /// cannot be a no-op on a form that already fits.
    #[test]
    fn the_custom_field_controls_are_reachable_at_the_minimum_window_size() {
        let mut cases = 0;
        for rows in [0usize, 6] {
            for height in [TINY_PANE_HEIGHT, MIN_PANE_HEIGHT] {
                cases += 1;
                let pane = egui::vec2(MIN_PANE_WIDTH, height);
                let ctx = styled_context(pane);
                // A login being edited: the body with the most rows above the
                // block, so this is the worst case for reaching it.
                let mut draft = EditDraft::empty_of(ItemKind::Login);
                for i in 0..rows {
                    let mut field = FieldDraft::new_of(FieldRole::Text);
                    field.name = format!("field {i}");
                    draft.fields.push(field);
                }
                assert_eq!(draft.fields.len(), rows, "the fixture is not the size asked for");

                let _ = frame(&ctx, pane, &mut draft, false, &[]);
                let before = frame(&ctx, pane, &mut draft, false, &[]);
                let bounds = Rect::from_min_size(Pos2::ZERO, pane);
                // The row count itself, asserted before anything else is read
                // off the frame: egui culls a shape outside the screen rect
                // and paints NOTHING, so a form whose rows were pushed off
                // the pane reads as a form with no rows -- and every loop
                // below would then be a loop over nothing.
                if height == TINY_PANE_HEIGHT {
                    assert!(
                        !before
                            .rects_of(FIELD_ADD_HIDDEN_BUTTON)
                            .iter()
                            .any(|r| bounds.contains_rect(*r)),
                        "the form with {rows} custom fields already fits a {}x{} pane, so this \
                         case is not exercising scrolling at all",
                        pane.x,
                        pane.y
                    );
                }

                // Scrolled in steps, not slammed to the bottom: this block
                // sits ABOVE the app block, so a single -8000 wheel event
                // takes it straight off the TOP of the viewport and would
                // report it unreachable when it is merely passed. The user
                // scrolls in steps too.
                let after = scroll_to_reveal(&ctx, pane, &mut draft, FIELD_ADD_HIDDEN_BUTTON);

                assert_inside("the Add a field button", FIELD_ADD_TEXT_BUTTON, pane, &after);
                assert_inside(
                    "the Add a hidden field button",
                    FIELD_ADD_HIDDEN_BUTTON,
                    pane,
                    &after,
                );
                // The action strip did not come with it.
                assert_inside("Save", SAVE, pane, &after);
            }
        }
        assert_eq!(cases, 4, "the case loop visited nothing, so it asserted nothing");
    }

    /// **A field row's own boxes are reachable at the minimum window size.**
    ///
    /// The buttons above are the bottom of the block and would still be
    /// reachable if every row between them and the heading were painted off
    /// the right-hand edge -- the axis this pane cannot scroll.
    #[test]
    fn a_custom_field_rows_boxes_are_reachable_at_the_minimum_window_width() {
        let pane = egui::vec2(MIN_PANE_WIDTH, UNCULLED_PANE_HEIGHT);
        let ctx = styled_context(pane);
        let mut draft = EditDraft::empty_of(ItemKind::Login);
        // SIX rows, four text and two hidden, and the counts below are exact.
        // Two rows would pass a form that drew only the first few and left
        // the rest with no boxes at all -- silently uneditable, which is the
        // shape this file keeps re-finding: correct for the case the test
        // happened to build, absent for the one the user has.
        for i in 0..4 {
            let mut text = FieldDraft::new_of(FieldRole::Text);
            text.name = format!("text {i}");
            draft.fields.push(text);
        }
        for i in 0..2 {
            let mut hidden = FieldDraft::new_of(FieldRole::Hidden);
            hidden.name = format!("hidden {i}");
            draft.fields.push(hidden);
        }

        let _ = frame(&ctx, pane, &mut draft, false, &[]);
        let painted = frame(&ctx, pane, &mut draft, false, &[]);

        // The premise: all six rows really were drawn. A row that was never
        // drawn -- or one culled off the pane -- paints nothing, and the loop
        // below would then be silent about it.
        assert_eq!(
            painted.rects_of(FIELD_NAME_LABEL).len(),
            6,
            "six rows did not draw six name labels: {:?}",
            painted.strings()
        );
        assert_eq!(
            painted.rects_of(FIELD_VALUE_LABEL).len(),
            4,
            "the four text rows' value labels"
        );
        assert_eq!(
            painted.rects_of(FIELD_HIDDEN_VALUE_LABEL).len(),
            2,
            "a hidden row must say it is hidden -- a masked box alone looks like a text box"
        );
        assert_eq!(
            painted.rects_of(FIELD_REMOVE_BUTTON).len(),
            6,
            "every row must offer its own remove"
        );

        let mut checked = 0;
        for label in [
            FIELD_NAME_LABEL,
            FIELD_VALUE_LABEL,
            FIELD_HIDDEN_VALUE_LABEL,
            FIELD_REMOVE_BUTTON,
        ] {
            for rect in painted.rects_of(label) {
                checked += 1;
                assert!(
                    within_pane(rect, pane),
                    "{label:?} is painted at x = {}..{} on a {}pt-wide pane",
                    rect.left(),
                    rect.right(),
                    pane.x
                );
            }
        }
        assert_eq!(checked, 18, "the row loop visited {checked} labels");
    }

    /// **And every one of those six rows can be reached on a pane the app can
    /// really be resized to.**
    ///
    /// The width test above draws its six rows on an `UNCULLED_PANE_HEIGHT`
    /// pane, which is the only way to have all six in one frame -- and that
    /// makes it a claim about DRAWING, not about reach. A form whose sixth row
    /// sat below the last scroll position would satisfy it and still be
    /// uneditable. This is the height half: `MIN_PANE_HEIGHT`, and each row
    /// scrolled to in turn by its own name, which is the only thing on a row
    /// that tells it from the other five.
    ///
    /// The rows are visited top-down because `scroll_to_reveal` only ever
    /// scrolls down; the first one is asserted reachable before any scrolling,
    /// and the LAST one is asserted culled before any, so the walk cannot be a
    /// no-op on a form that already fits.
    #[test]
    fn every_custom_field_row_is_reachable_at_the_minimum_window_height() {
        let pane = egui::vec2(MIN_PANE_WIDTH, MIN_PANE_HEIGHT);
        let ctx = styled_context(pane);
        let mut draft = EditDraft::empty_of(ItemKind::Login);
        let names: Vec<String> = (0..4)
            .map(|i| format!("text {i}"))
            .chain((0..2).map(|i| format!("hidden {i}")))
            .collect();
        assert_eq!(names.len(), 6, "the fixture is not six rows");
        for (i, name) in names.iter().enumerate() {
            let mut field =
                FieldDraft::new_of(if i < 4 { FieldRole::Text } else { FieldRole::Hidden });
            field.name = name.clone();
            draft.fields.push(field);
        }

        let _ = frame(&ctx, pane, &mut draft, false, &[]);
        let unscrolled = frame(&ctx, pane, &mut draft, false, &[]);
        let bounds = Rect::from_min_size(Pos2::ZERO, pane);
        assert!(
            !unscrolled
                .rects_of(names.last().expect("six names"))
                .iter()
                .any(|r| bounds.contains_rect(*r)),
            "the six-row form already fits a {}x{} pane, so this test is not exercising              scrolling at all",
            pane.x,
            pane.y
        );

        let mut reached = 0;
        for name in &names {
            let after = scroll_to_reveal(&ctx, pane, &mut draft, name);
            assert_inside(&format!("the {name:?} row's name box"), name, pane, &after);
            reached += 1;
        }
        assert_eq!(reached, 6, "the row walk reached {reached} rows, not 6");
    }

    /// **The TOTP seed is masked, and is never painted in the clear.**
    ///
    /// The seed is the whole of a second factor: anyone who reads it off the
    /// screen can generate that account's codes forever. It gets the
    /// password's treatment -- `theme::password_field` -- and the mutation
    /// this exists to catch is one character wide (`text_field` in its
    /// place), which is why it is asserted on the painted glyphs and not on
    /// which function was called.
    ///
    /// Three states, because a mask that is really a culled block would pass
    /// the first alone:
    ///
    /// * shut: the seed is not on screen anywhere, but the username beside it
    ///   is -- so the form really was drawn;
    /// * revealed: the seed IS on screen, so the assertion above is about the
    ///   mask and not about the box being empty;
    /// * a freshly opened form starts shut, so no form ever opens showing it.
    #[test]
    fn the_totp_seed_is_masked_and_never_painted_in_the_clear() {
        const SEED: &str = "JBSWY3DPEHPK3PXP";
        const USERNAME: &str = "ada@example.com";
        let pane = egui::vec2(MIN_PANE_WIDTH, UNCULLED_PANE_HEIGHT);
        let ctx = styled_context(pane);
        let mut draft = EditDraft::empty_of(ItemKind::Login);
        draft.username = USERNAME.into();
        draft.totp = SEED.into();
        assert!(!draft.reveal_totp, "a form opens with the seed already revealed");

        let _ = frame(&ctx, pane, &mut draft, false, &[]);
        let shut = frame(&ctx, pane, &mut draft, false, &[]);
        let strings = shut.strings();
        assert!(
            strings.iter().any(|s| s.contains(USERNAME)),
            "the username is not on screen either, so this frame is not showing the login \
             body at all: {strings:?}"
        );
        assert!(
            strings.iter().any(|s| *s == TOTP_LABEL),
            "the seed row was not drawn: {strings:?}"
        );
        assert!(
            !strings.iter().any(|s| s.contains(SEED)),
            "the TOTP seed is painted in the clear: {strings:?}"
        );

        // The control on the control: revealed, it really is the seed in that
        // box, so the check above is about masking.
        draft.reveal_totp = true;
        let _ = frame(&ctx, pane, &mut draft, false, &[]);
        let open = frame(&ctx, pane, &mut draft, false, &[]);
        assert!(
            open.strings().iter().any(|s| s.contains(SEED)),
            "the box does not hold the seed at all, so the mask assertion proved nothing: {:?}",
            open.strings()
        );
    }

    /// **A hidden field's value is masked, and a text field's is not.**
    ///
    /// Both halves, because the first alone is satisfied by a form that
    /// paints no values at all -- which is what a culled block looks like.
    #[test]
    fn a_hidden_fields_value_is_masked_and_a_text_fields_is_not() {
        const SECRET: &str = "recovery-s3cret";
        const PLAIN: &str = "account-12345";
        let pane = egui::vec2(MIN_PANE_WIDTH, UNCULLED_PANE_HEIGHT);
        let ctx = styled_context(pane);
        let mut draft = EditDraft::empty_of(ItemKind::Login);
        let mut text = FieldDraft::new_of(FieldRole::Text);
        text.name = "Account number".into();
        text.value = PLAIN.into();
        let mut hidden = FieldDraft::new_of(FieldRole::Hidden);
        hidden.name = "Recovery code".into();
        hidden.value = SECRET.into();
        draft.fields.push(text);
        draft.fields.push(hidden);

        let _ = frame(&ctx, pane, &mut draft, false, &[]);
        let painted = frame(&ctx, pane, &mut draft, false, &[]);
        let strings = painted.strings();

        assert!(
            strings.iter().any(|s| s.contains(PLAIN)),
            "the ordinary field's value is not on screen either, so the mask assertion below \
             would pass on a block that painted nothing: {strings:?}"
        );
        assert!(
            !strings.iter().any(|s| s.contains(SECRET)),
            "a hidden custom field's value is painted in the clear: {strings:?}"
        );
    }

    /// **The Add button is not merely painted: it can be reached and clicked
    /// at the app's minimum window size.**
    ///
    /// The separate test from the one above, and the reason is this file's
    /// own history: three times a text or layout change has pushed a control
    /// out of a scrolled pane, and the form's viewport at the minimum size is
    /// a few hundred points. The loop above runs on a 4000pt pane where
    /// nothing is culled, which answers "is it drawn" and says nothing about
    /// "can the user get to it". This scrolls the real form to the bottom and
    /// asks [`assert_inside`] -- rect AND glyphs, so a button squeezed to an
    /// ellipsis fails too.
    #[test]
    fn the_add_an_app_button_is_reachable_at_the_minimum_window_size() {
        let mut heights = 0;
        // Both heights, and the SHORT one is what carries the test. Measured:
        // an unbound login being created is 517pt of form, so at the app's
        // real minimum (600) the add control is already on screen without
        // scrolling -- which is the answer this test wants but not a case that
        // exercises the viewport at all. `TINY_PANE_HEIGHT` is shorter than
        // anything the window can be resized to, so there the control starts
        // culled and only scrolling brings it back.
        for height in [TINY_PANE_HEIGHT, MIN_PANE_HEIGHT] {
            heights += 1;
            let pane = egui::vec2(MIN_PANE_WIDTH, height);
            let ctx = styled_context(pane);
            // The tallest form that still has NO binding: a login being
            // created, which is the only body with a generator row. Its app
            // section is the add control, so this is the worst case for
            // reaching it.
            let mut draft = EditDraft::empty_of(ItemKind::Login);
            assert!(draft.app.is_none(), "the fixture already carries a binding");

            let _ = frame(&ctx, pane, &mut draft, true, &[]);
            let before = frame(&ctx, pane, &mut draft, true, &[]);
            let bounds = Rect::from_min_size(Pos2::ZERO, pane);
            if height == TINY_PANE_HEIGHT {
                // The control on the control: without this the scroll below
                // could be a no-op on a form that already fits, and the test
                // would say nothing about the viewport. "Not visible" is
                // checked with `rects_of` and not `rect_of`, because egui
                // culls a shape outside the screen rect and paints NOTHING.
                assert!(
                    !before.rects_of(APP_ADD_BUTTON).iter().any(|r| bounds.contains_rect(*r)),
                    "the unbound form already fits a {}x{} pane, so this height is not \
                     exercising scrolling at all",
                    pane.x,
                    pane.y
                );
            }

            let middle = Pos2::new(pane.x / 2.0, pane.y / 2.0);
            let scroll = vec![
                egui::Event::PointerMoved(middle),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, -4000.0),
                    modifiers: egui::Modifiers::NONE,
                    phase: egui::TouchPhase::Move,
                },
            ];
            let _ = frame(&ctx, pane, &mut draft, true, &scroll);
            let after = frame(&ctx, pane, &mut draft, true, &[]);

            assert_inside("the Add an app button", APP_ADD_BUTTON, pane, &after);
            // ...and the notice above it, which is the sentence that says what
            // the button is for. A button alone, with its explanation scrolled
            // off, is the state `aae9429` shipped.
            assert_inside("the unbound notice", APP_NONE_NOTICE, pane, &after);
            // The action strip did not come with it.
            assert_inside("Save", SAVE, pane, &after);
        }
        assert_eq!(heights, 2, "the height loop visited nothing, so it asserted nothing");
    }

    /// Inside the pane **on the axis the user cannot scroll**.
    ///
    /// Horizontal only, and the omission is deliberate rather than a
    /// weakening. Vertical reach on this form is already two other tests'
    /// subject -- `save_and_cancel_are_on_screen_at_the_minimum_window_size`
    /// and `the_form_scrolls_to_its_last_field_while_the_buttons_stay_put` --
    /// and it is answered by SCROLLING, so a y outside the pane is a thing the
    /// user can reach. An x outside it is not: this pane refuses horizontal
    /// scrolling on purpose. Width is what this test exists for, and width is
    /// where the 309.4pt card was.
    ///
    /// The y is also the one axis a glyph box cannot be trusted about.
    /// `Glyph::size` is the font's full ascent-plus-descent cell, not the ink,
    /// so the bottom-pinned Cancel button -- which the suite above already
    /// proves is on screen -- reports a box 2.1pt below a pane its galley fits
    /// inside. Asserting on that would be asserting about font metrics.
    fn within_pane(rect: Rect, pane: Vec2) -> bool {
        rect.left() >= 0.0 && rect.right() <= pane.x
    }

    /// **No two runs on the tallest form are drawn on top of each other.**
    ///
    /// Being inside the pane is not the same as being readable: two controls
    /// can both be in bounds and still occupy the same pixels, which
    /// `contains_rect` says nothing about. `horizontal_wrapped` is the fix for
    /// the width above, and a row that wrapped onto a line it had not reserved
    /// height for is precisely how that fix would go wrong.
    ///
    /// Asked of `Painted::glyphs` and NOT of `Painted::texts`: a galley laid
    /// inside a wrapped row reports the WRAP WIDTH as its size, so the word
    /// "seconds" claims a 93.7pt box for 40pt of ink and appears to sit on top
    /// of the wait field beside it. That is one false report, and one is
    /// enough to make an overlap assertion something you have to carry an
    /// exception list for -- which is how it stops being an assertion. The
    /// glyph positions are what egui really placed.
    #[test]
    fn no_two_runs_on_the_tallest_edit_form_overlap() {
        let pane = egui::vec2(MIN_PANE_WIDTH, UNCULLED_PANE_HEIGHT);
        let ctx = styled_context(pane);
        let mut draft = tallest_draft();
        let item = palette_item();
        let painted =
            tallest_form_with_the_builder_open(&ctx, pane, &mut draft, &item, &live_code());

        // The premise. A form drawn as three runs would pass the loop below
        // and prove nothing; the real one draws upwards of sixty.
        assert!(
            painted.glyphs.len() > 40,
            "only {} runs were painted -- the form was not drawn in full, so this test is \
             not exercising the layout",
            painted.glyphs.len()
        );

        for i in 0..painted.glyphs.len() {
            for j in (i + 1)..painted.glyphs.len() {
                let (left, a) = &painted.glyphs[i];
                let (right, b) = &painted.glyphs[j];
                let shared = a.intersect(*b);
                // Sub-pixel touching is antialiasing, not an overlap; half a
                // point in BOTH axes is ink on ink.
                assert!(
                    shared.width() <= 0.5 || shared.height() <= 0.5,
                    "{left:?} at {a:?} and {right:?} at {b:?} are drawn over each other, \
                     sharing {shared:?}"
                );
            }
        }
    }

    /// **The controls on the two assertions above.**
    ///
    /// Both are loops over what a frame happened to paint, and a loop over an
    /// empty list passes. These feed the same predicates hand-built geometry
    /// no layout produced and demand that each one refuses it -- the same
    /// shape, and the same reason, as `detail.rs`'s `assert_visible_refuses_*`
    /// pair.
    #[test]
    fn the_layout_predicates_refuse_the_geometry_they_exist_to_catch() {
        let pane = egui::vec2(MIN_PANE_WIDTH, UNCULLED_PANE_HEIGHT);

        // The card as it really was measured: 309.4 wide on a 298pt pane.
        let overflowing = Rect::from_min_max(Pos2::new(0.0, 41.0), Pos2::new(309.4, 913.9));
        assert!(
            !within_pane(overflowing, pane),
            "a 309.4pt-wide card is being called inside a {}pt pane",
            pane.x
        );
        // ...and the control on THAT: the fixed card is accepted, so the
        // refusal above is about the width and not about the rect being
        // hand-built.
        let fitting = Rect::from_min_max(Pos2::new(0.0, 41.0), Pos2::new(292.0, 913.9));
        assert!(within_pane(fitting, pane), "a 292pt card does not fit a {}pt pane", pane.x);
        // `within_pane` is horizontal by design, so it must also refuse a box
        // off the LEFT edge -- a one-sided check would pass everything the
        // wrap fix could get wrong on the other side.
        let off_left = Rect::from_min_max(Pos2::new(-3.0, 41.0), Pos2::new(200.0, 913.9));
        assert!(!within_pane(off_left, pane), "a box starting at x = -3 is being called inside");
        // ...and it must NOT refuse a box that is merely far below the pane,
        // which is the scrollable axis and which the glyph metrics lie about.
        let below = Rect::from_min_max(Pos2::new(10.0, 9000.0), Pos2::new(200.0, 9014.0));
        assert!(
            within_pane(below, pane),
            "a box below a pane that SCROLLS vertically is being called unreachable"
        );

        // Two runs sharing ink, which the overlap loop must refuse.
        let a = Rect::from_min_max(Pos2::new(19.0, 1213.9), Pos2::new(60.0, 1227.9));
        let b = Rect::from_min_max(Pos2::new(40.0, 1215.0), Pos2::new(108.0, 1229.9));
        let shared = a.intersect(b);
        assert!(
            shared.width() > 0.5 && shared.height() > 0.5,
            "two runs overlapping by {shared:?} are being called disjoint"
        );
        // And two that merely share an edge are not an overlap.
        let touching = Rect::from_min_max(Pos2::new(60.0, 1213.9), Pos2::new(108.0, 1227.9));
        let grazed = a.intersect(touching);
        assert!(
            grazed.width() <= 0.5 || grazed.height() <= 0.5,
            "two runs that share an edge ({grazed:?}) are being called an overlap"
        );
    }
}
