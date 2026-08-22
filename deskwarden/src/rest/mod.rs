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
//! deliberate. [`crypto`] is the whole of it: the client-side cryptography,
//! built and checked against published test vectors first, because everything
//! an HTTP layer above it could do is worthless if the decryption underneath
//! is wrong. No HTTP client, no login flow and no wiring exist yet.

pub mod crypto;
