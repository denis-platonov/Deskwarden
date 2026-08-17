//! A whole credential record, written into a Bitwarden Send and read back out
//! of one.
//!
//! Two halves, both pure and both free of I/O:
//!
//!  * [`payload`] — the [`Record`] type, its **versioned** JSON writer, and a
//!    deliberately strict reader. The writer is hand-rolled into a
//!    [`Zeroizing`](zeroize::Zeroizing) buffer rather than derived through
//!    `serde_json::to_string`, which would allocate the secret body into an
//!    ordinary `String` and hand it back to the allocator unwiped.
//!  * [`seal`] — passphrase sealing of the TOTP seed, and only the seed.
//!
//! **Why the seed is sealed twice over and nothing else is.** A Send's content
//! is protected by the fragment key, and that key is *in the link*: whoever has
//! the link has the content. For a username and a password that is the bargain
//! already accepted by sending them, because both can be rotated. A TOTP seed
//! cannot — "rotating" it means re-enrolling the second factor with the
//! service, which this app can neither do nor offer — so "whoever has the link"
//! is too weak a gate for it. The passphrase layer makes the link alone
//! insufficient, **but only if the passphrase travels out of band.**
//!
//! **Everything in a payload is data.** No field here is a command, a path, a
//! URL to fetch, or a key sequence to type. `notes` is text to store. Nothing
//! in this module interprets, opens or runs anything, and
//! `payload::tests::a_notes_field_that_looks_like_a_key_sequence_is_stored_as_text`
//! is that rule made checkable rather than merely stated.

pub mod payload;
pub mod seal;

pub use payload::{
    read_json, write_json, Record, RecordRefusal, MAX_PAYLOAD_BYTES, RECORD_FORMAT, RECORD_VERSION,
};
pub use seal::{seal, unseal, SealFailed, SealedSeed};
