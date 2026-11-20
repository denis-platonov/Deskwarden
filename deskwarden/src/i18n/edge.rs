//! Wording a sentence that arrived in English, at the edge of the screen.
//!
//! Some modules may not name `i18n`: `send`, `rest::send` and the runners
//! behind `job_object`'s fence, and the injector, whose refusals are log lines
//! as much as sentences for a person. They keep writing English. The screen
//! that shows one reads it WHOLE against the English catalog text of the keys
//! it knows -- the rule `login_ui::error_spot` follows -- and says it in the
//! language in force; a sentence it does not know is shown as it arrived.
//!
//! Logic never reads what this returns. It is only ever painted.

use super::{t, tf, with_language};

/// A key's English text, whatever language is in force.
pub fn english(key: &'static str) -> &'static str {
    with_language("en", || t(key))
}

/// The key, among `fixed`, whose English is exactly `message`.
pub fn fixed_key(fixed: &[&'static str], message: &str) -> Option<&'static str> {
    fixed.iter().copied().find(|key| english(key) == message)
}

/// Matches `message` against an English template with `{name}` holes (and
/// `{{`/`}}` for literal braces, as the catalog writes them): the hole values,
/// in order.
fn captures(template: &str, message: &str) -> Option<Vec<(String, String)>> {
    let mut literals: Vec<String> = vec![String::new()];
    let mut names: Vec<String> = Vec::new();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                literals.last_mut()?.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                literals.last_mut()?.push('}');
            }
            '{' => {
                let mut name = String::new();
                for n in chars.by_ref() {
                    if n == '}' {
                        break;
                    }
                    name.push(n);
                }
                names.push(name);
                literals.push(String::new());
            }
            other => literals.last_mut()?.push(other),
        }
    }
    let mut remaining = message.strip_prefix(literals[0].as_str())?;
    let mut found = Vec::new();
    for (i, name) in names.iter().enumerate() {
        let next = literals[i + 1].as_str();
        let last = i + 1 == names.len();
        let value = if last {
            remaining.strip_suffix(next)?
        } else if next.is_empty() {
            return None;
        } else {
            &remaining[..remaining.find(next)?]
        };
        found.push((name.clone(), value.to_string()));
        remaining = &remaining[value.len()..];
        remaining = if last { "" } else { remaining.strip_prefix(next)? };
    }
    remaining.is_empty().then_some(found)
}

/// `message` in the language in force: a fixed sentence from `fixed`, or one
/// of `templated` with its holes filled (each hole value passed through
/// `phrase(name, value)` first, for the holes that are phrases of ours).
/// `None` when it is none of them.
pub fn localize(
    fixed: &[&'static str],
    templated: &[&'static str],
    message: &str,
    phrase: impl Fn(&str, &str) -> String,
) -> Option<String> {
    if let Some(key) = fixed_key(fixed, message) {
        return Some(t(key).to_string());
    }
    for key in templated.iter().copied() {
        let Some(found) = captures(english(key), message) else { continue };
        let args: Vec<(String, String)> = found
            .into_iter()
            .map(|(name, value)| {
                let value = phrase(&name, &value);
                (name, value)
            })
            .collect();
        let borrowed: Vec<(&str, &str)> = args.iter().map(|(n, v)| (n.as_str(), v.as_str())).collect();
        return Some(tf(key, &borrowed));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_template_with_holes_and_literal_braces_matches_its_own_sentence() {
        let found = captures("presses {{{key}}}, which has no key code", "presses {TAB}, which has no key code")
            .expect("the key hole");
        assert_eq!(found, [("key".to_string(), "TAB".to_string())]);
        let found = captures("a {x} b {y}", "a 1 b 2").expect("two holes");
        assert_eq!(found, [("x".to_string(), "1".to_string()), ("y".to_string(), "2".to_string())]);
        assert!(captures("a {x} b", "a 1 c").is_none(), "the literal after the hole must be there");
        assert!(captures("a {x}", "b 1").is_none(), "the literal before the hole must be there");
    }
}
