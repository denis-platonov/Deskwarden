# Code Signing Policy

How Deskwarden's released binaries are built, who can cause one to be signed,
and what a signature on a Deskwarden artifact is and is not evidence of.

Published as a condition of the [SignPath Foundation](https://signpath.org)
free code signing programme for open source projects.

---

## Status: not yet in effect

**Deskwarden releases are not currently signed.** No certificate has been
issued, the SignPath project has not been approved, and the two
`Sign ... (SignPath)` steps in the release workflow are labelled TODO no-ops
that print a line and sign nothing. Every release published so far is an
unsigned build, and each one says so in its release notes.

Everything below this section describes the policy that **will** apply once a
certificate exists. It is written in the present tense because it is the
commitment being made as a condition of the programme, not a description of
controls already running. Where a statement is not true yet, it is not true
yet; the section immediately below says what the project does in the meantime.

### What is verified today, in place of a signature

The application's self-update path verifies the installer it downloads against
a **SHA-256 digest** published by the GitHub releases API for that asset, in
the same TLS response that supplies the download URL. The downloaded file is
hashed when it arrives and again immediately before it is launched, and any
mismatch, malformed digest, absent digest, or unreadable file is a refusal:
the file is deleted and nothing is started. See
[`deskwarden/src/updater.rs`](../deskwarden/src/updater.rs).

**This is not a substitute for code signing and is not described as one.** Its
trust root is TLS to `api.github.com` plus the integrity of the
`denis-platonov/deskwarden` account -- an attacker who can replace a release
asset can generally replace the digest published beside it. A signature would
let someone verify the publisher *without* trusting the distribution channel;
a digest from that channel cannot. What the digest does establish is that the
bytes which run are the bytes the API described, which catches corruption,
truncation, and substitution in flight, and which is strictly more than the
user got by downloading and running the installer by hand.

Until 2026-08-19 the updater instead gated on an Authenticode thumbprint
constant holding a placeholder value that could never match. Because releases
are unsigned, that gate refused **every** update it was ever offered, so the
self-update had never once succeeded and each release was installed manually
with nothing checked at all. The digest check replaces it. When a certificate
is issued, an Authenticode gate is intended to be added back **on top of** the
digest check rather than in place of it.

---

## What gets signed

Two artifacts, both produced by the release workflow from a `vX.Y.Z` tag:

- `deskwarden.exe` — the application binary.
- `deskwarden-<version>-installer.exe` — the Inno Setup installer wrapping it.

Nothing else is submitted for signing. In particular, **binaries from upstream
open source projects are never signed with this subscription.** Deskwarden
invokes the Bitwarden CLI (`bw`) but does not bundle, build, or redistribute
it; the user installs it themselves and Deskwarden verifies its Authenticode
signature at runtime before running it.

## Where builds come from

**Every signed artifact is built by GitHub Actions from a public commit in
this repository.** No artifact built on a developer machine is ever submitted
for signing.

- Workflow: [`.github/workflows/release.yml`](../.github/workflows/release.yml)
- Trigger: a pushed `v*.*.*` tag, or a manual `workflow_dispatch` against an
  existing tag ref. Both read `github.ref_name`, so a dispatched run and a
  pushed run produce identical output.
- Runner: `windows-latest`, GitHub-hosted. No self-hosted runners are used.
- Toolchain: `dtolnay/rust-toolchain@stable`.

The build is reproducible from the tagged source: the crate has no build step
that reaches outside the repository except normal dependency resolution
against crates.io, pinned by the committed `Cargo.lock`.

### Version consistency

The release workflow **fails before building** if the crate version in
`deskwarden/Cargo.toml` does not equal the tag. The executable's
`ProductName`, `FileDescription`, `ProductVersion` and `FileVersion` resource
attributes come from that same crate manifest via
[`build.rs`](../deskwarden/build.rs), so a signed binary's embedded version
always matches the tag it was built from.

### The build script is treated as source

`build.rs` runs arbitrary code at build time, so it is guarded like source
code rather than trusted as configuration: it is pinned by content hash, and
`deskwarden/Cargo.toml` is pinned by length and content hash alongside it, so
that re-pointing a dependency name at a fork or a local path fails a test
rather than silently changing what runs during a build. Changing either
requires editing the pinned values in the same commit, which is visible in
review.

## Who can do what

This project currently has **one maintainer**, who is consequently the author,
the reviewer, and the approver. That is stated plainly rather than dressed up
as separation of duties, because a policy that describes controls the project
does not have is worse than one that admits the gap.

| Role | Who | What they can do |
|---|---|---|
| **Author** | The maintainer, and any contributor via pull request | Propose changes. Contributors cannot merge. |
| **Reviewer** | The maintainer | Review and merge to `main`. |
| **Approver** | The maintainer | Approve a signing request in SignPath. |

**Signing requests are approved manually, one release at a time.** No
automatic or unattended approval is configured. An unexpected signing request
is therefore visible as a request that nobody made, and is to be rejected
rather than approved.

### Accounts and access

These are requirements on the maintainer, kept here so that a lapse is a
visible breach of a written policy rather than a private oversight:

- **Multi-factor authentication is required** on the GitHub account with write
  access to this repository, and on the SignPath account that approves signing
  requests.
- The signing certificate's private key is held by SignPath. It is never in
  the maintainer's possession, on a developer machine, or in this repository,
  so there is no key material here that could be leaked by a repository
  compromise.
- The API token that lets the release workflow submit a signing request is a
  GitHub Actions secret. It can request a signature; it cannot approve one.

## What a signature means

A valid Deskwarden signature is evidence that **this artifact was built by the
release workflow from a tagged commit in this public repository, and that a
human approved that specific signing request.**

It is not a warranty that the software is free of defects, and it is not an
audit of the source. The source is public precisely so that the signature does
not have to carry that weight.

Deskwarden is **unaffiliated with Bitwarden, Inc.** A signature identifies the
build's origin, not an endorsement by any third party.

## Reporting a problem

Suspected key misuse, an unexpected signing request, or a signed artifact that
does not correspond to a public tag: **denis@napps.pw**. Please report
privately first — see [CODE_OF_CONDUCT.md](../CODE_OF_CONDUCT.md).

## Known gaps

Recorded rather than omitted, because a policy that lists only its strengths
is not useful to someone deciding whether to trust a binary.

- **Single maintainer.** Author, reviewer and approver are the same person, so
  compromise of that one account is sufficient to obtain a signature on
  arbitrary code that has been pushed to this repository. The manual approval
  step is the only control standing between a malicious commit and a signed
  artifact.
- **No reproducible-build attestation yet.** The build is reproducible in
  practice from the tagged source, but this project does not currently publish
  a third-party rebuild or a provenance attestation that would let someone
  verify that independently.
- **No signing at all yet.** See [Status](#status-not-yet-in-effect) above.
  Until a certificate is issued, nothing in this document about signatures
  describes a control that is running, and the update path's only integrity
  check is the SHA-256 comparison described there -- whose trust root is the
  GitHub account and the TLS connection to it, not a publisher identity.
