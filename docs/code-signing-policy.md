# Code Signing Policy

How Deskwarden's released binaries are built, who can cause one to be signed,
and what a signature on a Deskwarden artifact is and is not evidence of.

Published as a condition of the [SignPath Foundation](https://signpath.org)
free code signing programme for open source projects.

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
