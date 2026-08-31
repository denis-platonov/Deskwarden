//! Finding, downloading and applying deskwarden's own updates.
//!
//! # What the update path actually trusts
//!
//! **The trust root here is TLS to `api.github.com` plus the integrity of the
//! `denis-platonov/deskwarden` GitHub account. Nothing else.** This is worth
//! stating without euphemism, because the module used to imply something
//! stronger.
//!
//! Deskwarden's releases are not code-signed. `.github/workflows/release.yml`
//! says so on every build, and until now this module gated the launch on an
//! Authenticode thumbprint constant that read
//! `PLACEHOLDER_SET_ONCE_SIGNPATH_CERT_ISSUED` -- a value chosen so it could
//! never match anything. The gate therefore refused *every* update that had
//! ever been offered. The self-update did not fail rarely; it had never once
//! succeeded, and the user was downloading and running each release by hand
//! instead.
//!
//! That is the trade this module now makes, and it is not the trade it looks
//! like from the diff. Removing a signature check normally means giving up a
//! guarantee. Here there was no guarantee to give up: the check could not
//! pass, so its whole effect was to move the act of running an unsigned
//! installer from this process -- where something could be verified -- into
//! the user's own hands, where nothing was. What replaces it is a SHA-256
//! comparison against a digest GitHub publishes for the asset, which is
//! strictly more checking than either the old gate (which refused everything)
//! or the manual download (which checked nothing).
//!
//! **This is not equivalent to a signed build and must not be described as
//! one.** A signature would let a user verify the publisher without trusting
//! the distribution channel. A digest fetched from that same channel cannot:
//! whoever can replace the asset can generally replace the digest beside it.
//! What the digest does buy is real but narrower --
//!
//!  * the bytes that run are the bytes GitHub's API described, so a corrupted,
//!    truncated or mid-flight-substituted download is caught;
//!  * the digest arrives on the SAME TLS response as the download URL
//!    ([`check_for_update`]), so it is not a second thing to fetch and not a
//!    second connection to attack -- rewriting one means being able to rewrite
//!    the other, rather than merely sitting on the CDN fetch;
//!  * and the file is re-hashed immediately before the spawn, so the window
//!    between "verified" and "executed" is closed.
//!
//! When a code-signing certificate is obtained, an Authenticode gate should be
//! added back ON TOP of this one -- `signature::is_trusted_signer` is still
//! present and still tested for exactly that day. See
//! `docs/code-signing-policy.md`.
//!
//! # Everything here fails closed
//!
//! There is deliberately no path on which "the digest could not be checked"
//! becomes "launch it anyway". A release whose installer asset carries no
//! digest, or a malformed one, is not reported as an available update at all;
//! a downloaded file whose digest disagrees -- or which cannot be read in
//! order to hash it -- is deleted and refused. See [`apply_update_with`].

use semver::Version;
use sha2::{Digest as _, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// A SHA-256 digest, as 32 raw bytes.
///
/// A newtype rather than a `String` so that the comparison at the gate is
/// between two things that are *already* known to be well-formed digests. A
/// string comparison would have to decide, at the gate, what to do about
/// casing, about a `sha256:` prefix that might or might not be there, and
/// about a value that is not hex at all -- and "decide at the gate" is where
/// a lenient answer turns into a launch. Parsing happens once, at the edge, in
/// [`parse_asset_digest`]; by the time a value of this type exists it is 32
/// bytes and nothing else, and `==` on it cannot mean anything but equality of
/// those bytes.
///
/// `Copy` because it is 32 bytes and gets carried around inside [`ReleaseInfo`]
/// clones; `Eq` because that is the whole point of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sha256Digest([u8; 32]);

impl std::fmt::Display for Sha256Digest {
    /// Lowercase hex, no prefix -- the spelling that appears in error messages
    /// when a digest is rejected, so a user can compare it by eye against
    /// what GitHub shows for the asset.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// The prefix GitHub puts on a release asset's `digest` field.
///
/// Required rather than tolerated-if-absent. The field is documented as
/// algorithm-qualified, and the qualifier is the only thing that says the 64
/// hex characters after it are a SHA-256 rather than something else this code
/// would then compare a SHA-256 against and always reject -- or, worse, some
/// future shorter algorithm it would compare against a truncation of.
const ASSET_DIGEST_PREFIX: &str = "sha256:";

/// Parses a release asset's `digest` field: `"sha256:"` followed by exactly 64
/// hex characters.
///
/// Every rejection here is a refusal to offer the update at all, which is the
/// intended direction: this runs inside [`check_for_update`], long before
/// anything has been downloaded, and a release this function cannot make sense
/// of is a release nothing in this module will go on to launch.
pub fn parse_asset_digest(field: &str) -> Result<Sha256Digest, String> {
    let hex = field.strip_prefix(ASSET_DIGEST_PREFIX).ok_or_else(|| {
        format!(
            "release asset digest '{field}' is not a SHA-256 digest (expected a \
             '{ASSET_DIGEST_PREFIX}' prefix)"
        )
    })?;
    if hex.len() != 64 {
        return Err(format!(
            "release asset digest '{field}' has {} hex characters after the prefix, not 64",
            hex.len()
        ));
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        // `from_str_radix` on a two-character slice, rather than a hand-rolled
        // nibble table, because it is the piece that rejects a non-hex
        // character -- and rejecting it is the job. Note it also rejects a
        // leading `+`, and `-`, and whitespace, all of which `u8::from_str`
        // would take. Slicing by byte index is safe here: the length check
        // above passed, and every character of a 64-length ASCII-hex string is
        // one byte. A multi-byte character makes `len()` exceed 64 and so
        // never reaches this loop.
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
            .map_err(|_| format!("release asset digest '{field}' is not hexadecimal"))?;
    }
    Ok(Sha256Digest(out))
}

/// `Clone` so the About page's download button can hand a copy to the
/// background download/verify thread (see `update_panel.rs`) while keeping its
/// own copy for the version and notes it is displaying, rather than moving the
/// app's record of the available update into a thread it can't get it back
/// from.
/// `PartialEq`/`Eq` so `update_panel::UpdateStage` -- which carries one -- can
/// answer "did this frame change anything", which is what stops the About page
/// from asking for a repaint it does not need.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseInfo {
    pub version: Version,
    pub installer_download_url: String,
    /// SHA-256 of the installer asset, from the SAME asset object in the SAME
    /// API response `installer_download_url` came from.
    ///
    /// # Why it is not an `Option`
    ///
    /// This is the fail-closed decision, made in the type rather than at the
    /// gate. If this were `Option<Sha256Digest>` then every consumer would
    /// have to decide what `None` means, and there would be one obvious wrong
    /// answer available to each of them. As a required field, a release whose
    /// installer asset has no usable digest cannot be REPRESENTED as an
    /// available update: [`check_for_update`] returns an error instead of
    /// `Some`, the About page shows that error, and no download and no launch
    /// ever begins. "We could not check" therefore cannot become "proceed"
    /// by anyone forgetting a branch, because there is no branch.
    ///
    /// # Why it comes from this response and not a second one
    ///
    /// Same reason [`ReleaseInfo::body`] does, plus a security one. The
    /// releases API returns a per-asset `digest` field of the form
    /// `sha256:<64 hex>` alongside `browser_download_url`, so the digest and
    /// the URL it describes are read out of one object, delivered by one TLS
    /// response, from one host. A `SHA256SUMS` asset fetched separately would
    /// be a second request that could fail on its own, be answered by a
    /// different CDN edge, or be omitted -- and "omitted" is a state that
    /// invites exactly the lenient branch the paragraph above exists to
    /// prevent.
    pub installer_sha256: Sha256Digest,
    /// The notes of every release this user skipped, newest first, each under
    /// its own version heading -- see [`notes_across`].
    ///
    /// **A union of several releases, while [`Self::installer_download_url`]
    /// and [`Self::installer_sha256`] describe exactly one.** That asymmetry
    /// is the point rather than an oversight: a user on 0.8.1 needs to read
    /// what changed in 0.8.2 and 0.8.3 as well as 0.8.4, and needs to install
    /// one file whose digest came from the same object as its URL.
    ///
    /// Kept here rather than fetched separately because the response
    /// [`check_for_update`] already parses for `tag_name` and `assets` carries
    /// it: a second request for text that arrived with the first one is a
    /// second thing to time out.
    ///
    /// **This is network-supplied text and nothing in this crate treats it as
    /// anything else.** It is never mined for links to follow, never fetched
    /// from, and never handed to a shell or a process. Its one consumer is the
    /// About page, which passes it through [`release_notes_blocks`] -- the
    /// strip, then the bounded Markdown subset above -- and paints styled text
    /// that no click can activate. Empty when no release in the range has a
    /// body: a release with no notes is normal, not an error.
    pub body: String,
}

/// The longest run of release notes the About page will render.
///
/// A GitHub release body has no length limit this crate can rely on, and the
/// About page's notes region lives inside a fixed-size window. That region
/// scrolls, so length alone cannot push a control off the page -- but an
/// unbounded string is still an unbounded per-frame layout cost, so it is cut
/// here as well.
pub const MAX_RELEASE_NOTES_CHARS: usize = 4000;

/// Appended when [`release_notes_for_display`] cuts, so a truncated read looks
/// truncated rather than looking like the notes simply ended.
const NOTES_TRUNCATION_MARK: &str = "\n[...]";

/// [`ReleaseInfo::body`] reduced to something safe to paint.
///
/// Three jobs, all of them about the string being *data*:
///
/// 1. **Control characters go.** A release body is UTF-8 from a web API and
///    may contain anything encodable. `\r\n` is normalised to `\n` and every
///    other control character -- including the bare `\r` a lone carriage
///    return would leave, and any bidirectional-override or zero-width
///    formatting character that could make the painted text disagree with the
///    bytes -- is dropped rather than handed to a text layout engine.
/// 2. **Length is bounded** to [`MAX_RELEASE_NOTES_CHARS`], cut on a `char`
///    boundary. Never a byte index: `&s[..n]` on a multi-byte codepoint
///    panics, and this string is chosen by a remote host.
/// 3. **Nothing is interpreted.** No markdown, no autolinking, no HTML, no
///    clickable anything. What comes back is a plain string and the caller
///    paints it as one.
pub fn release_notes_for_display(body: &str) -> String {
    let cleaned: String = body
        .replace("\r\n", "\n")
        .chars()
        .filter(|c| *c == '\n' || (!c.is_control() && !is_invisible_formatting(*c)))
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.chars().count() <= MAX_RELEASE_NOTES_CHARS {
        return trimmed.to_string();
    }
    let mut out: String = trimmed.chars().take(MAX_RELEASE_NOTES_CHARS).collect();
    out.push_str(NOTES_TRUNCATION_MARK);
    out
}

// --- the Markdown subset ---------------------------------------------------
//
// **What is in, what is out, and why the line is where it is.**
//
// A GitHub release body is Markdown, and painting it as literal characters
// meant `**` and `-` and `#` on screen as themselves: the notes read as a
// diff of a document rather than as the document. This renders a subset, and
// the subset is bounded deliberately rather than by what a parser happened to
// support.
//
// IN: headings (`#`..`######`), bullet lists (`-`/`*`/`+`, one nesting level
// per two leading spaces), bold (`**`/`__`), italic (`*`/`_`), inline code
// (`` ` ``), backslash escapes, and an `https` link -- its words, its
// destination shown beside them as text, and a click that opens it.
//
// # Links were excluded, and are no longer. What changed, and what did not
//
// **The original rule was that nothing here is clickable**, and the argument
// for it was this: every other thing in this subset can only make text LOOK
// wrong -- egui executes nothing, so a misparsed emphasis is a cosmetic
// defect -- while a link is the single element that turns styling into a
// place the user can be SENT, on the one page whose job is telling them what
// they are about to download and run, from text supplied by whoever published
// the release. Showing the destination as text was held to keep the
// information without the one-click path.
//
// That rule has been **reversed by the owner's decision**, and this paragraph
// exists so the next reader finds the current reasoning rather than the old
// one. The reversal is deliberate and its cost is understood: a release body
// can now put one click between a reader and an arbitrary `https` address.
//
// What survives the reversal, because none of it costs anything:
//
//  * **`https` only.** Any other scheme -- `http`, `file:`, `ms-settings:`,
//    `javascript:` -- is not a link at all here. Its words are painted as
//    ordinary text with the destination beside them, exactly as every link
//    was painted before. This is decided HERE, at the parse, rather than at
//    the paint: a refused URL never reaches a [`NotesSpan`], so there is no
//    downstream code that could be talked into opening one.
//  * **The destination stays visible.** The ` (url)` run beside the words is
//    unchanged. A user can see where a link goes without clicking it, which
//    is the property a clickable link that hid its URL would take away.
//  * **The words look like what they do.** A refused link is styled plain,
//    not blue and underlined, so nothing on this page is painted as a link
//    that will not act as one.
//  * **One way to open a URL.** The renderer opens through
//    `vault_window::webbrowser_open`, which goes via `ShellExecuteW` rather
//    than through `cmd.exe`, and which re-checks the scheme itself. There is
//    no second opener in this crate and this change did not add one.
//  * **[`release_notes_for_display`] is untouched**, so the sanitisation
//    below still runs before any of this.
//
// OUT, and each for its own reason:
//
//  * **Images.** `![alt](url)` renders its alt text and drops the URL: an
//    image is a network fetch, and this page makes no request it was not
//    asked for.
//  * **Raw HTML.** Not parsed, and so painted as the characters it is.
//  * **Tables, block quotes, ordered lists, nested emphasis, and multi-line
//    fenced code.** Not because they are dangerous but because each is more
//    parser, and everything unrecognised falls through to being painted as
//    plain text -- which is the old behaviour, and a perfectly good floor.
//
// [`release_notes_for_display`] still runs FIRST and unchanged: control
// characters, bidi overrides, zero-widths and the length bound are all
// applied before a single markup character is looked at. Nothing below can
// reintroduce them, because nothing below invents text -- every span's
// content is a slice of that already-cleaned string.

/// How one run of release-note text is painted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotesStyle {
    Plain,
    /// `**bold**` or `__bold__`.
    Strong,
    /// `*italic*` or `_italic_`.
    Emphasis,
    /// `` `inline code` ``.
    Code,
    /// The words of an `https` link. Styled as a link, and carrying the
    /// destination in [`NotesSpan::link`] -- this is the only style that
    /// ever does. A link the subset refused is [`NotesStyle::Plain`] with no
    /// destination attached, so "looks like a link" and "acts like a link"
    /// cannot come apart. See the section header above.
    LinkText,
    /// A link's destination, shown as text beside its words so the user can
    /// see and copy where it goes -- refused links included, which is how a
    /// refusal stays legible rather than looking like a dropped URL.
    LinkUrl,
}

/// One styled run of text within a line.
///
/// **`link` is the whole of the clickability decision.** It is `Some` only
/// on a [`NotesStyle::LinkText`] span whose destination the subset accepted
/// (`https`, and nothing else), and it holds the URL a renderer may open. A
/// renderer needs no policy of its own: a span with no `link` has nothing to
/// open, and a span with one has already been through the only check there
/// is. See the section header for what that check is and why the exclusion
/// it replaced was lifted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotesSpan {
    pub text: String,
    pub style: NotesStyle,
    /// Where this run leads when it is clicked, or `None` -- which is every
    /// span but an accepted link's words.
    pub link: Option<String>,
}

/// One span, spelled once.
///
/// A free function rather than an `impl NotesSpan { fn new(..) -> Self }`,
/// because `production_is_the_only_updater_env_a_shipping_build_has` counts
/// `-> Self` over this module's production code to prove `UpdaterEnv` has one
/// constructor. A convenience constructor for an unrelated type would spend
/// that budget and read as the guard failing for the reason it exists to
/// catch.
fn span(text: impl Into<String>, style: NotesStyle) -> NotesSpan {
    NotesSpan { text: text.into(), style, link: None }
}

/// The words of an accepted link, and where they lead.
///
/// The one constructor that produces a span a renderer may click, kept
/// beside [`span`] so the two are read together: everything else in this
/// parser goes through `span` and is therefore inert by construction.
fn link_span(text: impl Into<String>, url: String) -> NotesSpan {
    NotesSpan { text: text.into(), style: NotesStyle::LinkText, link: Some(url) }
}

/// The destination of a link the subset will let a reader follow, or `None`.
///
/// **One match on the scheme, and `https` is the whole of it.** A release
/// body is remote text on the page that says what is about to be installed;
/// `http` is refused alongside `file:` and `ms-settings:` not because it is
/// equally dangerous but because there is no release note that needs it, and
/// a rule with one arm cannot be misread. Everything refused is still shown
/// -- as words and a visible URL, which is how every link on this page used
/// to be shown.
///
/// Trimmed before checking and returned trimmed, so the string that was
/// judged is the string that would be opened; `webbrowser_open` makes the
/// same pairing for the same reason.
///
/// `https:/` and `https://` with nothing after it are refused: a scheme with
/// no host is not a destination, and offering a click that opens a blank is
/// worse than painting the text.
fn https_link(url: &str) -> Option<String> {
    const SCHEME: &str = "https://";
    let trimmed = url.trim();
    // `get`, not a slice: a URL can begin mid-character and indexing one
    // would panic where refusing it is the right answer anyway.
    let scheme = trimmed.get(..SCHEME.len())?;
    if !scheme.eq_ignore_ascii_case(SCHEME) || trimmed.len() == SCHEME.len() {
        return None;
    }
    Some(trimmed.to_string())
}

/// One line of release notes, and what kind of line it is.
///
/// **One block per SOURCE line, and blank lines kept as blocks of their own.**
/// This is the half of the subset that answers "new lines seems like [gone]":
/// a release body is a bulleted list under headings, and a renderer that
/// joined its lines into flowing paragraphs -- which is what Markdown proper
/// says to do -- would turn it into a wall. GitHub renders release bodies with
/// hard breaks on, so line-per-block is also what the author saw when they
/// wrote it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotesBlock {
    /// An empty source line: the paragraph break, kept because the gap
    /// between two sections is information.
    Blank,
    /// `#`-prefixed. `level` is 1..=6.
    Heading { level: u8, spans: Vec<NotesSpan> },
    /// A `-`/`*`/`+` list item. `depth` is one per two leading spaces.
    Bullet { depth: usize, spans: Vec<NotesSpan> },
    /// Anything else, including every construct outside the subset.
    Paragraph { spans: Vec<NotesSpan> },
}

/// The deepest bullet indent that changes the painted inset.
///
/// Bounded because the indent is multiplied into a layout measurement and the
/// string is chosen by a remote host: without this, a line of four hundred
/// spaces before a `-` would push its text off the card.
pub const MAX_BULLET_DEPTH: usize = 3;

/// [`ReleaseInfo::body`], sanitised and then parsed into the subset above.
///
/// **The single entry point the About page uses**, so the sanitisation and
/// the parse cannot be reached separately or in the wrong order.
///
/// Total: every branch below falls back to painting the characters as they
/// are. Malformed markup -- an unclosed `**`, a `[` with no `]`, a bracket
/// pair with no parentheses after it -- is not an error, is never dropped,
/// and cannot panic; it is text.
pub fn release_notes_blocks(body: &str) -> Vec<NotesBlock> {
    release_notes_for_display(body)
        .lines()
        .map(parse_notes_line)
        .collect()
}

/// One source line to one [`NotesBlock`].
fn parse_notes_line(line: &str) -> NotesBlock {
    // A hard line break in Markdown is two trailing spaces. Every line is
    // already its own block here, so the break is honoured by construction
    // and the spaces themselves are just trailing whitespace.
    let trimmed_end = line.trim_end();
    if trimmed_end.trim_start().is_empty() {
        return NotesBlock::Blank;
    }

    let indent = trimmed_end.chars().take_while(|c| *c == ' ').count();
    let rest = &trimmed_end[indent..];

    if let Some(after_hashes) = rest.strip_prefix('#') {
        let extra = after_hashes.chars().take_while(|c| *c == '#').count();
        let level = 1 + extra;
        let body = &after_hashes[extra..];
        // A space after the hashes is required, as GitHub requires it --
        // otherwise `#1234` (an issue number, which release notes are full
        // of) would become a heading reading "1234".
        if level <= 6 && body.starts_with(' ') {
            return NotesBlock::Heading {
                level: level as u8,
                spans: inline_spans(body.trim_start()),
            };
        }
    }

    for marker in ['-', '*', '+'] {
        if let Some(body) = rest.strip_prefix(marker) {
            if body.starts_with(' ') {
                return NotesBlock::Bullet {
                    depth: (indent / 2).min(MAX_BULLET_DEPTH),
                    spans: inline_spans(body.trim_start()),
                };
            }
        }
    }

    NotesBlock::Paragraph { spans: inline_spans(rest) }
}

/// One line's inline markup to styled runs.
///
/// **Flat, not recursive, and that is a decision rather than a limitation.**
/// The contents of a `**bold**` run are not parsed again, so `**a *b* c**` is
/// bold text containing literal asterisks. Recursion here would need a depth
/// bound, because the input is chosen by a remote host and nesting is free to
/// write; a parser with no stack cannot be given one to overflow. The cost is
/// a construct nobody puts in a release note rendering slightly plainly.
///
/// **One exception, and it is not recursion.** An emphasis run whose ENTIRE
/// contents are a single link is read as that link -- `**[Full changelog](
/// ...)**`, which is the shape `.github/workflows/release.yml` composes and
/// which otherwise painted its own square brackets in bold on the one line of
/// the panel most worth following. It costs one `parse_link` call on a slice,
/// that call looks at no emphasis of its own, and so the depth this reaches
/// cannot grow with the input. Everything else inside an emphasis run is
/// still literal.
fn inline_spans(line: &str) -> Vec<NotesSpan> {
    let chars: Vec<char> = line.chars().collect();
    let mut spans: Vec<NotesSpan> = Vec::new();
    let mut plain = String::new();
    let mut i = 0usize;

    while i < chars.len() {
        let c = chars[i];

        // A backslash escapes the next character, whatever it is, so a
        // release note can say `**` and mean it.
        if c == '\\' && i + 1 < chars.len() {
            plain.push(chars[i + 1]);
            i += 2;
            continue;
        }

        if c == '`' {
            if let Some(close) = find_char(&chars, i + 1, '`') {
                flush(&mut spans, &mut plain);
                spans.push(span(
                    chars[i + 1..close].iter().collect::<String>(),
                    NotesStyle::Code,
                ));
                i = close + 1;
                continue;
            }
        }

        // An image is recognised only to be STRIPPED: its alt text is kept
        // as ordinary words and its URL is dropped, because rendering it
        // would mean fetching it.
        if c == '!' && chars.get(i + 1) == Some(&'[') {
            if let Some((text, _url, next)) = parse_link(&chars, i + 1) {
                flush(&mut spans, &mut plain);
                if !text.is_empty() {
                    spans.push(span(text, NotesStyle::Plain));
                }
                i = next;
                continue;
            }
        }

        if c == '[' {
            if let Some((text, url, next)) = parse_link(&chars, i) {
                flush(&mut spans, &mut plain);
                push_link(&mut spans, text, url);
                i = next;
                continue;
            }
        }

        if c == '*' || c == '_' {
            let doubled = chars.get(i + 1) == Some(&c);
            let run = if doubled { 2 } else { 1 };
            if let Some(close) = find_run(&chars, i + run, c, run) {
                let inner: String = chars[i + run..close].iter().collect();
                // An empty run (`****`) is not emphasis of nothing; it is
                // four asterisks somebody typed.
                if !inner.is_empty() {
                    flush(&mut spans, &mut plain);
                    // **The one thing looked at inside an emphasis run, and
                    // it is not recursion.** The flat rule above stands:
                    // `**a *b* c**` is still bold text containing literal
                    // asterisks. What is handled here is the single case
                    // where the run's ENTIRE contents are one link, which is
                    // `**[Full changelog](...)**` -- the shape the release
                    // workflow composes and the one line on the panel most
                    // worth being able to follow. Without this it rendered as
                    // its own square brackets, in bold.
                    //
                    // Bounded, and cannot become a stack: `parse_link` is
                    // called once, on a slice, and looks at no emphasis of
                    // its own. Depth cannot grow with the input.
                    //
                    // The bold is dropped rather than combined, because the
                    // subset has no bold-link style and inventing one to say
                    // "important" over text that already says "follow me"
                    // buys nothing.
                    let inner_chars = &chars[i + run..close];
                    match parse_link(inner_chars, 0) {
                        Some((text, url, next)) if next == inner_chars.len() => {
                            push_link(&mut spans, text, url);
                        }
                        _ => spans.push(span(
                            inner,
                            if doubled { NotesStyle::Strong } else { NotesStyle::Emphasis },
                        )),
                    }
                    i = close + run;
                    continue;
                }
            }
        }

        plain.push(c);
        i += 1;
    }

    flush(&mut spans, &mut plain);
    spans
}

/// Appends one link's words and its visible destination.
///
/// The words, then the destination, and the destination is painted either
/// way. A link the subset accepts carries its URL and can be clicked; one it
/// refuses is painted as plain words beside the same visible URL, which is
/// exactly how every link on this page looked before links could be followed
/// at all. [`https_link`] is the only thing that decides, and it decides here
/// -- at the parse -- so a refused URL is never carried any further.
///
/// Spelled once because there are two places a link is recognised: on its own,
/// and as the entire contents of an emphasis run (see [`inline_spans`]). Two
/// copies of this would be two copies of the scheme rule, and the copy that
/// drifts is the one that forgets to check.
fn push_link(spans: &mut Vec<NotesSpan>, text: String, url: String) {
    let words = if text.is_empty() { url.clone() } else { text };
    spans.push(match https_link(&url) {
        Some(target) => link_span(words, target),
        None => span(words, NotesStyle::Plain),
    });
    spans.push(span(format!(" ({url})"), NotesStyle::LinkUrl));
}

/// Moves whatever has accumulated as unstyled text into a span.
fn flush(spans: &mut Vec<NotesSpan>, plain: &mut String) {
    if !plain.is_empty() {
        spans.push(span(std::mem::take(plain), NotesStyle::Plain));
    }
}

/// Index of the next `needle` at or after `from`, or `None`.
fn find_char(chars: &[char], from: usize, needle: char) -> Option<usize> {
    (from..chars.len()).find(|&i| chars[i] == needle)
}

/// Index of the next run of exactly `len` `needle`s at or after `from`.
///
/// "Exactly" matters for the single-character case: the `**` closing a bold
/// run must not be mistaken for the `*` closing an italic one.
fn find_run(chars: &[char], from: usize, needle: char, len: usize) -> Option<usize> {
    let mut i = from;
    while i < chars.len() {
        if chars[i] == needle {
            let run = chars[i..].iter().take_while(|c| **c == needle).count();
            if run == len {
                return Some(i);
            }
            i += run;
        } else {
            i += 1;
        }
    }
    None
}

/// Parses `[text](url)` starting at the `[`.
///
/// Returns the text, the destination, and the index just past the `)`.
/// `None` for anything that is not the whole shape, which is then painted as
/// the characters it is. Neither half may contain a newline (there are none
/// -- this runs per line) and the URL may not contain whitespace or a nested
/// bracket, which is what keeps a stray `(` in prose from swallowing a line.
fn parse_link(chars: &[char], open: usize) -> Option<(String, String, usize)> {
    if chars.get(open) != Some(&'[') {
        return None;
    }
    let close = find_char(chars, open + 1, ']')?;
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let end = find_char(chars, close + 2, ')')?;
    let url: String = chars[close + 2..end].iter().collect();
    if url.is_empty() || url.chars().any(|c| c.is_whitespace() || c == '(' || c == '[') {
        return None;
    }
    Some((chars[open + 1..close].iter().collect(), url, end + 1))
}

/// Characters that paint as nothing but change what the text around them
/// looks like, listed because `char::is_control` does not cover them: it is
/// about the C0/C1 ranges, and every character below is a Unicode *format*
/// character that sails straight through it.
///
/// These matter here for one specific reason. The notes are chosen by whoever
/// can publish a GitHub release, and a bidirectional override can make a
/// painted line read in the opposite order from the characters it is made of
/// -- so what the user sees and what the string says can be made to disagree,
/// on a page whose whole job is telling them what they are about to install.
/// Zero-width characters do the same trick by hiding joins. None of them have
/// any legitimate use in a release note, so they are dropped rather than
/// escaped.
fn is_invisible_formatting(c: char) -> bool {
    matches!(c,
        '\u{00ad}'                  // soft hyphen
        | '\u{061c}'                // arabic letter mark
        | '\u{200b}'..='\u{200f}'   // zero-width set, LRM, RLM
        | '\u{202a}'..='\u{202e}'   // bidi embeddings and overrides
        | '\u{2060}'..='\u{2064}'   // word joiner and the invisible operators
        | '\u{2066}'..='\u{206f}'   // bidi isolates and deprecated formatting
        | '\u{feff}'                // byte-order mark as a character
        | '\u{fff9}'..='\u{fffb}'   // interlinear annotation
    )
}

/// How long to wait for a TCP connection to establish before giving up.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Total-time bound for the releases-API check.
///
/// The response is one small JSON document, so total elapsed time is the right
/// shape here: anything near this number is a broken path, not a slow one. The
/// check runs on a background thread (`main.rs`), so a 30s wait costs the user
/// nothing but a late tray badge.
const API_DEADLINE: Duration = Duration::from_secs(30);

/// No-progress bound for the installer download.
///
/// The download is a ~6 MB stream and its legitimate duration is unknown -- it
/// depends entirely on the user's link -- so *total* time is the wrong thing
/// to bound. v0.3.0's first fix bounded it anyway, at 600s, and that number is
/// the proof: too tight to allow a genuinely slow download, too loose to be a
/// bound anyone benefits from, and it left the tray pinned on "Updating to vX"
/// for ten minutes with repeat clicks swallowed (`main.rs`) where a stalled
/// download used to fail in fifteen seconds.
///
/// So this bounds the gap *between* reads instead: 15s with no byte arriving
/// is a dead transfer at any link speed worth downloading over, while a
/// slow-but-steady stream runs as long as it needs to. Deliberately *not*
/// paired with a whole-request deadline -- see
/// [`crate::http_agent::bounded_stall`]; adding one is exactly what made this
/// setting inert before.
///
/// Not bounded here: a server that dribbles one byte every 14s forever. That
/// is indistinguishable from a very slow link by any time-based rule, and this
/// is the shape that says so rather than pretending otherwise.
const DOWNLOAD_STALL_TIMEOUT: Duration = Duration::from_secs(15);

/// Agent for [`check_for_update`]: a small JSON response, bounded by total
/// time.
///
/// Separate from [`build_download_agent`] because the two requests need
/// different *kinds* of bound and ureq 2.12.1 can express only one per agent.
/// They used to share one agent with both settings applied, which meant one of
/// the settings silently did nothing; see [`crate::http_agent`].
pub fn build_api_agent() -> crate::http_agent::TotalBounded {
    crate::http_agent::bounded_total(CONNECT_TIMEOUT, API_DEADLINE)
}

/// Agent for [`download_and_verify`]: a multi-megabyte stream, bounded by time
/// without progress. See [`build_api_agent`] for why this is its own agent.
pub fn build_download_agent() -> crate::http_agent::StallBounded {
    crate::http_agent::bounded_stall(CONNECT_TIMEOUT, DOWNLOAD_STALL_TIMEOUT)
}

/// How many releases one page of the list endpoint is asked for.
///
/// GitHub's maximum, and deliberately ONE page: `Link`-header pagination
/// would make the check an unbounded number of requests driven by a remote
/// host's `rel="next"`, on a background thread at startup, to answer a
/// question ("is there something newer") that the first page always answers
/// -- the endpoint returns releases newest first. A user more than this many
/// releases behind still gets the right installer and the newest hundred sets
/// of notes; what they lose is prose about versions from a year ago.
const RELEASES_PER_PAGE: usize = 100;

/// The most releases whose notes are joined into one panel.
///
/// Separate from [`RELEASES_PER_PAGE`] because it bounds a different thing:
/// not how much is fetched but how much is composed into a string that a
/// bounded-height region then lays out. `release_notes_for_display`'s character
/// bound is the backstop, and this keeps the work before that bound from
/// being a hundred bodies' worth of allocation for text nobody scrolls to.
const MAX_NOTES_RELEASES: usize = 20;

/// Checks for a newer release, and collects the notes of EVERY release the
/// user skipped.
///
/// # Why the list, not `/releases/latest`
///
/// A user on 0.8.1 updating to 0.8.4 was shown 0.8.4's notes and nothing
/// else, so two releases' worth of "what changed" was unreachable from the
/// page whose job is to say what changed. This reads the list and takes every
/// release newer than the running version, newest first.
///
/// # The artefact still comes from exactly ONE release
///
/// **This is the security-relevant half and it is deliberately narrow.** The
/// notes may be a union of several releases; the installer is not. The URL
/// and the SHA-256 both come from one asset object, of the one release with
/// the highest version -- the same single-object rule the old code had, moved
/// into [`installer_from`] so there is one place that reads a pair and no way
/// to read the halves from different releases. A list of releases must not
/// become ambiguity about which file is being verified and launched, so the
/// other releases in the range contribute prose and nothing else.
///
/// # What the list endpoint returns that `latest` did not
///
/// Drafts and prereleases. `latest` filtered them out server-side; this
/// filters them out here, before a version comparison is made, so neither can
/// be offered as an update nor contribute notes.
pub fn check_for_update(
    base_url: &str,
    current_version: &Version,
    agent: &crate::http_agent::TotalBounded,
) -> Result<Option<ReleaseInfo>, String> {
    let url = format!(
        "{base_url}/repos/denis-platonov/deskwarden/releases?per_page={RELEASES_PER_PAGE}"
    );
    let body: serde_json::Value = agent
        .get(&url)
        .call()
        .map_err(|e| format!("failed to reach GitHub releases API: {e}"))?
        .into_json()
        .map_err(|e| format!("failed to parse releases response: {e}"))?;

    let listed = body
        .as_array()
        .ok_or("releases response is not a list of releases")?;

    // Every newer, published release, newest first.
    //
    // A tag that is not semver is SKIPPED rather than fatal, which is the one
    // deliberate loosening against the old code. `latest` returned a single
    // release, so a malformed tag there meant the check had no answer at all;
    // in a list of a hundred, one odd historical tag would otherwise take the
    // whole update down with it.
    let mut newer: Vec<(Version, &serde_json::Value)> = listed
        .iter()
        .filter(|r| !r["draft"].as_bool().unwrap_or(false))
        .filter(|r| !r["prerelease"].as_bool().unwrap_or(false))
        .filter_map(|r| Some((release_version(r)?, r)))
        .filter(|(v, _)| v > current_version)
        .collect();
    newer.sort_by(|a, b| b.0.cmp(&a.0));

    // Nothing newer: either this build is current, or it is a local build
    // whose version is ahead of everything published. Both are "no update",
    // and both were before -- a local build simply finds nothing above it in
    // the list rather than nothing above it in one release.
    let Some((newest_version, newest)) = newer.first() else {
        return Ok(None);
    };

    let (installer_url, installer_sha256) = installer_from(newest)?;

    Ok(Some(ReleaseInfo {
        version: newest_version.clone(),
        installer_download_url: installer_url,
        installer_sha256,
        body: notes_across(&newer),
    }))
}

/// A release's version from its `tag_name`, or `None` for a tag this build
/// cannot read as a version. See [`check_for_update`] for why that is a skip.
fn release_version(release: &serde_json::Value) -> Option<Version> {
    let tag = release["tag_name"].as_str()?;
    Version::parse(tag.strip_prefix('v').unwrap_or(tag)).ok()
}

/// The installer asset's download URL and digest, read out of ONE asset
/// object of ONE release.
///
/// The asset is bound once and both fields are read from it. Deliberately not
/// two independent searches through `assets`: two searches can disagree about
/// which asset they found, and a digest taken from a different asset than the
/// URL is a check that passes on the wrong file or fails on the right one.
/// One object, one pair -- and, now that the caller has a list of releases in
/// hand, one release.
fn installer_from(release: &serde_json::Value) -> Result<(String, Sha256Digest), String> {
    let asset = release["assets"]
        .as_array()
        .and_then(|assets| {
            assets
                .iter()
                .find(|a| a["name"].as_str().map(|n| n.ends_with("-installer.exe")).unwrap_or(false))
        })
        .ok_or("release has no installer asset")?;

    let installer_url = asset["browser_download_url"]
        .as_str()
        .ok_or("release installer asset has no download URL")?
        .to_string();

    // Absent is an ERROR, unlike the notes. A release whose installer carries
    // no digest is one this build has no way to check, and the whole shape of
    // this module is that such a release is not offered rather than
    // offered-and-trusted. The message names the asset so the failure is
    // actionable rather than mysterious.
    let digest_field = asset["digest"].as_str().ok_or_else(|| {
        format!(
            "release installer asset '{}' carries no digest, so the download could not be \
             verified; refusing to offer this update",
            asset["name"].as_str().unwrap_or("<unnamed>")
        )
    })?;

    Ok((installer_url, parse_asset_digest(digest_field)?))
}

/// Heading a release's notes are filed under in the joined body.
fn notes_heading(version: &Version) -> String {
    format!("## Deskwarden {version}")
}

/// Stands in for a release in the range that published no notes.
///
/// **A version with nothing under it is not omitted.** The panel is claiming
/// to cover a RANGE, so a release that silently vanished from the list would
/// make the range a lie -- and "this one came with none" is a different fact
/// from "this one is not in your update", told here rather than left to the
/// reader to infer from a gap in the numbers.
const NO_NOTES_FOR_RELEASE: &str = "_This release came with no notes._";

/// Joins the notes of every skipped release, newest first, each under its own
/// version heading.
///
/// The heading is Markdown, which the About page renders (see the subset in
/// this module): the versions are the structure of this document and they
/// should read as headings rather than as a run-on of somebody else's
/// paragraphs.
///
/// Bounded twice over -- [`MAX_NOTES_RELEASES`] here, and
/// [`release_notes_for_display`]'s character cut at paint time -- and the cut
/// is named in the text rather than silent, because notes that stop without
/// saying so read as notes that ended.
fn notes_across(releases: &[(Version, &serde_json::Value)]) -> String {
    let mut out = String::new();
    for (version, release) in releases.iter().take(MAX_NOTES_RELEASES) {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&notes_heading(version));
        out.push('\n');
        // Absent, null, or a non-string are all one case: a release with no
        // notes. Not an error -- a release genuinely may have an empty body,
        // and failing over missing prose would turn "no notes" into "no
        // update".
        let notes = release["body"].as_str().unwrap_or_default().trim();
        out.push_str(if notes.is_empty() { NO_NOTES_FOR_RELEASE } else { notes });
    }
    if releases.len() > MAX_NOTES_RELEASES {
        out.push_str(&format!(
            "\n\n_And {} earlier releases, whose notes are on GitHub._",
            releases.len() - MAX_NOTES_RELEASES
        ));
    }
    out
}

/// File name a downloaded installer for `version` is stored under. Shared by
/// [`download_and_verify`] and [`cleanup_stale_downloads`] so the cleanup pass
/// can recognise exactly what the download pass writes, and nothing else.
fn installer_file_name(version: &Version) -> String {
    format!("deskwarden-{version}-installer.exe")
}

/// True for file names [`download_and_verify`] could have produced.
///
/// Matched by shape rather than by an exact version, because cleanup runs at
/// startup against leftovers from *previous* versions (whose numbers this
/// build has no list of). Anything else in the directory is left alone.
fn is_downloaded_installer(file_name: &str) -> bool {
    file_name.starts_with("deskwarden-") && file_name.ends_with("-installer.exe")
}

/// Deletes installers left behind in `dir` by earlier update attempts.
///
/// Called once at startup rather than after applying an update: `apply_update`
/// launches the installer and the app then exits immediately, so at that point
/// the file is the image of a *running* process and cannot be deleted. By the
/// next startup that installer has finished, so every downloaded installer
/// still sitting here is spent -- either it was applied (and this build is its
/// result) or the attempt failed -- and none of them are worth keeping.
///
/// Best-effort: a file that can't be removed (still locked by a slow
/// installer, say) is reported and skipped, never fatal. A missing directory
/// is success, not an error -- it just means nothing was ever downloaded.
pub fn cleanup_stale_downloads(dir: &Path) -> Result<usize, String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(format!("could not read {}: {e}", dir.display())),
    };

    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !is_downloaded_installer(name) {
            continue;
        }
        match std::fs::remove_file(entry.path()) {
            Ok(()) => removed += 1,
            Err(e) => log::warn!(
                "could not delete stale update download {}: {e}",
                entry.path().display()
            ),
        }
    }
    Ok(removed)
}

/// SHA-256 of the file at `path`, read as a stream.
///
/// **Streamed, in [`HASH_CHUNK`]-sized reads, rather than via
/// `std::fs::read`.** The installer is ~6 MB and this function runs twice per
/// update -- once at download time and once immediately before the launch --
/// so reading the whole thing into a `Vec` would mean two 6 MB allocations
/// that exist only to be fed forward a chunk at a time anyway.
///
/// An I/O error is an `Err`, never a zero hash or a partial one. That matters
/// more here than the usual amount: this function's return value is the only
/// thing standing between a file on disk and a process start, so a failure to
/// READ the file has to be as loud as a failure to MATCH it. Both callers
/// treat it as a refusal.
/// `pub(crate)` rather than private, for ONE reason: `bw_acquire` hashes the
/// Bitwarden CLI zip it just downloaded, and it must do it with this function
/// rather than a second one. A second hasher is a second set of decisions
/// about I/O errors, chunk sizes and what a partial read means -- and the one
/// decision that matters here (an unreadable file is an `Err`, never a zero
/// hash) is exactly the sort that a copy re-derives wrongly. Not `pub`: this
/// is not part of the crate's surface, only shared between two modules that
/// verify downloads.
pub(crate) fn file_sha256(path: &Path) -> Result<Sha256Digest, String> {
    let mut file = std::fs::File::open(path)
        .map_err(|e| format!("could not open {} to hash it: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; HASH_CHUNK];
    loop {
        let n = match std::io::Read::read(&mut file, &mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(format!("could not read {} to hash it: {e}", path.display())),
        };
        hasher.update(&buf[..n]);
    }
    Ok(Sha256Digest(hasher.finalize().into()))
}

/// Read size for [`file_sha256`]. The same 8 KiB `std::io::copy` uses, and the
/// same [`copy_reporting`] uses, for no deeper reason than that there is no
/// reason for this file to have two different opinions about a buffer size.
const HASH_CHUNK: usize = 8 * 1024;

/// Deletes an installer this module has just refused, and says so in the log.
///
/// Called from every refusal that happens with a file already on disk. **A
/// rejected installer must not be left in the download directory**: it is an
/// executable, in a predictable location, under the exact name
/// [`installer_file_name`] produces and [`apply_update_with`] reconstructs --
/// so leaving it there means a later run, or a user browsing the cache folder,
/// can run the very thing that was just judged unfit to run.
///
/// Best-effort, and deliberately does not turn a failed delete into a
/// different error: the caller is already returning a refusal, and that
/// refusal is the important half. A file that could not be deleted is logged
/// so it is not silent, and `cleanup_stale_downloads` sweeps it at the next
/// startup.
fn discard_rejected_installer(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => log::warn!("deleted rejected update download {}", path.display()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => log::warn!("could not delete rejected update download {}: {e}", path.display()),
    }
}

/// Streams the release installer to `dest_dir` and refuses anything whose
/// SHA-256 is not the one the releases API published for the asset.
///
/// The digest is taken from `release` -- i.e. from the API response that also
/// supplied the URL just downloaded from -- and never from a parameter. It
/// used to be a parameter: `expected_thumbprint: &str`, chosen by the caller,
/// which is the same weakness [`apply_update`] removed when it stopped taking
/// a path. A caller that picks the value a check is made against picks the
/// answer.
///
/// `agent` comes from [`build_download_agent`], and the type says so rather
/// than the comment: this is the one caller in the crate whose bound is "time
/// without progress" rather than "total time". That used to be prose only,
/// with both functions taking a bare `ureq::Agent`, so swapping the two
/// arguments at their `main.rs` call sites compiled -- and nothing tests
/// `main.rs`. The wrong direction is not cosmetic: [`build_api_agent`]'s 30s
/// *total* cap applied to a 6 MB stream aborts every legitimately slow
/// download, which is worse than the 600s cap this shape exists to remove.
///
/// `on_progress` is called with `(bytes_so_far, total_if_known)` as the stream
/// arrives. It exists because the About page reports this download to the user
/// who asked for it (see `update_panel.rs`), and a multi-megabyte transfer with
/// no visible progress is indistinguishable from a hung one. It is a `&dyn Fn`
/// rather than a channel so this function keeps knowing nothing about who is
/// watching; a caller that isn't watching passes [`NO_PROGRESS`].
///
/// **`on_progress` is called on the downloading thread, between reads.** It is
/// on the transfer's critical path, so anything expensive in it slows the
/// download; the About page's implementation is a `send` down an `mpsc`.
pub fn download_and_verify(
    release: &ReleaseInfo,
    dest_dir: &Path,
    agent: &crate::http_agent::StallBounded,
    on_progress: &dyn Fn(u64, Option<u64>),
) -> Result<PathBuf, String> {
    // Created here rather than assumed to exist: this is a dedicated cache
    // subdirectory (see `main.rs`), not the config directory the rest of the
    // app already creates at startup.
    std::fs::create_dir_all(dest_dir)
        .map_err(|e| format!("could not create {}: {e}", dest_dir.display()))?;
    let dest_path = dest_dir.join(installer_file_name(&release.version));

    let response = agent
        .get(&release.installer_download_url)
        .call()
        .map_err(|e| format!("failed to download installer: {e}"))?;
    // Advisory only. A server may omit it, and a chunked response has none at
    // all, so every consumer of this number has to cope with `None` -- which
    // is why it is passed on as an `Option` rather than being defaulted to
    // zero here and silently reported as "0 bytes total".
    let total = response
        .header("Content-Length")
        .and_then(|v| v.parse::<u64>().ok());
    let mut reader = response.into_reader();
    let mut file = std::fs::File::create(&dest_path).map_err(|e| e.to_string())?;
    // A stalled transfer aborts here, part-written, and the partial file must
    // not be left behind -- same cleanup the digest-failure branch below
    // does, for the same reason. `cleanup_stale_downloads` would eventually
    // catch it at the next startup, but this path stopped being near-
    // unreachable when the bound went from a 600s total to a 15s stall: a
    // flaky link now produces a partial installer every retry, and each one
    // sits in the cache directory until the app is next restarted.
    //
    // Hand-rolled rather than `std::io::copy` for one reason: `copy` cannot
    // say how far it has got, and the About page has to. The buffer is
    // `copy`'s own default size, and the failure handling is the same
    // remove-the-partial it always was.
    if let Err(e) = copy_reporting(&mut reader, &mut file, total, on_progress) {
        drop(file);
        let _ = std::fs::remove_file(&dest_path);
        return Err(e.to_string());
    }
    drop(file);

    // Both arms below delete the file, and that is the point of writing the
    // hash failure and the mismatch as two arms of one decision rather than as
    // a `?`: a hash that could not be COMPUTED is exactly as much a refusal as
    // a hash that did not MATCH, and neither may leave an executable behind.
    let actual = match file_sha256(&dest_path) {
        Ok(actual) => actual,
        Err(e) => {
            discard_rejected_installer(&dest_path);
            return Err(format!("downloaded installer could not be verified: {e}"));
        }
    };
    if actual != release.installer_sha256 {
        discard_rejected_installer(&dest_path);
        return Err(format!(
            "downloaded installer failed SHA-256 verification: the release published \
             {expected} and the downloaded file is {actual}",
            expected = release.installer_sha256
        ));
    }

    Ok(dest_path)
}

/// A progress sink for callers that aren't watching, so `download_and_verify`
/// never has to take an `Option` and branch on it per read.
pub const NO_PROGRESS: &dyn Fn(u64, Option<u64>) = &|_, _| {};

/// `std::io::copy` that also says how far it has got.
///
/// Its own function, over plain `Read`/`Write`, so the reporting contract is
/// testable without a socket: the tests below drive it from a `&[u8]` and
/// assert on what came out of the sink. What is pinned is the contract the
/// About page depends on -- **the final call always reports the total number
/// of bytes actually written** -- which is what stops a progress bar from
/// stopping at 97% on a transfer that finished.
///
/// `total` is passed through untouched rather than being reconciled against
/// what arrived. It is the server's claim; `copied` is the truth, and the
/// caller is shown both.
/// `pub(crate)` for `bw_acquire`, the second streamed download in this crate.
/// The contract pinned below -- **the final call always reports the total
/// number of bytes actually written** -- is what both progress bars depend
/// on, and it is cheaper to share the function than to pin the contract
/// twice.
pub(crate) fn copy_reporting(
    reader: &mut dyn std::io::Read,
    writer: &mut dyn std::io::Write,
    total: Option<u64>,
    on_progress: &dyn Fn(u64, Option<u64>),
) -> std::io::Result<u64> {
    let mut buf = vec![0u8; 8 * 1024];
    let mut copied: u64 = 0;
    on_progress(0, total);
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        writer.write_all(&buf[..n])?;
        copied += n as u64;
        on_progress(copied, total);
    }
    Ok(copied)
}

// **There is no signer thumbprint here any more, and no second placeholder
// standing in for one.**
//
// This is where `EXPECTED_SIGNER_THUMBPRINT` used to be, holding the literal
// string `PLACEHOLDER_SET_ONCE_SIGNPATH_CERT_ISSUED`, and where
// `installer_is_launchable` used to compare an Authenticode signature against
// it. Both are gone, and the reason is written here rather than in a commit
// message because the shape of the mistake is easy to re-create.
//
// The placeholder was chosen so it could never match, so that the module
// would fail closed until a real certificate existed. It did fail closed. It
// also failed closed on every genuine release, because the releases it was
// judging are unsigned by construction (`.github/workflows/release.yml`), so
// the gate refused 100% of real updates and 100% of hypothetical hostile
// ones alike. A gate with that discrimination is not a gate; it is an off
// switch on the feature, and its practical effect was to send the user to
// download and run the same unsigned installer by hand, with nothing checked
// at all.
//
// Three things could have replaced it, and two of them are the same bug
// again:
//
//  * **Keep verifying, but do not gate, and log the result.** Rejected. The
//    call would sit immediately before the spawn -- exactly where a reader
//    expects a gate -- and change nothing. That is the defect being fixed,
//    wearing a log line as a disguise.
//  * **Gate only when the constant is not the placeholder.** Rejected, and
//    it is the worst of the three: it makes the placeholder a SWITCH, so
//    whether this crate checks anything before starting a process becomes a
//    property of a string literal's value rather than of its code. Every
//    reader would have to know the current value to know what the code does.
//  * **Remove it, and gate on something that can actually pass.** Taken.
//    The launch gate is now `file_sha256` against
//    `ReleaseInfo::installer_sha256`, which is a check that succeeds on a
//    genuine release and fails on a substituted one -- the discrimination the
//    thumbprint gate never had.
//
// # What was NOT deleted
//
// `signature.rs` stays, whole and tested. It is not dead code: `main.rs`
// verifies the resolved `bw.exe` with `verify_authenticode` /
// `is_trusted_organization` / `dn_component`, which is a live trust decision
// on a different binary and is unaffected by any of this.
// `signature::is_trusted_signer` -- the pure predicate this module used to
// call -- also stays, with its own unit tests, because it is precisely what
// an Authenticode gate would need on the day a certificate is issued. The
// intent then is to add that gate ON TOP of the digest check, not in place
// of it: they answer different questions, and only one of them can be
// answered today.
//
// # The lesson from the old gate that DID survive the rewrite
//
// The doc comment removed from here also recorded a measured escape, which was
// worth keeping and has moved to `apply_update_with` -- onto the function that
// actually holds the gate, rather than sitting beside the deleted one.

/// The one place in this crate that turns the downloaded installer into a
/// running process.
///
/// Split out of [`apply_update`] so that the launch is a VALUE
/// ([`UpdaterEnv::launch`]) a test can substitute and observe, rather than a
/// statement no test may ever execute. It takes a path because it is the
/// bottom of the funnel, not the top: the path it is given is constructed by
/// [`apply_update_with`] from [`installer_file_name`], and reaching this
/// function without going through the gate above it is what
/// [`the_only_process_start_in_this_module_is_the_launch_seam`] forbids.
fn launch_installer(installer_path: &Path) -> Result<(), String> {
    // **Before the spawn, not after it.**
    //
    // `installer/deskwarden.iss` carries `AppMutex=`, so setup refuses to
    // install over a running Deskwarden and asks the user to close it first.
    // That is exactly right for a user who double-clicked the installer, and
    // exactly wrong here: this run is `/VERYSILENT /SUPPRESSMSGBOXES`,
    // started by the app itself, which is about to exit. There is no user to
    // ask and the prompt is suppressed, so the update would fail or hang on
    // whatever default that suppressed box carries.
    //
    // Releasing here is not a trick to dodge our own check -- it is the
    // accurate statement. At this instant this process has handed over to its
    // own replacement, so "a Deskwarden is running that setup must ask about"
    // has stopped being true. Everything the shutdown path does -- clearing
    // the clipboard, killing `bw serve`, zeroizing the cache -- still
    // happens: `main` reaches it as soon as this returns.
    crate::app_mutex::release();
    // `/MERGETASKS=!autostart` deselects the installer's autostart task for
    // THIS run only. A silent install applies the default task set, and that
    // task is checked by default (installer/deskwarden.iss), so without this
    // an update would write the `HKCU\...\Run` value back for a user who had
    // deliberately cleared it -- changing a system setting during an
    // unattended update, which is not this path's business either way.
    // Deselecting does not REMOVE an existing value, so a user who wants
    // autostart keeps it: an update leaves the setting exactly as it found
    // it, in both directions.
    Command::new(installer_path)
        .args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/MERGETASKS=!autostart"])
        .spawn()
        .map_err(|e| format!("failed to launch installer: {e}"))?;
    Ok(())
}

/// The two outside-world operations [`apply_update`] performs, as `fn`
/// pointers, so that the ROUTING between them can be tested.
///
/// # Why this exists
///
/// `apply_update` cannot be driven to its spawn by any test that is allowed to
/// exist in this crate: doing so would mean RUNNING the installer. So for three
/// revisions the trust decision was lifted into a pure predicate and pinned
/// over hand-built values, and the question "does the launcher still ask?" was
/// disclosed as untestable. It was not untestable. It was untested, and
/// measured survivors followed -- see [`apply_update_with`] for the numbers.
///
/// Behind this seam the launcher can be run end to end with **no real file and
/// no real process**: the harness answers `hash` by hand and records every
/// path that arrives at `launch`. The assertions are then about ROUTING --
/// that `launch` is NOT reached for a digest that differs from the release's,
/// nor for a file that could not be hashed at all, and IS reached, with
/// exactly the constructed path, for the matching one. That is the property
/// deletion and neutralisation both break and substitution also breaks, so one
/// shape of test covers all three.
///
/// The seam is `hash`, not "the whole check". The COMPARISON stays in
/// [`apply_update_with`], in production code, where no substitute can reach
/// it: a seam that returned a verdict rather than a digest would let a test
/// harness -- or anything else holding an env -- decide the answer, which is
/// the caller-chooses-the-check weakness this module has already removed twice
/// (see [`apply_update`] on why it takes no path, and [`download_and_verify`]
/// on why it takes no expected value). What crosses this boundary is a fact
/// about bytes; what stays inside is the decision made from it.
///
/// # `fn` pointers rather than `impl Fn`
///
/// Closures would be more convenient at the call site and would be the wrong
/// choice here, for the reason `VaultFrameEnv` in `vault_window/mod.rs`
/// records: a seam that is itself unpinned only MOVES the hole. A `fn` pointer
/// has an address, so [`production_holds_the_real_hash_and_the_real_launch`]
/// can assert that what [`UpdaterEnv::production`] hands over is the real
/// [`file_sha256`] and the real [`launch_installer`] BY IDENTITY, with
/// `std::ptr::fn_addr_eq`. A wrapper, a forwarder, a rename or a flag-gated
/// no-op is a different address and fails there, whatever it is spelled. That
/// matters most for `hash`: a substitute that returned a constant would make
/// the gate below always agree with itself.
pub struct UpdaterEnv {
    /// [`file_sha256`] in production -- the real bytes on disk, read again.
    hash: fn(&Path) -> Result<Sha256Digest, String>,
    /// [`launch_installer`] in production -- the module's only process start.
    launch: fn(&Path) -> Result<(), String>,
}

impl UpdaterEnv {
    /// The real world. The only constructor a shipping build compiles --
    /// pinned by [`production_is_the_only_updater_env_a_shipping_build_has`],
    /// which reads this file's production slice. The test-only substitute is
    /// written down in `mod tests` as an inherent impl, deliberately BELOW
    /// every source guard in this file, so that a test-gated item up here
    /// cannot truncate the slice those guards read.
    pub fn production() -> Self {
        Self { hash: file_sha256, launch: launch_installer }
    }
}

/// [`apply_update`]'s whole body, with the outside world as a parameter.
///
/// The gate is the point: `launch` is unreachable except through the digest
/// comparison below, and there is no other path to a process start in this
/// module. See [`UpdaterEnv`] for why this shape exists and
/// [`the_only_process_start_in_this_module_is_the_launch_seam`] for the guard
/// that keeps a second, ungated spawn from being written beside it.
///
/// # Why the gate lives HERE and not in a pure predicate
///
/// This is the lesson the deleted signature gate paid for, and it applies
/// unchanged to the digest one. With the trust decision merely lifted into a
/// pure function and pinned over hand-built values, replacing the entire
/// gating block with
///
/// ```ignore
/// let _ok = the_decision(&thing);
/// ```
///
/// -- which uses the value, uses the function, and removes the gate -- SURVIVED
/// the whole suite at zero warnings; composed with a one-line forwarder in
/// `accounts.rs` it restored an arbitrary-directory, check-free process
/// launcher on the crate's public surface. Substitution was killed; deletion
/// and NEUTRALISATION were both free, because a pin on a pure decision cannot
/// see whether the decision is in a GATING POSITION.
///
/// So the digest comparison is not a predicate pinned in isolation. It is a
/// `!=` in this body, held by the routing tests over this function, which
/// assert that the launch seam is NOT REACHED for a differing digest or an
/// unreadable file and IS reached for a matching one.
fn apply_update_with(dest_dir: &Path, release: &ReleaseInfo, env: &UpdaterEnv) -> Result<(), String> {
    let installer_path = dest_dir.join(installer_file_name(&release.version));
    // The path is folded into every error on this path deliberately: the one
    // thing a caller must be able to see is WHICH file was about to be
    // launched, and the hasher's own errors are about a read syscall rather
    // than about the update.
    //
    // Both failure arms below delete the file. A refusal that left the
    // installer sitting under its predictable name would leave the rejected
    // executable one double-click -- or one later run of this same function --
    // away from being run anyway, which is most of the way back to having no
    // gate at all.
    let actual = match (env.hash)(&installer_path) {
        Ok(actual) => actual,
        Err(e) => {
            discard_rejected_installer(&installer_path);
            return Err(format!("refusing to launch {}: {e}", installer_path.display()));
        }
    };
    if actual != release.installer_sha256 {
        discard_rejected_installer(&installer_path);
        return Err(format!(
            "refusing to launch {path}: its SHA-256 is {actual}, but the release published \
             {expected}",
            path = installer_path.display(),
            expected = release.installer_sha256
        ));
    }
    (env.launch)(&installer_path)
}

/// Launches the installer this module downloaded for `release`, and nothing
/// else.
///
/// # Why this does not take a path
///
/// It used to: `pub fn apply_update(installer_path: &Path)`, which spawned
/// whatever it was handed, with no job object and no further checks. That made
/// the updater a general-purpose, arbitrary-path process launcher standing
/// `pub` on the crate's surface -- and `updater.rs` is on the child-start
/// guard's `ALLOWED` list, so the guard reads none of it. Measured: a one-line
/// `pub fn zz_start(p: &Path) { crate::updater::apply_update(p) }` in
/// `accounts.rs` SURVIVED the whole suite at 2164 lib / 217 bin / 0 failed /
/// 0 warnings. A call to an existing `pub fn` is a QUIETER edit than the alias
/// lines the guard does catch, not a louder one.
///
/// So the capability is removed rather than guarded. The updater is the thing
/// that did the downloading; it knows where it wrote and under what name, so
/// it reconstructs the path from [`installer_file_name`] -- the same function
/// [`download_and_verify`] wrote it with and [`cleanup_stale_downloads`]
/// recognises it by. A caller chooses a directory and a version; it does not
/// choose a file.
///
/// # And it re-hashes
///
/// Naming the file is not on its own enough -- a caller can still name a
/// directory, and a directory is a place a file can be planted. So the digest
/// check is repeated here, immediately before the spawn, against
/// [`ReleaseInfo::installer_sha256`] rather than against anything the caller
/// supplied. [`download_and_verify`] already checks, but it checks at download
/// time and hands back a path; the gap between those two moments is exactly
/// where a swap goes. This is the check that is adjacent to the launch, and it
/// re-reads the bytes rather than remembering a verdict from earlier -- a
/// remembered verdict describes the file that WAS there.
///
/// The cost is one extra pass over ~6 MB, streamed (see [`file_sha256`]), on a
/// path that is about to start an installer and exit the process. That is not
/// a price worth optimising away by trusting the earlier answer.
///
/// # And the check is known to be CONSULTED
///
/// The body lives in [`apply_update_with`], over an [`UpdaterEnv`], and this is
/// a two-line wrapper over it holding production's env. That is not a
/// refactoring for its own sake: with the trust decision merely lifted into a
/// pure predicate and pinned there, neutralising the gate to `let _ok =
/// the_predicate(&thing);` was measured surviving the entire suite at zero
/// warnings. The routing tests behind the seam are what fail on that now.
pub fn apply_update(dest_dir: &Path, release: &ReleaseInfo) -> Result<(), String> {
    apply_update_with(dest_dir, release, &UpdaterEnv::production())
}

#[cfg(test)]
mod tests {
    use super::*;
    use semver::Version;

    /// The path [`check_for_update`] asks for, spelled once.
    ///
    /// The LIST endpoint, not `/releases/latest`: the panel shows every
    /// release the user skipped, so one release's worth of response is no
    /// longer what the check reads. The mock matches the path and leaves the
    /// query to `Matcher::Any`, so the `?per_page=` is not restated here.
    const RELEASES_PATH: &str = "/repos/denis-platonov/deskwarden/releases";

    /// Wraps a one-release fixture in the array the list endpoint returns.
    ///
    /// The fixtures below are still written as single releases, because what
    /// most of them are about -- which asset the digest came from, what a
    /// malformed digest does, what an absent one does -- is a property of ONE
    /// release and reads better as one. The tests that are about the range
    /// build their own arrays.
    fn release_list(release: &str) -> String {
        format!("[{release}]")
    }

    #[test]
    fn reports_a_newer_release_as_available() {
        let mut server = crate::test_http::server();
        let body = r#"{
            "tag_name": "v1.2.0",
            "assets": [
                {"name": "deskwarden-installer.exe", "browser_download_url": "https://example.com/deskwarden-installer.exe", "digest": "sha256:1111111111111111111111111111111111111111111111111111111111111111"}
            ]
        }"#;
        let _m = server
            .mock("GET", RELEASES_PATH)
            .match_query(crate::test_http::Matcher::Any)
            .with_status(200)
            .with_body(release_list(body))
            .create();

        let current = Version::parse("1.1.0").unwrap();
        let agent = build_api_agent();
        let result = check_for_update(&server.url(), &current, &agent).unwrap();

        let release = result.expect("expected an available update");
        assert_eq!(release.version, Version::parse("1.2.0").unwrap());
        assert_eq!(release.installer_download_url, "https://example.com/deskwarden-installer.exe");
        assert_eq!(
            release.installer_sha256,
            digest_of_byte("11"),
            "the digest the asset published did not come back with the URL it describes"
        );
        assert_eq!(
            release.body,
            format!("{}\n{NO_NOTES_FOR_RELEASE}", notes_heading(&release.version)),
            "a release whose JSON carries no `body` must be named and said to have none, \
             rather than dropped out of a range the panel claims to cover"
        );
    }

    /// **The release notes come out of the check that was already being made.**
    ///
    /// The About page renders `ReleaseInfo::body`, and the whole reason it is a
    /// field rather than a second call is that this one response already
    /// carries it. If the parse ever stops reading it, the page shows a release
    /// with no notes and looks merely like a release that has none -- which is
    /// a real state, so nothing else would fail. Hence a test that names the
    /// exact prose the mock served.
    #[test]
    fn the_release_notes_come_back_with_the_version_from_the_one_response() {
        let mut server = crate::test_http::server();
        let body = r#"{
            "tag_name": "v1.2.0",
            "body": "Fixed\n- the overlay no longer lies",
            "assets": [
                {"name": "deskwarden-installer.exe", "browser_download_url": "https://example.com/deskwarden-installer.exe", "digest": "sha256:1111111111111111111111111111111111111111111111111111111111111111"}
            ]
        }"#;
        let _m = server
            .mock("GET", RELEASES_PATH)
            .match_query(crate::test_http::Matcher::Any)
            .with_status(200)
            .with_body(release_list(body))
            .create();

        let current = Version::parse("1.1.0").unwrap();
        let release = check_for_update(&server.url(), &current, &build_api_agent())
            .unwrap()
            .expect("expected an available update");

        assert_eq!(
            release.body,
            "## Deskwarden 1.2.0\nFixed\n- the overlay no longer lies",
            "the notes must be the release's own prose, under a heading naming which \
             release they belong to"
        );
    }

    /// The notes are DATA. Everything markup-shaped in them survives this
    /// step verbatim -- this function interprets nothing, and is the gate
    /// [`release_notes_blocks`] runs BEFORE it looks at a markup character --
    /// and everything control-shaped in them does not, because a text layout
    /// engine is the last place a remote host's control characters should
    /// arrive.
    #[test]
    fn release_notes_are_stripped_of_control_characters_and_never_interpreted() {
        let hostile = "line one\r\n<script>alert(1)</script>\r\n[click](http://evil.example)\u{202e}\u{200b}\u{0007}end";
        let shown = release_notes_for_display(hostile);

        assert!(
            !shown.contains('\r')
                && !shown.contains('\u{202e}')
                && !shown.contains('\u{200b}')
                && !shown.contains('\u{0007}'),
            "a control or bidi-override character reached the painter: {shown:?}"
        );
        assert!(
            shown.contains("<script>alert(1)</script>")
                && shown.contains("[click](http://evil.example)"),
            "markup and link syntax must survive UNINTERPRETED, as the literal text it is: \
             {shown:?}"
        );
        assert_eq!(shown.lines().count(), 3, "newlines are the one control kept: {shown:?}");
    }

    /// The bound exists so a release body chosen by a remote host cannot become
    /// an unbounded per-frame layout. Cut on a `char` boundary, deliberately: a
    /// byte slice through a multi-byte codepoint panics, and this string is not
    /// ours.
    #[test]
    fn release_notes_are_bounded_and_cut_on_a_char_boundary() {
        let huge = "\u{00e9}".repeat(MAX_RELEASE_NOTES_CHARS * 3);
        let shown = release_notes_for_display(&huge);

        assert_eq!(
            shown.chars().count(),
            MAX_RELEASE_NOTES_CHARS + NOTES_TRUNCATION_MARK.chars().count()
        );
        assert!(shown.ends_with(NOTES_TRUNCATION_MARK), "a cut read must look cut: {shown:?}");
        // The real proof that the cut respected codepoints: every character is
        // still the one that went in, so nothing was sliced in half.
        assert!(shown.trim_end_matches(NOTES_TRUNCATION_MARK).chars().all(|c| c == '\u{00e9}'));
    }

    // -- the range of skipped releases --------------------------------------

    /// One release object, with whatever installer digest byte is given.
    fn a_release_json(tag: &str, notes: &str, extra: &str) -> String {
        format!(
            r#"{{"tag_name": "{tag}", "body": "{notes}", {extra}
               "assets": [{{"name": "deskwarden-installer.exe",
                 "browser_download_url": "https://example.com/{tag}-installer.exe",
                 "digest": "sha256:{}"}}]}}"#,
            "1".repeat(64)
        )
    }

    /// Serves `releases` from the list endpoint and runs the check.
    fn check_against(releases: &[String], current: &str) -> Result<Option<ReleaseInfo>, String> {
        let mut server = crate::test_http::server();
        let _m = server
            .mock("GET", RELEASES_PATH)
            .match_query(crate::test_http::Matcher::Any)
            .with_status(200)
            .with_body(format!("[{}]", releases.join(",")))
            .create();

        check_for_update(&server.url(), &Version::parse(current).unwrap(), &build_api_agent())
    }

    /// **The reported defect: a user two releases behind saw one release's
    /// notes.**
    ///
    /// Newest first, because the first thing on screen should be the thing
    /// being installed, and every skipped release named -- the range is the
    /// answer to "what changed", and a range missing its middle is not one.
    #[test]
    fn the_notes_cover_every_release_the_user_skipped_newest_first() {
        let releases = [
            a_release_json("v0.8.4", "the newest thing", ""),
            a_release_json("v0.8.3", "the middle thing", ""),
            a_release_json("v0.8.2", "the oldest thing", ""),
            a_release_json("v0.8.1", "already installed", ""),
        ];

        let release = check_against(&releases, "0.8.1").unwrap().expect("an update");

        for expected in ["0.8.4", "0.8.3", "0.8.2", "the newest thing", "the oldest thing"] {
            assert!(release.body.contains(expected), "{expected:?} missing: {:?}", release.body);
        }
        assert!(
            !release.body.contains("already installed"),
            "a release the user is already running is not something they skipped: {:?}",
            release.body
        );
        let newest = release.body.find("0.8.4").unwrap();
        let oldest = release.body.find("0.8.2").unwrap();
        assert!(newest < oldest, "the range is not newest-first: {:?}", release.body);
    }

    /// **The artefact comes from ONE release, and it is the newest.**
    ///
    /// The whole risk in reading a LIST is that the file being verified and
    /// the file being launched stop being obviously the same one. The notes
    /// are a union; the URL and the digest are not, and they come from the
    /// same asset object of the same release. The fixture gives the older
    /// releases their own installer URLs precisely so a parse that took the
    /// wrong one is visible rather than coincidentally right.
    #[test]
    fn the_installer_and_its_digest_come_from_the_newest_release_alone() {
        let releases = [
            a_release_json("v0.8.2", "older", ""),
            a_release_json("v0.8.4", "newest", ""),
            a_release_json("v0.8.3", "middle", ""),
        ];

        let release = check_against(&releases, "0.8.1").unwrap().expect("an update");

        assert_eq!(release.version, Version::parse("0.8.4").unwrap());
        assert_eq!(
            release.installer_download_url, "https://example.com/v0.8.4-installer.exe",
            "the installer must be the newest release's, whatever order the list arrived in"
        );
    }

    /// **Drafts and prereleases are not updates.**
    ///
    /// `/releases/latest` filtered these out server-side and the list endpoint
    /// does not, so the filter had to be brought here -- and it runs before
    /// the version comparison, so neither can be offered NOR contribute a
    /// line of notes.
    #[test]
    fn drafts_and_prereleases_are_excluded_from_the_range() {
        let releases = [
            a_release_json("v0.9.0", "a draft", r#""draft": true,"#),
            a_release_json("v0.8.9", "a prerelease", r#""prerelease": true,"#),
            a_release_json("v0.8.4", "the real one", ""),
        ];

        let release = check_against(&releases, "0.8.1").unwrap().expect("an update");

        assert_eq!(release.version, Version::parse("0.8.4").unwrap());
        assert!(
            !release.body.contains("a draft") && !release.body.contains("a prerelease"),
            "an unpublished release contributed notes: {:?}",
            release.body
        );
    }

    /// A local build whose version is ahead of everything published finds
    /// nothing above it and reports no update -- the same answer it got from
    /// `/releases/latest`, reached by the same comparison.
    #[test]
    fn a_build_newer_than_anything_published_is_offered_nothing() {
        let releases = [a_release_json("v0.8.4", "shipped", "")];

        assert!(check_against(&releases, "0.9.0-dev").unwrap().is_none());
        assert!(check_against(&[], "0.8.1").unwrap().is_none(), "an empty list is no update");
    }

    /// **A release in the range with no notes is named, not skipped.**
    ///
    /// The panel claims to cover a range of versions. A release that vanished
    /// from it would make that claim false, and leave the reader to infer
    /// from a gap in the numbers what the page could simply say.
    #[test]
    fn a_release_with_no_notes_is_still_named_in_the_range() {
        let releases = [
            a_release_json("v0.8.4", "the newest thing", ""),
            a_release_json("v0.8.3", "", ""),
        ];

        let release = check_against(&releases, "0.8.2").unwrap().expect("an update");

        assert!(release.body.contains("0.8.3"), "got {:?}", release.body);
        assert!(release.body.contains(NO_NOTES_FOR_RELEASE), "got {:?}", release.body);
    }

    /// **A tag this build cannot read is skipped, not fatal.**
    ///
    /// The one deliberate loosening against the old code, and it is a
    /// consequence of reading a list: `latest` returned one release, so a
    /// malformed tag there meant the check had no answer. In a list of a
    /// hundred, one odd historical tag would otherwise take the whole update
    /// down with it.
    #[test]
    fn a_tag_that_is_not_a_version_is_skipped_rather_than_fatal() {
        let releases = [
            a_release_json("nightly", "not a version", ""),
            a_release_json("v0.8.4", "the real one", ""),
        ];

        let release = check_against(&releases, "0.8.1").unwrap().expect("an update");

        assert_eq!(release.version, Version::parse("0.8.4").unwrap());
    }

    /// **The joined notes are bounded before they are ever laid out.**
    ///
    /// A user a very long way behind is the case this pairs with -- it is
    /// also the case that legitimately wants the scrollbar -- but "every
    /// release you skipped" must not become an unbounded string built from a
    /// remote host's list. Bounded here at [`MAX_NOTES_RELEASES`], and the
    /// cut is NAMED: notes that stop without saying so read as notes that
    /// ended.
    #[test]
    fn a_very_long_range_is_cut_and_says_that_it_was() {
        let releases: Vec<String> = (1..=MAX_NOTES_RELEASES + 5)
            .rev()
            .map(|n| a_release_json(&format!("v0.{n}.0"), &format!("release {n}"), ""))
            .collect();

        let release = check_against(&releases, "0.0.1").unwrap().expect("an update");

        assert!(
            release.body.contains("earlier releases"),
            "the cut is silent, so the notes look like they simply ended: {:?}",
            release.body
        );
        assert!(
            !release.body.contains("release 1\n") && !release.body.contains("release 2\n"),
            "the oldest releases were not cut: {:?}",
            release.body
        );
    }

    /// **One page, and the request says so.**
    ///
    /// `Link`-header pagination would make a startup check an unbounded
    /// number of requests driven by a remote host's `rel="next"`. This pins
    /// that the check asks for one page of a stated size -- a mock that
    /// matches ONLY that query, so a request without it 501s rather than
    /// passing.
    #[test]
    fn the_check_asks_for_exactly_one_page_of_a_stated_size() {
        let mut server = crate::test_http::server();
        let _m = server
            .mock("GET", RELEASES_PATH)
            .match_query(crate::test_http::Matcher::UrlEncoded(
                "per_page".into(),
                RELEASES_PER_PAGE.to_string(),
            ))
            .with_status(200)
            .with_body(release_list(&a_release_json("v0.8.4", "shipped", "")))
            .create();

        let current = Version::parse("0.8.1").unwrap();
        let release = check_for_update(&server.url(), &current, &build_api_agent())
            .expect("the check did not ask for the page it says it asks for")
            .expect("an update");

        assert_eq!(release.version, Version::parse("0.8.4").unwrap());
    }

    // -- the Markdown subset ------------------------------------------------

    /// The spans of the one block a body parses to, for the tests that are
    /// about inline markup rather than about block structure.
    fn spans_of(body: &str) -> Vec<NotesSpan> {
        match release_notes_blocks(body).into_iter().next() {
            Some(NotesBlock::Paragraph { spans })
            | Some(NotesBlock::Heading { spans, .. })
            | Some(NotesBlock::Bullet { spans, .. }) => spans,
            other => panic!("expected one block with spans, got {other:?}"),
        }
    }

    /// Everything a set of spans would paint, markup characters excluded by
    /// construction -- if a delimiter is in here, it was not consumed.
    fn text_of(spans: &[NotesSpan]) -> String {
        spans.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn the_subset_recognises_headings_bullets_and_their_nesting() {
        let blocks = release_notes_blocks("## Fixed\n- one\n  - nested\nplain\n");

        assert!(matches!(blocks[0], NotesBlock::Heading { level: 2, .. }));
        assert!(matches!(blocks[1], NotesBlock::Bullet { depth: 0, .. }));
        assert!(matches!(blocks[2], NotesBlock::Bullet { depth: 1, .. }));
        assert!(matches!(blocks[3], NotesBlock::Paragraph { .. }));
    }

    /// **A bullet's indent is a remote host's number, so it is bounded.**
    ///
    /// The depth is multiplied into a layout inset by the page; four hundred
    /// leading spaces would otherwise push a line off the card.
    #[test]
    fn a_deeply_indented_bullet_stops_at_the_bounded_depth() {
        // Not the first line: `release_notes_for_display` trims the body, so
        // leading spaces on line one are gone before the parser sees them.
        let blocks = release_notes_blocks(&format!("top\n{}- far\n", " ".repeat(400)));

        assert!(
            matches!(blocks[1], NotesBlock::Bullet { depth: MAX_BULLET_DEPTH, .. }),
            "got {:?}",
            blocks[1]
        );
    }

    /// **Blank lines are kept as blocks**, because the gap between two
    /// sections of a release body is something its author put there. The page
    /// paints them as space; dropping them here is what would make the notes
    /// a wall.
    #[test]
    fn a_blank_line_survives_as_a_block_of_its_own() {
        let blocks = release_notes_blocks("alpha\n\nbeta");

        assert_eq!(blocks.len(), 3, "got {blocks:?}");
        assert!(matches!(blocks[1], NotesBlock::Blank));
    }

    /// Everything one block would paint, its markup excluded by construction.
    ///
    /// The counterpart of [`text_of`] for the tests that are about whole
    /// bodies rather than one line: a delimiter appearing in here was NOT
    /// consumed by the parser and would reach the card as itself.
    fn painted(block: &NotesBlock) -> String {
        match block {
            NotesBlock::Blank => String::new(),
            NotesBlock::Heading { spans, .. }
            | NotesBlock::Bullet { spans, .. }
            | NotesBlock::Paragraph { spans } => text_of(spans),
        }
    }

    /// The release body the Release workflow composes for the release that is
    /// actually out, character for character.
    ///
    /// **Not a fixture written to pass.** It is the output of the "Compose
    /// the release body" step in `.github/workflows/release.yml`, run against
    /// this repository's own `CHANGELOG.md` with `APP_VERSION=0.14.0` -- the
    /// same PowerShell, the same file, the same section. That step is not
    /// reachable from `cargo test`, so this is the join between the two: the
    /// workflow's job log prints the body it publishes, and it has to be this.
    ///
    /// `concat!` of one-line literals rather than a raw string, because
    /// `nothing_but_gated_test_modules_lives_below_the_guards_cut` reads
    /// column-0 lines below the cut and every line of a multi-line literal is
    /// at column 0.
    const COMPOSED_BODY_0_14_0: &str = concat!(
        "## What's new in 0.14.0\n",
        "\n",
        "### Keeping an encrypted copy on this PC no longer asks for anything\n",
        "\n",
        "Turning that setting on used to stop Deskwarden starting at all: it wanted Windows Hello every launch, and when the prompt did not appear it waited for it forever, with no window and nothing in the log.\n",
        "\n",
        "The copy is now protected the same way the key that unlocks your vault already is — kept on this PC, protected by Windows. **No prompt, ever.** A PC without Windows Hello can use the setting now, which it could not before.\n",
        "\n",
        "**What that means, plainly:** anyone who can run programs as you on this PC can read the copy, and so can someone who takes this disk and knows your Windows password. That is the same protection your vault key already had, and the same trade other password managers make when you tell them not to lock. When it locks is still yours to choose, under **Lock the vault when you step away**.\n",
        "\n",
        "The setting's description said it was protected by a TPM chip. That stopped being true and now says what actually gates the file.\n",
        "\n",
        "**Your existing copy is rebuilt once** on first launch after updating, because the old one cannot be opened with the new key. Nothing is lost — it is a cache.\n",
        "\n",
        "**[Full changelog](https://github.com/denis-platonov/deskwarden/blob/v0.14.0/CHANGELOG.md)**\n",
        "\n",
        "---\n",
        "\n",
        "This build is unsigned; the updater checks the installer's SHA-256 against the digest GitHub publishes. [Code-signing policy](https://github.com/denis-platonov/deskwarden/blob/v0.14.0/docs/code-signing-policy.md)\n",
    );

    /// **The panel paints the release's prose, and paints none of its
    /// markup.** The whole point of composing the body from the changelog's
    /// section rather than from its headings is that this is what a user
    /// reads instead of a link they have to follow.
    #[test]
    fn the_composed_release_body_arrives_as_prose_and_not_as_markup() {
        let blocks = release_notes_blocks(COMPOSED_BODY_0_14_0);
        let painted: Vec<String> = blocks.iter().map(painted).collect();

        // 1. The shape. A level-2 title, then exactly one level-3 heading --
        //    the changelog's `### `, which used to be the entire body.
        assert!(
            matches!(&blocks[0], NotesBlock::Heading { level: 2, .. }) &&
                painted[0] == "What's new in 0.14.0",
            "got {:?}",
            &blocks[0]
        );
        let threes: Vec<&String> = blocks
            .iter()
            .zip(&painted)
            .filter(|(b, _)| matches!(b, NotesBlock::Heading { level: 3, .. }))
            .map(|(_, t)| t)
            .collect();
        assert_eq!(
            threes,
            vec!["Keeping an encrypted copy on this PC no longer asks for anything"],
            "the changelog's own `### ` heading is what names the change"
        );

        // 2. THE PROSE IS THERE, AND IT IS ONE BLOCK PER PARAGRAPH.
        //    This sentence is three separate lines in `CHANGELOG.md`, which
        //    is hard-wrapped at about 76 columns. `release_notes_blocks`
        //    makes one block per SOURCE line, so if the workflow published
        //    the section line for line this assertion fails and the card
        //    shows a column of stubs re-wrapped at a narrower width.
        const REJOINED: &str = concat!(
            "already is — kept on this PC, protected by Windows. No prompt, ever. ",
            "A PC without Windows Hello can use the setting now"
        );
        assert!(
            painted.iter().any(|t| t.contains(REJOINED)),
            "the wrapped source lines were not rejoined into a paragraph: {painted:?}"
        );

        // 3. Its emphasis is styled, not spelled. `**No prompt, ever.**` is
        //    the sentence the whole release turns on.
        let strong: Vec<&str> = blocks
            .iter()
            .filter_map(|b| match b {
                NotesBlock::Paragraph { spans } => Some(spans),
                _ => None,
            })
            .flatten()
            .filter(|s| s.style == NotesStyle::Strong)
            .map(|s| s.text.as_str())
            .collect();
        assert!(
            strong.contains(&"No prompt, ever.") &&
                strong.contains(&"Lock the vault when you step away"),
            "got {strong:?}"
        );

        // 4. NO MARKUP REACHES THE CARD. A body that painted its own
        //    asterisks, hashes and backticks would be worse than the list of
        //    headings this replaced.
        for text in &painted {
            assert!(
                !text.contains('*') && !text.contains('#') && !text.contains('`') &&
                    !text.starts_with('>'),
                "unconsumed markup would be painted: {text:?}"
            );
        }

        // 5. The tail survived. `release_notes_for_display` cuts at
        //    `MAX_RELEASE_NOTES_CHARS` from the END, which is where the link
        //    and the signing note are, so the workflow's own budget has to
        //    keep the whole body under it.
        assert!(
            COMPOSED_BODY_0_14_0.chars().count() < MAX_RELEASE_NOTES_CHARS,
            "the composed body is {} chars and would be cut",
            COMPOSED_BODY_0_14_0.chars().count()
        );
        let last = blocks.last().expect("a non-empty body");
        let link = match last {
            NotesBlock::Paragraph { spans } => spans
                .iter()
                .find(|s| s.style == NotesStyle::LinkText)
                .expect("the signing note ends on a link"),
            other => panic!("expected the signing note last, got {other:?}"),
        };
        assert_eq!(link.text, "Code-signing policy");
        assert_eq!(
            link.link.as_deref(),
            Some("https://github.com/denis-platonov/deskwarden/blob/v0.14.0/docs/code-signing-policy.md")
        );
        assert!(
            !painted.concat().contains("[...]"),
            "the notes were truncated, so the reader sees the cut mark instead of the tail"
        );
    }

    /// **Why the workflow strips a `> ` rather than publishing it.**
    ///
    /// A positive control on a construct the subset does NOT cover. The
    /// changelog uses block quotes for release-wide notes (0.9.0 and 0.8.5
    /// both open on one), and published as written the marker is painted as
    /// itself down the left of the card. The degraded form -- the same words,
    /// the marker gone -- is an ordinary paragraph with its emphasis intact,
    /// which is the whole of what that quote was saying.
    #[test]
    fn a_block_quote_paints_its_own_marker_which_is_why_the_workflow_strips_it() {
        const QUOTED: &str = "> Heads up: this change is **not reversible**";
        const DEGRADED: &str = "Heads up: this change is **not reversible**";

        let as_written = painted(&release_notes_blocks(QUOTED)[0]);
        assert!(
            as_written.starts_with('>'),
            "control: the subset grew block quotes, so the workflow's strip is now a downgrade \
             rather than a rescue -- publish the `> ` instead. Painted: {as_written:?}"
        );

        let blocks = release_notes_blocks(DEGRADED);
        assert_eq!(blocks.len(), 1, "got {blocks:?}");
        assert_eq!(painted(&blocks[0]), "Heads up: this change is not reversible");
        assert!(
            spans_of(DEGRADED)
                .iter()
                .any(|s| s.style == NotesStyle::Strong && s.text == "not reversible"),
            "the quote's emphasis has to survive the strip"
        );
    }

    /// **Why the workflow drops a fence delimiter and keeps what it wrapped.**
    ///
    /// The other positive control. Three backticks on a line of their own are
    /// not a construct the subset knows, so they arrive as characters; the
    /// command between them is perfectly readable without them.
    #[test]
    fn a_code_fence_paints_its_own_backticks_which_is_why_the_workflow_drops_it() {
        const FENCED: &str = "```powershell\ndeskwarden.exe --reset-tray\n```";

        let as_written: String =
            release_notes_blocks(FENCED).iter().map(painted).collect::<Vec<_>>().join("\n");
        assert!(
            as_written.contains('`') || as_written.contains("powershell"),
            "control: the subset grew fenced code, so the workflow should publish the fence \
             rather than drop it. Painted: {as_written:?}"
        );

        let blocks = release_notes_blocks("deskwarden.exe --reset-tray");
        assert_eq!(blocks.len(), 1, "got {blocks:?}");
        assert_eq!(painted(&blocks[0]), "deskwarden.exe --reset-tray");
    }

    #[test]
    fn bold_italic_and_code_become_styles_and_lose_their_delimiters() {
        let spans = spans_of("a **strong** and *soft* and `literal` word");

        assert!(spans.iter().any(|s| s.style == NotesStyle::Strong && s.text == "strong"));
        assert!(spans.iter().any(|s| s.style == NotesStyle::Emphasis && s.text == "soft"));
        assert!(spans.iter().any(|s| s.style == NotesStyle::Code && s.text == "literal"));
        assert!(
            !text_of(&spans).contains('*') && !text_of(&spans).contains('`'),
            "a delimiter survived into the painted text: {spans:?}"
        );
    }

    /// **A link's words, its destination, AND a way to follow it.**
    ///
    /// The destination half is the older assertion and the part that did not
    /// change when links became clickable: a reader can see where these words
    /// go without going there. The `link` half is the reversal -- see the
    /// subset header for what the old rule was and why it was lifted.
    #[test]
    fn a_link_keeps_its_words_and_shows_its_destination_as_text() {
        let spans = spans_of("see [the notes](https://example.invalid/x) please");

        let words = spans
            .iter()
            .find(|s| s.style == NotesStyle::LinkText)
            .unwrap_or_else(|| panic!("no link span at all: {spans:?}"));
        assert_eq!(words.text, "the notes");
        assert_eq!(
            words.link.as_deref(),
            Some("https://example.invalid/x"),
            "an accepted link carries the destination a renderer opens"
        );
        assert!(
            spans
                .iter()
                .any(|s| s.style == NotesStyle::LinkUrl
                    && s.text.contains("https://example.invalid/x")),
            "the destination must be readable rather than hidden behind words: {spans:?}"
        );
        assert!(!text_of(&spans).contains('['), "got {spans:?}");
    }

    /// **Everything that is not `https` is text, and carries nothing to
    /// open.**
    ///
    /// The refusal is asserted on the SPAN rather than on a renderer,
    /// because that is where it is made: a refused URL never becomes a
    /// `link`, so no caller can be talked into opening one. Both halves are
    /// checked -- no destination attached, and not painted in the style that
    /// promises a click -- since a plain-looking span that was still
    /// clickable and a blue underlined one that was not are both defects.
    #[test]
    fn only_https_links_can_be_followed() {
        for hostile in [
            "http://example.invalid/x",
            "file:///C:/Windows/System32/calc.exe",
            "ms-settings:windowsupdate",
            "javascript:alert(1)",
            "//example.invalid/x",
            "https:/example.invalid/x",
            "https://",
            "/relative/path",
        ] {
            let spans = spans_of(&format!("see [the notes]({hostile}) please"));
            assert!(
                spans.iter().all(|s| s.link.is_none()),
                "{hostile:?} became something a click could open: {spans:?}"
            );
            assert!(
                spans.iter().all(|s| s.style != NotesStyle::LinkText),
                "{hostile:?} is painted as a link it will not behave as: {spans:?}"
            );
            // Refused, not swallowed: the words and the destination are both
            // still on screen, which is how a reader can tell what the
            // release author wrote.
            assert!(
                text_of(&spans).contains("the notes") && text_of(&spans).contains(hostile),
                "{hostile:?} lost its words or its destination: {spans:?}"
            );
        }
    }

    /// **The workflow's own "Full changelog" line, end to end.**
    ///
    /// `**[text](url)**` is the exact shape `.github/workflows/release.yml`
    /// composes for every release, so this is a fixture in the sense that it
    /// is a copy of production output rather than an invented string. Before
    /// the emphasis run learned to contain one whole link it rendered as
    /// literal square brackets in bold, on the line of the panel a reader is
    /// most likely to want to follow.
    #[test]
    fn a_link_that_is_the_whole_of_a_bold_run_is_still_a_link() {
        let spans = spans_of("**[Full changelog](https://example.invalid/CHANGELOG.md)**");

        let words = spans
            .iter()
            .find(|s| s.style == NotesStyle::LinkText)
            .unwrap_or_else(|| panic!("the bold run swallowed the link: {spans:?}"));
        assert_eq!(words.text, "Full changelog");
        assert_eq!(words.link.as_deref(), Some("https://example.invalid/CHANGELOG.md"));
        assert!(
            !text_of(&spans).contains('[') && !text_of(&spans).contains('*'),
            "a delimiter survived into the painted text: {spans:?}"
        );
    }

    /// The exception above is exactly that -- an emphasis run that merely
    /// CONTAINS a link, rather than being one, is untouched, and the flat
    /// rule the module header states still holds everywhere else.
    #[test]
    fn emphasis_around_more_than_a_link_stays_flat() {
        for line in [
            "**see [notes](https://example.invalid/x) now**",
            "**a *b* c**",
            "**plain**",
        ] {
            let spans = spans_of(line);
            assert!(
                spans.iter().any(|s| s.style == NotesStyle::Strong),
                "{line:?} stopped being bold: {spans:?}"
            );
            assert!(
                spans.iter().all(|s| s.link.is_none()),
                "{line:?} produced a followable link out of literal text: {spans:?}"
            );
        }
    }

    /// The scheme is matched without regard to case, and the destination
    /// handed on is the string that was judged rather than a normalised
    /// rewrite of it.
    ///
    /// `HTTPS://` is a legal spelling of the scheme, and a check that only
    /// knew the lowercase one would refuse a perfectly ordinary link -- the
    /// failure mode of a scheme test that compares bytes.
    #[test]
    fn the_scheme_check_reads_the_url_the_way_it_would_be_opened() {
        let spans = spans_of("see [notes](HTTPS://Example.invalid/X) please");
        let words = spans
            .iter()
            .find(|s| s.style == NotesStyle::LinkText)
            .unwrap_or_else(|| panic!("an https link in capitals was refused: {spans:?}"));
        assert_eq!(
            words.link.as_deref(),
            Some("HTTPS://Example.invalid/X"),
            "the destination must be the string the check passed, not a rewrite of it"
        );
    }

    /// An image keeps its alt text and loses its URL: rendering it is a
    /// network fetch, and this page makes none it was not asked for.
    #[test]
    fn an_image_keeps_its_alt_text_and_drops_its_source() {
        let spans = spans_of("look ![a shot](https://example.invalid/x.png) here");

        assert!(text_of(&spans).contains("a shot"), "got {spans:?}");
        assert!(!text_of(&spans).contains("example.invalid"), "got {spans:?}");
    }

    /// **Malformed markup is text, never a panic and never a loss.**
    ///
    /// The input is chosen by a remote host, so "total" is the requirement
    /// rather than a nicety: every unrecognised or unterminated construct
    /// falls through to being painted as the characters it is, which is
    /// exactly the behaviour this page had before there was a parser at all.
    #[test]
    fn malformed_markup_is_painted_as_the_characters_it_is() {
        for body in [
            "**unclosed",
            "`unclosed",
            "[text](no-close",
            "[text] (spaced)",
            "![alt](",
            "****",
            "_",
            "#nospace",
            "#",
            "-",
            "*",
            "a ) stray ( paren",
            "\\**escaped\\**",
            "\u{00e9}**\u{00e9}**\u{00e9}",
        ] {
            let blocks = release_notes_blocks(body);
            let painted: String = blocks
                .iter()
                .map(|b| match b {
                    NotesBlock::Blank => String::new(),
                    NotesBlock::Heading { spans, .. }
                    | NotesBlock::Bullet { spans, .. }
                    | NotesBlock::Paragraph { spans } => text_of(spans),
                })
                .collect();
            assert!(
                !painted.is_empty() || body.trim().is_empty(),
                "{body:?} was dropped entirely rather than painted as text"
            );
        }
    }

    /// **Nothing the parser produces was invented by the parser.**
    ///
    /// Every span's text is a slice of the sanitised string, which is what
    /// makes "the strip still runs" a property of the shape rather than a
    /// habit: a bidi override cannot reappear downstream of a function that
    /// only ever copies characters that survived the strip.
    #[test]
    fn the_strip_runs_before_the_parse_and_the_parse_adds_nothing() {
        let hostile = "**safe\u{202e}txet**\u{200b}\u{0007}\r\n- [x](http://e.example)\u{feff}";
        let blocks = release_notes_blocks(hostile);
        let painted: String = blocks
            .iter()
            .map(|b| match b {
                NotesBlock::Blank => String::new(),
                NotesBlock::Heading { spans, .. }
                | NotesBlock::Bullet { spans, .. }
                | NotesBlock::Paragraph { spans } => text_of(spans),
            })
            .collect();

        for bad in ['\u{202e}', '\u{200b}', '\u{0007}', '\u{feff}', '\r'] {
            assert!(!painted.contains(bad), "{bad:?} reached the page: {painted:?}");
        }
    }

    /// The length bound is the sanitiser's and the parser does not escape it:
    /// a body chosen by a remote host cannot become an unbounded number of
    /// blocks, because every block comes from a line of an already-cut
    /// string.
    #[test]
    fn the_parsed_blocks_are_bounded_by_the_sanitisers_cut() {
        let huge = "- x\n".repeat(MAX_RELEASE_NOTES_CHARS);
        let blocks = release_notes_blocks(&huge);

        assert!(
            blocks.len() <= MAX_RELEASE_NOTES_CHARS,
            "the parse outran the cut: {} blocks",
            blocks.len()
        );
    }

    /// **The last progress report is the whole file.**
    ///
    /// This is the contract the About page's progress bar rests on: without it
    /// a completed download can leave the bar short of the end, which reads as
    /// a transfer that stalled at the finish line.
    #[test]
    fn the_download_reports_progress_and_ends_on_the_byte_count_it_wrote() {
        let payload = vec![7u8; 20 * 1024];
        let mut source = payload.as_slice();
        let mut sink: Vec<u8> = Vec::new();
        let seen = std::sync::Mutex::new(Vec::new());
        let total = payload.len() as u64;

        let copied =
            copy_reporting(&mut source, &mut sink, Some(total), &|done, declared| {
                seen.lock().unwrap().push((done, declared));
            })
            .unwrap();

        let seen = seen.into_inner().unwrap();
        assert_eq!(copied, total);
        assert_eq!(sink.len(), payload.len(), "the bytes must still all arrive");
        assert!(seen.len() > 2, "a 20 KiB stream reported only {} time(s)", seen.len());
        assert_eq!(seen.first().copied(), Some((0, Some(total))));
        assert_eq!(
            seen.last().copied(),
            Some((total, Some(total))),
            "the final report must be the total actually written, or the bar never fills"
        );
        assert!(
            seen.windows(2).all(|w| w[0].0 <= w[1].0),
            "progress went backwards, which no bar can render honestly"
        );
    }

    /// A server that declares no length is normal (a chunked response has
    /// none), and the caller has to be told so rather than being handed a zero
    /// it would render as a 0-byte download.
    #[test]
    fn an_unknown_total_is_reported_as_unknown_rather_than_as_zero() {
        let mut source: &[u8] = b"0123456789";
        let mut sink: Vec<u8> = Vec::new();
        let last = std::sync::Mutex::new(None);

        copy_reporting(&mut source, &mut sink, None, &|done, declared| {
            *last.lock().unwrap() = Some((done, declared));
        })
        .unwrap();

        assert_eq!(last.into_inner().unwrap(), Some((10, None)));
    }

    #[test]
    fn selects_the_installer_asset_even_when_a_bare_exe_is_also_present() {
        // Task 6's release workflow publishes both a bare `deskwarden.exe` and
        // a `*-installer.exe`. This test pins the selection logic to picking
        // the installer specifically, regardless of asset order.
        let mut server = crate::test_http::server();
        let body = r#"{
            "tag_name": "v1.2.0",
            "assets": [
                {"name": "deskwarden.exe", "browser_download_url": "https://example.com/deskwarden.exe", "digest": "sha256:2222222222222222222222222222222222222222222222222222222222222222"},
                {"name": "deskwarden-installer.exe", "browser_download_url": "https://example.com/deskwarden-installer.exe", "digest": "sha256:1111111111111111111111111111111111111111111111111111111111111111"}
            ]
        }"#;
        let _m = server
            .mock("GET", RELEASES_PATH)
            .match_query(crate::test_http::Matcher::Any)
            .with_status(200)
            .with_body(release_list(body))
            .create();

        let current = Version::parse("1.1.0").unwrap();
        let agent = build_api_agent();
        let result = check_for_update(&server.url(), &current, &agent).unwrap();

        let release = result.expect("expected an available update");
        assert_eq!(release.installer_download_url, "https://example.com/deskwarden-installer.exe");
        // **The digest is read from the SAME asset object as the URL.** The
        // two fixtures publish different digests on purpose: a parse that
        // searched `assets` twice, or took the first digest it saw, would
        // bring back the bare exe's here and be checking the wrong file's
        // hash against the right file's bytes.
        assert_eq!(
            release.installer_sha256,
            digest_of_byte("11"),
            "the digest came from a different asset than the download URL did"
        );
        assert_ne!(release.installer_sha256, digest_of_byte("22"));
    }

    /// **A release whose installer asset carries no digest is not offered.**
    ///
    /// The failure direction is the point. There is nothing this build could
    /// check such a download against, so it declines to report an update at
    /// all rather than reporting one it would go on to run unverified. "We
    /// could not check" must never become "proceed", and the earliest place
    /// to make that true is here, before anything is downloaded.
    #[test]
    fn a_release_whose_installer_has_no_digest_is_refused_rather_than_offered() {
        let mut server = crate::test_http::server();
        let body = r#"{
            "tag_name": "v1.2.0",
            "assets": [
                {"name": "deskwarden-installer.exe", "browser_download_url": "https://example.com/deskwarden-installer.exe"}
            ]
        }"#;
        let _m = server
            .mock("GET", RELEASES_PATH)
            .match_query(crate::test_http::Matcher::Any)
            .with_status(200)
            .with_body(release_list(body))
            .create();

        let current = Version::parse("1.1.0").unwrap();
        let error = check_for_update(&server.url(), &current, &build_api_agent())
            .expect_err("a release with no digest must not be offered as an update");

        assert!(
            error.contains("digest"),
            "the refusal does not say what was missing: {error}"
        );
    }

    /// A digest that is present but not something this code can make sense of
    /// is the same refusal, for the same reason. Each fixture below is a
    /// different way of being wrong, and none of them may be leniently
    /// accepted into a value the gate would then compare against.
    #[test]
    fn a_release_whose_digest_is_malformed_is_refused_rather_than_offered() {
        let sixty_four = "a".repeat(64);
        for bad in [
            // No algorithm qualifier: 64 hex characters that might be anything.
            sixty_four.clone(),
            // A different algorithm, whose length happens to fit.
            format!("sha512:{sixty_four}"),
            // Right prefix, too short -- a truncated compare's favourite input.
            "sha256:abcdef".to_string(),
            // Right prefix, right length, not hexadecimal.
            format!("sha256:{}", "z".repeat(64)),
            // Empty.
            String::new(),
        ] {
            let mut server = crate::test_http::server();
            let body = format!(
                r#"{{"tag_name": "v1.2.0", "assets": [{{"name": "deskwarden-installer.exe",
                   "browser_download_url": "https://example.com/i-installer.exe",
                   "digest": "{bad}"}}]}}"#
            );
            let _m = server
                .mock("GET", RELEASES_PATH)
                .match_query(crate::test_http::Matcher::Any)
                .with_status(200)
                .with_body(release_list(&body))
                .create();

            let current = Version::parse("1.1.0").unwrap();
            let result = check_for_update(&server.url(), &current, &build_api_agent());

            assert!(
                result.is_err(),
                "the digest {bad:?} was accepted; a value the gate cannot trust must not \
                 become an offered update"
            );
        }
    }

    /// The parser, directly, in both directions.
    #[test]
    fn a_well_formed_digest_parses_to_its_bytes_and_back() {
        let parsed = parse_asset_digest(SHA256_OF_ABC).expect("the NIST vector is well-formed");
        assert_eq!(
            parsed.to_string(),
            SHA256_OF_ABC.trim_start_matches("sha256:"),
            "a parsed digest must render back as the hex it came from"
        );

        // Hex is case-insensitive, and GitHub's own casing is not something
        // this crate should depend on.
        let upper = format!("sha256:{}", SHA256_OF_ABC.trim_start_matches("sha256:").to_uppercase());
        assert_eq!(
            parse_asset_digest(&upper).expect("upper-case hex is still hex"),
            parsed
        );

        // And the control: two different digests are not equal, so the
        // equality above is not something every pair of these has.
        assert_ne!(parsed, digest_of_byte("11"));
    }

    /// **The hasher computes SHA-256, over the whole file, in one pass.**
    ///
    /// Pinned against published vectors rather than against itself. The empty
    /// case catches a hasher that never runs; the multi-chunk case catches a
    /// streaming loop that drops or double-counts a chunk -- which a
    /// single-read fixture cannot see, because it never crosses
    /// [`HASH_CHUNK`].
    #[test]
    fn the_hasher_is_sha256_over_the_whole_stream() {
        let dir = scratch_dir("hasher");

        let empty = dir.join("empty.bin");
        std::fs::write(&empty, b"").unwrap();
        assert_eq!(
            file_sha256(&empty).unwrap().to_string(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "the published SHA-256 of the empty input"
        );

        let abc = dir.join("abc.bin");
        std::fs::write(&abc, b"abc").unwrap();
        assert_eq!(
            file_sha256(&abc).unwrap(),
            parse_asset_digest(SHA256_OF_ABC).unwrap()
        );

        // Larger than HASH_CHUNK, so the loop runs several times. A million
        // 'a's is the third standard vector.
        let long_bytes = "a".repeat(1_000_000);
        let long = dir.join("long.bin");
        std::fs::write(&long, long_bytes.as_bytes()).unwrap();
        assert!(
            long_bytes.len() > HASH_CHUNK,
            "control: the fixture really does span more than one read"
        );
        assert_eq!(
            file_sha256(&long).unwrap().to_string(),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0",
            "the published SHA-256 of one million 'a' characters"
        );

        // A file that is not there is an Err, never a hash of nothing: the
        // empty input has a perfectly good digest, and confusing the two
        // would make a missing installer indistinguishable from an empty one.
        let missing = dir.join("not-here.bin");
        assert!(file_sha256(&missing).is_err());
        assert_ne!(
            file_sha256(&empty).unwrap().to_string(),
            String::new(),
            "control: the empty file hashes to something, so the Err above is about absence"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// **The download pass refuses and DELETES an installer whose bytes are
    /// not the ones the release published.**
    ///
    /// Served over a real socket by [`crate::test_http`], so the bytes travel the same
    /// path a real download does. The pair below is one server response apart:
    /// same code, same fixture size, different digest on the release.
    #[test]
    fn a_download_whose_digest_does_not_match_is_deleted_and_refused() {
        let mut server = crate::test_http::server();
        let _m = server
            .mock("GET", "/deskwarden-9.9.9-installer.exe")
            .with_status(200)
            .with_body("abc")
            .create();

        let dir = scratch_dir("download-mismatch");
        let release = ReleaseInfo {
            version: Version::parse("9.9.9").unwrap(),
            installer_download_url: format!("{}/deskwarden-9.9.9-installer.exe", server.url()),
            // The release claims something the served body is not.
            installer_sha256: digest_of_byte("11"),
            body: String::new(),
        };

        let error = download_and_verify(&release, &dir, &build_download_agent(), NO_PROGRESS)
            .expect_err("a download whose digest disagrees must not be a success");

        assert!(
            error.contains("SHA-256"),
            "the refusal does not say what failed: {error}"
        );
        let written = dir.join(installer_file_name(&release.version));
        assert!(
            !written.exists(),
            "the rejected download is still at {}; it must not be left in the cache directory \
             where a later run -- or the user -- could execute it",
            written.display()
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The counterpart: a download whose digest DOES match is kept, and the
    /// path handed back is the file that was hashed.
    ///
    /// Without this, the test above is satisfied by a download pass that
    /// rejects everything -- which is precisely the updater this change
    /// replaced.
    #[test]
    fn a_download_whose_digest_matches_is_kept_and_its_path_returned() {
        let mut server = crate::test_http::server();
        let _m = server
            .mock("GET", "/deskwarden-9.9.9-installer.exe")
            .with_status(200)
            .with_body("abc")
            .create();

        let dir = scratch_dir("download-match");
        let release = ReleaseInfo {
            version: Version::parse("9.9.9").unwrap(),
            installer_download_url: format!("{}/deskwarden-9.9.9-installer.exe", server.url()),
            installer_sha256: parse_asset_digest(SHA256_OF_ABC).unwrap(),
            body: String::new(),
        };

        let path = download_and_verify(&release, &dir, &build_download_agent(), NO_PROGRESS)
            .expect("a download matching the published digest must be accepted");

        assert_eq!(path, dir.join(installer_file_name(&release.version)));
        assert!(path.exists(), "an ACCEPTED download was deleted");
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"abc",
            "the file left on disk is not the one that was served"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn reports_no_update_when_current_version_is_latest() {
        let mut server = crate::test_http::server();
        let body = r#"{
            "tag_name": "v1.1.0",
            "assets": [
                {"name": "deskwarden-installer.exe", "browser_download_url": "https://example.com/deskwarden-installer.exe", "digest": "sha256:1111111111111111111111111111111111111111111111111111111111111111"}
            ]
        }"#;
        let _m = server
            .mock("GET", RELEASES_PATH)
            .match_query(crate::test_http::Matcher::Any)
            .with_status(200)
            .with_body(release_list(body))
            .create();

        let current = Version::parse("1.1.0").unwrap();
        let agent = build_api_agent();
        let result = check_for_update(&server.url(), &current, &agent).unwrap();

        assert!(result.is_none());
    }

    #[test]
    fn reports_no_update_when_current_version_is_newer() {
        let mut server = crate::test_http::server();
        let body = r#"{
            "tag_name": "v1.0.0",
            "assets": [
                {"name": "deskwarden-installer.exe", "browser_download_url": "https://example.com/deskwarden-installer.exe", "digest": "sha256:1111111111111111111111111111111111111111111111111111111111111111"}
            ]
        }"#;
        let _m = server
            .mock("GET", RELEASES_PATH)
            .match_query(crate::test_http::Matcher::Any)
            .with_status(200)
            .with_body(release_list(body))
            .create();

        let current = Version::parse("1.1.0").unwrap();
        let agent = build_api_agent();
        let result = check_for_update(&server.url(), &current, &agent).unwrap();

        assert!(result.is_none());
    }

    /// A unique scratch directory, same `temp_dir()` + nanos pattern
    /// `session_store`/`logging`'s tests already use (no `tempfile`
    /// dev-dependency in this crate).
    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "deskwarden-updater-test-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn cleanup_removes_downloaded_installers_of_any_version() {
        let dir = scratch_dir("cleanup");
        std::fs::write(dir.join("deskwarden-0.1.0-installer.exe"), b"old").unwrap();
        std::fs::write(dir.join("deskwarden-0.2.0-installer.exe"), b"newer").unwrap();

        let removed = cleanup_stale_downloads(&dir).unwrap();

        assert_eq!(removed, 2);
        assert!(!dir.join("deskwarden-0.1.0-installer.exe").exists());
        assert!(!dir.join("deskwarden-0.2.0-installer.exe").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cleanup_leaves_unrelated_files_alone() {
        // The download directory is deliberately separate from the config
        // directory (which holds `session.bin` and the log), but the cleanup
        // pass still only ever deletes what it recognises as its own.
        let dir = scratch_dir("unrelated");
        std::fs::write(dir.join("session.bin"), b"secret").unwrap();
        std::fs::write(dir.join("deskwarden.log"), b"log").unwrap();
        std::fs::write(dir.join("deskwarden-0.1.0-installer.exe"), b"old").unwrap();

        let removed = cleanup_stale_downloads(&dir).unwrap();

        assert_eq!(removed, 1);
        assert!(dir.join("session.bin").exists());
        assert!(dir.join("deskwarden.log").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cleanup_treats_a_missing_directory_as_nothing_to_do() {
        let dir = std::env::temp_dir().join("deskwarden-updater-test-missing-dir-never-created");
        assert!(!dir.exists());
        assert_eq!(cleanup_stale_downloads(&dir).unwrap(), 0);
    }

    #[test]
    fn cleanup_recognises_exactly_what_download_and_verify_writes() {
        // Pins the two halves together: if the download file name ever
        // changes, this fails rather than silently leaving downloads to
        // accumulate forever.
        let version = Version::parse("1.2.3").unwrap();
        assert!(is_downloaded_installer(&installer_file_name(&version)));
    }

    #[test]
    fn errors_when_no_installer_asset_is_present() {
        let mut server = crate::test_http::server();
        let body = r#"{"tag_name": "v1.2.0", "assets": []}"#;
        let _m = server
            .mock("GET", RELEASES_PATH)
            .match_query(crate::test_http::Matcher::Any)
            .with_status(200)
            .with_body(release_list(body))
            .create();

        let current = Version::parse("1.1.0").unwrap();
        let agent = build_api_agent();
        let result = check_for_update(&server.url(), &current, &agent);

        assert!(result.is_err());
    }

    /// Pins that the *production* download agent -- not a test-built one --
    /// really is the non-pooling, stall-bounded shape.
    ///
    /// Connection reuse is the whole question: on a reused socket ureq has
    /// cleared the read timeout, and this agent deliberately carries no
    /// whole-request deadline to fall back on, so a pooled second request
    /// would be unbounded. `bounded_stall`'s `max_idle_connections(0)` is what
    /// makes that impossible; this asserts `build_download_agent` actually
    /// goes through it. Counted, not timed, so it cannot flake.
    #[test]
    fn the_production_download_agent_never_reuses_a_connection() {
        use std::io::{Read as _, Write as _};
        use std::net::{TcpListener, TcpStream};
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        fn read_head(stream: &mut TcpStream) -> bool {
            let mut seen = Vec::new();
            let mut byte = [0u8; 1];
            while stream.read(&mut byte).unwrap_or(0) == 1 {
                seen.push(byte[0]);
                if seen.ends_with(b"\r\n\r\n") {
                    return true;
                }
            }
            false
        }

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let accepts = Arc::new(AtomicUsize::new(0));

        let counted = Arc::clone(&accepts);
        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                counted.fetch_add(1, Ordering::SeqCst);
                std::thread::spawn(move || {
                    while read_head(&mut stream) {
                        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi");
                        let _ = stream.flush();
                    }
                });
            }
        });

        let agent = build_download_agent();
        let url = format!("http://127.0.0.1:{port}/installer.exe");
        for _ in 0..2 {
            agent.get(&url).call().unwrap().into_string().unwrap();
        }

        assert_eq!(
            accepts.load(Ordering::SeqCst),
            2,
            "the download agent pooled a connection, so its stall bound is not in force"
        );
    }

    /// A download that stalls part-way must not leave the partial installer
    /// behind.
    ///
    /// `cleanup_stale_downloads` would eventually collect it, but only at the
    /// *next* startup -- and this path stopped being near-unreachable when the
    /// bound became a 15s stall rather than a 600s total, so on a flaky link
    /// every retry now leaves another partial file in the cache directory for
    /// the rest of the session. The verification-failure branch already
    /// cleaned up after itself; this is the same courtesy on the branch that
    /// got common.
    ///
    /// Stalls the body rather than the head on purpose: a failure before the
    /// response arrives never reaches `File::create` and so proves nothing
    /// about the file. The stall bound here is the test's own 1s, not the
    /// production 15s, so this costs about a second.
    #[test]
    fn a_stalled_download_leaves_no_partial_installer_behind() {
        use std::io::{Read as _, Write as _};
        use std::net::{TcpListener, TcpStream};

        fn read_head(stream: &mut TcpStream) -> bool {
            let mut seen = Vec::new();
            let mut byte = [0u8; 1];
            while stream.read(&mut byte).unwrap_or(0) == 1 {
                seen.push(byte[0]);
                if seen.ends_with(b"\r\n\r\n") {
                    return true;
                }
            }
            false
        }

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_head(&mut stream);
            // Promise a megabyte, send ten bytes, then hold the socket open
            // and silent: a stalled transfer, not a closed one.
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000000\r\n\r\n0123456789");
            let _ = stream.flush();
            std::thread::sleep(Duration::from_secs(10));
        });

        let dir = scratch_dir("partial");
        let release = ReleaseInfo {
            version: Version::parse("9.9.9").unwrap(),
            installer_download_url: format!("http://127.0.0.1:{port}/installer.exe"),
            // Never reached: the transfer dies before anything is hashed. The
            // value is irrelevant and deliberately unmatchable.
            installer_sha256: digest_of_byte("cd"),
            body: String::new(),
        };
        let agent =
            crate::http_agent::bounded_stall(Duration::from_secs(1), Duration::from_secs(1));

        let result = download_and_verify(&release, &dir, &agent, NO_PROGRESS);

        assert!(result.is_err(), "a transfer that stopped moving must not look like success");
        let partial = dir.join(installer_file_name(&release.version));
        assert!(
            !partial.exists(),
            "a stalled download left {} on disk; it would sit there until the next startup",
            partial.display()
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    // -------------------------------------------------------------------
    // THE GATE, OVER REAL FILES ON REAL DISK
    //
    // The routing tests further down answer `hash` by hand, which proves the
    // ROUTING but says nothing about the hasher. These two run the production
    // `file_sha256` over real bytes in a real directory, with only the process
    // start substituted, and they are a matched pair on purpose: the refusal
    // and the launch come out of the SAME call to `apply_update_with`, one
    // digest apart. Either one alone is worthless -- a gate that refuses
    // everything passes the first, and the shipped updater passed exactly that
    // sort of test for four releases while never once applying an update.
    // -------------------------------------------------------------------

    /// What [`apply_over_real_file`] reports: the gate's verdict, every path
    /// that reached the substituted launch, and the installer path the module
    /// constructed.
    type AppliedOnDisk = (Result<(), String>, Vec<PathBuf>, PathBuf);

    /// Every path the substituted launch was asked to start, for the on-disk
    /// pair below. Separate from the routing `Recorder`, which validates its
    /// entries against generation-tagged synthetic directories these tests do
    /// not use.
    static DISK_LAUNCHES: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

    /// Serialises the two on-disk tests, so [`DISK_LAUNCHES`] means one run.
    static DISK_LOCK: Mutex<()> = Mutex::new(());

    fn disk_launch(path: &Path) -> Result<(), String> {
        DISK_LAUNCHES
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(path.to_path_buf());
        Ok(())
    }

    /// Runs `apply_update_with` over a REAL directory with the REAL hasher and
    /// only the launch substituted, and reports what happened.
    ///
    /// `bytes` are written to the file `apply_update` will look for; `claimed`
    /// is the digest the release publishes. Nothing else differs between the
    /// two tests below.
    fn apply_over_real_file(tag: &str, bytes: &[u8], claimed: Sha256Digest) -> AppliedOnDisk {
        let _serial = DISK_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        DISK_LAUNCHES.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clear();

        let dir = scratch_dir(tag);
        let release = ReleaseInfo {
            version: Version::parse("9.9.9").unwrap(),
            installer_download_url: String::new(),
            installer_sha256: claimed,
            body: String::new(),
        };
        let installer = dir.join(installer_file_name(&release.version));
        std::fs::write(&installer, bytes).unwrap();

        // The REAL `file_sha256`, not a substitute: these two tests are the
        // only ones that exercise the hasher through the gate.
        let env = UpdaterEnv::substitute(file_sha256, disk_launch);
        let result = apply_update_with(&dir, &release, &env);
        let launched =
            DISK_LAUNCHES.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        (result, launched, installer)
    }

    /// The published SHA-256 of `b"abc"`, written out rather than computed.
    ///
    /// A test that hashed the fixture with the same code under test would
    /// agree with any hash function at all, including a broken one. This is
    /// the standard NIST vector, so the positive case below also asserts that
    /// [`file_sha256`] computes SHA-256 and not merely something consistent.
    const SHA256_OF_ABC: &str =
        "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    /// **The actual attack: a file that is present, readable and well-formed,
    /// and is not the release.**
    ///
    /// It is refused, no process is started, and -- the part a refusal alone
    /// does not give you -- the rejected executable is DELETED. Left on disk
    /// it would sit at the exact predictable path `apply_update` reconstructs,
    /// where the next run of this same function, or a user in the cache
    /// folder, could run it.
    #[test]
    fn an_installer_whose_bytes_are_not_the_release_is_refused_deleted_and_never_launched() {
        let (result, launched, installer) = apply_over_real_file(
            "apply-wrong-bytes",
            b"a perfectly readable impostor",
            parse_asset_digest(SHA256_OF_ABC).unwrap(),
        );

        let error = result.expect_err("an installer with the wrong bytes must not be a success");
        assert!(
            launched.is_empty(),
            "an installer whose bytes are not the release reached the launch seam \
             ({launched:?}); in production that is a process start"
        );
        assert!(
            !installer.exists(),
            "the rejected installer is still at {}; it must not be left where a later run or \
             the user could execute it",
            installer.display()
        );
        assert!(
            error.contains("refusing to launch") && error.contains("SHA-256"),
            "the refusal does not say what it refused or why: {error}"
        );
        std::fs::remove_dir_all(installer.parent().unwrap()).ok();
    }

    /// **The counterpart, from the same code path, one digest apart: a file
    /// whose bytes ARE the release is launched.**
    ///
    /// This is the assertion whose absence let the shipped updater refuse
    /// every release for four versions without a single red test. It is also
    /// what says the deletion above is a consequence of the MISMATCH rather
    /// than something `apply_update_with` does to every file it looks at.
    #[test]
    fn an_installer_whose_bytes_are_the_release_is_launched_and_kept() {
        let (result, launched, installer) = apply_over_real_file(
            "apply-right-bytes",
            b"abc",
            parse_asset_digest(SHA256_OF_ABC).unwrap(),
        );

        assert!(result.is_ok(), "the matching installer was refused: {result:?}");
        assert_eq!(
            launched,
            vec![installer.clone()],
            "the launch seam did not receive exactly the file the module constructed the path to"
        );
        assert!(
            installer.exists(),
            "an ACCEPTED installer was deleted; the deletion belongs to the refusal path only"
        );
        std::fs::remove_dir_all(installer.parent().unwrap()).ok();
    }

    /// The narrowing itself, asserted rather than left to the doc comment: the
    /// caller hands over a directory and a release, and the file that would be
    /// launched is the one `download_and_verify` writes -- not one the caller
    /// named.
    ///
    /// A directory holding a *differently* named executable therefore has
    /// nothing in it `apply_update` will touch, and the error says which file
    /// it looked for. Safe to run against the PRODUCTION env -- no substituted
    /// launch -- precisely because the file it names is not there: the hasher
    /// fails to open it and the refusal happens before any spawn.
    #[test]
    fn apply_update_launches_only_the_file_the_download_pass_wrote() {
        let dir = scratch_dir("apply-constructs");
        let release = ReleaseInfo {
            version: Version::parse("9.9.9").unwrap(),
            installer_download_url: String::new(),
            installer_sha256: digest_of_byte("11"),
            body: String::new(),
        };
        // A plausible decoy, sitting right beside where the real one would go.
        std::fs::write(dir.join("setup.exe"), b"MZ not the one you want").unwrap();

        let error = apply_update(&dir, &release).expect_err("the named file is not there");

        let wanted = dir.join(installer_file_name(&release.version));
        assert!(
            error.contains(&wanted.display().to_string()),
            "apply_update reported {error}, which does not name {}; it is not constructing the \
             path from the release",
            wanted.display()
        );
        assert!(
            !error.contains("setup.exe"),
            "apply_update went looking at a file the caller merely left lying around: {error}"
        );
        assert!(
            dir.join("setup.exe").exists(),
            "apply_update deleted a file it was never asked about; the discard is for the \
             installer it names, not for the directory"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// [`download_and_verify`]'s exact shape, named so the pin below reads as
    /// one statement rather than as four lines of punctuation.
    type DownloadFn = fn(
        &ReleaseInfo,
        &Path,
        &crate::http_agent::StallBounded,
        &dyn Fn(u64, Option<u64>),
    ) -> Result<PathBuf, String>;

    /// The expected digest is not something a caller supplies. If it ever
    /// becomes a parameter again -- of `apply_update` or of
    /// `download_and_verify` -- these lines stop compiling.
    #[test]
    fn the_launch_time_digest_is_not_something_a_caller_supplies() {
        let narrowed: fn(&Path, &ReleaseInfo) -> Result<(), String> = apply_update;
        let _ = narrowed;
        let download: DownloadFn = download_and_verify;
        let _ = download;
    }

    /// Pins the two production numbers against the failure each was chosen to
    /// avoid, rather than leaving them to be re-tuned by feel.
    ///
    /// The 600s whole-request deadline this replaced is the reason: it pinned
    /// the tray's "Updating to vX" state -- and swallowed repeat clicks
    /// (`main.rs`) -- for ten minutes on a stalled download. A no-progress
    /// bound has to stay short enough that the tray recovers on a human
    /// timescale.
    #[test]
    fn the_download_stall_bound_stays_short_enough_for_the_tray_to_recover() {
        assert!(
            DOWNLOAD_STALL_TIMEOUT <= Duration::from_secs(60),
            "a stalled download must not pin the tray label for minutes"
        );
        // And long enough that a brief hiccup on a slow link isn't mistaken
        // for a dead transfer.
        assert!(DOWNLOAD_STALL_TIMEOUT >= Duration::from_secs(10));
        // The API check is bounded by *total* time, the download by time
        // without progress -- different quantities, so this is not a
        // "one must exceed the other" ordering claim. It is only a guard that
        // the two never collapse back into one shared number, which is the
        // arrangement that produced the inert setting in the first place.
        assert_ne!(API_DEADLINE, DOWNLOAD_STALL_TIMEOUT);
    }

    // ---------------------------------------------------------------------
    // THE GATE, AS ROUTING
    //
    // Everything above pins what the launcher COMPUTES. This section pins
    // that the launcher ASKS -- the property the previous shape of this
    // module disclosed as untestable and that a measured one-line
    // neutralisation (`let _launchable = installer_is_launchable(&info);`)
    // walked straight through at 2168 passed / 0 failed / 0 warnings.
    //
    // No fixture, no network, no signed file, no process: the harness hands
    // `apply_update_with` a `SignatureInfo` built by hand and records every
    // path that reaches the launch seam.
    // ---------------------------------------------------------------------

    use std::sync::atomic::AtomicU64;
    use std::sync::{Mutex, MutexGuard};

    /// What the substitute seam saw, for the whole PROCESS.
    ///
    /// # This used to be `thread_local!`, and that was the defect
    ///
    /// The previous shape of this harness kept `VERIFY_ANSWER`, `HASHED`
    /// and `LAUNCHED` in `thread_local!` storage and its doc called that a
    /// feature: "per-test state with no ordering assumption". It was a hole,
    /// and a measured one. Inserted into [`apply_update_with`] immediately
    /// BEFORE the `verify` call:
    ///
    /// ```ignore
    /// let zz_p = installer_path.clone();
    /// let zz_l = env.launch;
    /// let _ = std::thread::Builder::new().spawn(move || zz_l(&zz_p));
    /// ```
    ///
    /// -- which in production starts the real installer, unverified and
    /// ungated, on a background thread -- SURVIVED the entire suite at 2182
    /// lib / 217 bin / 0 failed and ZERO warnings. The launch happened on a
    /// thread that was not the test's, so every one of the six routing tests
    /// read an empty thread-local recorder and passed. The liveness control
    /// at the identical site, the same three lines with `let _ = zz_l(&zz_p);`
    /// in place of the `Builder` wrapper, was KILLED by all six at 2176/6.
    /// The ONLY difference between the survivor and the kill was which thread
    /// the call was on.
    ///
    /// So the recorder is global. **A launch on any thread in this process,
    /// however that thread was created, is written here.** The seam is a bare
    /// `fn` pointer by design (see [`UpdaterEnv`]) and cannot capture, so a
    /// `static` is the only place it can write; the per-test isolation the
    /// thread-locals used to give is now supplied by [`ROUTE_LOCK`] instead,
    /// which is a stronger property because it also serialises the
    /// harness-owned threads a mutant might create.
    struct Recorder {
        /// Which routing window is open -- or, on the odd bumps
        /// [`Session::drop`] makes, that NO window is open at all.
        ///
        /// # This tag used to be write-only, and that made the suite lie
        ///
        /// It was stamped onto every entry below and read by nothing:
        /// `Session::launched()`/`hashed()` were `.map(|(_, p)| p.clone())`
        /// over the whole vector, and [`assert_no_late_launch`] only asked
        /// whether that vector was empty. Measured on `0cd9fe0`, replacing
        /// `let generation = r.generation;` with `let generation = 0;` in
        /// [`record_launch`] SURVIVED at 2192 / 0 failed / 0 warnings, while
        /// the liveness control at the identical statement
        /// (`r.launched.push((generation, path.to_path_buf()));` becoming
        /// `let _ = (generation, path);`) was KILLED at 2188/4. The second
        /// tuple element was load-bearing; the first was inert.
        ///
        /// The doc that used to stand here claimed the stamp "is what makes
        /// it red that window's assertions instead of vanishing". That was
        /// exactly backwards, and it cost this suite an intermittent red whose
        /// message was a FALSE ALARM about code signing. A launch from the
        /// detached-thread witness that misses its own [`Session::settle`]
        /// lands after `Session::drop` has cleared, after the NEXT
        /// `Session::open` has cleared, and is therefore stamped with the new
        /// window's generation -- so
        /// [`a_well_formed_installer_with_the_wrong_digest_never_reaches_the_launch_seam`]
        /// reported that an installer with the wrong digest had reached the
        /// launch seam, in a run where nothing of the sort happened. Measured over 30 isolated `updater::` runs under
        /// concurrent compilation: 6 red, across three different victims.
        ///
        /// So the tag is READ now, in two places that between them leave a
        /// stray nowhere to be silently attributed:
        ///
        ///  * [`Session::launched`] and [`Session::hashed`] go through
        ///    [`entries_of_window`], which panics -- naming the WINDOW, not
        ///    the digest -- on any entry stamped with a different one.
        ///  * [`Session::drop`] bumps the generation to a value no session
        ///    owns before it releases [`ROUTE_LOCK`], and waits out
        ///    [`CLOSE_GRACE`] while still holding it. A launch arriving in
        ///    that band carries the unowned generation and is left in place,
        ///    so the next [`Session::open`]'s emptiness assertion says so. It
        ///    is no longer erased, and it can no longer be inherited.
        generation: u64,
        /// What the substitute `verify` answers on its next call.
        answer: Option<Result<Sha256Digest, String>>,
        /// Every path the substitute `verify` was asked about.
        hashed: Vec<(u64, PathBuf)>,
        /// Every path that reached the launch seam. **If this is ever
        /// non-empty when it should be empty, an unverified installer would
        /// have been started for real.**
        launched: Vec<(u64, PathBuf)>,
    }

    static RECORDER: Mutex<Recorder> = Mutex::new(Recorder {
        generation: 0,
        answer: None,
        hashed: Vec::new(),
        launched: Vec::new(),
    });

    /// Held for the whole of one routing window, so exactly one window is
    /// open at a time and the global recorder above is unambiguous.
    static ROUTE_LOCK: Mutex<()> = Mutex::new(());

    /// Poisoning is recovered from deliberately: a routing test that fails
    /// panics while a `Session` is alive, and a poisoned recorder would then
    /// turn one real failure into five misleading ones.
    fn recorder() -> MutexGuard<'static, Recorder> {
        RECORDER.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The one write that says "a process would have started here".
    fn record_launch(path: &Path) {
        let mut r = recorder();
        let generation = r.generation;
        r.launched.push((generation, path.to_path_buf()));
    }

    fn substitute_hash(path: &Path) -> Result<Sha256Digest, String> {
        {
            let mut r = recorder();
            let generation = r.generation;
            r.hashed.push((generation, path.to_path_buf()));
        }
        recorder()
            .answer
            .take()
            .expect("the test did not program a hash answer")
    }

    fn substitute_launch(path: &Path) -> Result<(), String> {
        record_launch(path);
        Ok(())
    }

    /// Panics if anything reached the seam AFTER the window that caused it
    /// had closed.
    ///
    /// Free rather than written inline in [`Session::open`] so that it can
    /// have a liveness control of its own. Nothing in this suite legitimately
    /// leaves a late launch behind, so an assertion written inline here is
    /// INERT -- and measurably so: neutralising it to `true || ..` survived
    /// the whole suite at 2188 / 217 / 0 failed / 0 warnings. A backstop with
    /// no control is a backstop nobody knows the shape of, so it is a
    /// function with a `#[should_panic]` test on it instead.
    fn assert_no_late_launch(launched: &[(u64, PathBuf)]) {
        assert!(
            launched.is_empty(),
            "a launch reached the seam AFTER the routing window that caused it had \
             closed: {launched:?}. In production that is a process start no assertion \
             was looking at"
        );
    }

    /// [`assert_no_late_launch`] for the VERIFY recorder.
    ///
    /// `Session::open` asserted emptiness of `launched` and silently cleared
    /// `hashed`, so a verification that outlived its window left no trace
    /// at all. A late verify starts no process -- this is the cosmetic half
    /// -- but it is the same evidence of a thread outliving the window that
    /// a late launch is, and swallowing it means the loud half is the only
    /// way that thread is ever noticed.
    ///
    /// A function with a `#[should_panic]` control rather than an inline
    /// assertion, for the reason given on [`assert_no_late_launch`]:
    /// nothing in this suite legitimately leaves one behind, so written
    /// inline it would be inert and nobody would know.
    fn assert_no_late_hash(hashed: &[(u64, PathBuf)]) {
        assert!(
            hashed.is_empty(),
            "a verification reached the seam AFTER the routing window that caused it \
             had closed: {hashed:?}. Nothing was started by it, but the thread that \
             did it outlived the window that asked for it"
        );
    }

    /// The directory every routing window hands to `apply_update_with`, TAGGED
    /// WITH THE WINDOW. Never created, never touched: nothing on the routing
    /// path reads the disk.
    ///
    /// The tag is what makes a late launch attributable, and it is the second
    /// half of the fix -- the generation STAMP alone is not enough. A stamp is
    /// read at the moment of recording, so a launch caused by window N but
    /// landing after window N+1 has opened is stamped N+1 and looks native.
    /// Measured: with the stamp alone, a detached launch delayed 900ms (past
    /// both [`SETTLE_QUIET`] and [`CLOSE_GRACE`]) still landed in the next
    /// window and reported a doubled launch vector there.
    ///
    /// The PATH, by contrast, is built by the window that caused the launch
    /// and travels with it. So a stray names the window it came from however
    /// late it arrives, and [`entries_of_window`] can say so.
    const ROUTING_DIR_PREFIX: &str = r"Z:\deskwarden-routing-test-never-created-";

    fn routing_dir(window: u64) -> PathBuf {
        PathBuf::from(format!("{ROUTING_DIR_PREFIX}{window}"))
    }

    /// Which window built `p`, for a path that came out of [`routing_dir`].
    /// `None` for anything else -- a real production path carries no tag, and
    /// then the generation stamp is all there is.
    fn window_of_path(p: &Path) -> Option<u64> {
        let s = p.to_string_lossy().into_owned();
        let rest = s.strip_prefix(ROUTING_DIR_PREFIX)?;
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        digits.parse().ok()
    }

    /// Every path recorded under `window`, and a LOUD, SPECIFIC failure for
    /// anything that belongs to a different one -- by its generation stamp, or
    /// by the window its own path names.
    ///
    /// This is the read that makes [`Recorder::generation`] more than
    /// decoration. A stray cannot be quietly folded into the window that
    /// happens to be open when it lands: the panic names the window and says
    /// what went wrong, so the run is red for the reason it is actually red
    /// rather than red about code signing.
    fn entries_of_window(entries: &[(u64, PathBuf)], window: u64, what: &str) -> Vec<PathBuf> {
        let strays: Vec<&(u64, PathBuf)> = entries
            .iter()
            .filter(|(g, p)| *g != window || window_of_path(p).is_some_and(|w| w != window))
            .collect();
        assert!(
            strays.is_empty(),
            "a {what} from a previous window arrived late: {strays:?}, read inside routing \
             window {window}. It is NOT this window's -- attributing it here is how a stray \
             detached launch used to red an unrelated gating assertion with a message that \
             was a false alarm. Whatever else this run reports, something reached the seam \
             after the window that caused it had closed"
        );
        entries.iter().map(|(_, p)| p.clone()).collect()
    }

    /// How long the recorder must go unchanged before [`Session::settle`]
    /// calls a window settled.
    ///
    /// # This number is the witness, so it is pinned rather than felt
    ///
    /// The shape this replaced counted "12 consecutive unchanged 10ms polls",
    /// i.e. it could return as early as **120ms** -- while its own doc claimed
    /// the budget was 600ms, understating the gap by 5x. Nothing pinned it:
    /// measured on `0cd9fe0`, `if stable >= 12` -> `if stable >= 0` (still one
    /// 10ms sleep) SURVIVED at 2192 / 0 failed / 0 warnings. Only "at least
    /// one poll" was held by anything.
    ///
    /// 120ms was also not enough. Under concurrent compilation a detached
    /// thread routinely does not get scheduled inside it, which is what made
    /// the witness miss and the stray contaminate the next window.
    /// [`the_settle_window_waits_out_a_launch_it_did_not_start`] pins the
    /// WAITING behaviourally, against a launch placed inside the window by
    /// construction rather than by the clock, and
    /// [`the_settle_budget_is_not_a_token_one`] pins the NUMBER, so a shrink
    /// is caught without anything being timed at all.
    const SETTLE_QUIET: Duration = Duration::from_millis(500);

    /// Poll interval for [`Session::settle`].
    const SETTLE_POLL: Duration = Duration::from_millis(10);

    /// How many times a [`Session::settle`] loop has SAMPLED the recorder,
    /// counted for the whole process.
    ///
    /// # Why the settle window is instrumented rather than out-waited
    ///
    /// [`the_settle_window_waits_out_a_launch_it_did_not_start`] has to put a
    /// launch INSIDE a settle window that is already running. It used to do
    /// that by sleeping 200ms on a detached thread and trusting 200ms of
    /// sleep to fit inside [`SETTLE_QUIET`]. It does not always fit: thread
    /// start-up plus the sleep's own overshoot ran past the whole 500ms
    /// window under `-j 8` on a loaded machine, `settle` returned having seen
    /// nothing, and the assertion printed `left: []` -- which reads as "no
    /// launch happened", the same false alarm the generation tag was
    /// introduced to stop this module telling. Reproduced at `0fec1e9` on run
    /// 3 of 6 full `-j 8` suites (that run took 82s against a 53s median).
    ///
    /// Widening `SETTLE_QUIET` would only have moved the bet, and the number
    /// is pinned by [`the_settle_budget_is_not_a_token_one`] anyway. This
    /// counter removes the bet instead: the delaying thread waits for the
    /// window to SAMPLE, which is a condition rather than a duration, so its
    /// launch is a change `settle` has still to notice on any machine at any
    /// load.
    static SETTLE_POLLS: AtomicU64 = AtomicU64::new(0);

    fn settle_polls() -> u64 {
        SETTLE_POLLS.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Blocks until [`Session::settle`] has sampled the recorder at least `n`
    /// times since `from`.
    ///
    /// The ceiling is not a margin to be tuned: it exists so that a `settle`
    /// which never polls -- exactly the mutant this instrument is here to
    /// catch -- leaves its caller free to make the launch LATE, and so red
    /// about a missed launch, rather than hanging the suite.
    fn await_settle_polls(from: u64, n: u64, ceiling: Duration) {
        let started = std::time::Instant::now();
        while settle_polls() < from + n && started.elapsed() < ceiling {
            std::thread::sleep(SETTLE_POLL);
        }
    }

    /// Total bound on [`Session::settle_witnessing`]. Generous on purpose: it
    /// is not a budget anything is expected to approach, it is the point at
    /// which a witness that will never arrive stops pretending to wait.
    const SETTLE_DEADLINE: Duration = Duration::from_secs(30);

    /// How long a closing window keeps [`ROUTE_LOCK`] after retiring its own
    /// generation, so that a launch still in flight lands under a generation
    /// NO session owns rather than under the next window's.
    const CLOSE_GRACE: Duration = Duration::from_millis(120);

    /// One routing window.
    ///
    /// Opening one takes [`ROUTE_LOCK`], asserts the recorder is EMPTY --
    /// anything in it arrived after the previous window closed, which is
    /// itself a launch nobody witnessed in time -- then bumps the generation
    /// and installs the programmed `verify` answer. Dropping one retires that
    /// generation, waits out [`CLOSE_GRACE`] still holding the lock, and only
    /// then releases it.
    struct Session {
        /// The generation this window owns. Everything it reads must carry
        /// this number; see [`entries_of_window`].
        generation: u64,
        _serial: MutexGuard<'static, ()>,
    }

    impl Session {
        fn open(answer: Option<Result<Sha256Digest, String>>) -> Self {
            let serial = ROUTE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            let mut r = recorder();
            r.generation += 1;
            let generation = r.generation;
            r.answer = answer;
            // **The previous window's leavings are TAKEN before they are
            // judged, and the recorder is released before either judgement.**
            //
            // Asserting first and clearing after STRANDED the stray. The
            // panic unwinds before `Session` is constructed, so `_serial` is
            // never a field of anything, [`Session::drop`] never runs and
            // [`CLOSE_GRACE`] never elapses -- and the stray is still in the
            // recorder when the next `open` reads it, which panics
            // identically. One late launch then reds EVERY routing test in
            // the process, and the first failure in the output is not
            // necessarily the causal one. Taking the vectors out first means
            // exactly one test reds: the one that was actually holding the
            // stray.
            let late_launches = std::mem::take(&mut r.launched);
            let late_verifies = std::mem::take(&mut r.hashed);
            drop(r);
            assert_no_late_launch(&late_launches);
            assert_no_late_hash(&late_verifies);
            Session { generation, _serial: serial }
        }

        fn launched(&self) -> Vec<PathBuf> {
            entries_of_window(&recorder().launched, self.generation, "launch")
        }

        fn hashed(&self) -> Vec<PathBuf> {
            entries_of_window(&recorder().hashed, self.generation, "verify")
        }

        /// Wait for the recorder to stop changing before it is read.
        ///
        /// A launch on a thread the code under test created lands after
        /// `apply_update_with` has already returned, so reading the recorder
        /// the instant the call finishes would still miss it. This waits for
        /// [`SETTLE_QUIET`] of no change.
        ///
        /// **What this does not see, said plainly:** a launch delayed past
        /// [`SETTLE_QUIET`] after the last change. That is no longer a launch
        /// that VANISHES, though, which is the part that used to matter: it
        /// lands under a generation no window owns (see [`Session::drop`]) and
        /// reds the next [`Session::open`] with a message about a late launch,
        /// rather than being inherited by the next window and reported as a
        /// gate failure.
        fn settle(&self) {
            // BOTH recorders, not just `launched`: a hash still arriving is
            // a thread still running, and a window that stopped waiting while
            // one was in flight closes on top of it.
            let sizes = || {
                // Counted so a test can place a launch inside this window by
                // construction instead of guessing at the clock; see
                // [`SETTLE_POLLS`]. The count is the only thing this adds --
                // the reading below is unchanged.
                SETTLE_POLLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let r = recorder();
                (r.launched.len(), r.hashed.len())
            };
            let mut last = sizes();
            let mut quiet_since = std::time::Instant::now();
            while quiet_since.elapsed() < SETTLE_QUIET {
                std::thread::sleep(SETTLE_POLL);
                let now = sizes();
                if now != last {
                    last = now;
                    quiet_since = std::time::Instant::now();
                }
            }
        }

        /// [`Session::settle`] for a window that is EXPECTING a launch: waits
        /// until at least `n` have been recorded, then for the usual quiet.
        ///
        /// A witness that can time out has to fail loudly when it does. The
        /// old settle could not: it returned quietly after ~120ms whether or
        /// not the launch it existed to witness had arrived, and the caller's
        /// `assert_eq!` then reported an empty vector -- which reads as "no
        /// launch happened", the opposite of what had happened. This panics,
        /// and says which.
        fn settle_witnessing(&self, n: usize) {
            let started = std::time::Instant::now();
            while recorder().launched.len() < n {
                assert!(
                    started.elapsed() < SETTLE_DEADLINE,
                    "the settle window timed out after {:?} still waiting for launch {n} of \
                     this routing window; only {} arrived. This is the witness FAILING, not \
                     the absence of a launch -- do not read it as one",
                    SETTLE_DEADLINE,
                    recorder().launched.len()
                );
                std::thread::sleep(SETTLE_POLL);
            }
            self.settle();
        }
    }

    /// What closing a routing window does to the recorder, as a function so
    /// [`a_closing_window_retires_its_generation_and_keeps_only_foreign_entries`]
    /// can drive it. A `Drop` body no test can call is a `Drop` body nothing
    /// knows the shape of, which is the same mistake
    /// [`assert_no_late_launch`] was pulled out of an inline assertion to
    /// avoid.
    fn retire_window(r: &mut Recorder, generation: u64) {
        r.answer = None;
        // Only THIS window's entries are cleared. A stray carrying another
        // generation is left where it is, for `Session::open`'s emptiness
        // assertion to find -- clearing it here is precisely what used to
        // erase the evidence.
        r.hashed.retain(|(g, _)| *g != generation);
        r.launched.retain(|(g, _)| *g != generation);
        // Retire the generation. From here until the next `open` the recorder
        // stamps entries no session will ever claim.
        r.generation += 1;
    }

    impl Drop for Session {
        fn drop(&mut self) {
            {
                let mut r = recorder();
                retire_window(&mut r, self.generation);
            }
            // `_serial` is a field, so it is dropped AFTER this body: the lock
            // is still held here. A launch still in flight therefore lands in
            // this grace period, under the retired generation, and cannot be
            // swallowed by the next window's clear.
            std::thread::sleep(CLOSE_GRACE);
        }
    }

    impl UpdaterEnv {
        /// The test-only substitute.
        ///
        /// An inherent impl written from `mod tests` rather than a
        /// test-gated method beside [`UpdaterEnv::production`], for the
        /// reason `vault_window`'s seam records: every source guard in a file
        /// cuts its production slice at the FIRST test gate in the text, so a
        /// gated item up beside `production` would truncate the slice
        /// [`production_is_the_only_updater_env_a_shipping_build_has`] and
        /// [`the_only_process_start_in_this_module_is_the_launch_seam`] read,
        /// and blind both of them to everything below it.
        fn substitute(
            hash: fn(&Path) -> Result<Sha256Digest, String>,
            launch: fn(&Path) -> Result<(), String>,
        ) -> Self {
            Self { hash, launch }
        }
    }

    /// Everything one routing window observed.
    struct Routed {
        result: Result<(), String>,
        launched: Vec<PathBuf>,
        hashed: Vec<PathBuf>,
        /// The installer path THIS window's directory produces -- what a
        /// correct launch must equal. Carried out of the window rather than
        /// recomputed by the caller, because the window number is part of it.
        expected: PathBuf,
    }

    /// Runs `apply_update_with` against the recording seam, with `verify`
    /// programmed to answer `answer`, and reports every path that reached
    /// either half of the seam ON ANY THREAD.
    fn route_recording(answer: Result<Sha256Digest, String>) -> Routed {
        let session = Session::open(Some(answer));

        // A directory that does not exist and is never created: nothing on
        // this path may touch the disk, because nothing on this path reads the
        // disk any more. Tagged with the window, so that whatever comes back
        // out of the seam names the window that sent it in.
        let dir = routing_dir(session.generation);
        let release = ReleaseInfo {
            version: Version::parse("9.9.9").unwrap(),
            installer_download_url: String::new(),
            // The digest the release CLAIMS. Every routing case below is a
            // statement about what the seam answers relative to this value.
            installer_sha256: release_digest(),
            body: String::new(),
        };
        let env = UpdaterEnv::substitute(substitute_hash, substitute_launch);
        let result = apply_update_with(&dir, &release, &env);
        session.settle();
        Routed {
            result,
            launched: session.launched(),
            hashed: session.hashed(),
            expected: routed_path(session.generation),
        }
    }

    /// [`route_recording`] for the cases that only care about `launch`.
    fn route(answer: Result<Sha256Digest, String>) -> (Result<(), String>, Vec<PathBuf>) {
        let routed = route_recording(answer);
        (routed.result, routed.launched)
    }

    /// A `Sha256Digest` built from one repeated hex byte. The routing cases
    /// care only about whether two digests are the SAME one, never about what
    /// any particular file hashes to, so a recognisable constant is clearer
    /// here than a real hash would be.
    fn digest_of_byte(byte: &str) -> Sha256Digest {
        parse_asset_digest(&format!("sha256:{}", byte.repeat(32)))
            .expect("the routing fixtures are well-formed digests")
    }

    /// The digest a routing window's release publishes -- the value the gate
    /// in `apply_update_with` compares against.
    fn release_digest() -> Sha256Digest {
        digest_of_byte("11")
    }

    /// A well-formed digest that is NOT [`release_digest`].
    ///
    /// **This is the shape of the actual attack.** Not a missing file, not a
    /// truncated one, not one that fails to parse: a perfectly readable,
    /// perfectly well-formed installer sitting at exactly the path the module
    /// constructs, whose bytes are somebody else's. Every "never reaches the
    /// launch seam" assertion that used a MALFORMED input would pass on a
    /// gate that only rejects malformed inputs.
    fn wrong_digest() -> Sha256Digest {
        digest_of_byte("22")
    }

    /// The path a routing window builds, for the assertions below to compare
    /// against. Takes the window, because the window is IN the path -- see
    /// [`ROUTING_DIR_PREFIX`].
    fn routed_path(window: u64) -> PathBuf {
        routing_dir(window).join(installer_file_name(&Version::parse("9.9.9").unwrap()))
    }

    /// **The gate is consulted: a well-formed installer with the WRONG
    /// digest is never launched.**
    ///
    /// This is the case that matters, because it is what a swapped installer
    /// looks like -- a file that opens, reads and hashes perfectly well, whose
    /// hash is simply not the one the release published. `hash` succeeds, the
    /// comparison says no, and the question is whether anything downstream
    /// cares. Deleting the `if`, or neutralising it to a `let _`, makes
    /// `launched` non-empty here.
    #[test]
    fn a_well_formed_installer_with_the_wrong_digest_never_reaches_the_launch_seam() {
        let (result, launched) = route(Ok(wrong_digest()));

        assert!(
            launched.is_empty(),
            "an installer whose SHA-256 is not the released one reached the launch seam              ({launched:?}); in production that is a process start"
        );
        let error = result.expect_err("a wrong-digest installer must not be a success");
        assert!(
            error.contains(&wrong_digest().to_string())
                && error.contains(&release_digest().to_string()),
            "the refusal names neither what it found nor what it wanted: {error}"
        );
    }

    /// A digest that differs from the release's in ONE BYTE is refused just as
    /// flatly as one that differs in all of them.
    ///
    /// Written separately because "close" is where a lenient comparison hides:
    /// a prefix match, a truncated compare, or a `starts_with` would pass the
    /// test above -- where the two digests share no bytes at all -- and fail
    /// here.
    #[test]
    fn a_digest_differing_in_one_byte_never_reaches_the_launch_seam() {
        let mut off_by_one = release_digest();
        off_by_one.0[31] ^= 0x01;
        assert_ne!(off_by_one, release_digest(), "control: the fixture really does differ");

        let (result, launched) = route(Ok(off_by_one));

        assert!(
            launched.is_empty(),
            "an installer whose SHA-256 differs from the released one in a single byte              reached the launch seam ({launched:?})"
        );
        assert!(result.is_err());
    }

    /// `hash` returning `Err` -- the answer is UNKNOWN, not "matching" -- is
    /// also a refusal, and the failure is propagated rather than swallowed
    /// into a default verdict.
    ///
    /// **This is the "we could not check" path, and it must not become
    /// "proceed".** The mutant here is shaped as "the result is ignored rather
    /// than unused": a body that turns the failure arm into an
    /// `unwrap_or(release.installer_sha256)` still USES `hash`, still USES the
    /// comparison, and warns about nothing.
    #[test]
    fn an_installer_that_could_not_be_hashed_never_reaches_the_launch_seam() {
        let (result, launched) = route(Err("the file could not be read".into()));

        assert!(
            launched.is_empty(),
            "an installer that could not be hashed at all reached the launch seam              ({launched:?})"
        );
        let error = result.expect_err("an unknown verdict is not a success");
        assert!(
            error.contains("the file could not be read"),
            "the hasher's own failure was swallowed: {error}"
        );
    }

    /// **The counterpart, without which every assertion above is vacuous:**
    /// the matching case IS launched, and with exactly the path the module
    /// constructed -- not one the caller named, and not some other file in the
    /// same directory.
    ///
    /// A gate that refuses everything passes the three tests above and is
    /// exactly the updater this change exists to fix: the placeholder
    /// thumbprint refused every release for four versions and no test
    /// noticed, because nothing asserted that a GOOD update gets through.
    ///
    /// A launcher that launches a DIFFERENT path than the one it hashed
    /// passes them too, and is the swap the re-hashing exists to close; the
    /// path equality here is what says the file that was checked is the file
    /// that runs.
    #[test]
    fn the_matching_installer_is_launched_and_it_is_the_file_that_was_hashed() {
        let Routed { result, launched, hashed, expected } = route_recording(Ok(release_digest()));

        assert!(result.is_ok(), "the matching installer was refused: {result:?}");
        assert_eq!(
            launched,
            vec![expected],
            "the launch seam did not receive exactly the one path the module constructed"
        );
        assert_eq!(
            hashed, launched,
            "the file that was HASHED is not the file that was LAUNCHED; the gap between              those two paths is exactly where a swap goes"
        );
    }

    /// **A launch on a thread this test did not create is still witnessed.**
    ///
    /// Without this, every `launched.is_empty()` above is a claim about ONE
    /// thread rather than about the process, which is exactly the hole the
    /// `std::thread::Builder::new().spawn(move || zz_l(&zz_p))` mutant walked
    /// through at a full 2182 / 0 failed / 0 warnings. This is the control
    /// that says the new recorder does not have that shape: the thread here
    /// is created by the harness rather than by production code, but the
    /// recorder cannot tell the difference and that is the point.
    #[test]
    fn a_launch_on_a_thread_the_test_does_not_own_is_witnessed() {
        let session = Session::open(None);
        let path = routed_path(session.generation);
        let handle = std::thread::Builder::new()
            .spawn(move || {
                let _ = substitute_launch(&path);
            })
            .expect("could not start the witness thread");
        handle.join().expect("the witness thread panicked");
        session.settle_witnessing(1);

        assert_eq!(
            session.launched(),
            vec![routed_path(session.generation)],
            "the recorder did not witness a launch made on another thread, so every \
             `launched.is_empty()` assertion in this module is a claim about one thread \
             rather than about this process"
        );
    }

    /// The same, DETACHED and never joined, and landing after the call that
    /// started it has already returned -- the exact shape of the survivor.
    /// [`Session::settle_witnessing`] is what closes the gap.
    ///
    /// This test used to be the suite's own flake. It waited on a plain
    /// `settle()` that returned after ~120ms whether or not the thread had run
    /// yet, and under concurrent compilation it often had not: the assertion
    /// then read an empty vector, and -- worse -- the launch landed later, in
    /// somebody else's window. `settle_witnessing` waits for the thing it is
    /// witnessing and fails as a TIMEOUT if it never comes, so this test now
    /// only goes red for its own reason.
    #[test]
    fn a_launch_on_a_detached_thread_is_witnessed_by_the_settle_window() {
        let session = Session::open(None);
        let path = routed_path(session.generation);
        let _ = std::thread::Builder::new().spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(5));
            let _ = substitute_launch(&path);
        });
        session.settle_witnessing(1);

        assert_eq!(
            session.launched(),
            vec![routed_path(session.generation)],
            "a launch on a detached thread was not witnessed inside the settle window"
        );
    }

    /// **The plain settle window keeps waiting for a launch that arrives
    /// after it has already started looking.**
    ///
    /// [`a_launch_on_a_detached_thread_is_witnessed_by_the_settle_window`]
    /// waits for its launch by count, so it says nothing about what the
    /// unassisted window does. This one calls the PLAIN [`Session::settle`]
    /// -- the one `route_recording` uses, and therefore the one that has to
    /// catch a mutant which slips a detached spawn into `apply_update_with`.
    ///
    /// # The launch is placed inside the window, not timed to fall in it
    ///
    /// The delay used to be `sleep(200ms)` against a 500ms `SETTLE_QUIET`,
    /// i.e. a bet that thread start-up plus sleep overshoot stayed under
    /// 300ms. That bet lost under `-j 8`; see [`SETTLE_POLLS`] for the
    /// measurement. There is no sleep here now. The thread reports itself on
    /// CPU BEFORE the window opens, so start-up is not part of what has to
    /// fit; then it waits for the window to have SAMPLED twice -- its
    /// pre-loop reading plus at least one poll -- so the launch is provably
    /// a change `settle` has still to notice and wait out.
    ///
    /// What that leaves pinned is what was pinned before: a `settle` that
    /// reads the recorder once and returns never sees this launch, and this
    /// goes red with an empty vector. Measured: `while false && ..` on the
    /// quiet loop is KILLED here. The SIZE of the quiet period is pinned
    /// separately, and without a clock, by
    /// [`the_settle_budget_is_not_a_token_one`]; neither test pins the
    /// restart-on-change inside the loop, which was true of the shape this
    /// replaced as well.
    #[test]
    fn the_settle_window_waits_out_a_launch_it_did_not_start() {
        let session = Session::open(None);
        let path = routed_path(session.generation);
        let polls_before = settle_polls();
        let (on_cpu, started) = std::sync::mpsc::channel();
        let _ = std::thread::Builder::new().spawn(move || {
            on_cpu.send(()).expect("nobody is waiting for the delaying thread");
            // Deliberately unconditional: if the window never samples, the
            // ceiling lapses and this launch lands LATE, which is the red
            // this test is for. Refusing to launch would hide it.
            await_settle_polls(polls_before, 2, SETTLE_DEADLINE);
            let _ = substitute_launch(&path);
        });
        started.recv().expect("the delaying thread never reached the CPU");
        session.settle();

        assert_eq!(
            session.launched(),
            vec![routed_path(session.generation)],
            "a launch made while the plain settle window was already polling was not \
             witnessed by it, so a mutant that puts the real launch on a detached thread \
             would be recorded by nobody in `route_recording` -- which is the exact \
             survivor this harness exists to kill"
        );
    }

    /// **A launch that lands well past the quiet window is still witnessed,
    /// because the witness WAITS for it.**
    ///
    /// The band this covers is the one the old shape lost things in: after
    /// `settle` would have given up, and before `Session::drop` clears. A
    /// launch landing there used to be erased outright and then blamed on the
    /// next window. Here the delay is deliberately longer than
    /// [`SETTLE_QUIET`], so a [`Session::settle_witnessing`] that stopped
    /// waiting for its count -- and merely settled -- would miss it and this
    /// would go red.
    #[test]
    fn a_launch_landing_past_the_quiet_window_is_still_waited_for() {
        let session = Session::open(None);
        let path = routed_path(session.generation);
        let _ = std::thread::Builder::new().spawn(move || {
            std::thread::sleep(SETTLE_QUIET + Duration::from_millis(400));
            let _ = substitute_launch(&path);
        });
        session.settle_witnessing(1);

        assert_eq!(
            session.launched(),
            vec![routed_path(session.generation)],
            "a launch that landed after the quiet window expired was not waited for. It is \
             not lost quietly any more, but a witness that gives up on the thing it is \
             witnessing is not a witness"
        );
    }

    /// And the number itself, so a shrink is caught on a machine lucky enough
    /// to schedule the thread above immediately.
    #[test]
    fn the_settle_budget_is_not_a_token_one() {
        assert!(
            SETTLE_QUIET >= Duration::from_millis(400),
            "the settle window's quiet period is {SETTLE_QUIET:?}. The shape this replaced \
             returned after 120ms while its doc claimed 600ms, and `if stable >= 12` -> \
             `if stable >= 0` was measured SURVIVING the whole suite: the budget was pinned \
             by nothing at all"
        );
        assert!(
            SETTLE_POLL <= Duration::from_millis(20),
            "the poll interval is coarser than the window it is sampling"
        );
        assert!(
            CLOSE_GRACE >= Duration::from_millis(100),
            "a closing window must hold the route lock long enough that an in-flight launch \
             lands under the RETIRED generation rather than under the next window's"
        );
        assert!(
            SETTLE_DEADLINE >= Duration::from_secs(5),
            "a witness that gives up in seconds is a flake, not a witness"
        );
    }

    /// **A launch that missed its window is named as a LATE LAUNCH, never
    /// counted as this window's.**
    ///
    /// This is the finding, reduced to an assertion. When the generation tag
    /// was write-only, a stray detached launch that landed after the recorder
    /// had been cleared was read as the NEXT window's launch -- and the next
    /// window happened to be
    /// [`a_well_formed_installer_with_the_wrong_digest_never_reaches_the_launch_seam`],
    /// so a run in which every digest matched reported that an installer with
    /// the wrong one had reached the launch seam. Here that is a different
    /// failure with a different message.
    ///
    /// Pure over the tag, so it is deterministic and touches no window: the
    /// payload of every launch in this module is a recorded no-op, never a
    /// real spawn.
    #[test]
    #[should_panic(expected = "arrived late")]
    fn a_launch_from_a_previous_window_is_never_attributed_to_this_one() {
        let entries = vec![(6, PathBuf::from(r"Z:\late-installer.exe"))];
        let _ = entries_of_window(&entries, 7, "launch");
    }

    /// **And the half the generation stamp cannot see: a launch whose STAMP
    /// says this window but whose PATH names the one before it.**
    ///
    /// This is precisely the shape of a launch caused by window N and landing
    /// after window N+1 has opened -- the stamp is read at recording time, so
    /// it says N+1 and looks native. Measured with the stamp alone in place: a
    /// detached launch delayed 900ms (past both `SETTLE_QUIET` and
    /// `CLOSE_GRACE`) landed in the next window and was reported there as that
    /// window's own doubled launch. The path tag is what tells them apart.
    #[test]
    #[should_panic(expected = "arrived late")]
    fn a_launch_whose_own_path_names_another_window_is_never_attributed_here() {
        let entries = vec![(7, routed_path(6))];
        let _ = entries_of_window(&entries, 7, "launch");
    }

    /// The control without which the two above are checks that always fire:
    /// entries carrying the open window's own generation are read normally, in
    /// order -- including one whose path carries this window's own tag, so the
    /// tag check is not simply refusing every tagged path.
    #[test]
    fn entries_of_the_open_window_are_read_in_order() {
        let entries = vec![
            (7, PathBuf::from(r"Z:\a.exe")),
            (7, PathBuf::from(r"Z:\b.exe")),
        ];
        assert_eq!(
            entries_of_window(&entries, 7, "launch"),
            vec![PathBuf::from(r"Z:\a.exe"), PathBuf::from(r"Z:\b.exe")]
        );
        assert!(entries_of_window(&[], 7, "launch").is_empty());
        assert_eq!(
            entries_of_window(&[(7, routed_path(7))], 7, "launch"),
            vec![routed_path(7)],
            "control: a path carrying this window's OWN tag was rejected, so the tag check \
             refuses everything and the assertion above it is vacuous"
        );
    }

    /// **A closing window retires its generation and clears only its OWN
    /// entries.**
    ///
    /// Both halves matter and both were absent. Clearing wholesale is what
    /// erased the evidence of a late launch; not retiring the generation is
    /// what let the next window inherit one. Everything here happens while the
    /// window is open -- so [`ROUTE_LOCK`] is held throughout and no other
    /// test can observe the scratch state -- and [`retire_window`] is the same
    /// code [`Session::drop`] runs, minus its sleep.
    #[test]
    fn a_closing_window_retires_its_generation_and_keeps_only_foreign_entries() {
        let session = Session::open(None);
        let owned = session.generation;
        let orphan = owned.wrapping_add(1_000);
        let left;
        let after;
        {
            let mut r = recorder();
            r.launched.push((owned, PathBuf::from(r"Z:\mine.exe")));
            r.launched.push((orphan, PathBuf::from(r"Z:\stray.exe")));
            retire_window(&mut r, owned);
            after = r.generation;
            left = r.launched.clone();
            // Restored before the lock is released, so this scratch state is
            // invisible to every other test.
            r.launched.clear();
            r.hashed.clear();
        }
        drop(session);

        assert_eq!(
            left,
            vec![(orphan, PathBuf::from(r"Z:\stray.exe"))],
            "closing a window either kept its own entries or threw away a foreign one. \
             Throwing the foreign one away is how a late launch used to vanish without \
             anybody being told"
        );
        assert_ne!(
            after, owned,
            "a closing window left its own generation current, so a launch still in flight \
             lands under a generation that a window is about to claim"
        );
    }

    /// The liveness control for [`assert_no_late_launch`], which no other
    /// test in this file can reach: a well-behaved suite never leaves a late
    /// launch in the recorder, so the only way to know the check would fire
    /// is to hand it one.
    #[test]
    #[should_panic(expected = "AFTER the routing window")]
    fn a_late_launch_left_in_the_recorder_is_a_failure() {
        assert_no_late_launch(&[(7, PathBuf::from(r"Z:\late-installer.exe"))]);
    }

    /// The same control for [`assert_no_late_hash`], and for the same
    /// reason: no test here can leave one behind, so it must be handed one.
    #[test]
    #[should_panic(expected = "AFTER the routing window")]
    fn a_late_hash_left_in_the_recorder_is_a_failure() {
        assert_no_late_hash(&[(7, PathBuf::from(r"Z:\late-installer.exe"))]);
    }

    // ---------------------------------------------------------------------
    // AND THE SEAM ITSELF
    //
    // A seam that is not pinned only moves the hole one level out: the tests
    // above observe what the HARNESS supplied, never what production supplies.
    // These two are what join them.
    // ---------------------------------------------------------------------

    /// **Both fields of the production [`UpdaterEnv`] are the real functions,
    /// compared BY ADDRESS.**
    ///
    /// The same hold `vault_window`'s
    /// `production_hands_the_window_the_real_functions` puts on its five spawn
    /// fields, and for the same measured reason: a wrapper written at module
    /// level -- `fn hash_when_enabled(p: &Path) -> Result<Sha256Digest,
    /// String> { if CHECKS_ENABLED { file_sha256(p) } else {
    /// Ok(whatever_the_release_said()) } }` -- still spells the real name,
    /// still leaves `production` defining nothing of its own, and is invisible
    /// to every routing test above, because those substitute this very
    /// pointer. It is a different address, so it fails here.
    ///
    /// This matters more for `hash` than it did for the signature check it
    /// replaced. The gate is an equality between what this function returns
    /// and what the release published, so a `hash` that returned the
    /// release's own digest would make the comparison agree with itself and
    /// leave every routing test above still green.
    ///
    /// What this does NOT cover, plainly: it says the pointer is the right
    /// FUNCTION, never what that function does. A hollowed-out `file_sha256`
    /// passes this -- and is why
    /// [`an_installer_whose_bytes_are_the_release_is_launched_and_kept`] pins
    /// the hasher against a published NIST vector rather than against itself.
    ///
    /// And one profile caveat, measured rather than assumed: `fn_addr_eq`
    /// does not survive identical-code folding, so under this crate's release
    /// profile (`lto = true, codegen-units = 1`) a byte-identical twin of
    /// [`launch_installer`] compares EQUAL to it and would pass this pin. A
    /// probe crate with that profile measured exactly that; in debug, which is
    /// what `cargo test` builds and what every number in this file's ledger
    /// was measured under, the two are distinguished.
    #[test]
    fn production_holds_the_real_hash_and_the_real_launch() {
        let env = UpdaterEnv::production();

        // Typed `let`s rather than casts off the `fn` items, so each is a `fn`
        // POINTER of exactly the field's type before any address is taken: a
        // signature drift is a compile error here, not a silently different
        // address.
        let real_hash: fn(&Path) -> Result<Sha256Digest, String> = file_sha256;
        let real_launch: fn(&Path) -> Result<(), String> = launch_installer;

        assert!(
            std::ptr::fn_addr_eq(env.hash, real_hash),
            "`UpdaterEnv::production` hands the launcher something other than the real \
             `file_sha256`. A wrapper, a forwarder or a flag-gated pass-through still SPELLS \
             the name, and the routing tests cannot see it because they substitute this \
             pointer. This is the assertion it fails"
        );
        assert!(
            std::ptr::fn_addr_eq(env.launch, real_launch),
            "`UpdaterEnv::production` hands the launcher something other than the real \
             `launch_installer`"
        );

        // CONTROL: the comparison discriminates. A function of the right
        // SIGNATURE that is not the right function reads as different, so the
        // assertions above are not something every pair of `fn` pointers has.
        let decoy: fn(&Path) -> Result<(), String> = not_the_launcher;
        assert!(
            !std::ptr::fn_addr_eq(env.launch, decoy),
            "control: a different function of the same signature compares EQUAL to the \
             production launcher, so every assertion above is vacuous"
        );
        // ...and the real one really does compare equal to itself, so the
        // control is not passing because comparison always answers `false`.
        assert!(
            std::ptr::fn_addr_eq(real_launch, real_launch),
            "control: `fn_addr_eq` answers `false` for one function against itself"
        );
    }

    /// The decoy [`production_holds_the_real_hash_and_the_real_launch`]
    /// compares against: `launch`'s signature exactly, and nothing else.
    fn not_the_launcher(_: &Path) -> Result<(), String> {
        unreachable!("never called -- this exists to have an address")
    }

    // ---------------------------------------------------------------------
    // THE SOURCE GUARDS, AND WHAT ANCHORS THEM
    //
    // Both guards below read the file's PRODUCTION SLICE. Two separate
    // things have to hold for that to mean anything: the slice has to end
    // where production code ends, and what is read out of it has to be code
    // rather than text that merely looks like code. Each was a measured
    // survivor; see `production_slice` and `code_chars` for the numbers.
    // ---------------------------------------------------------------------

    /// The marker that ends the production half of this file.
    ///
    /// Deliberately the gate and the module opener TOGETHER, and deliberately
    /// required to be UNIQUE -- see [`production_slice`].
    ///
    /// Spelled through `concat!` so that the ATTRIBUTE ITSELF appears in this
    /// file exactly once, at the one real gate. [`CUT_GATE`] and the walk
    /// below count that attribute in the raw text, and a copy of it sitting
    /// inside this constant's own source would make the count 2 for no reason
    /// anyone could act on.
    const TEST_MODULE_MARKER: &str = concat!("\r\n#[cfg(", "test)]\r\nmod tests {\r\n");

    /// Just the module opener, counted separately so that a forgery of only
    /// half the marker is caught too.
    const TEST_MODULE_OPENER: &str = "\r\nmod tests {\r\n";

    /// The production slice of this file: everything above the test module.
    /// Both source guards below read it, so neither can be blinded by
    /// anything written inside the test module.
    ///
    /// # The cut is anchored by UNIQUENESS, because it was forgeable
    ///
    /// Two earlier cuts both failed, in opposite directions.
    ///
    /// Cutting at the first `cfg` gate in the text failed LOUDLY: a doc
    /// comment that merely mentions the gate in a code span truncates the
    /// slice to nothing and reds both guards. Annoying, but it fails.
    ///
    /// Cutting at the first module opener failed SILENTLY, which is worse,
    /// and it was measured. Inserted immediately above the test module:
    ///
    /// ```ignore
    /// const ZZ_LAYOUT: &str = r"
    /// mod tests {
    /// ";
    ///
    /// fn zz_second_launch(p: &Path) {
    ///     let _ = ZZ_LAYOUT;
    ///     let _ = Command::new(p).spawn();
    /// }
    /// ```
    ///
    /// plus a call to it as the second line of [`apply_update_with`], SURVIVED
    /// at 2182 lib / 217 bin / 0 failed / 0 warnings. The raw string's second
    /// line became the FIRST occurrence of the marker, so the cut moved up
    /// above the forged helper, both guards read a slice in which nothing had
    /// changed, and a plain `Command::new(p).spawn()` ran before verification.
    /// The liveness control at the identical site -- the same helper and the
    /// same call with only the four-line raw string removed -- was KILLED at
    /// 2181/1. The raw string was the whole difference.
    ///
    /// The lesson is that a cut chosen by "first occurrence" can be MOVED by
    /// production text, and production text is what the guard is supposed to
    /// be judging. So the cut is not chosen by position any more: the marker
    /// must occur EXACTLY ONCE in the file, and so must the bare module opener
    /// on its own. A forgery does not move the cut, it adds a second
    /// occurrence -- and a second occurrence is the failure. Forging the gate
    /// and the opener together does not help, because that is two occurrences
    /// of both.
    ///
    /// The cost is the same loudness the first cut had, and it is the right
    /// trade: this file may no longer write the exact byte sequence
    /// `<CRLF>mod tests {<CRLF>` anywhere except at its one real test module,
    /// not in a doc comment, not in a raw string, not in a test fixture. That
    /// is a rule a reader can check by eye, and breaking it reds two tests
    /// with a message that says so.
    fn production_slice() -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/updater.rs");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("could not read {}: {e}", path.display()));

        let markers = text.matches(TEST_MODULE_MARKER).count();
        assert_eq!(
            markers, 1,
            "updater.rs contains {markers} occurrences of the test-module marker, not 1. \
             The production slice both source guards read is cut at this marker, so a \
             SECOND one -- in a raw string, a doc comment or a fixture -- would let \
             production code choose where the guards stop reading. That was measured \
             surviving at a full 2182 / 0 failed / 0 warnings, so it is a failure here \
             rather than a matter of taste"
        );
        let openers = text.matches(TEST_MODULE_OPENER).count();
        assert_eq!(
            openers, 1,
            "updater.rs contains {openers} occurrences of the bare test-module opener, \
             not 1; half a forged marker moves the cut just as well as a whole one"
        );

        let cut = text
            .find(TEST_MODULE_MARKER)
            .expect("updater.rs no longer declares a test-gated `mod tests`");

        // # And the slice is a PREFIX, so what is BELOW it is part of the
        // # question
        //
        // Everything the two guards read is `text[..cut]`. The cut POSITION is
        // unforgeable (above), but nothing about that says the FILE ends
        // there, and a plain non-gated module appended after the test module's
        // closing brace ships, is invisible to all three statements at once,
        // and was measured surviving at a full 2192 / 0 failed / 0 warnings
        // with a live `Command::new(..).spawn()` in it.
        //
        // The walk is called from HERE, rather than only from its own test, on
        // purpose: a mutant that closes `mod tests` early and re-gates the
        // remainder deletes any single guard test, and a guard that only one
        // test calls is exactly that cheap to delete. Every source guard in
        // this file goes through `production_slice`, so hiding an item below
        // the cut now means excising the launch-seam guard as well.
        // **The walk's result is BOUND HERE, not only in its own test.**
        // Calling it and dropping the tuple made it a statement whose only
        // failure mode is a panic -- and a walk handed a region that ends at
        // the gate panics about nothing at all. Handing it
        // `text[..text.find(CUT_GATE).unwrap() + CUT_GATE.len()]` was
        // measured surviving the whole suite at 2200 / 0 failed / 0 warnings.
        // The four controls below are what `breach.rs` carries at its own
        // helper, and they are here rather than in a test because a check
        // only one test performs is exactly that cheap to delete: the
        // depth-stuck mutant plus the deletion of that single test measured
        // 2199 / 0 failed / 0 warnings.
        let (visited, modules, closes, depth) = walk_below_the_cut(&text);
        assert!(
            visited > 100,
            "the region below the cut is {visited} lines, which is not a test module's \
             worth: the walk was handed an empty or truncated region and proves nothing"
        );
        assert_eq!(
            depth, 0,
            "the walk ran off the end of the file inside a module, so it stopped \
             inspecting top-level lines part way down"
        );
        assert_eq!(
            modules,
            crate::below_cut::column_zero_module_openers(
                &text[text.find(CUT_GATE).expect("the walk just found it")..],
            ),
            "the walk opened a different number of modules below the cut than there are \
             column-0 module openers down there. DERIVED from the source rather than pinned \
             to a digit: a bare literal plus a gated second module were two coordinated \
             edits that between them widened this control without touching a word of its \
             prose. This is a NON-VACUITY control and nothing more -- it shares the opener \
             predicate with the walk it controls, so it proves the walk really opened what \
             is there, not that the predicate is right. What catches a planted item is the \
             brace-matched close, above."
        );
        assert_eq!(
            closes, modules,
            "control: every module the walk opened must also have been closed"
        );

        text[..cut].to_string()
    }

    /// One character of this file that the COMPILER would see.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    struct CodeChar {
        ch: char,
        in_literal: bool,
    }

    /// `text` with every comment removed and every run of whitespace removed,
    /// as a sequence of characters each tagged with whether it came from
    /// inside a string or character literal.
    ///
    /// # Why the guards below read this rather than the raw text
    ///
    /// A guard that counts a needle in raw source counts it in prose and in
    /// string data too, which cuts both ways: a doc comment that mentions the
    /// needle reds the guard for nothing, and -- the direction that matters --
    /// a needle a guard is counting UP TO A LIMIT can be spent harmlessly
    /// inside a literal. Stripping comments and tagging literals is what makes
    /// a count a statement about code.
    ///
    /// It also makes whitespace irrelevant, which is what closes the UFCS and
    /// spacing families in one move: `Command :: new`, `Command\n    ::new` and
    /// `Command::new` all render identically here.
    ///
    /// Handles line comments, nested block comments, normal strings with
    /// escapes, raw strings with any number of hashes, byte and C string
    /// prefixes, and character literals -- distinguished from lifetimes by
    /// looking for the closing quote.
    fn code_chars(text: &str) -> Vec<CodeChar> {
        let chars: Vec<char> = text.chars().collect();
        let n = chars.len();
        let at = |k: usize| -> char {
            if k < n {
                chars[k]
            } else {
                '\0'
            }
        };
        let mut out: Vec<CodeChar> = Vec::new();
        let push = |ch: char, in_literal: bool, out: &mut Vec<CodeChar>| {
            if !ch.is_whitespace() {
                out.push(CodeChar { ch, in_literal });
            }
        };
        let mut i = 0usize;
        while i < n {
            let c = chars[i];

            if c == '/' && at(i + 1) == '/' {
                while i < n && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            if c == '/' && at(i + 1) == '*' {
                let mut depth = 1usize;
                i += 2;
                while i < n && depth > 0 {
                    if chars[i] == '/' && at(i + 1) == '*' {
                        depth += 1;
                        i += 2;
                    } else if chars[i] == '*' && at(i + 1) == '/' {
                        depth -= 1;
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                continue;
            }

            // A raw string, possibly behind a `b` or `c` prefix. Only if the
            // `r` does not continue an identifier, so `for` and `str` are not
            // mistaken for one.
            let raw_r = if c == 'r' {
                Some(i)
            } else if (c == 'b' || c == 'c') && at(i + 1) == 'r' {
                Some(i + 1)
            } else {
                None
            };
            if let Some(r) = raw_r {
                let fresh = out
                    .last()
                    .map_or(true, |p| !(p.ch.is_alphanumeric() || p.ch == '_'));
                let mut h = r + 1;
                while at(h) == '#' {
                    h += 1;
                }
                let hashes = h - r - 1;
                if fresh && at(h) == '"' {
                    for k in i..=h {
                        push(chars[k], true, &mut out);
                    }
                    i = h + 1;
                    while i < n {
                        if chars[i] == '"' {
                            let mut k = i + 1;
                            let mut got = 0usize;
                            while got < hashes && at(k) == '#' {
                                k += 1;
                                got += 1;
                            }
                            if got == hashes {
                                for m in i..k {
                                    push(chars[m], true, &mut out);
                                }
                                i = k;
                                break;
                            }
                        }
                        push(chars[i], true, &mut out);
                        i += 1;
                    }
                    continue;
                }
            }

            if c == '"' {
                push(c, true, &mut out);
                i += 1;
                while i < n {
                    let d = chars[i];
                    if d == '\\' {
                        push(d, true, &mut out);
                        if i + 1 < n {
                            push(chars[i + 1], true, &mut out);
                        }
                        i += 2;
                        continue;
                    }
                    push(d, true, &mut out);
                    i += 1;
                    if d == '"' {
                        break;
                    }
                }
                continue;
            }

            // A character literal, as opposed to a lifetime: `'\n'` or `'x'`.
            if c == '\'' && (at(i + 1) == '\\' || at(i + 2) == '\'') {
                push(c, true, &mut out);
                i += 1;
                while i < n {
                    let d = chars[i];
                    if d == '\\' {
                        push(d, true, &mut out);
                        if i + 1 < n {
                            push(chars[i + 1], true, &mut out);
                        }
                        i += 2;
                        continue;
                    }
                    push(d, true, &mut out);
                    i += 1;
                    if d == '\'' {
                        break;
                    }
                }
                continue;
            }

            push(c, false, &mut out);
            i += 1;
        }
        out
    }

    /// The code with every literal's contents ERASED: what is left is only
    /// what the programmer wrote as syntax. Counting identifiers here cannot
    /// be inflated or satisfied by string data.
    fn code_without_literals(cc: &[CodeChar]) -> String {
        cc.iter().filter(|c| !c.in_literal).map(|c| c.ch).collect()
    }

    /// The code with literals KEPT: used only for the one exact-equality pin
    /// below, where the literals are half the thing being pinned and equality
    /// cannot be forged by adding text elsewhere.
    fn code_with_literals(cc: &[CodeChar]) -> String {
        cc.iter().map(|c| c.ch).collect()
    }

    /// The whole of `fn <header> { .. }`, located by BRACE DEPTH over code
    /// characters, so a brace inside a comment or a string cannot move its
    /// end. Rendered with literals kept and whitespace removed.
    fn code_fn(cc: &[CodeChar], header: &str) -> Option<String> {
        let flat = code_with_literals(cc);
        // `flat` is one char per entry of `cc`, so a char index into one is a
        // char index into the other.
        let flat_chars: Vec<char> = flat.chars().collect();
        let needle: Vec<char> = header.chars().collect();
        let start = (0..flat_chars.len().saturating_sub(needle.len()) + 1)
            .find(|&s| flat_chars[s..s + needle.len()] == needle[..])?;
        let mut depth = 0usize;
        let mut seen_open = false;
        for k in start..cc.len() {
            if cc[k].in_literal {
                continue;
            }
            if cc[k].ch == '{' {
                depth += 1;
                seen_open = true;
            } else if cc[k].ch == '}' {
                depth -= 1;
                if seen_open && depth == 0 {
                    return Some(flat_chars[start..=k].iter().collect());
                }
            }
        }
        None
    }

    /// The header of the one function in this module allowed to start a
    /// process, and its whole body, pinned exactly.
    ///
    /// **Re-pinned when the app mutex was added.** The `app_mutex::release()`
    /// line below is part of the pin for the same reason the two silent-install
    /// flags are: without it, setup finds this still-running app holding the
    /// mutex `installer/deskwarden.iss` names in `AppMutex=` and stops to ask a
    /// user who is not there, on a run whose message boxes are suppressed.
    /// Deleting it is as much a broken self-update as deleting `/VERYSILENT`,
    /// and it is exactly as invisible.
    const LAUNCH_SEAM_HEADER: &str = "fnlaunch_installer(installer_path:&Path)->Result<(),String>";
    const LAUNCH_SEAM: &str = concat!(
        "fnlaunch_installer(installer_path:&Path)->Result<(),String>{",
        "crate::app_mutex::release();",
        "Command::new(installer_path)",
        ".args([\"/VERYSILENT\",\"/SUPPRESSMSGBOXES\",\"/MERGETASKS=!autostart\"])",
        ".spawn()",
        ".map_err(|e|format!(\"failedtolaunchinstaller:{e}\"))?;",
        "Ok(())}",
    );

    /// **[`UpdaterEnv::production`] is the only constructor a shipping build
    /// compiles.**
    ///
    /// [`production_holds_the_real_hash_and_the_real_launch`] pins the value
    /// `production()` builds. It is worth nothing if a shipping build has a
    /// SECOND constructor that some call site can reach instead -- a
    /// `pub fn permissive() -> Self` is one line, and the address test never
    /// looks at it. Every constructor of this type has to spell `-> Self` or
    /// `-> UpdaterEnv`; the production slice contains exactly one, counted over
    /// CODE rather than over text, so a doc comment or a string cannot add to
    /// or excuse the count.
    ///
    /// The test-only substitute is written in the test module, below the cut,
    /// and is therefore not compiled into a shipping build at all.
    #[test]
    fn production_is_the_only_updater_env_a_shipping_build_has() {
        let code = code_without_literals(&code_chars(&production_slice()));
        let constructors = code.matches("->Self").count() + code.matches("->UpdaterEnv").count();
        assert_eq!(
            constructors, 1,
            "the production half of updater.rs declares {constructors} functions returning an \
             `UpdaterEnv`, not 1. `production` is meant to be the only env a shipping build can \
             build; a second one is a launcher whose signature check is whatever its caller \
             picked"
        );
        assert!(
            code.contains("pubfnproduction()->Self"),
            "updater.rs's production slice no longer declares `production() -> Self`"
        );
    }

    /// **The module starts a process in exactly one place, that place is
    /// [`launch_installer`], and its body is exactly these bytes.**
    ///
    /// # This guard used to be a denylist, and the denylist kept losing
    ///
    /// It counted the strings `Command::new` and `.spawn()` in the production
    /// slice and required one of each. Every measured escape from it was a
    /// different SPELLING of the same act:
    ///
    ///  * `<Command>::new(&p).args([..]).status()` -- UFCS breaks the
    ///    `Command::new` substring and `.status()` is not `.spawn()`. SURVIVED
    ///    at 2182 / 0 failed / 0 warnings.
    ///  * `.output()` in place of `.status()`, identically.
    ///  * `type C = std::process::Command; C::new(p)`.
    ///  * `let f = Command::new;` -- the constructor as a value, never called
    ///    by that name.
    ///  * `std::thread::Builder::new().spawn(..)` -- which is not a process
    ///    start at all, but got a launch onto a thread no assertion watched.
    ///
    /// Widening the list is what lost four times. So the list is gone. What
    /// replaces it is three closed statements about the production slice, read
    /// as CODE (see [`code_chars`]) rather than as text:
    ///
    /// 1. **The seam's body is pinned exactly.** Not "contains a spawn" --
    ///    equals [`LAUNCH_SEAM`], arguments included. This is also the only
    ///    thing holding the installer's silent-install flags: deleting the
    ///    whole `.args([..])` line was measured SURVIVING the previous shape of
    ///    this suite at a full 2182 / 217 / 0 failed / 0 warnings, which would
    ///    have shipped an updater that pops an interactive installer UI, and
    ///    which says the seam's ARGUMENTS were never pinned by anything.
    /// 2. **`Command` is named exactly twice in the whole production slice**:
    ///    once by the `use` that imports it and once inside the pinned body.
    ///    This is not a list of spellings -- it is the observation that every
    ///    way to start a child through `std::process` has to NAME the type
    ///    somewhere, whatever punctuation surrounds the name and whichever of
    ///    `spawn`, `status` or `output` finishes the job. UFCS names it. An
    ///    alias names it. A `use .. as` names it. Taking the constructor as a
    ///    value names it.
    /// 3. **The production slice contains no `unsafe` and no `thread`.** The
    ///    only way left to start a process without naming `Command` is to call
    ///    Win32 directly, which needs `unsafe`; and the only way to get a call
    ///    onto a thread no routing assertion is watching is to name a thread.
    ///    Both are zero here and neither is anything this module has ever had a
    ///    use for, so both are cheap.
    ///
    /// # What this does NOT cover
    ///
    /// It reads THIS FILE only. `updater.rs` is on `job_object.rs`'s child-
    /// start `ALLOWED` list, so a spawn moved into another module and called
    /// from here is that guard's business, not this one's -- and a `pub`
    /// forwarder in a non-`ALLOWED` file has its own history there.
    #[test]
    fn the_only_process_start_in_this_module_is_the_launch_seam() {
        let slice = production_slice();
        let cc = code_chars(&slice);
        let code = code_without_literals(&cc);

        // 1. The seam, byte for byte.
        let body = code_fn(&cc, LAUNCH_SEAM_HEADER)
            .expect("updater.rs no longer declares `launch_installer` with its pinned header");
        assert_eq!(
            body, LAUNCH_SEAM,
            "the one process start in updater.rs is no longer exactly the pinned seam. Its \
             body, its arguments and its error mapping are all part of the pin: dropping \
             `/VERYSILENT` and `/SUPPRESSMSGBOXES` is an interactive installer on a user's \
             screen, and adding anything is a second thing happening at the one point in \
             this crate that turns a file into a running process"
        );

        // 2. The type is named twice: the import, and the seam.
        let named = code.matches("Command").count();
        assert_eq!(
            named, 2,
            "updater.rs's production code names `Command` {named} times, not 2 (the `use` \
             and the launch seam). Every way to start a child through `std::process` names \
             the type somewhere -- `<Command>::new`, `type C = Command`, `use .. as C`, \
             `let f = Command::new`, `.status()`, `.output()` -- so this count is the \
             statement, not a list of the spellings"
        );
        assert!(
            code.contains("usestd::process::Command;"),
            "updater.rs no longer imports `Command` by its own name, so the count above is \
             counting something else"
        );
        assert_eq!(
            body.matches("Command").count(),
            1,
            "the pinned seam does not name `Command`, so the two names counted above are \
             both somewhere else"
        );

        // 3. No Win32 process creation, and no threads.
        let unsafes = code.matches("unsafe").count();
        assert_eq!(
            unsafes, 0,
            "updater.rs's production code contains {unsafes} `unsafe` blocks. It has never \
             needed one, and `unsafe` is what a direct `CreateProcessW` would need -- the \
             one way left to start a process without naming `Command`"
        );
        let threads = code.matches("thread").count();
        assert_eq!(
            threads, 0,
            "updater.rs's production code names `thread` {threads} times. It must name it \
             none: a call moved onto a thread is a call the routing tests observe only by \
             the grace of a timing window, and \
             `std::thread::Builder::new().spawn(move || zz_l(&zz_p))` inserted above the \
             verify call was measured SURVIVING the whole suite at 2182 / 0 failed / 0 \
             warnings for exactly that reason"
        );
    }

    // ---------------------------------------------------------------------
    // AND THE SCANNER THE TWO GUARDS ABOVE STAND ON
    //
    // `code_chars` is now load-bearing for three counts and one equality. A
    // scanner that quietly returned an empty string would make all four
    // vacuous, so it is pinned here directly.
    // ---------------------------------------------------------------------

    fn erased(text: &str) -> String {
        code_without_literals(&code_chars(text))
    }

    fn kept(text: &str) -> String {
        code_with_literals(&code_chars(text))
    }

    #[test]
    fn the_scanner_drops_comments_and_whitespace() {
        assert_eq!(erased("let a = 1; // Command::new"), "leta=1;");
        assert_eq!(erased("/// Command::new\r\nlet a = 1;"), "leta=1;");
        assert_eq!(erased("/* Command::new */ let a = 1;"), "leta=1;");
        assert_eq!(erased("/* a /* b */ Command::new */ let a = 1;"), "leta=1;");
        assert_eq!(erased("Command :: new ( p )"), "Command::new(p)");
        assert_eq!(erased("Command\r\n    ::new(p)"), "Command::new(p)");
        assert_eq!(erased("<Command>::new(p)"), "<Command>::new(p)");
    }

    #[test]
    fn the_scanner_erases_literals_but_keeps_their_shape() {
        assert_eq!(erased("let s = \"Command::new\";"), "lets=;");
        assert_eq!(kept("let s = \"Command::new\";"), "lets=\"Command::new\";");
        assert_eq!(erased("let s = r\"Command::new\";"), "lets=;");
        assert_eq!(erased("let s = r#\"a \" Command::new\"#;"), "lets=;");
        assert_eq!(erased("let s = \"a \\\" Command::new\";"), "lets=;");
        assert_eq!(erased("let c = '\\'';let d = 1;"), "letc=;letd=1;");
        // A lifetime is not a character literal.
        assert_eq!(erased("fn f<'a>(x: &'a str) {}"), "fnf<'a>(x:&'astr){}");
        // `for` and `str` do not open a raw string.
        assert_eq!(erased("for x in y {}"), "forxiny{}");
    }

    #[test]
    fn the_scanner_finds_a_function_by_brace_depth_not_by_text() {
        let src = concat!(
            "fn f(a: u8) -> u8 {\r\n",
            "    // }\r\n",
            "    let s = \"}\";\r\n",
            "    if a > 0 { return 1; }\r\n",
            "    s.len() as u8\r\n",
            "}\r\n",
            "fn g() {}\r\n",
        );
        let cc = code_chars(src);
        assert_eq!(
            code_fn(&cc, "fnf(a:u8)->u8").unwrap(),
            "fnf(a:u8)->u8{lets=\"}\";ifa>0{return1;}s.len()asu8}"
        );
        assert_eq!(code_fn(&cc, "fnnosuchfn()"), None);
    }

    /// The scanner is not silently returning nothing: the real file's
    /// production slice renders to something substantial, and the needles the
    /// guards count are actually present in it.
    #[test]
    fn the_scanner_reads_this_file_as_code_rather_than_as_nothing() {
        let code = erased(&production_slice());
        assert!(
            code.len() > 1000,
            "updater.rs's production slice renders to {} characters of code; the scanner is \
             returning nothing and every count held over it is vacuous",
            code.len()
        );
        assert!(code.contains("fnlaunch_installer("));
        assert!(code.contains("fnapply_update_with("));
        assert!(code.contains("pubfnapply_update("));
    }

    // ---------------------------------------------------------------------
    // AND WHAT "PRODUCTION" MEANS, WHICH IS NO LONGER "A PREFIX"
    //
    // [`production_slice`] returns `text[..cut]`, and all three statements in
    // `the_only_process_start_in_this_module_is_the_launch_seam` plus the
    // constructor count in `production_is_the_only_updater_env_a_shipping_
    // build_has` read that prefix and nothing else. C2 made the cut POSITION
    // unforgeable -- the marker and the bare opener are each required to
    // occur exactly once -- and that is a real property, but it is a property
    // about where the prefix ENDS. Nothing said the FILE ends there.
    //
    // Measured on 0cd9fe0: a plain, non-gated
    //
    //     mod zz_below { pub fn go(p: &Path) -> Result<(), String> { .. } }
    //
    // appended after the test module's closing brace, containing a
    // `Command::new(p).spawn()` behind an unsatisfiable condition, plus a call
    // to it immediately above the `(env.verify)` call in `apply_update_with`,
    // SURVIVED at 2192 / 0 failed / 0 warnings -- and `cargo build --lib`
    // confirms it is genuinely compiled into a shipping build. Below the cut,
    // `Command`, `unsafe` and `thread` are all free, so ALL THREE statements
    // fall at once; and `job_object.rs`'s crate-wide child-start walk does not
    // help, because `updater.rs` is on its `ALLOWED` list. The liveness
    // control was the byte-identical module and call site with the module
    // placed ABOVE the marker: KILLED at 2191/1, on the `Command` count of 3.
    // The only difference between the two was which side of the cut it sat on.
    //
    // So production is defined here instead: everything above the cut, PLUS
    // the standing fact that below the cut there is nothing but test-gated
    // modules. The two-state walk that says so is the shape `breach.rs`,
    // `vault_export.rs` and `send.rs` already carry and that survived
    // adversarial review there, reused rather than reinvented.
    // ---------------------------------------------------------------------

    /// The `cfg` attribute that makes a module test-only, split so this
    /// constant is not itself one and cannot be found by a search for the real
    /// attribute. The same reason [`TEST_MODULE_MARKER`] is a `concat!`.
    const CUT_GATE: &str = concat!("#[cfg(", "test)]");

    /// Column-0 lines below the cut that are the CONTENTS OF A STRING LITERAL
    /// rather than source. Empty today, and controlled by the walk: a line
    /// that stops being one fails rather than being quietly forgiven.
    const BELOW_CUT_STRING_LINES: &[&str] = &[];

    /// `true` for `mod NAME {`, `pub mod NAME {` and `pub(crate) mod NAME {`,
    /// and for nothing else. Exact rather than a `starts_with`: a whole module
    /// written on one line is not a module opener here and must fail.
    fn below_cut_is_module_opener(line: &str) -> bool {
        let t = line.strip_prefix("pub(crate) ").unwrap_or(line);
        let t = t.strip_prefix("pub ").unwrap_or(t);
        let rest = match t.strip_prefix("mod ") {
            Some(rest) => rest,
            None => return false,
        };
        let name = match rest.strip_suffix(" {") {
            Some(name) => name,
            None => return false,
        };
        !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    }

    /// The byte offset, within `region`, of the `}` that matches the `{`
    /// at `open`, by a real brace count over source text.
    ///
    /// # Why a brace count rather than one more line rule
    ///
    /// The walk's model of "the module ended" was the line `}` at column 0,
    /// and inside a test module every item is indented -- so a module close
    /// written INDENTED (legal Rust) was not recognised, the walk stayed at
    /// depth 1 to EOF, and every line after it went uninspected at any
    /// indentation: top-level items, at file scope, below the cut, in the
    /// half no guard here reads. Appending a column-0 `}` further down
    /// rebalanced `closes` and `depth` as well, so the non-vacuity controls
    /// could not see it either.
    ///
    /// **The matcher itself lives in [`crate::below_cut`] and NOT here.**
    /// It was copy-pasted into three files and had to reach six; all three
    /// copies carried an identical off-by-one on `'\''` that let a payload
    /// open a phantom string, swallow a module's closing brace, and survive
    /// the whole suite while shipping. One implementation, six callers.
    /// The two-state walk from the cut to EOF over whatever text it is handed.
    /// Returns `(visited, modules, closes, depth)` so the caller can control it
    /// for non-vacuity.
    ///
    /// **Line-ending agnostic on purpose.** `lines()` strips a trailing
    /// carriage return, so every comparison is against the line's real text on
    /// a CRLF working tree and on an LF one alike.
    fn walk_below_the_cut(source: &str) -> (usize, usize, usize, usize) {
        let cut = source
            .find(CUT_GATE)
            .expect("the cut marker is controlled by the caller");
        let mut depth = 0usize;
        // The walked region BEGINS with the gate, so nothing inside it is
        // taken on trust: the first line seen is the attribute itself.
        let mut gated = false;
        let (mut modules, mut closes, mut visited) = (0usize, 0usize, 0usize);
        // Byte offsets are carried alongside each line so a module opener can
        // be brace-matched and its REAL close pinned; see
        // [`crate::below_cut::match_brace`] for what that closes.
        let region = &source[cut..];
        let mut expected_close: Option<usize> = None;
        let mut at = 0usize;
        let mut numbered: Vec<(usize, &str)> = Vec::new();
        for raw in region.split_inclusive('\n') {
            numbered.push((at, raw.trim_end_matches('\n').trim_end_matches('\r')));
            at += raw.len();
        }
        for &(offset, line) in &numbered {
            visited += 1;
            if depth == 0 {
                // Between modules NOTHING is allowed but blanks, comments, the
                // gate and a module opener -- at ANY indentation, because an
                // indented `fn` at file scope is still a top-level item and a
                // column-0-only filter would walk straight past it.
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with("//") {
                    continue;
                }
                if trimmed == CUT_GATE {
                    gated = true;
                    continue;
                }
                assert!(
                    !line.starts_with(char::is_whitespace) && below_cut_is_module_opener(trimmed),
                    "top-level source below the cut: {line:?}. `production_slice` is a PREFIX \
                     of this file, so every guard here reads only the half above the cut: an \
                     item down here can name `Command`, write `unsafe`, name a thread or add \
                     a second `-> Self` and every one of those counts stays word-perfect. \
                     Move it above the test module."
                );
                assert!(
                    gated,
                    "the module {line:?} below the cut is not test-gated, so it SHIPS -- and it \
                     ships in the half of the file no guard here reads. That exact shape was \
                     measured surviving the whole suite at 2192 / 0 failed / 0 warnings with a \
                     live `Command::new(..).spawn()` inside it"
                );
                gated = false;
                depth = 1;
                modules += 1;
                // Where this module REALLY ends, by brace count. Only that
                // line may be accepted as its close.
                let brace = offset
                    + line
                        .rfind('{')
                        .expect("a module opener ends in an opening brace");
                expected_close = Some(crate::below_cut::match_brace(region, brace));
            } else if !line.is_empty() && !line.starts_with(char::is_whitespace) {
                // Inside a test module every item is indented, so the only
                // column-0 line is the module's own closing brace.
                if line == "}" {
                    assert_eq!(
                        Some(offset),
                        expected_close,
                        "the column-0 `}}` at byte {offset} below the cut is not the brace \
                         that closes the module it appears to close ({expected_close:?}). \
                         The module was closed EARLIER, by an indented brace the line rule \
                         cannot see, and everything between the two was walked as if it \
                         were still module contents -- top-level items at file scope, in \
                         the half of this file no guard reads. Measured surviving at \
                         2199 / 0 failed / 0 warnings."
                    );
                    expected_close = None;
                    depth = 0;
                    closes += 1;
                    continue;
                }
                assert!(
                    BELOW_CUT_STRING_LINES.contains(&line),
                    "a column-0 line inside a test module below the cut: {line:?}. Either a \
                     top-level item escaped the brace count, or this is the contents of a \
                     string literal and belongs in BELOW_CUT_STRING_LINES"
                );
            }
        }
        (visited, modules, closes, depth)
    }

    #[test]
    fn nothing_but_gated_test_modules_lives_below_the_guards_cut() {
        let source = include_str!("updater.rs");

        // 1. The cut lands where the guards think it does, and there is
        //    exactly one place it could land -- so it cannot move UP into a
        //    comment or a string and silently truncate the half they read.
        let seen = source.matches(CUT_GATE).count();
        assert_eq!(
            seen, 1,
            "the test gate occurs {seen} times in this file. `production_slice` cuts at the \
             FIRST, so a second occurrence is a cut that can move up and vacate every guard \
             below the truncation while their own text stays word-perfect"
        );
        let cut = source.find(CUT_GATE).expect("counted exactly one just above");
        assert!(
            cut > 0 && source.as_bytes()[cut - 1] == b'\n',
            "the cut landed in the MIDDLE of a line, so the gate was matched inside a comment \
             or a string literal rather than at a real attribute"
        );

        // 2. Positive control on WHERE the cut is: the production half still
        //    reaches the last production item in the file.
        const LAST_PRODUCTION_ITEM: &str =
            concat!("apply_update_with(dest_dir, release, ", "&UpdaterEnv::production())");
        assert_eq!(
            source.matches(LAST_PRODUCTION_ITEM).count(),
            1,
            "control: the anchor is not in this file exactly once, so it pins nothing -- \
             repoint it at the last production item above the test module"
        );
        let anchor = source.find(LAST_PRODUCTION_ITEM).expect("counted just above");
        assert!(
            anchor < cut,
            "the last production item this control knows about is BELOW the cut, so the cut \
             moved up and the production half every guard reads is truncated"
        );
        assert!(
            cut - anchor < 4_000,
            "the cut is more than 4000 bytes past the last production item this control knows \
             about: either production was appended below the anchor (repoint the anchor) or \
             the cut moved down"
        );

        // 3. The walk, over an LF copy and a CRLF copy of the same text, which
        //    must agree. Built both ways rather than compared against the bytes
        //    on disk: this repository stores LF blobs and only
        //    `core.autocrlf=true` makes a working tree CRLF, so a control that
        //    asserted "this file is CRLF" would pass here and fail on Linux.
        let lf = source.replace("\r\n", "\n");
        let crlf = lf.replace('\n', "\r\n");
        assert_ne!(
            lf, crlf,
            "control: the two copies are the same string, so comparing the walk over them \
             compares it with itself -- this file has no line endings at all"
        );
        assert_eq!(
            walk_below_the_cut(&lf),
            walk_below_the_cut(&crlf),
            "the walk gives a different answer on an LF copy of this file than on a CRLF one"
        );
        let on_disk = walk_below_the_cut(source);
        assert!(
            on_disk == walk_below_the_cut(&lf) || on_disk == walk_below_the_cut(&crlf),
            "this file's line endings are mixed: the walk over it agrees with neither the \
             all-LF nor the all-CRLF copy of its own text"
        );

        // 4. The walk is not vacuous, and it finished.
        let (visited, modules, closes, depth) = on_disk;
        assert!(
            visited > 100,
            "control: the walk visited only {visited} lines below the cut, which is not a test \
             module's worth -- the slice is empty and this test proves nothing"
        );
        assert_eq!(
            (modules, closes, depth),
            (1, 1, 0),
            "below the cut there is no longer exactly one opened-and-closed test module: \
             {modules} opened, {closes} closed, ending at depth {depth}"
        );

        // 5. Controls on the walk itself: it really refuses production code
        //    down there. Without these the walk could be a no-op that visits
        //    lines and asserts nothing.
        let with_an_appended_item = format!("{lf}\npub fn sneaked() {{}}\n");
        assert!(
            std::panic::catch_unwind(|| walk_below_the_cut(&with_an_appended_item)).is_err(),
            "control: the walk accepted a `pub fn` appended below the test module, which is \
             the exact mutation it exists to catch"
        );
        // And an INDENTED one, which a column-0 filter would miss.
        // The
        // payload is an indented, GATED module opener and not a `struct`: a
        // struct is refused whether or not indentation is checked, because
        // it is not a module opener either way, so it left the indentation
        // rule unmeasured. This shape the opener predicate accepts and the
        // walk would otherwise ACCEPT outright, so only the indentation rule
        // refuses it and deleting that rule reds this control.
        let with_an_indented_item =
            format!("{lf}\n{CUT_GATE}\n    mod sneaked_indented {{\n}}\n");
        assert!(
            std::panic::catch_unwind(|| walk_below_the_cut(&with_an_indented_item)).is_err(),
            "control: the walk accepted an INDENTED top-level item appended below the test \
             module"
        );
        // And the measured survivor itself: an ungated module, which ships.
        let with_an_ungated_module = format!("{lf}\nmod zz_below {{\n}}\n");
        assert!(
            std::panic::catch_unwind(|| walk_below_the_cut(&with_an_ungated_module)).is_err(),
            "control: the walk accepted an UNGATED module below the cut, which ships -- that \
             is the survivor, verbatim"
        );
        // And a module whose whole body is on one line, so the brace count
        // never sees an opener and would otherwise walk on at depth 0.
        let with_a_one_line_module = format!("{lf}\nmod zz_below {{ pub fn go() {{}} }}\n");
        assert!(
            std::panic::catch_unwind(|| walk_below_the_cut(&with_a_one_line_module)).is_err(),
            "control: the walk accepted a whole module written on ONE LINE below the cut"
        );
        // And a gate that is not THE gate: `#[cfg(not(test))]` ships.
        let with_an_inverted_gate =
            format!("{lf}\n#[cfg(not(test))]\nmod zz_below {{\n}}\n");
        assert!(
            std::panic::catch_unwind(|| walk_below_the_cut(&with_an_inverted_gate)).is_err(),
            "control: the walk accepted `#[cfg(not(test))]` as a test gate, which is the one \
             attribute that means the OPPOSITE"
        );
    }
}
