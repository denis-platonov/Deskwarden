//! The user's Windows display languages, in preference order.

use windows::core::PWSTR;
use windows::Win32::Globalization::{GetUserPreferredUILanguages, MUI_LANGUAGE_NAME};

/// The tags Windows lists under Settings > Time & language > Language, most
/// preferred first (`en-US`, `zh-CN`, `pt-BR`...). Empty when the call
/// fails -- the caller treats that as "no preference" and shows English.
pub fn preferred_languages() -> Vec<String> {
    let mut count = 0u32;
    let mut len = 0u32;
    // SAFETY: the first call passes no buffer and only asks how long one has
    // to be; the second passes a buffer of exactly that many u16s, which
    // outlives the call.
    unsafe {
        if GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, PWSTR::null(), &mut len)
            .is_err()
            || len == 0
        {
            return Vec::new();
        }
        let mut buffer = vec![0u16; len as usize];
        if GetUserPreferredUILanguages(
            MUI_LANGUAGE_NAME,
            &mut count,
            PWSTR(buffer.as_mut_ptr()),
            &mut len,
        )
        .is_err()
        {
            return Vec::new();
        }
        split_multi_string(&buffer)
    }
}

/// A double-NUL-terminated list of NUL-separated UTF-16 strings, read into
/// the strings. Stops at the first empty one.
fn split_multi_string(buffer: &[u16]) -> Vec<String> {
    buffer
        .split(|unit| *unit == 0)
        .take_while(|piece| !piece.is_empty())
        .map(String::from_utf16_lossy)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_multi_string_is_split_at_its_nuls_and_ends_at_the_empty_one() {
        let utf16: Vec<u16> = "en-US\0de-DE\0\0".encode_utf16().collect();
        assert_eq!(split_multi_string(&utf16), ["en-US", "de-DE"]);
        assert!(split_multi_string(&[0, 0]).is_empty());
        assert!(split_multi_string(&[]).is_empty());
    }

    /// The real call, on whatever machine runs the suite: it must answer
    /// without panicking, and what it answers must be tags, not noise.
    #[test]
    fn the_real_call_answers_language_tags() {
        for tag in preferred_languages() {
            assert!(
                tag.len() >= 2 && tag.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
                "`{tag}` does not look like a language tag"
            );
        }
    }
}
