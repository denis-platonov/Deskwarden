//! One pasted Send link, taken apart. **Pure: nothing here does any I/O.**
//!
//! This is a module of its own and not a private helper inside
//! [`crate::rest::send`] because a link carries a **host**, and reading a host
//! somebody else wrote is a trust decision rather than a string split.
//! [`crate::rest::send_crypto::access_url`] builds a link out of *this*
//! client's own base URL; this file reads one back out of text the user
//! pasted, which is the opposite direction and the dangerous one.
//!
//! # The refusal that matters
//!
//! A link on a host that is not the account's configured server is
//! **refused**, and both hosts are named. Bitwarden's own CLI prompts
//! interactively to override -- "Do not proceed if you do not trust ..." --
//! and this app has no such prompt on this screen. Inventing a modal is a
//! separate change; guessing is not an option at all, because the alternative
//! is this process making an HTTPS request that carries a PBKDF2 hash of a
//! password the user typed to a host chosen by whoever wrote the link.
//!
//! The comparison is **exact**, never a suffix and never a substring, for the
//! reason [`crate::backend_policy::is_self_hosted`] already writes down about
//! `vault.bitwarden.community`: a host that merely *contains* the configured
//! one is a different host, and `vault.example.com.evil.test` is the whole
//! attack. It goes through [`crate::favicon::host_from_url`] -- a fourth
//! caller of the one host reader this crate has, and not a fourth copy of it.

use crate::rest::send_crypto::SendKey;
use crate::send::SendError;

/// Bitwarden's send key is 128 bits, so its fragment is 22 unpadded base64url
/// characters. Not a parameter: see [`SendKey`]'s own note on the length.
const SEND_KEY_LEN: usize = 16;

/// One pasted Send link, taken apart.
///
/// **Not `Debug`**, by the rule [`SendKey`], `Challenge`,
/// `service_token::Token` and `SendInvocation` already follow: this holds the
/// key that decrypts the Send for anyone who has it, and a `Debug` is what
/// ends up in a log file.
pub struct SendLink {
    access_id: String,
    key: SendKey,
}

impl SendLink {
    /// The base64url GUID the server decodes with `CoreHelpers.Base64UrlDecode`.
    pub(crate) fn access_id(&self) -> &str {
        &self.access_id
    }

    /// The 16 bytes out of the fragment.
    pub(crate) fn key(&self) -> &SendKey {
        &self.key
    }
}

/// Parses a Send link, and refuses rather than guesses.
///
/// `configured` is the account's own server URL -- the one every request this
/// module makes is addressed to. The link's own origin is used for **nothing
/// but the comparison**: a link that passes is fetched from the configured
/// base URL, so even a comparison that were somehow too generous could not
/// send a request somewhere new.
///
/// The id and the key are the **last two** `/`-separated segments of the
/// fragment, which is `receive.command.ts`'s `getIdAndKey` verbatim:
///
/// ```ts
/// const result = url.hash.slice(1).split("/").slice(-2);
/// ```
///
/// That is what makes `#/send/{id}/{key}` and a bare `#{id}/{key}` both work,
/// and it is why this does not split on a fixed `#/send/` prefix.
///
/// # Errors
///
/// [`SendError::Rejected`], with a sentence naming what was wrong, for a link
/// with no fragment, a fragment with fewer than two segments, an origin that
/// is not the configured server's, or a key that is not 16 bytes. A truncated
/// key is refused rather than padded: it produces a link that opens nothing,
/// which is the position [`SendKey::from_wrapped`] already takes.
pub fn parse(link: &str, configured: &str) -> Result<SendLink, SendError> {
    let link = link.trim();
    let (before, fragment) = link.split_once('#').ok_or_else(|| {
        rejected(
            "That does not look like a Send link: the key travels after a `#` and there is no \
             `#` in it.",
        )
    })?;

    // The host comparison comes FIRST, before the key is decoded, so a link
    // aimed somewhere else is refused without this process having looked at
    // its key at all.
    let theirs = crate::favicon::host_from_url(before);
    let ours = crate::favicon::host_from_url(configured);
    if theirs.is_empty() {
        return Err(rejected(&format!(
            "That Send link names no server. This account is on {ours}."
        )));
    }
    if theirs != ours {
        return Err(rejected(&format!(
            "That Send link is on {theirs}, but this account is on {ours}. Deskwarden will not \
             send your share password to a server you did not configure."
        )));
    }

    // `getIdAndKey`: the LAST TWO segments, so both link shapes work.
    let segments: Vec<&str> = fragment.split('/').filter(|s| !s.is_empty()).collect();
    let [.., access_id, encoded] = segments.as_slice() else {
        return Err(rejected(
            "That Send link is missing its id or its key. A whole link ends in `/{id}/{key}`.",
        ));
    };

    let bytes = crate::rest::api::base64_url_decode(encoded).ok_or_else(|| {
        rejected("That Send link's key is not readable. Copy the whole link and try again.")
    })?;
    let bytes: [u8; SEND_KEY_LEN] = bytes.as_slice().try_into().map_err(|_| {
        rejected(
            "That Send link's key is the wrong length, so it is not a whole link. Copy all of \
             it and try again.",
        )
    })?;

    Ok(SendLink { access_id: (*access_id).to_string(), key: SendKey::from_bytes(bytes) })
}

/// Every refusal here is [`SendError::Rejected`] and none is ambiguous:
/// nothing has been sent, because nothing in this file sends.
fn rejected(why: &str) -> SendError {
    SendError::Rejected(why.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const OURS: &str = "https://vault.example.com";

    /// A known 16 bytes, and the fragment they encode to.
    fn a_key() -> (SendKey, String) {
        let key = SendKey::from_bytes([6u8; SEND_KEY_LEN]);
        let fragment = key.fragment();
        (key, fragment)
    }

    /// **Both link shapes the official client accepts**, because it takes the
    /// LAST TWO fragment segments -- `receive.command.ts`'s `getIdAndKey`.
    ///
    /// A parser that split on a fixed `#/send/` prefix passes the first of
    /// these and fails the second, which is exactly why both are here.
    #[test]
    fn the_id_and_key_are_the_last_two_fragment_segments_in_both_link_shapes() {
        let (_, fragment) = a_key();

        let web = parse(&format!("{OURS}/#/send/acc-1/{fragment}"), OURS).expect("the web shape");
        assert_eq!(web.access_id(), "acc-1");
        assert_eq!(web.key().fragment(), fragment, "the key did not survive the parse");

        let bare = parse(&format!("{OURS}/#acc-1/{fragment}"), OURS).expect("the bare shape");
        assert_eq!(bare.access_id(), "acc-1");
        assert_eq!(bare.key().fragment(), fragment);

        // A deeper prefix than either -- still the last two segments.
        let deep =
            parse(&format!("{OURS}/#/x/y/send/acc-1/{fragment}"), OURS).expect("a deep shape");
        assert_eq!(deep.access_id(), "acc-1");
    }

    /// **The refusal that matters.** A link whose origin is not the account's
    /// configured server is refused, and BOTH hosts are named in the sentence
    /// -- a refusal that says only "bad link" sends the user to look at the
    /// key, which is the one part that was fine.
    ///
    /// Control, in the same test: the identical link on the CONFIGURED origin
    /// parses. Without it this passes for a parser that refuses everything.
    #[test]
    fn a_link_on_a_host_that_is_not_the_configured_server_is_refused_and_the_same_link_on_it_is_not()
    {
        let (_, fragment) = a_key();
        let tail = format!("/#/send/acc-1/{fragment}");

        let refused = parse(&format!("https://vault.evil.test{tail}"), OURS)
            .err()
            .expect("a foreign host is refused");
        let sentence = refused.user_message();
        assert!(
            sentence.contains("vault.evil.test"),
            "the refusal does not name the link's host: {sentence}"
        );
        assert!(
            sentence.contains("vault.example.com"),
            "the refusal does not name the configured host: {sentence}"
        );

        // The control: the SAME link, on the configured origin, parses.
        assert!(
            parse(&format!("{OURS}{tail}"), OURS).is_ok(),
            "the parser refuses its own server's links, so the assertion above is vacuous"
        );
    }

    /// **Exact origin, never a suffix.** `backend_policy::is_self_hosted`'s
    /// rule and its stated reason: a host that merely contains the configured
    /// one is a different host.
    #[test]
    fn a_host_that_merely_contains_the_configured_one_is_a_different_host() {
        let (_, fragment) = a_key();
        let tail = format!("/#/send/acc-1/{fragment}");
        for foreign in [
            "https://vault.example.com.evil.test",
            "https://evil-vault.example.com",
            "https://example.com",
            "https://vault.example.com.",
        ] {
            assert!(
                parse(&format!("{foreign}{tail}"), OURS).is_err(),
                "{foreign:?} was accepted as {OURS:?}"
            );
        }
        // The control: the exact host is still accepted, so the loop above is
        // not passing because the parser refuses every host there is.
        assert!(parse(&format!("{OURS}{tail}"), OURS).is_ok(), "the exact host is refused");
    }

    /// A fragment that is not 16 bytes is **refused rather than padded**: a
    /// truncated key produces a link that opens nothing, and
    /// `SendKey::from_wrapped` already takes that position for the wrapped
    /// case.
    ///
    /// Controls: the 22-character fragment from a known 16 bytes parses, and
    /// round-trips to the same bytes.
    #[test]
    fn a_key_of_any_length_but_sixteen_bytes_is_refused_and_the_right_one_round_trips() {
        let (key, fragment) = a_key();
        assert_eq!(fragment.len(), 22, "the control fixture is not a 16-byte fragment");

        for wrong in [
            crate::rest::api::base64_url_no_pad(&[6u8; 15]),
            crate::rest::api::base64_url_no_pad(&[6u8; 17]),
            crate::rest::api::base64_url_no_pad(&[6u8; 32]),
            String::new(),
            "not-base64-@@@".to_string(),
        ] {
            assert!(
                parse(&format!("{OURS}/#/send/acc-1/{wrong}"), OURS).is_err(),
                "a key of {} characters was accepted",
                wrong.len()
            );
        }

        // The control, and the round trip: the right length parses AND comes
        // back as the same bytes, so this test cannot pass for a parser that
        // refuses every key.
        let parsed = parse(&format!("{OURS}/#/send/acc-1/{fragment}"), OURS).expect("16 bytes");
        assert_eq!(parsed.key().fragment(), key.fragment(), "the key did not round-trip");
    }

    /// A link with no `#` at all, and one whose fragment has only one
    /// segment, are refused -- not read as an id with an empty key.
    #[test]
    fn a_link_with_no_fragment_or_half_a_fragment_is_refused() {
        let (_, fragment) = a_key();
        assert!(parse(&format!("{OURS}/send/acc-1/{fragment}"), OURS).is_err(), "no `#`");
        assert!(parse(&format!("{OURS}/#{fragment}"), OURS).is_err(), "one segment");
        assert!(parse(&format!("{OURS}/#/"), OURS).is_err(), "an empty fragment");
        // The control: two segments parse.
        assert!(parse(&format!("{OURS}/#acc-1/{fragment}"), OURS).is_ok());
    }

    /// **`SendLink` cannot be written to a log.** Checked the way this crate
    /// already checks it for `Challenge` and `SendKey`: the source is read and
    /// the derive is asserted absent, with the positive control that a derive
    /// IS found somewhere in the same file -- a search that matched nothing at
    /// all would pass the absence assertion while reaching nothing.
    #[test]
    fn the_parsed_link_cannot_be_written_to_a_log() {
        let source = include_str!("send_link.rs").replace("\r\n", "\n");
        let marker = "\n#[cfg(test)]\nmod tests {";
        let cut = source.find(marker).expect("the test module marker was not found");
        let production = &source[..cut];

        // Controls: the cut landed where it was meant to, and both halves are
        // real -- a slice cut wrong makes every assertion below vacuous.
        assert!(production.contains("pub struct SendLink"), "the cut lost the type");
        assert!(!production.contains("mod tests"), "the cut left test code in production");

        assert!(
            !production.contains("derive(Debug") && !production.contains("Debug)"),
            "`SendLink` is in a file that derives `Debug`, and it holds the Send's key"
        );
        // The positive control for the assertion above: the word `Debug` IS
        // findable in this file, in the doc comment that forbids it. A test
        // that could not find the word at all would pass regardless.
        assert!(
            production.contains("Not `Debug`"),
            "control: the word `Debug` is not in this file's production at all, so the absence \
             assertion above is searching text it could never have found"
        );
    }
}
