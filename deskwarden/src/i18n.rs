//! Translation: one English source catalog, thirty-five others, and the
//! functions every screen asks for a sentence with.
//!
//! ```text
//! i18n::t("login.title")                        // &'static str
//! i18n::tf("login.welcome", &[("name", name)])  // String, {name} filled in
//! i18n::tn("vault.items", n, &[])               // String, plural form for n
//! ```
//!
//! **Always call them qualified** -- `i18n::t(..)`, never a bare `t(..)`: the
//! test that checks every key in the code against `en.txt`, and the
//! extraction script, find calls by that spelling, and a short name like `t`
//! collides with test helpers.
//!
//! # Where the text lives
//!
//! `locales/<tag>.txt` beside `Cargo.toml`, one file per language, each
//! embedded with `include_str!` -- nothing is read from disk at run time and
//! nothing is downloaded. `locales/en.txt` is the source: every key is
//! written there first, and a key a translation lacks reads as the English
//! one. The other thirty-five files are created empty so the list of
//! languages is fixed by the build, not by whoever translates next.
//!
//! # The file format
//!
//! ```text
//! # A comment: a line whose first non-blank character is `#`.
//! login.title = Sign in to Deskwarden
//!
//! # `{name}` is a placeholder, filled by `tf`/`tn`. `{{` and `}}` are
//! # literal braces. Every translation must use exactly the placeholders
//! # the English line does -- a test fails otherwise.
//! login.welcome = Welcome back, {name}
//!
//! # A trailing `\` continues the value on the next line (whose leading
//! # blanks are dropped). `\n` is a line break, `\t` a tab, `\\` a
//! # backslash, `\s` a space that survives trimming, `\u{2026}` any
//! # character by code point.
//! login.hint = Your master password never leaves this computer, \
//!              and cannot be recovered if you forget it.
//!
//! # Plural forms are the key plus `[category]`. English needs `one` and
//! # `other`; a language needs the categories its rule uses (see below).
//! # `{n}` is always available in a plural entry.
//! vault.items[one]   = {n} item
//! vault.items[other] = {n} items
//! ```
//!
//! * A key is lower-case ASCII letters, digits, `_` and `.`, starting with a
//!   letter, and is named `area.thing` (`login.title`, `tray.open_vault`).
//!   A key is never reused for a different meaning: a second use that needs
//!   different words in some language is a second key.
//! * Leading and trailing blanks of a value are dropped; `\s` keeps one.
//! * A key defined twice, an unknown escape, a stray `{` or `}` or a plural
//!   category that is not `zero`/`one`/`two`/`few`/`many`/`other` is reported
//!   by the parser and skipped -- the string falls back to English, it never
//!   breaks the screen. `every_catalog_parses_without_a_problem` makes that
//!   report a red build.
//! * A value read through [`t`] must hold no placeholder and no braces;
//!   anything with `{..}` is read through [`tf`] or [`tn`].
//!
//! # Plurals
//!
//! [`tn`] picks the form by the CLDR rule of the active language, from the
//! small built-in table in [`plural`] -- exactly the locales below, whole
//! numbers only. A language lacking the exact form falls back to its own
//! `other`, then to English. Languages with no plural distinction (ja, ko,
//! zh, vi, id, ms) write `other` only.
//!
//! # Fallbacks, and what never happens
//!
//! A key missing from the active language reads as English. A key missing
//! from English reads as the key itself and is logged once at debug level. A
//! locale file that is empty, absent or unparsable is an English app. None
//! of these panic.
//!
//! # The active language
//!
//! Process-wide, set once at startup by [`init`]: the saved
//! `settings.json` `language` if there is one, else the first of the
//! Windows UI languages (`GetUserPreferredUILanguages`) that has a catalog,
//! else English. [`set_language`] changes it later (the Settings picker);
//! windows re-run `theme::apply` to pick up the fonts the new language needs.
//! [`with_language`] overrides it for the current thread only, which is how
//! tests and `ui_preview` render a screen in another language without
//! disturbing the test next to them.
//!
//! # The languages, and the four that are not here
//!
//! English (the source), then bg ca cs da de el es et fi fr hr hu id it ja
//! ko lt lv ms nb nl pl pt-BR pt-PT ro ru sk sl sr sv tr uk vi zh-Hans
//! zh-Hant.
//!
//! **Arabic, Hebrew, Hindi and Thai are not in the list and cannot be added
//! yet.** egui 0.35 does no text shaping and no right-to-left layout: Arabic
//! would draw as disconnected isolated letters in the wrong direction, Hebrew
//! in the wrong order, and Hindi and Thai without their conjuncts and
//! stacked marks. That is a limit of the toolkit, not of this module.
//!
//! Serbian is written in Latin script (Windows' own default, `sr-Latn`);
//! Norwegian is Bokmal (`nb`), and Windows' `no`/`nn` map to it.
//!
//! # Fonts
//!
//! Latin, Vietnamese, Cyrillic come from the bundled faces. Greek, Chinese,
//! Japanese and Korean come from system fonts in `C:\Windows\Fonts`, loaded
//! by `theme::apply` only when that language is active (a CJK face is 14-20
//! MB). See [`Locale::fonts`].

pub mod catalog;
pub mod plural;
mod system;

use std::cell::Cell;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use catalog::{Catalog, Piece};
use plural::{Category, Rule};

/// Which system fonts a language needs on top of the bundled ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fonts {
    /// The bundled Archivo and Noto Cyrillic faces cover it.
    Bundled,
    /// Greek: Segoe UI, because Archivo has no Greek.
    Greek,
    /// Simplified Chinese: Microsoft YaHei.
    ChineseSimplified,
    /// Traditional Chinese: Microsoft JhengHei.
    ChineseTraditional,
    /// Japanese: Yu Gothic, then Meiryo.
    Japanese,
    /// Korean: Malgun Gothic.
    Korean,
}

/// One language the app can be shown in.
#[derive(Debug)]
pub struct Locale {
    /// The BCP 47 tag, and the catalog's file name: `de`, `pt-BR`, `zh-Hans`.
    pub tag: &'static str,
    /// The language's name in itself, for the picker: `Deutsch`.
    pub native_name: &'static str,
    /// The name in English, for logs and for sorting a picker by it.
    pub english_name: &'static str,
    /// Its CLDR plural rule.
    pub rule: Rule,
    /// The fonts it needs beyond the bundled ones.
    pub fonts: Fonts,
    source: &'static str,
}

macro_rules! locale {
    ($tag:literal, $native:literal, $english:literal, $rule:expr, $fonts:expr) => {
        Locale {
            tag: $tag,
            native_name: $native,
            english_name: $english,
            rule: $rule,
            fonts: $fonts,
            source: include_str!(concat!("../locales/", $tag, ".txt")),
        }
    };
}

/// Every language, English first and then by tag. The index into this array
/// is the language's identity everywhere inside the module.
static LOCALES: [Locale; 36] = [
    locale!("en", "English", "English", Rule::OneOther, Fonts::Bundled),
    locale!("bg", "\u{411}\u{44a}\u{43b}\u{433}\u{430}\u{440}\u{441}\u{43a}\u{438}", "Bulgarian", Rule::OneOther, Fonts::Bundled),
    locale!("ca", "Catal\u{e0}", "Catalan", Rule::OneManyOther, Fonts::Bundled),
    locale!("cs", "\u{10c}e\u{161}tina", "Czech", Rule::CzechSlovak, Fonts::Bundled),
    locale!("da", "Dansk", "Danish", Rule::OneOther, Fonts::Bundled),
    locale!("de", "Deutsch", "German", Rule::OneOther, Fonts::Bundled),
    locale!("el", "\u{395}\u{3bb}\u{3bb}\u{3b7}\u{3bd}\u{3b9}\u{3ba}\u{3ac}", "Greek", Rule::OneOther, Fonts::Greek),
    locale!("es", "Espa\u{f1}ol", "Spanish", Rule::OneManyOther, Fonts::Bundled),
    locale!("et", "Eesti", "Estonian", Rule::OneOther, Fonts::Bundled),
    locale!("fi", "Suomi", "Finnish", Rule::OneOther, Fonts::Bundled),
    locale!("fr", "Fran\u{e7}ais", "French", Rule::ZeroOneManyOther, Fonts::Bundled),
    locale!("hr", "Hrvatski", "Croatian", Rule::SouthSlavic, Fonts::Bundled),
    locale!("hu", "Magyar", "Hungarian", Rule::OneOther, Fonts::Bundled),
    locale!("id", "Bahasa Indonesia", "Indonesian", Rule::Other, Fonts::Bundled),
    locale!("it", "Italiano", "Italian", Rule::OneManyOther, Fonts::Bundled),
    locale!("ja", "\u{65e5}\u{672c}\u{8a9e}", "Japanese", Rule::Other, Fonts::Japanese),
    locale!("ko", "\u{d55c}\u{ad6d}\u{c5b4}", "Korean", Rule::Other, Fonts::Korean),
    locale!("lt", "Lietuvi\u{173}", "Lithuanian", Rule::Lithuanian, Fonts::Bundled),
    locale!("lv", "Latvie\u{161}u", "Latvian", Rule::Latvian, Fonts::Bundled),
    locale!("ms", "Bahasa Melayu", "Malay", Rule::Other, Fonts::Bundled),
    locale!("nb", "Norsk bokm\u{e5}l", "Norwegian Bokmal", Rule::OneOther, Fonts::Bundled),
    locale!("nl", "Nederlands", "Dutch", Rule::OneOther, Fonts::Bundled),
    locale!("pl", "Polski", "Polish", Rule::Polish, Fonts::Bundled),
    locale!("pt-BR", "Portugu\u{ea}s (Brasil)", "Portuguese (Brazil)", Rule::ZeroOneManyOther, Fonts::Bundled),
    locale!("pt-PT", "Portugu\u{ea}s (Portugal)", "Portuguese (Portugal)", Rule::OneManyOther, Fonts::Bundled),
    locale!("ro", "Rom\u{e2}n\u{103}", "Romanian", Rule::Romanian, Fonts::Bundled),
    locale!("ru", "\u{420}\u{443}\u{441}\u{441}\u{43a}\u{438}\u{439}", "Russian", Rule::EastSlavic, Fonts::Bundled),
    locale!("sk", "Sloven\u{10d}ina", "Slovak", Rule::CzechSlovak, Fonts::Bundled),
    locale!("sl", "Sloven\u{161}\u{10d}ina", "Slovenian", Rule::Slovenian, Fonts::Bundled),
    locale!("sr", "Srpski", "Serbian", Rule::SouthSlavic, Fonts::Bundled),
    locale!("sv", "Svenska", "Swedish", Rule::OneOther, Fonts::Bundled),
    locale!("tr", "T\u{fc}rk\u{e7}e", "Turkish", Rule::OneOther, Fonts::Bundled),
    locale!("uk", "\u{423}\u{43a}\u{440}\u{430}\u{457}\u{43d}\u{441}\u{44c}\u{43a}\u{430}", "Ukrainian", Rule::EastSlavic, Fonts::Bundled),
    locale!("vi", "Ti\u{1ebf}ng Vi\u{1ec7}t", "Vietnamese", Rule::Other, Fonts::Bundled),
    locale!("zh-Hans", "\u{7b80}\u{4f53}\u{4e2d}\u{6587}", "Chinese (Simplified)", Rule::Other, Fonts::ChineseSimplified),
    locale!("zh-Hant", "\u{7e41}\u{9ad4}\u{4e2d}\u{6587}", "Chinese (Traditional)", Rule::Other, Fonts::ChineseTraditional),
];

/// The index of English, the source and the fallback.
const ENGLISH: usize = 0;

/// The languages the picker lists, English first. Each carries its own name
/// in its own script (`native_name`).
///
/// Drawing a CJK or Greek name needs that language's system font loaded,
/// which `theme::apply` only does for the *active* language -- see
/// `theme::set_fonts_for` for the picker's way to load the rest.
pub fn available() -> &'static [Locale] {
    &LOCALES
}

/// The locale with this exact tag, or one Windows or a user may have spelled
/// differently (`zh-CN`, `pt`, `nb-NO`, `en-GB`): see [`match_tag`].
pub fn locale(tag: &str) -> Option<&'static Locale> {
    match_tag(tag).map(|i| &LOCALES[i])
}

/// Which catalog a Windows or user-written language tag reads from.
///
/// Case-insensitive, `_` or `-`. A region the app has no catalog for falls
/// back to the language (`de-AT` is `de`); Chinese is split by script, with
/// the region deciding when no script is named; bare `pt` is Brazilian; `no`
/// and `nn` are Bokmal; `sr` in any script is Serbian; `in` is Indonesian
/// (Windows' legacy code).
pub fn match_tag(tag: &str) -> Option<usize> {
    let lower = tag.trim().to_ascii_lowercase().replace('_', "-");
    let mut parts = lower.split('-');
    let language = parts.next().filter(|l| !l.is_empty())?;
    let rest: Vec<&str> = parts.collect();
    let wanted: &str = match language {
        "zh" => {
            let traditional = rest.contains(&"hant")
                || (!rest.contains(&"hans")
                    && rest.iter().any(|r| matches!(*r, "tw" | "hk" | "mo")));
            if traditional {
                "zh-Hant"
            } else {
                "zh-Hans"
            }
        }
        "pt" => {
            if rest.contains(&"pt") {
                "pt-PT"
            } else {
                "pt-BR"
            }
        }
        "no" | "nn" | "nb" => "nb",
        "in" => "id",
        other => {
            return LOCALES.iter().position(|l| l.tag.eq_ignore_ascii_case(other));
        }
    };
    LOCALES.iter().position(|l| l.tag == wanted)
}

// ---- the active language ----------------------------------------------

static ACTIVE: AtomicUsize = AtomicUsize::new(ENGLISH);

thread_local! {
    /// [`with_language`]'s override: the language of this thread alone.
    static THREAD_OVERRIDE: Cell<Option<usize>> = const { Cell::new(None) };
}

fn active_index() -> usize {
    THREAD_OVERRIDE
        .with(|o| o.get())
        .unwrap_or_else(|| ACTIVE.load(Ordering::Relaxed))
}

/// The language in force: this thread's override if one is set, else the
/// process's.
pub fn current() -> &'static Locale {
    &LOCALES[active_index()]
}

/// Sets the language for the whole process. A tag with no catalog leaves it
/// unchanged and answers `false`.
pub fn set_language(tag: &str) -> bool {
    match match_tag(tag) {
        Some(i) => {
            ACTIVE.store(i, Ordering::Relaxed);
            true
        }
        None => false,
    }
}

/// The startup choice: the saved language when there is one and it names a
/// catalog, else the first Windows UI language that has a catalog, else
/// English. Sets it and answers what was chosen.
///
/// Called once, early, by every process that draws anything (the daemon and
/// each `--ui` window), before the first string is asked for.
pub fn init(saved: Option<&str>) -> &'static Locale {
    let chosen = choose(saved, &system::preferred_languages());
    ACTIVE.store(chosen, Ordering::Relaxed);
    log::info!("language: {}", LOCALES[chosen].tag);
    &LOCALES[chosen]
}

/// [`init`]'s decision, apart from the two things it reads.
fn choose(saved: Option<&str>, windows_preferred: &[String]) -> usize {
    saved
        .and_then(match_tag)
        .or_else(|| windows_preferred.iter().find_map(|tag| match_tag(tag)))
        .unwrap_or(ENGLISH)
}

/// Runs `body` with `tag` as the language of THIS THREAD, then puts the
/// thread back. Nothing else in the process sees it, so a test or a preview
/// can render in Japanese beside a test that expects English. A tag with no
/// catalog is English.
pub fn with_language<R>(tag: &str, body: impl FnOnce() -> R) -> R {
    struct Restore(Option<usize>);
    impl Drop for Restore {
        fn drop(&mut self) {
            THREAD_OVERRIDE.with(|o| o.set(self.0));
        }
    }
    let previous = THREAD_OVERRIDE.with(|o| o.replace(Some(match_tag(tag).unwrap_or(ENGLISH))));
    let _restore = Restore(previous);
    body()
}

// ---- catalogs ------------------------------------------------------------

static CATALOGS: [OnceLock<Catalog>; 36] = [const { OnceLock::new() }; 36];

fn catalog_of(index: usize) -> &'static Catalog {
    CATALOGS[index].get_or_init(|| {
        let parsed = catalog::parse(LOCALES[index].source);
        for problem in &parsed.problems {
            log::debug!("i18n: {}.txt: {problem}", LOCALES[index].tag);
        }
        parsed.catalog
    })
}

/// The parsed catalog of a locale, for tooling and tests.
pub fn catalog_for(locale: &Locale) -> &'static Catalog {
    let index = LOCALES
        .iter()
        .position(|l| std::ptr::eq(l, locale))
        .unwrap_or(ENGLISH);
    catalog_of(index)
}

/// A locale's raw catalog text, for tooling and tests.
pub fn source_of(locale: &Locale) -> &'static str {
    locale.source
}

/// The template for `key` (in plural form `plural`) in the active language,
/// else in English.
fn lookup(key: &str, plural: Option<Category>) -> Option<&'static str> {
    let active = active_index();
    if active != ENGLISH {
        if let Some(found) = catalog_of(active).get(key, plural) {
            return Some(found);
        }
    }
    catalog_of(ENGLISH).get(key, plural)
}

static REPORTED: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

/// A key English does not have: said once, at debug level, and the key stands
/// in for the text.
fn missing(key: &'static str) -> &'static str {
    if let Ok(mut reported) = REPORTED.lock() {
        if !reported.contains(&key) {
            reported.push(key);
            log::debug!("i18n: no English text for key `{key}`");
        }
    }
    key
}

// ---- the three calls ---------------------------------------------------------

/// The text for `key` in the active language, or English, or (for a key the
/// catalog does not have at all) the key itself. For text with no
/// placeholders; see [`tf`].
pub fn t(key: &'static str) -> &'static str {
    match lookup(key, None) {
        Some(text) => text,
        None => missing(key),
    }
}

/// The text for `key` with each `{name}` replaced by the matching value in
/// `args`. A placeholder with no argument stays as written, which is visible
/// and findable rather than silently empty.
pub fn tf(key: &'static str, args: &[(&str, &str)]) -> String {
    match lookup(key, None) {
        Some(template) => fill(template, args),
        None => missing(key).to_owned(),
    }
}

/// The plural form of `key` for `n` in the active language, with `{n}` and
/// any `args` filled in.
///
/// The active language's category for `n` is tried first, then its `other`,
/// and when the language has neither, English's form for `n`.
pub fn tn(key: &'static str, n: u64, args: &[(&str, &str)]) -> String {
    let active = active_index();
    let english = (catalog_of(ENGLISH), LOCALES[ENGLISH].rule);
    let found = if active == ENGLISH {
        render_plural(&[english], key, n, args)
    } else {
        render_plural(&[(catalog_of(active), LOCALES[active].rule), english], key, n, args)
    };
    found.unwrap_or_else(|| missing(key).to_owned())
}

/// The first catalog in `chain` with a form of `key` for `n` -- by that
/// catalog's own rule, else its `other` -- filled with `{n}` and `args`.
fn render_plural(
    chain: &[(&Catalog, Rule)],
    key: &str,
    n: u64,
    args: &[(&str, &str)],
) -> Option<String> {
    let count = n.to_string();
    for (catalog, rule) in chain {
        let form = catalog
            .get(key, Some(rule.category(n)))
            .or_else(|| catalog.get(key, Some(Category::Other)));
        if let Some(template) = form {
            let mut all: Vec<(&str, &str)> = Vec::with_capacity(args.len() + 1);
            all.push(("n", count.as_str()));
            all.extend_from_slice(args);
            return Some(fill(template, &all));
        }
    }
    None
}

/// Fills a template. Public so a caller with a template in hand (a test, a
/// tool) fills it the same way the three calls do.
pub fn fill(template: &str, args: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    for piece in catalog::pieces(template) {
        match piece {
            Piece::Text(text) => out.push_str(text),
            Piece::Brace(c) => out.push(c),
            Piece::Placeholder(name) => match args.iter().find(|(k, _)| *k == name) {
                Some((_, value)) => out.push_str(value),
                None => {
                    let _ = write!(out, "{{{name}}}");
                }
            },
            // The parser rejects these; a template that still has one is
            // shown as written.
            Piece::Bad(_) => {}
        }
    }
    out
}

/// A key, marked as one. `tk!("tray.open")` is just the literal -- it exists
/// so a table of keys (a `const` array of menu rows, say) is read by the
/// extraction script and the coverage test the same way a `t("..")` call is,
/// and the row is later passed to [`t`].
#[macro_export]
macro_rules! tk {
    ($key:literal) => {
        $key
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    fn parsed(text: &'static str) -> Catalog {
        catalog::parse(text).catalog
    }

    #[test]
    fn the_language_list_is_the_one_the_owner_asked_for() {
        let tags: Vec<&str> = available().iter().map(|l| l.tag).collect();
        assert_eq!(
            tags,
            [
                "en", "bg", "ca", "cs", "da", "de", "el", "es", "et", "fi", "fr", "hr", "hu",
                "id", "it", "ja", "ko", "lt", "lv", "ms", "nb", "nl", "pl", "pt-BR", "pt-PT",
                "ro", "ru", "sk", "sl", "sr", "sv", "tr", "uk", "vi", "zh-Hans", "zh-Hant"
            ]
        );
        for l in available() {
            assert!(!l.native_name.is_empty() && !l.english_name.is_empty(), "{}", l.tag);
        }
        for excluded in ["ar", "he", "hi", "th"] {
            assert!(match_tag(excluded).is_none(), "{excluded} needs shaping egui 0.35 lacks");
        }
    }

    #[test]
    fn every_catalog_parses_without_a_problem() {
        for l in available() {
            let problems = catalog::parse(l.source).problems;
            assert!(problems.is_empty(), "locales/{}.txt: {problems:#?}", l.tag);
        }
    }

    #[test]
    fn windows_and_user_spellings_find_their_catalog() {
        let tag = |s: &str| match_tag(s).map(|i| LOCALES[i].tag);
        assert_eq!(tag("en-US"), Some("en"));
        assert_eq!(tag("en-GB"), Some("en"));
        assert_eq!(tag("de-AT"), Some("de"));
        assert_eq!(tag("zh-CN"), Some("zh-Hans"));
        assert_eq!(tag("zh-SG"), Some("zh-Hans"));
        assert_eq!(tag("zh-Hans-CN"), Some("zh-Hans"));
        assert_eq!(tag("zh-TW"), Some("zh-Hant"));
        assert_eq!(tag("zh-HK"), Some("zh-Hant"));
        assert_eq!(tag("zh-Hant-TW"), Some("zh-Hant"));
        assert_eq!(tag("zh-Hant-HK"), Some("zh-Hant"));
        assert_eq!(tag("pt-BR"), Some("pt-BR"));
        assert_eq!(tag("pt-pt"), Some("pt-PT"));
        assert_eq!(tag("pt"), Some("pt-BR"));
        assert_eq!(tag("pt_BR"), Some("pt-BR"));
        assert_eq!(tag("nb-NO"), Some("nb"));
        assert_eq!(tag("nn-NO"), Some("nb"));
        assert_eq!(tag("no"), Some("nb"));
        assert_eq!(tag("sr-Latn-RS"), Some("sr"));
        assert_eq!(tag("sr-Cyrl-RS"), Some("sr"));
        assert_eq!(tag("in-ID"), Some("id"));
        assert_eq!(tag("ja-JP"), Some("ja"));
        assert_eq!(tag("UK"), Some("uk"));
        assert_eq!(tag("ar-SA"), None);
        assert_eq!(tag("he-IL"), None);
        assert_eq!(tag(""), None);
        assert_eq!(tag("-"), None);
    }

    /// A saved choice beats Windows; Windows is read in order and skips a
    /// language with no catalog; nothing at all is English.
    #[test]
    fn the_startup_choice_prefers_the_saved_language_then_windows_then_english() {
        let win = |tags: &[&str]| tags.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let name = |i: usize| LOCALES[i].tag;
        assert_eq!(name(choose(Some("de"), &win(&["fr-FR"]))), "de");
        assert_eq!(name(choose(None, &win(&["fr-FR", "de-DE"]))), "fr");
        assert_eq!(name(choose(None, &win(&["ar-SA", "he-IL", "de-DE"]))), "de");
        assert_eq!(name(choose(Some("xx"), &win(&["ja-JP"]))), "ja");
        assert_eq!(name(choose(None, &win(&["ar-SA"]))), "en");
        assert_eq!(name(choose(None, &[])), "en");
    }

    #[test]
    fn a_thread_override_is_scoped_to_the_thread_and_the_call() {
        assert_eq!(current().tag, "en");
        with_language("ja", || {
            assert_eq!(current().tag, "ja");
            with_language("zh-TW", || assert_eq!(current().tag, "zh-Hant"));
            assert_eq!(current().tag, "ja");
            // The process-wide language is untouched: the override is the thread's alone.
            assert_eq!(LOCALES[ACTIVE.load(Ordering::Relaxed)].tag, "en");
        });
        assert_eq!(current().tag, "en");
        with_language("nonsense", || assert_eq!(current().tag, "en"));
    }

    #[test]
    fn an_override_is_undone_when_the_body_panics() {
        let caught = std::panic::catch_unwind(|| with_language("de", || panic!("boom")));
        assert!(caught.is_err());
        assert_eq!(current().tag, "en");
    }

    /// **English is what the screens always said.** Each line pairs what the converted code
    /// answers now with the literal the code held before the catalog existed. The catalog
    /// text itself is checked against the original sources by `scripts/i18n/verify_english.py`;
    /// this is the same fact asked of the running functions, on the screens people see first.
    #[test]
    fn english_is_what_the_screens_always_said() {
        use crate::{kit::auth, login_ui, second_factor_ui as sf, tray, tray_popup};
        let pins: Vec<(String, &str)> = vec![
            (auth::headline().into(), "Your Bitwarden vault, in every app."),
            (auth::brand_foot().into(), "Uses your existing Bitwarden account"),
            (auth::hello_label().into(), "Unlock with Windows Hello"),
            (auth::hello_checking().into(), "Checking\u{2026}"),
            (auth::use_password_label().into(), "Use master password instead"),
            (auth::lock_heading().into(), "Welcome back"),
            (auth::lock_sub().into(), "Your vault is locked"),
            (login_ui::enter_email().into(), "Enter your email."),
            (login_ui::bad_email().into(), "That doesn't look like an email address."),
            (login_ui::enter_password().into(), "Enter your master password."),
            (login_ui::too_many_title().into(), "Too many tries"),
            (login_ui::sign_in_title().into(), "Log in"),
            (login_ui::friendly_auth_error("invalid master password"), "That master password didn't work. Check it and try again."),
            (login_ui::friendly_auth_error("rate limit"), "Too many failed attempts. Wait a few minutes, then try again."),
            (sf::factor_title(&crate::rest::api::SecondFactor::Authenticator).into(), "Authenticator app"),
            (sf::provider_name(7), "a security key (WebAuthn)"),
            (sf::provider_name(9), "an unrecognised two-step method (9)"),
            (sf::send_code_label().into(), "Send code"),
            (sf::message_for(sf::Trouble::CodeRejected).into(), "That code didn't work. Check it and try again \u{2014} codes change every 30 seconds."),
            (tray::fill_hint(&crate::hotkey::HotkeyStatus::Armed, "Ctrl+Alt+B"), "Fill:  Ctrl+Alt+B"),
            (tray_popup::label_for(true, 0, 0), "RECENT"),
            (tray_popup::label_for(false, 3, 9), "TOP 3 OF 9 MATCHES"),
            (tray_popup::toast_for(tray_popup::Field::Password, tray_popup::CopyResult::Copied).into(), "Password copied"),
        ];
        for (actual, english) in pins {
            assert_eq!(actual, english);
        }
    }

    /// In every language, a sentence its catalog has reads in that language and one
    /// it lacks reads in English -- so a half-translated build never shows a key.
    /// Asked of every non-plural English key, whatever the catalogs hold today.
    #[test]
    fn every_language_says_its_own_words_or_english_never_a_key() {
        let english = catalog_of(ENGLISH);
        for locale in available() {
            let own = catalog_for(locale);
            with_language(locale.tag, || {
                for (entry, english_text) in english.entries() {
                    if entry.plural.is_some() || english_text.contains('{') {
                        continue;
                    }
                    let expected = own.get(entry.key, None).unwrap_or(english_text);
                    assert_eq!(t(entry.key), expected, "{} in {}", entry.key, locale.tag);
                }
            });
        }
    }

    #[test]
    fn a_missing_key_reads_as_the_key_and_never_panics() {
        assert_eq!(t("no.such.key"), "no.such.key");
        assert_eq!(tf("no.such.key", &[("a", "b")]), "no.such.key");
        assert_eq!(tn("no.such.key", 3, &[]), "no.such.key");
        // And in a language with an empty catalog, the same.
        with_language("ja", || assert_eq!(t("no.such.key"), "no.such.key"));
    }

    #[test]
    fn placeholders_are_filled_and_unfilled_ones_stay_visible() {
        assert_eq!(fill("Hello, {name}!", &[("name", "Ada")]), "Hello, Ada!");
        assert_eq!(fill("{a}{b}{a}", &[("a", "1"), ("b", "2")]), "121");
        assert_eq!(fill("Hello, {name}!", &[]), "Hello, {name}!");
        assert_eq!(fill("{{literal}} {x}", &[("x", "v")]), "{literal} v");
        assert_eq!(fill("no placeholders", &[("x", "v")]), "no placeholders");
        // A value that itself contains braces is inserted as written, not
        // re-read as a template.
        assert_eq!(fill("{x}", &[("x", "{y}")]), "{y}");
    }

    #[test]
    fn a_plural_uses_the_active_languages_rule_and_falls_back_to_other_then_english() {
        let ru = parsed(
            "n[one] = {n} \u{43f}\u{430}\u{440}\u{43e}\u{43b}\u{44c}\n\
             n[few] = {n} \u{43f}\u{430}\u{440}\u{43e}\u{43b}\u{44f}\n\
             n[many] = {n} \u{43f}\u{430}\u{440}\u{43e}\u{43b}\u{435}\u{439}\n\
             n[other] = {n} \u{43f}\u{430}\u{440}\u{43e}\u{43b}\u{44f}\n\
             only_other[other] = {n} x\n",
        );
        let en = parsed("n[one] = {n} password\nn[other] = {n} passwords\nonly_en[one] = {n} thing\nonly_en[other] = {n} things\nonly_other[one] = one\nonly_other[other] = many\n");
        let chain = [(&ru, Rule::EastSlavic), (&en, Rule::OneOther)];
        let say = |key, n| render_plural(&chain, key, n, &[]).unwrap();
        assert!(say("n", 1).ends_with("\u{43f}\u{430}\u{440}\u{43e}\u{43b}\u{44c}"));
        assert!(say("n", 3).ends_with("\u{43f}\u{430}\u{440}\u{43e}\u{43b}\u{44f}"));
        assert!(say("n", 5).ends_with("\u{43f}\u{430}\u{440}\u{43e}\u{43b}\u{435}\u{439}"));
        assert_eq!(say("n", 21).split(' ').next(), Some("21"));
        // The language has the key but only `other`: its `other`, whatever n.
        assert_eq!(say("only_other", 1), "1 x");
        // The language lacks the key: English, by English's own rule.
        assert_eq!(say("only_en", 1), "1 thing");
        assert_eq!(say("only_en", 5), "5 things");
        assert_eq!(render_plural(&chain, "absent", 1, &[]), None);
    }

    #[test]
    fn a_plural_can_take_its_own_arguments_beside_n() {
        let en = parsed("moved[one] = {n} item moved to {folder}\nmoved[other] = {n} items moved to {folder}\n");
        let chain = [(&en, Rule::OneOther)];
        assert_eq!(
            render_plural(&chain, "moved", 2, &[("folder", "Work")]).unwrap(),
            "2 items moved to Work"
        );
    }

    // ---- the source and the catalogs agree -----------------------------------

    fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                rust_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    /// Every file of the crate that can ask for a string, as `(name, text)`.
    /// This module's own files are left out: their prose and tests are full of
    /// example calls.
    fn sources() -> Vec<(String, String)> {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        rust_files(&src, &mut files);
        files
            .into_iter()
            .filter(|p| {
                let rel = p.strip_prefix(&src).unwrap().to_string_lossy().replace('\\', "/");
                rel != "i18n.rs" && !rel.starts_with("i18n/")
            })
            .map(|p| {
                let text = std::fs::read_to_string(&p).unwrap();
                (p.strip_prefix(&src).unwrap().to_string_lossy().replace('\\', "/"), text)
            })
            .collect()
    }

    /// How a key was asked for.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Call {
        T,
        Tf,
        Tn,
        Tk,
    }

    /// Every `t("key")`, `tf("key"`, `tn("key"` and `tk!("key")` in the
    /// sources, as `(file, call, key)`. Whole lines of `//` comments are
    /// skipped; a call whose first argument is not a literal is not seen,
    /// which is why the extraction script refuses to convert one.
    fn used_keys() -> Vec<(String, Call, String)> {
        let mut found = Vec::new();
        for (file, text) in sources() {
            scan_text(&file, &text, &mut found);
        }
        found
    }

    fn scan_text(file: &str, text: &str, found: &mut Vec<(String, Call, String)>) {
        let needles = [
            (concat!("tk", "!(\""), Call::Tk),
            (concat!("i18n::tf", "(\""), Call::Tf),
            (concat!("i18n::tn", "(\""), Call::Tn),
            (concat!("i18n::t", "(\""), Call::T),
        ];
        for line in text.lines() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            for (needle, call) in needles {
                let mut from = 0;
                while let Some(at) = line[from..].find(needle) {
                    let start = from + at;
                    let key_from = start + needle.len();
                    if let Some(len) = line[key_from..].find('"') {
                        found.push((file.to_owned(), call, line[key_from..key_from + len].to_owned()));
                    }
                    from = key_from;
                }
            }
        }
    }

    fn english() -> &'static Catalog {
        catalog_of(ENGLISH)
    }

    /// The control: the scan really sees calls, so "every key exists" below is
    /// not vacuous -- and does not see look-alikes.
    #[test]
    fn the_source_scan_sees_the_calls_it_is_meant_to_see() {
        let sample = concat!(
            "let a = i18n::t(\"a.one\"); let b = crate::i18n::tf(\"b.two\", &[]);\n",
            "let c = i18n::tn(\"c.three\", 3, &[]); const D: &str = tk!(\"d.four\");\n",
            "// i18n::t(\"commented.out\")\n",
            "let e = set(\"not.a.key\"); let f = t(\"nor.this\"); let g = i18n::t(&format!(\"dynamic\"));\n",
        );
        let mut found = Vec::new();
        scan_text("sample.rs", sample, &mut found);
        let seen: Vec<(Call, &str)> = found.iter().map(|(_, c, k)| (*c, k.as_str())).collect();
        assert_eq!(
            seen,
            [
                (Call::Tf, "b.two"),
                (Call::T, "a.one"),
                (Call::Tk, "d.four"),
                (Call::Tn, "c.three"),
            ]
        );
    }

    #[test]
    fn every_key_the_source_asks_for_is_in_english() {
        let mut missing = BTreeSet::new();
        for (file, call, key) in used_keys() {
            let plural = matches!(call, Call::Tn);
            let present = if plural {
                english().get(&key, Some(Category::Other)).is_some()
            } else if call == Call::Tk {
                english().get(&key, None).is_some() || english().get(&key, Some(Category::Other)).is_some()
            } else {
                english().get(&key, None).is_some()
            };
            if !present {
                missing.insert(format!("{key}  ({file}, {call:?})"));
            }
        }
        assert!(missing.is_empty(), "keys used in code but not in locales/en.txt:\n{missing:#?}");
    }

    /// `t` returns the template as written, so a key read through it must have
    /// nothing in it that `tf` would have filled or unescaped.
    #[test]
    fn a_key_read_with_t_has_no_placeholders_or_braces() {
        let mut bad = BTreeSet::new();
        for (file, call, key) in used_keys() {
            if call != Call::T {
                continue;
            }
            if let Some(template) = english().get(&key, None) {
                if template.contains('{') || template.contains('}') {
                    bad.insert(format!("{key} ({file}) = {template}"));
                }
            }
        }
        assert!(bad.is_empty(), "use `tf` for these:\n{bad:#?}");
    }

    #[test]
    fn a_key_is_plural_exactly_when_it_is_read_with_tn() {
        let mut bad = BTreeSet::new();
        for (file, call, key) in used_keys() {
            let is_plural = english().get(&key, Some(Category::Other)).is_some();
            let is_plain = english().get(&key, None).is_some();
            match call {
                Call::Tn if !is_plural => {
                    bad.insert(format!("{key} ({file}): read with tn, not plural in en.txt"))
                }
                Call::T | Call::Tf if is_plural && !is_plain => {
                    bad.insert(format!("{key} ({file}): plural in en.txt, read without tn"))
                }
                _ => false,
            };
        }
        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[test]
    fn english_writes_both_forms_of_every_plural() {
        for (entry, _) in english().entries() {
            if entry.plural.is_some() {
                for form in [Category::One, Category::Other] {
                    assert!(
                        english().get(entry.key, Some(form)).is_some(),
                        "`{}` in en.txt lacks [{}]",
                        entry.key,
                        form.name()
                    );
                }
                assert!(
                    english().get(entry.key, None).is_none(),
                    "`{}` in en.txt is both plain and plural",
                    entry.key
                );
            }
        }
    }

    /// A key nobody asks for is a string a translator is paid to translate
    /// for nothing. The key must appear, quoted, somewhere in the sources --
    /// which also covers a table of keys written as `tk!` or as bare literals.
    #[test]
    fn every_english_key_is_used_somewhere() {
        let sources = sources();
        let mut orphans = BTreeSet::new();
        for (entry, _) in english().entries() {
            let quoted = format!("\"{}\"", entry.key);
            if !sources.iter().any(|(_, text)| text.contains(&quoted)) {
                orphans.insert(entry.key);
            }
        }
        assert!(orphans.is_empty(), "keys in en.txt that no source file mentions:\n{orphans:#?}");
    }

    /// A translation may have fewer keys than English -- those read as
    /// English -- and may never have more, or different placeholders, or a
    /// plural form its language's rule never answers.
    #[test]
    fn every_translation_only_translates_what_english_says() {
        for l in available().iter().filter(|l| l.tag != "en") {
            let theirs = catalog_for(l);
            for (entry, template) in theirs.entries() {
                let name = format!("locales/{}.txt: `{}`", l.tag, entry.key);
                let Some(source) = english()
                    .get(entry.key, entry.plural)
                    .or_else(|| english().get(entry.key, entry.plural.map(|_| Category::Other)))
                else {
                    panic!("{name} does not exist in en.txt");
                };
                assert_eq!(
                    english().get(entry.key, None).is_some(),
                    entry.plural.is_none(),
                    "{name} is plural in one language and not the other"
                );
                if let Some(form) = entry.plural {
                    assert!(
                        l.rule.categories().contains(&form),
                        "{name}[{}] is a form {} never uses; it uses {:?}",
                        form.name(),
                        l.english_name,
                        l.rule.categories().iter().map(|c| c.name()).collect::<Vec<_>>()
                    );
                }
                let strip_n = |mut set: BTreeSet<String>| {
                    if entry.plural.is_some() {
                        set.remove("n");
                    }
                    set
                };
                assert_eq!(
                    strip_n(catalog::placeholders(template)),
                    strip_n(catalog::placeholders(source)),
                    "{name} uses different placeholders from English"
                );
            }
        }
    }
}
