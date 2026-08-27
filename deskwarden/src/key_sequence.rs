//! The auto-type keystroke sequence: its stored spelling, its parsed form, and
//! the preview that resolves it against a real vault item.
//!
//! # Why a sequence exists at all
//!
//! Filling a login by typing the user name, a Tab and the password works for
//! one form on one screen. It cannot log into Microsoft 365, Okta, or any of
//! the other multi-screen sign-ins that ask for the address, navigate, and
//! *then* ask for the password: by the time the password is typed the second
//! page has not loaded, and the keystrokes land in nothing. Those need explicit
//! keys and explicit waits -- `{USERNAME}{ENTER}{DELAY 2000}{PASSWORD}{ENTER}`
//! -- which is a program, and a program needs a notation.
//!
//! # Why KeePass's notation and not one of our own
//!
//! The user asked for portability in as many words ("so it is portable"). This
//! spelling is KeePass's auto-type syntax, which KeePass, KeePassXC and several
//! other managers already read and write, so a sequence typed here can be
//! pasted there and back. Inventing a notation would have bought nothing and
//! cost every user who already has one written down.
//!
//! What is modelled, and where the line is:
//!
//!  * **Fields** -- `{USERNAME}`, `{PASSWORD}`, `{TOTP}` and `{S:Name}` for a
//!    custom field. These are the placeholders this app can resolve, so they
//!    are the ones it models.
//!  * **Keys** -- [`KEYS`], the KeePass names, aliases included and *kept as
//!    aliases* (see [`KeyDef`]).
//!  * **Delays** -- `{DELAY 1000}` (wait this long here) and `{DELAY=50}` (type
//!    at this rate from here on). Both, because they mean different things and
//!    a file can contain either.
//!  * **Modifiers** -- a bare `+`, `^`, `%` or `~`, which KeePass reads as
//!    Shift, Ctrl, Alt and Enter applied to what follows.
//!  * **Everything else in braces** -- [`Token::Unknown`], carried verbatim.
//!
//! # The rule the whole module is built around: nothing is ever dropped
//!
//! A sequence is JSON in a custom field on a real vault item. It can have been
//! written by KeePass, by a future build of this app, or by hand, and this
//! build's job on meeting a construct it does not model is to *show it and give
//! it back unchanged* -- never to silently discard it. That is the same
//! discipline [`crate::app_match::AppMatch`] applies to unknown JSON keys and
//! [`crate::vault_bridge::VaultField`] to unknown field keys, and it is why
//! [`Token::Unknown`], [`Token::Grouping`] and the alias entries in [`KEYS`]
//! exist rather than a parser that normalises.
//!
//! [`render`] and [`parse`] are each other's inverse on canonical input, which
//! [`render`]'s output always is; the property tests at the bottom of this file
//! pin both directions over generated token lists rather than a handful of
//! examples.
//!
//! # What this module deliberately does NOT do
//!
//! **It does not type anything.** There is no runner here, and that is not an
//! oversight: sending synthetic input is a separate problem (focus, elevation,
//! per-key timing, cancellation) and mixing it into the commit that introduces
//! the notation would have shipped a parser whose only reader was a component
//! nobody could see. The reader that ships *with* the notation is
//! [`resolve_preview`], which is on screen behind the eye toggle and exercises
//! every branch of the parser against the live item on the frame it is drawn.

use crate::vault_bridge::VaultItem;
use crate::vault_window::detail::TotpState;

/// One key this app knows the name of.
///
/// **Aliases are separate entries on purpose.** KeePass writes `{BKSP}`,
/// `{BS}` and `{BACKSPACE}` for the same key, and a parser that folded the
/// first two onto the third would re-render someone's file with different
/// bytes than it arrived with -- a rewrite of the user's data to say something
/// this app merely finds tidier. Each spelling is its own `KeyDef` carrying its
/// own `token`, so [`render`] gives back exactly what [`parse`] was handed,
/// while `label` and `symbol` (which the palette and the preview use) are
/// shared and so the user still sees one key.
///
/// `palette` is what keeps the picker short: every spelling round-trips, but
/// only the canonical one is offered as a button, because a palette listing
/// three buttons that insert the same key is a palette that has to be read
/// rather than clicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyDef {
    /// The text between the braces, exactly as stored.
    pub token: &'static str,
    /// What the palette button says.
    pub label: &'static str,
    /// What the preview draws instead of the word. An `{ENTER}` in a sequence
    /// is a key press, and rendering it as the word "ENTER" in a preview whose
    /// whole purpose is to show *what would be typed* would say the five
    /// letters would be typed.
    pub symbol: &'static str,
    /// Whether the palette offers a button for this spelling.
    pub palette: bool,
}

/// Every key spelling this build recognises.
///
/// The names are KeePass's. Anything not here still parses -- as
/// [`Token::Unknown`], which renders back byte-for-byte -- so this list being
/// incomplete costs discoverability, never data.
pub const KEYS: &[KeyDef] = &[
    KeyDef { token: "ENTER", label: "Enter", symbol: "\u{21b5}", palette: true },
    KeyDef { token: "TAB", label: "Tab", symbol: "\u{21e5}", palette: true },
    KeyDef { token: "ESC", label: "Esc", symbol: "\u{238b}", palette: true },
    KeyDef { token: "SPACE", label: "Space", symbol: "\u{2423}", palette: true },
    KeyDef { token: "BACKSPACE", label: "Backspace", symbol: "\u{232b}", palette: true },
    KeyDef { token: "BKSP", label: "Backspace", symbol: "\u{232b}", palette: false },
    KeyDef { token: "BS", label: "Backspace", symbol: "\u{232b}", palette: false },
    KeyDef { token: "DELETE", label: "Delete", symbol: "\u{2326}", palette: true },
    KeyDef { token: "DEL", label: "Delete", symbol: "\u{2326}", palette: false },
    KeyDef { token: "INSERT", label: "Insert", symbol: "Ins", palette: false },
    KeyDef { token: "INS", label: "Insert", symbol: "Ins", palette: false },
    KeyDef { token: "HOME", label: "Home", symbol: "Home", palette: false },
    KeyDef { token: "END", label: "End", symbol: "End", palette: false },
    KeyDef { token: "PGUP", label: "Page Up", symbol: "PgUp", palette: false },
    KeyDef { token: "PGDN", label: "Page Down", symbol: "PgDn", palette: false },
    KeyDef { token: "UP", label: "Up", symbol: "\u{2191}", palette: true },
    KeyDef { token: "DOWN", label: "Down", symbol: "\u{2193}", palette: true },
    KeyDef { token: "LEFT", label: "Left", symbol: "\u{2190}", palette: true },
    KeyDef { token: "RIGHT", label: "Right", symbol: "\u{2192}", palette: true },
    KeyDef { token: "F1", label: "F1", symbol: "F1", palette: false },
    KeyDef { token: "F2", label: "F2", symbol: "F2", palette: false },
    KeyDef { token: "F3", label: "F3", symbol: "F3", palette: false },
    KeyDef { token: "F4", label: "F4", symbol: "F4", palette: false },
    KeyDef { token: "F5", label: "F5", symbol: "F5", palette: false },
    KeyDef { token: "F6", label: "F6", symbol: "F6", palette: false },
    KeyDef { token: "F7", label: "F7", symbol: "F7", palette: false },
    KeyDef { token: "F8", label: "F8", symbol: "F8", palette: false },
    KeyDef { token: "F9", label: "F9", symbol: "F9", palette: false },
    KeyDef { token: "F10", label: "F10", symbol: "F10", palette: false },
    KeyDef { token: "F11", label: "F11", symbol: "F11", palette: false },
    KeyDef { token: "F12", label: "F12", symbol: "F12", palette: false },
];

/// The key spelling named by `token`, matched **exactly**.
///
/// Case-sensitive, and that is the same no-rewrite rule as the aliases above:
/// KeePass writes these names in upper case, and a hand-written `{tab}` that
/// this app folded to `{TAB}` would be a silent edit of the user's field. A
/// lower-case spelling parses as [`Token::Unknown`] instead, which is shown as
/// an opaque chip and given back unchanged.
pub fn key_named(token: &str) -> Option<&'static KeyDef> {
    KEYS.iter().find(|k| k.token == token)
}

/// The value a `{...}` field placeholder refers to.
///
/// `Serialize`/`Deserialize` because it travels in the encrypted vault
/// cache's facts section: a projection says WHICH fields an item has so the
/// picker can offer them without opening a secret. It names fields and
/// never carries one -- `Custom` holds a field's name, which is what the
/// user typed on the item, not its value.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FieldRef {
    Username,
    Password,
    Totp,
    /// `{S:Name}` -- a custom field on the item, by name. The name is stored
    /// exactly as written, case included: it is matched against the item's own
    /// field names, and those are the user's.
    Custom(String),
}

impl FieldRef {
    /// The text between the braces.
    pub fn token(&self) -> String {
        match self {
            Self::Username => "USERNAME".to_string(),
            Self::Password => "PASSWORD".to_string(),
            Self::Totp => "TOTP".to_string(),
            Self::Custom(name) => format!("S:{name}"),
        }
    }

    /// What a chip and a palette button call it.
    pub fn label(&self) -> String {
        match self {
            Self::Username => "Username".to_string(),
            Self::Password => "Password".to_string(),
            Self::Totp => "TOTP".to_string(),
            Self::Custom(name) => name.clone(),
        }
    }
}

/// A bare `+`, `^`, `%` or `~`: KeePass reads these as modifiers on whatever
/// follows rather than as text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modifier {
    Shift,
    Ctrl,
    Alt,
    /// `~` -- KeePass's shorthand for Enter.
    Enter,
}

impl Modifier {
    pub fn from_char(c: char) -> Option<Self> {
        match c {
            '+' => Some(Self::Shift),
            '^' => Some(Self::Ctrl),
            '%' => Some(Self::Alt),
            '~' => Some(Self::Enter),
            _ => None,
        }
    }

    pub fn as_char(self) -> char {
        match self {
            Self::Shift => '+',
            Self::Ctrl => '^',
            Self::Alt => '%',
            Self::Enter => '~',
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Shift => "Shift+",
            Self::Ctrl => "Ctrl+",
            Self::Alt => "Alt+",
            Self::Enter => "Enter",
        }
    }
}

/// Characters that mean something other than themselves and so must be written
/// `{+}`, `{(}` and so on to be typed literally.
pub const SPECIAL_CHARS: &[char] = &['+', '^', '%', '~', '(', ')', '[', ']', '{', '}'];

/// One element of a parsed sequence.
///
/// The variants are exactly the things that must survive a round trip
/// distinctly. [`Self::Unknown`] and [`Self::Grouping`] are the two that carry
/// no meaning this build understands and exist purely so nothing is lost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    /// Text to be typed as-is. Never empty, and never adjacent to another
    /// `Literal` in a list [`parse`] produced -- see [`merge_literals`].
    Literal(String),
    Field(FieldRef),
    Key(&'static KeyDef),
    /// `{DELAY 1000}` -- pause here, in milliseconds.
    Delay(u32),
    /// `{DELAY=50}` -- from here on, this many milliseconds between
    /// keystrokes. A different thing from [`Self::Delay`] and stored as a
    /// different token so it is not silently turned into one.
    DelayRate(u32),
    Modifier(Modifier),
    /// A bare `(`, `)`, `[` or `]`. KeePass groups keystrokes with these; this
    /// build does not interpret the grouping, but it must not eat it either,
    /// so the character rides through as itself and is drawn as itself.
    Grouping(char),
    /// The raw text between a `{` and its `}` for a construct this build does
    /// not model -- `{PICKCHARS}`, `{APPACTIVATE Foo}`, anything a later
    /// KeePass adds. Rendered back as `{` + this + `}`, byte for byte.
    Unknown(String),
}

impl Token {
    /// The chip caption, for a sequence being edited.
    pub fn chip_label(&self) -> String {
        match self {
            Self::Literal(text) => text.clone(),
            Self::Field(field) => field.label(),
            Self::Key(key) => key.label.to_string(),
            Self::Delay(ms) => wait_label(*ms),
            Self::DelayRate(ms) => format!("Type every {ms} ms"),
            Self::Modifier(m) => m.label().to_string(),
            Self::Grouping(c) => c.to_string(),
            Self::Unknown(raw) => format!("{{{raw}}}"),
        }
    }

    /// Whether this chip is one this build understands. The builder marks the
    /// rest visibly, so a user looking at a sequence imported from elsewhere
    /// can see which parts this app will act on rather than discovering it at
    /// fill time.
    pub fn is_understood(&self) -> bool {
        !matches!(self, Self::Unknown(_))
    }
}

/// "Wait 1.5s" for 1500. The user asked for this in seconds ("Wait N sec");
/// the format stores milliseconds, so the conversion happens here, in one
/// place, rather than in the two widgets that would otherwise each do it.
///
/// One decimal, and no trailing `.0`, so the common whole-second waits read as
/// "Wait 2s" and a 250ms one still reads honestly as "Wait 0.3s" rather than
/// rounding to "Wait 0s" -- a wait the user set and the app claims is nothing.
/// Sub-50ms waits, which no number of seconds can show, are given in ms.
pub fn wait_label(ms: u32) -> String {
    if ms < 50 {
        return format!("Wait {ms} ms");
    }
    let tenths = (ms + 50) / 100;
    if tenths % 10 == 0 {
        format!("Wait {}s", tenths / 10)
    } else {
        format!("Wait {}.{}s", tenths / 10, tenths % 10)
    }
}

/// Seconds as the user typed them into the wait box, as milliseconds.
///
/// `None` for anything that is not a non-negative number, and for anything
/// over an hour: a wait is a pause in a sequence the user is watching, and a
/// mistyped `36000` that parked the fill for ten hours would look exactly like
/// a hang. Accepts a decimal point or a comma, because both are decimal
/// separators on the keyboards this app ships to.
pub fn wait_ms_from_seconds(text: &str) -> Option<u32> {
    let text = text.trim().replace(',', ".");
    if text.is_empty() {
        return None;
    }
    let seconds: f64 = text.parse().ok()?;
    if !seconds.is_finite() || seconds < 0.0 || seconds > 3600.0 {
        return None;
    }
    Some((seconds * 1000.0).round() as u32)
}

/// `text` with every character that means something else escaped, so it is
/// typed as itself.
///
/// **This is why the builder can have a plain text box.** A user typing a
/// literal `{` or a `+` should not have to know that either is special; they
/// type it, this escapes it, and [`parse`] gives back the very character they
/// typed. Doing it anywhere but here would mean two spellings of the same
/// rule.
pub fn escape_literal(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if SPECIAL_CHARS.contains(&c) {
            out.push('{');
            out.push(c);
            out.push('}');
        } else {
            out.push(c);
        }
    }
    out
}

/// `tokens` as the string that is stored in the vault field.
///
/// The inverse of [`parse`]: `parse(&render(t)) == merge_literals(t)` for every
/// token list, and `render(&parse(s)) == s` for every `s` [`render`] produced.
pub fn render(tokens: &[Token]) -> String {
    let mut out = String::new();
    for token in tokens {
        match token {
            Token::Literal(text) => out.push_str(&escape_literal(text)),
            Token::Field(field) => {
                out.push('{');
                out.push_str(&field.token());
                out.push('}');
            }
            Token::Key(key) => {
                out.push('{');
                out.push_str(key.token);
                out.push('}');
            }
            Token::Delay(ms) => out.push_str(&format!("{{DELAY {ms}}}")),
            Token::DelayRate(ms) => out.push_str(&format!("{{DELAY={ms}}}")),
            Token::Modifier(m) => out.push(m.as_char()),
            Token::Grouping(c) => out.push(*c),
            Token::Unknown(raw) => {
                out.push('{');
                out.push_str(raw);
                out.push('}');
            }
        }
    }
    out
}

/// Adjacent [`Token::Literal`]s joined, and empty ones dropped.
///
/// [`parse`] can only ever produce a list in this shape (a literal run is
/// accumulated and flushed once), so this is what makes the round trip an
/// equality rather than an "equivalent to": a hand-built
/// `[Literal("a"), Literal("b")]` renders to `"ab"` and parses back as
/// `[Literal("ab")]`, which is the same sequence spelled once.
pub fn merge_literals(tokens: &[Token]) -> Vec<Token> {
    let mut out: Vec<Token> = Vec::with_capacity(tokens.len());
    for token in tokens {
        match (out.last_mut(), token) {
            (_, Token::Literal(text)) if text.is_empty() => {}
            (Some(Token::Literal(prev)), Token::Literal(text)) => prev.push_str(text),
            _ => out.push(token.clone()),
        }
    }
    out
}

/// The stored string as tokens.
///
/// **Total: there is no error case.** Every byte of the input ends up in
/// exactly one token, including bytes that make no sense -- an unterminated
/// `{` is the literal characters that follow it, and a `{WHATEVER}` this build
/// has never heard of is [`Token::Unknown`]. A parser that could fail would
/// need a failure path in the builder, and the only honest thing that path
/// could do with a user's stored sequence is refuse to show it.
pub fn parse(sequence: &str) -> Vec<Token> {
    let mut tokens: Vec<Token> = Vec::new();
    let mut literal = String::new();
    let chars: Vec<char> = sequence.chars().collect();
    let mut i = 0;

    let flush = |literal: &mut String, tokens: &mut Vec<Token>| {
        if !literal.is_empty() {
            tokens.push(Token::Literal(std::mem::take(literal)));
        }
    };

    while i < chars.len() {
        let c = chars[i];
        if c == '{' {
            // `{{}` and `{}}` are the escaped braces, and they have to be
            // tested before the general "scan to the next `}`" below -- that
            // scan would stop at the inner brace of `{{}` and read an empty
            // placeholder.
            if i + 2 < chars.len() && chars[i + 2] == '}' && (chars[i + 1] == '{' || chars[i + 1] == '}')
            {
                literal.push(chars[i + 1]);
                i += 3;
                continue;
            }
            if let Some(close) = (i + 1..chars.len()).find(|&j| chars[j] == '}') {
                let inner: String = chars[i + 1..close].iter().collect();
                i = close + 1;
                // A single special character in braces is that character,
                // typed. This is `escape_literal`'s inverse and the reason a
                // user can put a `+` in their sequence at all.
                let mut inner_chars = inner.chars();
                if let (Some(only), None) = (inner_chars.next(), inner_chars.next()) {
                    if SPECIAL_CHARS.contains(&only) {
                        literal.push(only);
                        continue;
                    }
                }
                flush(&mut literal, &mut tokens);
                tokens.push(placeholder(&inner));
                continue;
            }
            // An unterminated `{`. Nothing after it can be a placeholder, so
            // the brace and the rest are text -- which is what round-trips,
            // because `escape_literal` will write the brace back as `{{}`.
            literal.push('{');
            i += 1;
            continue;
        }
        if let Some(modifier) = Modifier::from_char(c) {
            flush(&mut literal, &mut tokens);
            tokens.push(Token::Modifier(modifier));
            i += 1;
            continue;
        }
        if matches!(c, '(' | ')' | '[' | ']') {
            flush(&mut literal, &mut tokens);
            tokens.push(Token::Grouping(c));
            i += 1;
            continue;
        }
        literal.push(c);
        i += 1;
    }
    flush(&mut literal, &mut tokens);
    tokens
}

/// The token for the text between one pair of braces.
fn placeholder(inner: &str) -> Token {
    match inner {
        "USERNAME" => return Token::Field(FieldRef::Username),
        "PASSWORD" => return Token::Field(FieldRef::Password),
        "TOTP" => return Token::Field(FieldRef::Totp),
        _ => {}
    }
    if let Some(name) = inner.strip_prefix("S:") {
        // An empty name (`{S:}`) refers to no field and is not a reference:
        // left opaque so it renders back as it arrived rather than becoming a
        // custom-field chip with a blank name.
        if !name.is_empty() {
            return Token::Field(FieldRef::Custom(name.to_string()));
        }
    }
    if let Some(rest) = inner.strip_prefix("DELAY ") {
        if let Some(ms) = whole_number(rest) {
            return Token::Delay(ms);
        }
    }
    if let Some(rest) = inner.strip_prefix("DELAY=") {
        if let Some(ms) = whole_number(rest) {
            return Token::DelayRate(ms);
        }
    }
    if let Some(key) = key_named(inner) {
        return Token::Key(key);
    }
    Token::Unknown(inner.to_string())
}

/// `text` as a `u32`, only when it is digits and nothing else and renders back
/// as itself. A leading `+`, a leading zero or a space would parse as a number
/// and then be written back differently -- so they are refused here and ride
/// through as [`Token::Unknown`] instead.
fn whole_number(text: &str) -> Option<u32> {
    if text.is_empty() || !text.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let value: u32 = text.parse().ok()?;
    (value.to_string() == text).then_some(value)
}

// ---------------------------------------------------------------------------
// What an empty sequence means
// ---------------------------------------------------------------------------

/// The sequence that is typed when the item stores none.
///
/// **Empty means "the fill this app has always done", not "type nothing".**
/// Every item in every existing vault has no sequence, so the other reading
/// would turn this commit into one that silently stopped filling every login
/// in the vault. "Type nothing" is also not a thing a user can want from a
/// fill; it is what a bug looks like. So the empty string keeps its existing
/// behaviour, and a user who wants something else writes it down.
pub const DEFAULT_SEQUENCE: &str = "{USERNAME}{TAB}{PASSWORD}";

/// [`parse`], with [`DEFAULT_SEQUENCE`] standing in for an empty stored value.
///
/// This is the one place the default is expanded, and the preview is its
/// reader: an item with no sequence shows what the default would type, so the
/// decision above is visible on screen rather than asserted in a doc comment.
pub fn effective_tokens(sequence: &str) -> Vec<Token> {
    if sequence.is_empty() {
        parse(DEFAULT_SEQUENCE)
    } else {
        parse(sequence)
    }
}

// ---------------------------------------------------------------------------
// The palette
// ---------------------------------------------------------------------------

/// The field buttons offered for **this item**, in the order they are shown.
///
/// Derived from the item, never hardcoded: that is the whole point of the
/// palette. `{S:PIN}` is discoverable only because the item really has a field
/// called `PIN`, and an item with no TOTP secret is not offered a `{TOTP}`
/// button it could only ever fail to resolve.
///
/// `deskwarden:`-prefixed fields are this app's own bookkeeping (the app match
/// itself, and the sequence) and are not offered: typing this app's JSON into
/// a login form is not something a user is trying to do.
pub fn field_palette(item: &VaultItem) -> Vec<FieldRef> {
    let mut out = Vec::new();
    let login = item.login.as_ref();
    if login.and_then(|l| l.username.as_deref()).is_some_and(|u| !u.is_empty()) {
        out.push(FieldRef::Username);
    }
    if login.and_then(|l| l.password.as_deref()).is_some_and(|p| !p.is_empty()) {
        out.push(FieldRef::Password);
    }
    if login.and_then(|l| l.totp.as_deref()).is_some_and(|t| !t.is_empty()) {
        out.push(FieldRef::Totp);
    }
    for field in &item.fields {
        let Some(name) = field.name.as_deref() else { continue };
        if name.is_empty() || name.starts_with("deskwarden:") {
            continue;
        }
        let candidate = FieldRef::Custom(name.to_string());
        if !out.contains(&candidate) {
            out.push(candidate);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// The preview -- the eye toggle
// ---------------------------------------------------------------------------

/// One run of the resolved preview.
///
/// **These are drawn and dropped, never stored.** The whole value is built
/// inside the frame that paints it and goes out of scope at the end of it: it
/// is not a field on the draft, not a field on the window, and not carried on
/// any action. That is deliberately the same care
/// [`crate::vault_window::detail::DetailAction`]'s secret variants take --
/// they name the item and let the one component that already holds the
/// plaintext go and fetch it, rather than carrying a copy that would become a
/// second, non-zeroizing home for it. A preview that lived on the draft would
/// be exactly that second home, kept for the whole edit session, whether or
/// not the eye was still open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewPart {
    /// Literal characters from the sequence itself.
    Text(String),
    /// A resolved field's value -- the part the eye exists to reveal.
    Value(String),
    /// A key, as its symbol. Never as its name: this preview says what would
    /// be *typed*, and `{ENTER}` does not type the word "ENTER".
    Key(&'static str),
    /// A pause, in the user's words.
    Wait(String),
    /// Something that cannot be resolved, with the reason. **Never an empty
    /// string**: a `{S:Missing}` rendered as nothing is a preview that says
    /// the sequence is fine when it is not, and the sequence would type
    /// nothing there.
    Unresolved(String),
    /// A `{TOTP}` whose code has not arrived yet.
    Pending,
    /// A construct this build does not model, shown as itself so the user can
    /// see it is being carried rather than acted on.
    Opaque(String),
}

/// Everything [`resolve_preview`] reads, borrowed for the length of one frame.
///
/// Borrowed rather than owned so building it copies no secret: the strings
/// point at the item the pane already holds.
pub struct ResolveSource<'a> {
    pub username: &'a str,
    pub password: &'a str,
    pub custom: Vec<(&'a str, &'a str)>,
    /// The **existing** TOTP state, computed once per frame by the vault
    /// window's own poll. Not a second TOTP path: this preview cannot fetch a
    /// code and does not try, it renders whichever of the five states the one
    /// poll has reached.
    pub totp: &'a TotpState,
}

/// The item's custom fields as name/value pairs, borrowed.
///
/// Shared by [`ResolveSource`] and by the edit form, so "which fields exist"
/// has one answer rather than two that can drift -- a palette offering
/// `{S:PIN}` while the preview cannot find `PIN` would be the drift.
pub fn custom_pairs(item: &VaultItem) -> Vec<(&str, &str)> {
    item.fields
        .iter()
        .filter_map(|f| Some((f.name.as_deref()?, f.value.as_ref().map_or("", |v| v.as_str()))))
        .collect()
}

impl<'a> ResolveSource<'a> {
    fn custom_value(&self, name: &str) -> Option<&'a str> {
        self.custom.iter().find(|(n, _)| *n == name).map(|(_, v)| *v)
    }
}

/// The reason shown in place of a value that is not there. Phrased as the fact,
/// with the reference in it, because the user's next move is to go and fix the
/// named thing.
fn missing(what: &str) -> PreviewPart {
    PreviewPart::Unresolved(format!("\u{2039}{what}\u{203a}"))
}

/// What this sequence would actually type, against this item, right now.
///
/// The parser's live reader: every variant of [`Token`] is resolved here, so a
/// branch of [`parse`] that produced the wrong token would be visible on
/// screen on the frame it was introduced.
pub fn resolve_preview(tokens: &[Token], source: &ResolveSource<'_>) -> Vec<PreviewPart> {
    tokens
        .iter()
        .map(|token| match token {
            Token::Literal(text) => PreviewPart::Text(text.clone()),
            Token::Field(FieldRef::Username) => {
                if source.username.is_empty() {
                    missing("no username on this item")
                } else {
                    PreviewPart::Value(source.username.to_string())
                }
            }
            Token::Field(FieldRef::Password) => {
                if source.password.is_empty() {
                    missing("no password on this item")
                } else {
                    PreviewPart::Value(source.password.to_string())
                }
            }
            Token::Field(FieldRef::Totp) => match source.totp {
                TotpState::Code { code, .. } => PreviewPart::Value(code.clone()),
                TotpState::Fetching => PreviewPart::Pending,
                TotpState::NoSecret => missing("no TOTP on this item"),
                TotpState::NoCodeReported => missing("no current TOTP"),
                TotpState::Unavailable => missing("TOTP unavailable"),
            },
            Token::Field(FieldRef::Custom(name)) => match source.custom_value(name) {
                Some(value) if !value.is_empty() => PreviewPart::Value(value.to_string()),
                Some(_) => missing(&format!("{name} is empty")),
                None => missing(&format!("no field called {name}")),
            },
            Token::Key(key) => PreviewPart::Key(key.symbol),
            Token::Delay(ms) => PreviewPart::Wait(wait_label(*ms)),
            Token::DelayRate(ms) => PreviewPart::Wait(format!("{ms} ms/key")),
            Token::Modifier(m) => PreviewPart::Opaque(m.label().to_string()),
            Token::Grouping(c) => PreviewPart::Opaque(c.to_string()),
            Token::Unknown(raw) => PreviewPart::Opaque(format!("{{{raw}}}")),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault_bridge::{LoginData, VaultField};

    // -- fixtures -----------------------------------------------------------
    //
    // Every value here is DELIBERATELY DIFFERENT from every other one. A test
    // item whose password happened to equal its username would let a
    // `{PASSWORD}` that resolved the user name pass -- five defects this
    // session were fixtures whose two inputs agreed.

    const USERNAME: &str = "ada@example.test";
    const PASSWORD: &str = "correct-horse-battery";
    const PIN: &str = "8421";
    const TOTP_CODE: &str = "776699";

    fn field(name: &str, value: &str) -> VaultField {
        VaultField {
            name: Some(name.to_string()),
            value: Some(zeroize::Zeroizing::new(value.to_string())),
            other: serde_json::Map::new(),
        }
    }

    fn item() -> VaultItem {
        VaultItem {
            id: "item-1".to_string(),
            name: "Contoso 365".to_string(),
            fields: vec![field("PIN", PIN), field("Employee ID", "E-99")],
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

    /// A [`ResolveSource`] over the whole item. The shipped preview builds one
    /// over the *draft* (see `detail_edit::sequence_source`), because what the
    /// user is about to save is what the preview must show; this is the same
    /// value against an item that has not been edited.
    fn source<'a>(item: &'a VaultItem, totp: &'a TotpState) -> ResolveSource<'a> {
        let login = item.login.as_ref();
        ResolveSource {
            username: login.and_then(|l| l.username.as_deref()).unwrap_or(""),
            password: login
                .and_then(|l| l.password.as_deref())
                .map(|p| p.as_str())
                .unwrap_or(""),
            custom: custom_pairs(item),
            totp,
        }
    }

    fn live_code() -> TotpState {
        TotpState::Code { code: TOTP_CODE.to_string(), seconds_left: 18 }
    }

    // -- round trip ---------------------------------------------------------

    /// The generator. A hand-written list of examples tests the cases the
    /// author thought of, which are exactly the cases the parser was written
    /// for; this walks a deterministic pseudo-random sequence over EVERY
    /// variant, including the two that exist only to carry things this build
    /// does not understand.
    fn generated_tokens(seed: u64) -> Vec<Token> {
        let mut state = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let mut next = move || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (state >> 33) as usize
        };
        let literals = [
            "hello",
            "2134",
            "a{b",
            "+^%~",
            "()[]",
            "}",
            "space here",
            "\u{e9}\u{fc}",
            "{USERNAME}",
        ];
        let unknowns = ["PICKCHARS", "APPACTIVATE Foo", "S:", "DELAY x", "DELAY  9", "delay 9", "tab"];
        let customs = ["PIN", "Employee ID", "weird:name", "S:nested"];
        let count = next() % 12 + 1;
        (0..count)
            .map(|_| match next() % 9 {
                0 => Token::Literal(literals[next() % literals.len()].to_string()),
                1 => Token::Field(FieldRef::Username),
                2 => Token::Field(FieldRef::Password),
                3 => match next() % 2 {
                    0 => Token::Field(FieldRef::Totp),
                    _ => Token::Field(FieldRef::Custom(customs[next() % customs.len()].to_string())),
                },
                4 => Token::Key(&KEYS[next() % KEYS.len()]),
                5 => Token::Delay((next() % 10_000) as u32),
                6 => Token::DelayRate((next() % 500) as u32),
                7 => match next() % 2 {
                    0 => Token::Modifier(
                        [Modifier::Shift, Modifier::Ctrl, Modifier::Alt, Modifier::Enter]
                            [next() % 4],
                    ),
                    _ => Token::Grouping(['(', ')', '[', ']'][next() % 4]),
                },
                _ => Token::Unknown(unknowns[next() % unknowns.len()].to_string()),
            })
            .collect()
    }

    #[test]
    fn parsing_a_rendered_token_list_reproduces_the_tokens() {
        for seed in 0..2000u64 {
            let tokens = merge_literals(&generated_tokens(seed));
            let rendered = render(&tokens);
            assert_eq!(
                parse(&rendered),
                tokens,
                "seed {seed}: rendered as {rendered:?}"
            );
        }
    }

    #[test]
    fn rendering_a_parsed_sequence_reproduces_the_string() {
        for seed in 0..2000u64 {
            let rendered = render(&generated_tokens(seed));
            assert_eq!(
                render(&parse(&rendered)),
                rendered,
                "seed {seed}"
            );
        }
    }

    /// The generator is only worth anything if it really produces every
    /// variant. Without this, a bug in `generated_tokens` would silently
    /// narrow the two properties above to whatever it happened to emit.
    #[test]
    fn the_generator_reaches_every_token_variant() {
        let all: Vec<Token> = (0..2000u64).flat_map(generated_tokens).collect();
        for (what, seen) in [
            ("Literal", all.iter().any(|t| matches!(t, Token::Literal(_)))),
            ("Username", all.contains(&Token::Field(FieldRef::Username))),
            ("Password", all.contains(&Token::Field(FieldRef::Password))),
            ("Totp", all.contains(&Token::Field(FieldRef::Totp))),
            ("Custom", all.iter().any(|t| matches!(t, Token::Field(FieldRef::Custom(_))))),
            ("Key", all.iter().any(|t| matches!(t, Token::Key(_)))),
            ("Delay", all.iter().any(|t| matches!(t, Token::Delay(_)))),
            ("DelayRate", all.iter().any(|t| matches!(t, Token::DelayRate(_)))),
            ("Modifier", all.iter().any(|t| matches!(t, Token::Modifier(_)))),
            ("Grouping", all.iter().any(|t| matches!(t, Token::Grouping(_)))),
            ("Unknown", all.iter().any(|t| matches!(t, Token::Unknown(_)))),
        ] {
            assert!(seen, "the generator never produced a {what} token");
        }
        // ...and every key spelling, so the alias entries are really covered.
        for key in KEYS {
            assert!(
                all.iter().any(|t| matches!(t, Token::Key(k) if k.token == key.token)),
                "the generator never produced {}",
                key.token
            );
        }
    }

    /// The sequences a real KeePass file contains, byte for byte in and out.
    /// This is the question the user will ask of a vault they share with
    /// another manager.
    #[test]
    fn a_sequence_written_by_keepass_survives_a_load_and_save_unchanged() {
        for stored in [
            "{USERNAME}{TAB}{PASSWORD}{ENTER}",
            "{USERNAME}{ENTER}{DELAY 2000}{PASSWORD}{ENTER}",
            "{USERNAME}{TAB}{S:PIN}{TAB}{TOTP}{ENTER}",
            "{DELAY=50}{USERNAME}{TAB}{PASSWORD}",
            "^v{TAB}+{TAB}%{F4}",
            "{PICKCHARS}{APPACTIVATE Notepad}",
            "2134{TOTP}",
            "{BKSP}{BS}{DEL}{INS}",
            "(({USERNAME}))[{TAB}]",
            "{{}literal{}}",
            "{+}{^}{%}{~}",
            "~{USERNAME}",
            "{S:Custom Field With Spaces}",
            "{DELAY  9}{delay 9}{tab}",
        ] {
            assert_eq!(render(&parse(stored)), stored, "stored: {stored:?}");
        }
    }

    #[test]
    fn an_unknown_construct_survives_as_one_opaque_chip() {
        let tokens = parse("{USERNAME}{PICKCHARS}{TAB}");
        assert_eq!(
            tokens,
            vec![
                Token::Field(FieldRef::Username),
                Token::Unknown("PICKCHARS".to_string()),
                Token::Key(key_named("TAB").unwrap()),
            ]
        );
        assert_eq!(tokens[1].chip_label(), "{PICKCHARS}");
        assert!(!tokens[1].is_understood());
        assert!(tokens[0].is_understood() && tokens[2].is_understood());
    }

    #[test]
    fn a_lower_case_key_name_is_carried_rather_than_rewritten() {
        // The user's file said `{tab}`. This build does not act on it, and it
        // does not quietly turn it into something else either.
        assert_eq!(parse("{tab}"), vec![Token::Unknown("tab".to_string())]);
        assert_eq!(render(&parse("{tab}")), "{tab}");
    }

    #[test]
    fn an_alias_key_is_not_folded_onto_its_canonical_spelling() {
        assert_eq!(render(&parse("{BKSP}")), "{BKSP}");
        assert_eq!(render(&parse("{BACKSPACE}")), "{BACKSPACE}");
        // ...and the user sees one key, not three.
        assert_eq!(key_named("BKSP").unwrap().label, key_named("BACKSPACE").unwrap().label);
        assert_eq!(KEYS.iter().filter(|k| k.palette && k.label == "Backspace").count(), 1);
    }

    // -- escaping -----------------------------------------------------------

    #[test]
    fn a_literal_the_user_typed_round_trips_through_escaping() {
        for typed in [
            "{",
            "}",
            "+",
            "^",
            "%",
            "~",
            "(",
            ")",
            "[",
            "]",
            "2134",
            "a+b{c}d",
            "{USERNAME}",
            "100% sure",
        ] {
            let stored = escape_literal(typed);
            assert_eq!(
                parse(&stored),
                vec![Token::Literal(typed.to_string())],
                "typed {typed:?} stored as {stored:?}"
            );
        }
    }

    #[test]
    fn a_literal_brace_is_not_read_as_an_empty_placeholder() {
        assert_eq!(parse("{{}"), vec![Token::Literal("{".to_string())]);
        assert_eq!(parse("{}}"), vec![Token::Literal("}".to_string())]);
    }

    #[test]
    fn an_unterminated_brace_is_text_and_round_trips() {
        assert_eq!(parse("{USERNAME"), vec![Token::Literal("{USERNAME".to_string())]);
        assert_eq!(render(&parse("{USERNAME")), "{{}USERNAME");
        // ...and having been normalised once, it stays put.
        assert_eq!(render(&parse("{{}USERNAME")), "{{}USERNAME");
    }

    #[test]
    fn a_number_that_would_not_round_trip_is_not_read_as_a_delay() {
        for stored in ["{DELAY 0100}", "{DELAY +5}", "{DELAY }", "{DELAY= 5}", "{DELAY=}"] {
            let tokens = parse(stored);
            assert!(
                tokens.iter().all(|t| !matches!(t, Token::Delay(_) | Token::DelayRate(_))),
                "{stored:?} parsed as {tokens:?}"
            );
            assert_eq!(render(&tokens), stored);
        }
        assert_eq!(parse("{DELAY 100}"), vec![Token::Delay(100)]);
        assert_eq!(parse("{DELAY=50}"), vec![Token::DelayRate(50)]);
    }

    #[test]
    fn an_empty_custom_field_name_is_not_a_reference() {
        assert_eq!(parse("{S:}"), vec![Token::Unknown("S:".to_string())]);
    }

    // -- the user's own case ------------------------------------------------

    #[test]
    fn literal_text_next_to_a_field_is_expressible_and_resolves() {
        // "and it will be possible to do something like 2134{TOTP} -- which
        // will return as 2134776699 for example?"
        let tokens = parse("2134{TOTP}");
        assert_eq!(
            tokens,
            vec![Token::Literal("2134".to_string()), Token::Field(FieldRef::Totp)]
        );
        let totp = live_code();
        let item = item();
        let parts = resolve_preview(&tokens, &source(&item, &totp));
        assert_eq!(
            parts,
            vec![
                PreviewPart::Text("2134".to_string()),
                PreviewPart::Value(TOTP_CODE.to_string()),
            ]
        );
        assert_eq!(preview_text(&parts), format!("2134{TOTP_CODE}"));
    }

    /// The concatenation of everything the preview would type, for the one
    /// assertion the user actually stated ("2134776699"). Test-only on
    /// purpose: the shipped preview draws the parts separately, in different
    /// colours, and never builds one string of them.
    fn preview_text(parts: &[PreviewPart]) -> String {
        parts
            .iter()
            .map(|p| match p {
                PreviewPart::Text(t) | PreviewPart::Value(t) => t.clone(),
                _ => String::new(),
            })
            .collect()
    }

    // -- the preview --------------------------------------------------------

    #[test]
    fn each_field_resolves_to_its_own_value_and_not_a_neighbours() {
        let item = item();
        let totp = live_code();
        let source = source(&item, &totp);
        for (sequence, expected) in [
            ("{USERNAME}", USERNAME),
            ("{PASSWORD}", PASSWORD),
            ("{TOTP}", TOTP_CODE),
            ("{S:PIN}", PIN),
            ("{S:Employee ID}", "E-99"),
        ] {
            assert_eq!(
                resolve_preview(&parse(sequence), &source),
                vec![PreviewPart::Value(expected.to_string())],
                "{sequence}"
            );
        }
        // The control on the fixture: no two of these agree, so the
        // assertions above really did tell them apart.
        let values = [USERNAME, PASSWORD, TOTP_CODE, PIN, "E-99"];
        for (i, a) in values.iter().enumerate() {
            for b in &values[i + 1..] {
                assert_ne!(a, b, "the fixture's values must all differ");
            }
        }
    }

    #[test]
    fn a_key_previews_as_a_symbol_and_never_as_its_name() {
        let item = item();
        let totp = live_code();
        let source = source(&item, &totp);
        let parts = resolve_preview(&parse("{ENTER}{TAB}"), &source);
        assert_eq!(parts, vec![PreviewPart::Key("\u{21b5}"), PreviewPart::Key("\u{21e5}")]);

        // **The structural half, which is the one that matters.** A few keys
        // have no glyph anyone would recognise (`{HOME}`, `{PGUP}`) and their
        // symbol is a short word; what makes them un-confusable with typed
        // text is not the shape but the variant -- a key NEVER produces a
        // `Text` or a `Value`, so it is never drawn in the run of characters
        // that would land in the box, and `sequence_preview` brackets it.
        // Rendering `{ENTER}` as `PreviewPart::Text("ENTER")` fails here.
        for key in KEYS {
            let parts =
                resolve_preview(&[Token::Key(key)], &source);
            assert_eq!(
                parts,
                vec![PreviewPart::Key(key.symbol)],
                "{} does not preview as a key press",
                key.token
            );
            assert!(!key.symbol.is_empty(), "{} previews as nothing at all", key.token);
        }
        // ...and no key's symbol is the whole of its own KeePass name in the
        // cases where a real symbol exists for it, which is every key the
        // palette offers.
        for key in KEYS.iter().filter(|k| k.palette) {
            assert!(
                !key.symbol.eq_ignore_ascii_case(key.token),
                "{} previews as its own name, so the preview claims those letters are typed",
                key.token
            );
        }
    }

    #[test]
    fn a_missing_custom_field_is_shown_as_unresolved_and_never_as_nothing() {
        let item = item();
        let totp = live_code();
        let source = source(&item, &totp);
        let parts = resolve_preview(&parse("{S:Missing}"), &source);
        match &parts[..] {
            [PreviewPart::Unresolved(why)] => {
                assert!(why.contains("Missing"), "{why:?} does not name the field");
                assert!(!why.trim().is_empty());
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(preview_text(&parts), "", "an unresolved part must type nothing");
    }

    #[test]
    fn an_empty_custom_field_is_told_apart_from_a_missing_one() {
        let mut item = item();
        item.fields.push(field("Blank", ""));
        let totp = live_code();
        let source = source(&item, &totp);
        let blank = resolve_preview(&parse("{S:Blank}"), &source);
        let absent = resolve_preview(&parse("{S:Nope}"), &source);
        assert!(matches!(blank[..], [PreviewPart::Unresolved(_)]));
        assert!(matches!(absent[..], [PreviewPart::Unresolved(_)]));
        assert_ne!(blank, absent, "an empty field reads the same as a missing one");
    }

    #[test]
    fn every_totp_state_previews_distinctly() {
        let item = item();
        let states = [
            TotpState::Code { code: TOTP_CODE.to_string(), seconds_left: 4 },
            TotpState::Fetching,
            TotpState::NoSecret,
            TotpState::NoCodeReported,
            TotpState::Unavailable,
        ];
        let parts: Vec<PreviewPart> = states
            .iter()
            .map(|state| {
                resolve_preview(&parse("{TOTP}"), &source(&item, state))
                    .remove(0)
            })
            .collect();
        for (i, a) in parts.iter().enumerate() {
            for b in &parts[i + 1..] {
                assert_ne!(a, b, "two TOTP states preview identically: {parts:?}");
            }
        }
        assert_eq!(parts[0], PreviewPart::Value(TOTP_CODE.to_string()));
        assert_eq!(parts[1], PreviewPart::Pending);
    }

    #[test]
    fn an_item_with_no_username_says_so_rather_than_previewing_a_blank() {
        let mut item = item();
        item.login.as_mut().unwrap().username = None;
        let totp = live_code();
        let parts =
            resolve_preview(&parse("{USERNAME}"), &source(&item, &totp));
        assert!(matches!(parts[..], [PreviewPart::Unresolved(_)]));
    }

    #[test]
    fn an_unknown_construct_previews_as_itself_and_types_nothing() {
        let item = item();
        let totp = live_code();
        let parts = resolve_preview(
            &parse("{PICKCHARS}"),
            &source(&item, &totp),
        );
        assert_eq!(parts, vec![PreviewPart::Opaque("{PICKCHARS}".to_string())]);
        assert_eq!(preview_text(&parts), "");
    }

    // -- the default --------------------------------------------------------

    #[test]
    fn an_empty_sequence_means_the_fill_this_app_has_always_done() {
        assert_eq!(effective_tokens(""), parse(DEFAULT_SEQUENCE));
        assert_eq!(
            effective_tokens(""),
            vec![
                Token::Field(FieldRef::Username),
                Token::Key(key_named("TAB").unwrap()),
                Token::Field(FieldRef::Password),
            ],
            "the default must be the username-Tab-password fill, not nothing"
        );
        // The unsafe reading, stated so a change to it fails here.
        assert!(!effective_tokens("").is_empty(), "an empty sequence must not type nothing");
    }

    #[test]
    fn a_stored_sequence_is_used_instead_of_the_default() {
        assert_eq!(effective_tokens("{TAB}"), vec![Token::Key(key_named("TAB").unwrap())]);
    }

    #[test]
    fn the_default_is_previewed_for_an_item_that_stores_no_sequence() {
        let item = item();
        let totp = live_code();
        let parts = resolve_preview(&effective_tokens(""), &source(&item, &totp));
        assert_eq!(
            parts,
            vec![
                PreviewPart::Value(USERNAME.to_string()),
                PreviewPart::Key("\u{21e5}"),
                PreviewPart::Value(PASSWORD.to_string()),
            ]
        );
    }

    // -- waits --------------------------------------------------------------

    #[test]
    fn a_wait_is_labelled_in_the_seconds_the_user_asked_for() {
        for (ms, expected) in [
            (0, "Wait 0 ms"),
            (30, "Wait 30 ms"),
            (250, "Wait 0.3s"),
            (500, "Wait 0.5s"),
            (1000, "Wait 1s"),
            (1500, "Wait 1.5s"),
            (2000, "Wait 2s"),
            (10_000, "Wait 10s"),
        ] {
            assert_eq!(wait_label(ms), expected, "{ms} ms");
        }
    }

    #[test]
    fn a_wait_typed_in_seconds_becomes_milliseconds() {
        for (typed, expected) in [
            ("1", Some(1000)),
            ("1.5", Some(1500)),
            ("1,5", Some(1500)),
            (" 2 ", Some(2000)),
            ("0", Some(0)),
            ("0.25", Some(250)),
            ("", None),
            ("abc", None),
            ("-1", None),
            ("3601", None),
            ("1e9", None),
        ] {
            assert_eq!(wait_ms_from_seconds(typed), expected, "{typed:?}");
        }
    }

    #[test]
    fn a_wait_round_trips_through_its_own_label_and_box() {
        for seconds in ["0.5", "1", "1.5", "2", "5"] {
            let ms = wait_ms_from_seconds(seconds).unwrap();
            assert_eq!(render(&[Token::Delay(ms)]), format!("{{DELAY {ms}}}"));
            assert_eq!(parse(&render(&[Token::Delay(ms)])), vec![Token::Delay(ms)]);
        }
    }

    // -- the palette --------------------------------------------------------

    #[test]
    fn the_palette_lists_the_items_own_fields() {
        assert_eq!(
            field_palette(&item()),
            vec![
                FieldRef::Username,
                FieldRef::Password,
                FieldRef::Totp,
                FieldRef::Custom("PIN".to_string()),
                FieldRef::Custom("Employee ID".to_string()),
            ]
        );
    }

    #[test]
    fn the_palette_offers_no_totp_button_for_an_item_with_no_secret() {
        let mut item = item();
        item.login.as_mut().unwrap().totp = None;
        assert!(!field_palette(&item).contains(&FieldRef::Totp));
        // ...and the control: the same item with a secret is offered one.
        assert!(field_palette(&self::item()).contains(&FieldRef::Totp));
    }

    #[test]
    fn the_palette_hides_this_apps_own_bookkeeping_fields() {
        let mut item = item();
        item.fields.push(field(crate::app_match::APP_MATCH_FIELD_NAME, "{}"));
        let palette = field_palette(&item);
        assert!(
            !palette.iter().any(|f| matches!(f, FieldRef::Custom(n) if n.starts_with("deskwarden:"))),
            "{palette:?}"
        );
        assert!(palette.contains(&FieldRef::Custom("PIN".to_string())));
    }

    #[test]
    fn a_palette_entry_renders_to_the_placeholder_it_promises() {
        for field in field_palette(&item()) {
            let rendered = render(&[Token::Field(field.clone())]);
            assert_eq!(parse(&rendered), vec![Token::Field(field.clone())], "{rendered}");
        }
        assert_eq!(
            render(&[Token::Field(FieldRef::Custom("PIN".to_string()))]),
            "{S:PIN}"
        );
    }

    // -- chips --------------------------------------------------------------

    #[test]
    fn every_token_has_a_chip_caption_that_is_not_empty() {
        for seed in 0..200u64 {
            for token in generated_tokens(seed) {
                let label = token.chip_label();
                assert!(!label.trim().is_empty(), "{token:?} has a blank chip");
            }
        }
    }
}
