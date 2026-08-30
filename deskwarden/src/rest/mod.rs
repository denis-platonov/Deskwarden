//! Talking to a Bitwarden-compatible server **directly**, without the `bw`
//! CLI in between.
//!
//! Today every vault operation in this app goes through `bw serve` on
//! localhost ([`crate::bw_serve`], [`crate::vault_bridge`]). That has one
//! large property this module gives up, and it is stated at the top of
//! [`crypto`] rather than buried: `bw` holds the master password and the
//! keys derived from it, and this process holds only a DPAPI-wrapped session
//! token. A direct backend derives those keys **here**.
//!
//! Nothing in this module is reachable from the running app yet, and that is
//! deliberate. The four submodules are layered so that each can be checked
//! without the one above it:
//!
//! * [`crypto`] -- the client-side cryptography, and **no I/O of any kind**.
//!   Built and checked against published test vectors first, because
//!   everything an HTTP layer above it could do is worthless if the
//!   decryption underneath is wrong.
//! * [`api`] -- prelogin, the OAuth password grant, token refresh and
//!   `GET /api/sync`. Every test drives it through `mockito`; nothing here
//!   may reach a real server, the real vault, `%APPDATA%` or `bw`.
//! * [`sync`] -- the sync payload, decrypted and mapped into the
//!   [`crate::vault_bridge`] shapes the rest of the app already depends on.
//! * [`write`] -- the inverse of [`sync`]: an item re-encrypted into the wire
//!   cipher shape. Its one non-obvious rule is the reason it is a module of
//!   its own, and it is stated at the top of the file: a `PUT` replaces the
//!   whole cipher, so the body is built by laying the modelled fields **over**
//!   the JSON [`sync`] retained, never from the model alone.
//!
//! **What is written over REST, and what is not.** [`api`] does create,
//! update, trash, restore and hard delete for *ciphers*, and create and
//! delete for *folders*.
//!
//! **Sends** are here now, in [`send`] and [`send_crypto`]: a text Send can
//! be published, listed and revoked without `bw.exe`. Two halves of the
//! feature are not, and each is named rather than left to be discovered --
//! **file Sends** (creatable on neither backend today, so listing and
//! revoking them is parity and not a subtraction) and **receiving a Send
//! from a link**, which still runs `send::cli_send_receive`.
//!
//! Still missing, and each is what keeps `bw.exe` on the machine: that
//! receive path, **attachments** (not decrypted by [`sync`], not creatable
//! or deletable by [`write`]), and **organisations** (decrypted, but never
//! shown working against a real server).
//!
//! This paragraph said "no folder write" until folder writes landed and
//! nobody came back to it, and it said Sends were absent until they were not -- a module doc that lists what is missing is a
//! promise to keep the list current, and it is the kind of sentence a
//! reader trusts precisely because it is inconvenient. Two-factor
//! authentication is
//! *completed* now rather than refused by name: authenticator, email and
//! YubiKey, plus the personal API-key grant for the accounts none of those
//! reach. See [`api::LoginOutcome`] and [`api::Challenge`].
//! [`api::RestError::TwoFactorRequired`] survives for the callers that
//! cannot prompt.
//!
//! # What a write does not carry, and what that costs
//!
//! An item's attachments and `fido2Credentials` are still not *decrypted*
//! ([`sync`]'s own doc says so). They now survive an edit -- they ride the
//! retained JSON through [`write`] byte-for-byte, which is the whole point of
//! that module -- but this crate cannot create, replace or delete one.
//!
//! # What is verified, and where the boundary now sits
//!
//! [`crypto`]'s module docs said the Bitwarden-specific composition could not
//! be confirmed without a real payload. This layer moved that boundary rather
//! than leaving it where it was: the master key (both KDFs), the password
//! hash, the key stretch, the assignment of an `EncString`'s three parts and
//! the enc-then-mac split of a protected key are now each pinned against a
//! vector **Bitwarden's own client asserts on** -- and one of those vectors
//! caught a real unit bug here (`KdfMemory` is MiB, not KiB). What is still
//! open, and what would settle it, is enumerated at
//! `crypto::tests::the_composition_is_still_not_fully_pinned_and_here_is_what_is_left`.

//! # The server this was written against, and what that does not cover
//!
//! The account driving this work is on a **self-hosted, Bitwarden-compatible
//! server that is neither Bitwarden nor Vaultwarden** -- one running on
//! Cloudflare Workers, which documents organisations, collections, roles,
//! SSO and SCIM as *not implemented*. Three things follow, and they are
//! written here rather than assumed:
//!
//! * **Treat the API as a subset.** A field official Bitwarden returns is not
//!   guaranteed present. Every wire struct in [`sync`] is optional-by-default
//!   and every read is a tolerant one with a named refusal; there is no
//!   `unwrap` on a server-supplied value anywhere in this module. A mapper
//!   that unwraps where a fixture was generous panics on the real payload.
//! * **The organisation path is kept, and it is untested end to end.** It is
//!   kept because other users *do* have organisations and the RSA unwrap
//!   underneath it is the one piece with genuine external ground truth (an
//!   OpenSSL ciphertext). It is untested end to end because this server will
//!   never send an org cipher, so no live payload can exercise it -- only
//!   `sync::tests::an_organisation_cipher_decrypts_through_the_rsa_wrapped_org_key`,
//!   whose own doc says exactly this.
//! * **A Worker is not a long-lived process.** Connection reuse, keep-alive
//!   and cold starts behave differently from a persistent server, which is
//!   why [`api`]'s connect timeout is ten seconds rather than
//!   `vault_bridge`'s three. Nothing else in this module depends on
//!   connection behaviour: no request here is correct only because a previous
//!   one warmed a socket, and pooling is a performance property, not a
//!   correctness one.

pub mod api;
/// The four modules below, assembled into one
/// [`crate::vault_backend::VaultBackend`]. No route, no cryptography and no
/// mapping of its own -- and six of the twenty operations refused by name
/// rather than faked. See its own docs for what one call costs.
pub mod backend;
pub mod crypto;
/// Sends over REST: the three operations `crate::send` runs the CLI for.
pub mod send;
/// A Send's own key hierarchy, which is not the vault's. Pure; no I/O.
pub mod send_crypto;
/// One pasted Send link, taken apart -- and refused when its host is not the
/// account's own. Pure; no I/O.
pub mod send_link;
pub mod sync;
pub mod write;
