//! The vault window's detail pane in edit mode, and the "+ New" creation
//! flow (they share one form: creating is editing against an empty draft
//! with no item id yet). See `detail.rs` for why this is a separate file
//! from read mode.

use crate::app_identity::{self, AppIdentityCache};
use crate::app_match::{AppMatch, TriggerMode};
use crate::theme;
use crate::vault_bridge::{
    CardData, Folder, GenerateRequest, IdentityData, ItemKind, NewItem, PassphraseRecipe,
    PasswordRecipe, VaultItem,
};
#[cfg(test)]
use crate::vault_bridge::{LoginData, UriEntry};
use crate::vault_window::{detail, sidebar};
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

/// The SSH-key-specific half of a draft (`type: 5`).
///
/// The three field names are the wire keys captured from a real type-5 item on
/// 2026-08-01 (`.superpowers/sdd/item-shapes-capture.md`): `privateKey`,
/// `publicKey`, `keyFingerprint`. There is no `SshKeyData` to mirror yet --
/// the struct lands with the `ssh-key-type` branch and
/// [`NewItem::ssh_key`] takes the three strings directly in the meantime --
/// so this draft is written against that constructor.
///
/// **Create only.** [`EditDraft::apply_to`] never writes these back, because
/// [`VaultItem`] has no `sshKey` field in this build: an existing key's object
/// rides the `other` catch-all and is preserved precisely *because* nothing
/// touches it. [`form_body`] is where that asymmetry is decided, so the edit
/// form cannot offer boxes whose contents would be silently dropped.
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
}

impl AppMatchDraft {
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
        }
    }

    /// The binding this draft would save.
    ///
    /// `args` is passed straight through -- **not trimmed, not re-quoted, not
    /// split**. See [`AppMatch::args`]: the string is the user's, and this app
    /// has no tokenisation of a Windows command line that it could apply
    /// without guessing.
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
            args: self.args.clone(),
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
    /// unrelated field (the arguments, the trigger) un-editable on any item
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
    /// for a hosted row**. `trigger` and `args` survive, because they are the
    /// user's settings for this item and not facts about the window.
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
/// name, and it keeps working. Withholding Save would make the trigger and the
/// arguments un-editable on exactly the items whose path most needs correcting,
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
            card: CardDraft::default(),
            identity: IdentityDraft::default(),
            ssh_key: SshKeyDraft::default(),
            note_body: String::new(),
            generator: GeneratorDraft::default(),
            app: None,
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
        self.card = CardDraft::default();
        self.identity = IdentityDraft::default();
        self.ssh_key = SshKeyDraft::default();
        self.note_body = String::new();
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

const APP_REMOVED_NOTICE: &str =
    "This app will stop filling from this item when you save. Undo, or Cancel the edit, to keep \
     it.";

/// The three trigger pills, bound to the draft rather than reporting a click.
///
/// The order, the words and the caption are `detail`'s -- the read pane's card
/// and this block must offer the user the same three choices under the same
/// three names, and a second copy of the vocabulary here is how they would
/// drift.
fn app_trigger_pills(ui: &mut egui::Ui, current: &mut TriggerMode) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for mode in detail::TRIGGER_ORDER {
            let selected = mode == *current;
            let button = egui::Button::new(
                theme::semibold(detail::trigger_label(mode), 12.0).color(if selected {
                    egui::Color32::WHITE
                } else {
                    theme::INK
                }),
            )
            .fill(if selected { theme::BLUE } else { theme::CARD })
            .stroke(if selected {
                Stroke::NONE
            } else {
                Stroke::new(1.0, theme::BORDER_STRONG)
            })
            .corner_radius(CornerRadius::same(7));
            if ui.add(button).clicked() {
                *current = mode;
            }
        }
    });
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

/// The whole app block. Returns an [`EditAction`] when it needs the caller to
/// do something the form cannot (open the file dialog).
fn app_block(
    ui: &mut egui::Ui,
    app: &mut AppMatchDraft,
    apps: &mut AppIdentityCache,
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

    theme::field_label(ui, "Autofill");
    app_trigger_pills(ui, &mut app.trigger);
    ui.add_space(4.0);
    ui.label(
        RichText::new(detail::trigger_caption(app.trigger))
            .size(11.0)
            .color(theme::TEXT_FAINT),
    );
    ui.add_space(10.0);

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

pub fn draw_detail_edit(
    ui: &mut egui::Ui,
    draft: &mut EditDraft,
    folders: &[Folder],
    creating: bool,
    apps: &mut AppIdentityCache,
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
                    ui.horizontal(|ui| {
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

            // Between the kind's own fields and the folder, because a
            // binding is neither: it is about what Deskwarden does with this
            // item, which is the same argument that puts the read pane's
            // `MATCHED APP` card last among the body cards.
            //
            // Drawn only for an item that HAS a binding -- see `EditDraft::app`
            // and `AppMatchDraft`.
            if let Some(app) = draft.app.as_mut() {
                if let Some(requested) = app_block(ui, app, apps) {
                    action = requested;
                }
                ui.add_space(4.0);
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

    // A create of a kind `NewItem` cannot express has no payload at all (see
    // `EditDraft::to_new_item`), so Save is withheld rather than left to
    // produce nothing when clicked. Unreachable through the "+ New" menu,
    // which offers only `CREATABLE_KINDS` -- this is the backstop, and it says
    // why instead of doing nothing quietly. An *edit* of such an item is
    // fine: the name and folder still save.
    let creatable = !creating || is_creatable(draft.kind);
    if !creatable {
        ui.add_space(6.0);
        ui.label(
            RichText::new(
                "Deskwarden does not know this item type and cannot create one. Create it in \
                 the Bitwarden web vault or app.",
            )
            .size(12.0)
            .color(theme::ERROR),
        );
    }
    if !draft.is_valid() {
        ui.add_space(6.0);
        ui.label(RichText::new("Name is required.").size(12.0).color(theme::ERROR));
    }

    ui.add_space(12.0);
    ui.horizontal(|ui| {
        let save = egui::Button::new(if draft.is_valid() { "Save" } else { "Save (needs a name)" });
        if ui.add_enabled(draft.is_valid() && creatable, save).clicked() {
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
    fn an_item_with_no_binding_gives_the_form_no_block() {
        // The positive control for the test above: `app` must not be `Some` for
        // every item, which is what a block drawn unconditionally would need.
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

    #[test]
    fn saving_a_changed_trigger_reaches_the_item() {
        let item = bound_item(&chrome_match());
        let mut draft = EditDraft::from_item(&item);
        draft.app.as_mut().unwrap().trigger = TriggerMode::Auto;
        let stored = crate::vault_bridge::extract_app_match(&draft.apply_to(&item)).unwrap();
        assert_eq!(stored.trigger, TriggerMode::Auto);
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
            value: Some(stored.to_string()),
            other: serde_json::Map::new(),
        });
        let draft = EditDraft::from_item(&item);
        let saved = draft.apply_to(&item);
        assert_eq!(
            saved.fields[0].value.as_deref(),
            Some(stored),
            "an untouched binding was rewritten on save"
        );
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
    const BODY: egui::Vec2 = egui::vec2(560.0, 900.0);

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
            action = draw_detail_edit(ui, draft, &[], false, &mut apps);
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
            path: r"C:\Deskwarden Test\Chrome\chrome.exe".to_string(),
            args: "--profile-directory=WorkProfile".to_string(),
            trigger: TriggerMode::Prompt,
        }
    }

    fn store() -> AppMatch {
        AppMatch {
            process: "Speedtest.exe".to_string(),
            title: "Speedtest".to_string(),
            hosted: true,
            path: String::new(),
            args: String::new(),
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
        assert!(strings.contains(&"chrome.exe"), "no app name painted: {strings:?}");
    }

    #[test]
    fn a_form_with_no_binding_draws_no_app_block() {
        // The positive control for the test above: without it, a block drawn
        // unconditionally would satisfy it.
        let ctx = styled_context();
        let mut draft = EditDraft::empty();
        draft.name = "Ledgerline".to_string();
        let (_, painted) = frame(&ctx, &mut draft, &[]);
        assert!(
            !painted.strings().contains(&APP_BLOCK_HEADING),
            "an unbound item was offered an app block: {:?}",
            painted.strings()
        );
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

    #[test]
    fn clicking_a_trigger_pill_changes_the_trigger() {
        // Mutation this catches: an inert pill row. `app_trigger_pills` writes
        // straight into the draft, so nothing else in the crate would notice.
        let ctx = styled_context();
        let mut draft = app_draft(&chrome());
        let (_, painted) = frame(&ctx, &mut draft, &[]);
        let auto = painted.rect_of(detail::trigger_label(TriggerMode::Auto));

        let _ = frame(&ctx, &mut draft, &click(auto.center()));
        assert_eq!(
            draft.app.as_ref().unwrap().trigger,
            TriggerMode::Auto,
            "clicking the Auto pill left the trigger where it was"
        );
    }

    #[test]
    fn a_store_app_gets_a_read_only_path_row_and_keeps_its_trigger_and_remove() {
        // The user's own choice for this case: state the reason, disable the
        // file picker, leave the trigger and Remove working.
        let ctx = styled_context();
        let mut draft = app_draft(&store());
        let (_, painted) = frame(&ctx, &mut draft, &[]);
        let strings = painted.strings();

        assert!(
            strings.contains(&APP_PATH_STORE_APP),
            "a Store binding does not say why it has no path: {strings:?}"
        );
        assert!(strings.contains(&"Speedtest"), "the window title is not shown: {strings:?}");
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

        // ...and the trigger still works, which is the half of the requirement a
        // wholesale `add_enabled(false)` around the block would have broken.
        let (_, painted) = frame(&ctx, &mut draft, &[]);
        let auto = painted.rect_of(detail::trigger_label(TriggerMode::Auto));
        let _ = frame(&ctx, &mut draft, &click(auto.center()));
        assert_eq!(draft.app.as_ref().unwrap().trigger, TriggerMode::Auto);
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
        assert!(theme::icon_probe::gears(&all).is_empty(), "a bitmap was read as a gear");
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
