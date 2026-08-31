# The CLI Arrives When It Is Needed Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A user installs Deskwarden, opens it, and picks `bitwarden.com`. The
card tells them plainly that bitwarden.com requires the Bitwarden CLI and that
Deskwarden will download and install it — **before** anything is fetched, while
they can still choose otherwise. They press Continue; a progress bar says it is
downloading the Bitwarden CLI from Bitwarden, then that it is checking the
signature; then they are signed in. A user who picks Self-hosted sees none of
it, ever. A user who already has a Bitwarden-signed `bw.exe` sees none of it
either — no notice, no request. Every way this can fail says which thing was
required, that bitwarden.com cannot be used without it, and that a self-hosted
server can. A binary that cannot be proved to be Bitwarden's is deleted rather
than run.

**Architecture:** This implements
`docs/superpowers/specs/2026-08-31-the-cli-arrives-when-it-is-needed-design.md`.
The work lands in one new module, `deskwarden/src/bw_acquire.rs`, plus one
insertion into `login_ui.rs`'s Submit arm between lines 2985 and 2986. The new
module **invents no download and no trust mechanism**: it composes
`http_agent::bounded_total`/`bounded_stall`, `updater::parse_asset_digest`,
`updater::file_sha256`, `updater::copy_reporting`, `signature::verify_authenticode`
and `signature::is_trusted_organization`, and installs to the path
`bw_path` itself computes. `bw_path.rs`, `signature.rs`, `updater.rs` and
`backend_policy.rs` gain no behaviour.

The design's governing rule, from the owner's ruling — *"yes, no silent - we say
that it is requared period"* — and every task below follows from it:

> **Nothing here is silent.** The app names the Bitwarden CLI, says it is
> required for the server the user chose, and says it is downloading and
> installing it. The requirement is stated **before** the first byte, not as a
> retroactive explanation. Every failure says plainly that bitwarden.com cannot
> be used without it and that a self-hosted server can.

This does **not** contradict "users should not know about how it works
underhood". That rule bans internal machinery the user has no decision to make
about; this is a third-party program being installed on their computer as a
hard requirement of a choice they just made. The design's *Nothing here is
silent* section states the distinction and the test that separates the two —
**read it before softening any string in this plan.**

And the ordering rule that is not a preference:

> **This plan ships before `2026-08-31-the-installer-forgets-the-cli.md`.**
> `choose(None, true)` is `BwServe`, so a fresh install with no CLI hits
> `main`'s `fatal_startup_error` today. Acquisition must exist before the
> installer stops providing the file.

**Tech Stack:** Rust, `windows` crate (via `signature.rs`, no new FFI), `ureq`
behind `http_agent`'s newtypes, `zip` extraction, egui/eframe for the one status
line, and the crate's `fn`-pointer seam idiom (`UpdaterEnv`, `SecondFactorSeam`).

## Global Constraints

- `cfg(test)` seams are banned crate-wide; seams are `fn`-pointer structs in production code.
- Build with `RUSTFLAGS="-D warnings"`, on the build **and** on `cargo test --no-run`; zero warnings.
- `export CARGO_TARGET_DIR=/e/_dw_agent/run` — never create a second target directory.
- Tests must not pass vacuously: every negative assertion carries a positive control. The house defect is "a test that passes because it never reached the thing it names".
- Judge a failing test by reading it, never by its name prefix.
- Never a magic numeric literal for a Win32 constant.

Additionally, and specific to this branch:

- **No test may reach the network.** Every HTTP path goes through the seam. A test that fetches from `api.github.com` is not a test of this feature, it is a test of GitHub.
- **No test may execute `bw.exe`, and neither may production.** Task 6 pins that the module contains no process start at all. If a task appears to need one, stop and report.
- **Do not edit `signature.rs`, `updater.rs`, `http_agent.rs`, `backend_policy.rs`, or `bw_path.rs`'s production half.** They are consumed, not changed. If a task appears to need a change there, stop and report: it means the seam is drawn in the wrong place.
- **Never widen `main::TRUSTED_BW_SIGNER_ORGANIZATIONS`.** Acquisition reads it; it does not get its own list. A second list is how the two come to disagree.
- Commit with explicit paths and `-F` a message file. Never `git add -A`, `--amend`, `reset`, `rebase`, or `git stash`.

## File Structure

| File | Responsibility |
| --- | --- |
| `deskwarden/src/bw_acquire.rs` (new) | The seam, the resolver, the digest check, the signature check, the install. All of it. |
| `deskwarden/src/lib.rs` (modify) | One `pub mod bw_acquire;`. |
| `deskwarden/src/login_ui.rs` (modify) | The gate at 2985/2986, one worker channel, one status line. |
| `deskwarden/src/job_object.rs` (modify) | One line in `THREAD_SPAWN_SITES`. |
| `deskwarden/Cargo.toml` (modify) | `zip`, if it is not already a dependency. Check before adding. |

## Interfaces

```rust
/// The artefact the resolver settled on: where it is and what it must hash to.
pub struct BwArtefact {
    pub url: String,
    pub digest: crate::updater::Sha256Digest,
    pub asset_name: String,
}

/// Why acquisition stopped. One variant per row of the design's failure
/// matrix, because the user reads a different sentence for each.
pub enum AcquireRefusal {
    Offline(String),
    NoArtefact(String),
    DigestMismatch,
    NotBitwardenSigned { subject_dn: Option<String> },
    Unverifiable(String),
    CouldNotInstall(String),
}

impl AcquireRefusal {
    /// The sentence shown in the sign-in card. Exactly one variant names
    /// machinery, and `NotBitwardenSigned` is it.
    pub fn message(&self) -> String;
    /// Whether Continue offers another attempt. False for
    /// `NotBitwardenSigned` alone.
    pub fn retryable(&self) -> bool;
}

/// The `fn`-pointer seam. Same shape and same discipline as
/// `updater::UpdaterEnv`: private fields, one `production()`, and a
/// `fn_addr_eq` guard that it holds the real ones.
pub struct BwAcquireEnv { /* six fn pointers, see Task 1 */ }
impl BwAcquireEnv { pub fn production() -> Self; }

/// The whole feature, as one call. `None` means "already there, nothing to
/// do" and is the common case.
pub fn acquire_if_needed(
    env: &BwAcquireEnv,
    on_progress: &dyn Fn(u64, Option<u64>),
) -> Result<Option<std::path::PathBuf>, AcquireRefusal>;

/// Whether this sign-in needs the CLI at all. A thin reading of
/// `backend_policy::choose`, **not a second decision.**
pub fn this_sign_in_needs_the_cli(server_url: Option<&str>, use_official_bw_crypto: bool) -> bool;
```

---

### Task 1: The seam, and the gate that is not a second decision

**Files:** `deskwarden/src/bw_acquire.rs` (new), `deskwarden/src/lib.rs`

**Interfaces**

- *Consumes:* `crate::backend_policy::{choose, VaultBackendChoice}`, `crate::updater::Sha256Digest`, `crate::signature::SignatureInfo`, `crate::http_agent::{TotalBounded, StallBounded}`.
- *Produces:* `BwArtefact`, `AcquireRefusal`, `BwAcquireEnv`, `BwAcquireEnv::production`, `this_sign_in_needs_the_cli`.

Nothing downloads in this task. The seam and the gate exist first so that every
later task has a substitutable environment to test against and a predicate
already under oath.

- [ ] **Step 1: Write the failing tests**

```rust
    /// **The gate is `backend_policy`'s answer, not a second one.**
    ///
    /// Without this, `this_sign_in_needs_the_cli` could be written as
    /// `!is_self_hosted(server)` — which agrees today and drifts the first
    /// time `choose` gains an input. The whole table, both directions.
    #[test]
    fn the_gate_is_exactly_the_backend_policy_decision() {
        use crate::backend_policy::{choose, VaultBackendChoice};
        for server in [
            None,
            Some(""),
            Some("https://vault.example.com"),
            Some("https://vault.bitwarden.com"),
            Some("https://bitwarden.eu"),
            Some("https://vault.bitwarden.community"),
        ] {
            for use_official in [true, false] {
                assert_eq!(
                    this_sign_in_needs_the_cli(server, use_official),
                    choose(server, use_official) == VaultBackendChoice::BwServe,
                    "the gate disagrees with `backend_policy::choose` for \
                     server={server:?} use_official={use_official}"
                );
            }
        }
    }

    /// The one arm that must never acquire, asserted on its own so a
    /// refactor of the table above cannot quietly lose it.
    #[test]
    fn a_self_hosted_account_on_the_built_in_client_never_needs_the_cli() {
        assert!(!this_sign_in_needs_the_cli(Some("https://vault.example.com"), false));
        // Positive control: the same server with the official CLI chosen
        // DOES need it, so the assertion above is reading both inputs.
        assert!(this_sign_in_needs_the_cli(Some("https://vault.example.com"), true));
    }

    /// `production()` holds the real functions. Copied in shape from
    /// `updater::production_holds_the_real_hash_and_the_real_launch`, which
    /// exists because a `production()` quietly wired to a stub is a seam
    /// that tests everything except what ships.
    #[test]
    fn production_holds_the_real_six() {
        let env = BwAcquireEnv::production();
        assert!(std::ptr::fn_addr_eq(env.already_present, resolve_present_and_trusted as fn() -> Option<std::path::PathBuf>));
        assert!(std::ptr::fn_addr_eq(env.resolve, resolve_artefact as fn(&TotalBounded) -> Result<BwArtefact, AcquireRefusal>));
        assert!(std::ptr::fn_addr_eq(env.verify, verify_is_bitwardens as fn(&std::path::Path) -> Result<(), AcquireRefusal>));
        // ...and the remaining three, each named.
    }

    /// **Every refusal names the Bitwarden CLI and the way out.**
    ///
    /// The owner's ruling — "yes, no silent - we say that it is requared
    /// period" — held by the file rather than by review. An earlier draft of
    /// this test asserted the OPPOSITE (that only the signature failure named
    /// the CLI, everything else being euphemised to "setup"); it was wrong,
    /// and it is recorded here so nobody re-derives it from the
    /// "underhood" rule. See the design's *Nothing here is silent* section
    /// for why the two rules do not conflict.
    #[test]
    fn every_refusal_names_the_cli_the_server_and_the_alternative() {
        let all = [
            AcquireRefusal::Offline("x".into()),
            AcquireRefusal::NoArtefact("x".into()),
            AcquireRefusal::DigestMismatch,
            AcquireRefusal::Unverifiable("x".into()),
            AcquireRefusal::CouldNotInstall("x".into()),
            AcquireRefusal::NotBitwardenSigned { subject_dn: None },
        ];
        for r in &all {
            let m = r.message();
            assert!(m.contains("Bitwarden CLI"), "{m:?} does not name what is required");
            assert!(m.contains("bitwarden.com"), "{m:?} does not name the server that requires it");
            assert!(m.contains("self-hosted"), "{m:?} does not name the way out");
            // An empty or one-word message passes a `contains` test against
            // nothing, so the length floor is part of the assertion.
            assert!(m.len() > 60, "{m:?} is too short to have said any of that");
        }

        // **Positive control for the predicate, not for the messages.** A
        // euphemism of the kind the ruling forbids must FAIL the same check
        // the six above pass -- otherwise the loop is asserting nothing.
        let euphemism = "Something went wrong while setting up. Try again in a moment, please.";
        assert!(euphemism.len() > 60, "control: the euphemism clears the length floor");
        assert!(
            !euphemism.contains("Bitwarden CLI"),
            "control: the predicate does not distinguish a euphemism from a real message"
        );
    }

    /// Retryability is per-variant and the signature failure is the one that
    /// is not: a retry loop against a substituted artefact is a loop.
    #[test]
    fn only_the_signature_refusal_refuses_to_retry() {
        assert!(!AcquireRefusal::NotBitwardenSigned { subject_dn: None }.retryable());
        // Control: the others do, so the assertion above is reading the
        // variant and not a `retryable()` hardcoded to false.
        for r in [
            AcquireRefusal::Offline("x".into()),
            AcquireRefusal::DigestMismatch,
            AcquireRefusal::CouldNotInstall("x".into()),
        ] {
            assert!(r.retryable(), "{:?} should offer another attempt", r.message());
        }
    }
```

Run: all four fail to compile — nothing exists yet. That is the expected
failure. **If any compiles, the module already exists and this task is
mis-scoped.**

- [ ] **Step 2: Make them pass**

Write `bw_acquire.rs` with the six-field `BwAcquireEnv`, `production()`, the two
enums, `this_sign_in_needs_the_cli` as a two-line reading of `choose`, and
`message()`/`retryable()`. The six production functions may be `todo!()`-free
stubs returning `Err(AcquireRefusal::Offline(...))` for now — Tasks 2–5 fill
them — but they must be real `fn` items so `fn_addr_eq` has something to
compare. Add `pub mod bw_acquire;` to `lib.rs`.

Run the four tests. All green.

- [ ] **Step 3: Verify the whole file, then commit**

`cargo test --lib bw_acquire`, then the full suite, then commit
`deskwarden/src/bw_acquire.rs deskwarden/src/lib.rs`.

---

### Task 2: The right release, and the right asset out of thirteen

**Files:** `deskwarden/src/bw_acquire.rs`

**Interfaces**

- *Consumes:* `crate::updater::{parse_asset_digest, Sha256Digest, ASSET_DIGEST_PREFIX}`, `crate::http_agent::TotalBounded`.
- *Produces:* `resolve_artefact`, and a pure `pick_artefact(&[Release]) -> Result<BwArtefact, AcquireRefusal>` that the tests drive.

**This is the task the house defect is most likely to hide in.** A resolver that
"found an asset" passes a naive test while returning `bw-oss-windows`, or
`bw-linux`, or a prerelease. Every test below is a pair.

- [ ] **Step 1: Write the failing tests**

```rust
    /// **The OSS build is beside the real one and a glob matches both.**
    /// Fixture mirrors the real `cli-v2026.8.0` asset list, verified
    /// 2026-08-31: thirteen assets, `bw-oss-windows-2026.8.0.zip` sorted
    /// BEFORE `bw-windows-2026.8.0.zip`, so a "first match" implementation
    /// picks the wrong one.
    #[test]
    fn the_oss_windows_build_is_never_the_one_chosen() {
        let picked = pick_artefact(&[stable_cli_release_2026_8_0()]).expect("an asset");
        assert_eq!(picked.asset_name, "bw-windows-2026.8.0.zip");
        assert!(!picked.asset_name.contains("oss"));
    }

    /// Positive control for the test above: with `bw-windows-*` removed from
    /// the same fixture it REFUSES, rather than falling back to the OSS
    /// build or to a Linux one. Without this, an implementation that always
    /// returns the last asset would pass the test above.
    #[test]
    fn a_release_without_the_windows_build_is_refused_not_substituted() {
        let mut release = stable_cli_release_2026_8_0();
        release.assets.retain(|a| a.name != "bw-windows-2026.8.0.zip");
        assert!(matches!(pick_artefact(&[release]), Err(AcquireRefusal::NoArtefact(_))));
    }

    /// The monorepo problem, from `bootstrap-bw.ps1`'s own finding: `cli`,
    /// `desktop`, `browser` and `web` interleave by date, so "newest release"
    /// is not "newest CLI".
    #[test]
    fn a_newer_desktop_release_does_not_win_over_the_newest_cli() {
        let picked = pick_artefact(&[
            desktop_release_newer_than_the_cli(),
            stable_cli_release_2026_8_0(),
        ]).expect("the cli release");
        assert_eq!(picked.asset_name, "bw-windows-2026.8.0.zip");
    }

    /// A prerelease tagged `cli-v*` ahead of its stable promotion is skipped.
    #[test]
    fn a_newer_cli_prerelease_loses_to_the_older_stable_one() {
        let picked = pick_artefact(&[
            cli_release("cli-v2026.9.0", /* prerelease */ true, /* draft */ false),
            stable_cli_release_2026_8_0(),
        ]).expect("the stable release");
        assert_eq!(picked.asset_name, "bw-windows-2026.8.0.zip");
    }

    /// **Positive control for the flag, not the ordering.** With the
    /// prerelease flag cleared on the same newer release it DOES win — so
    /// the test above is reading `prerelease` and not merely preferring the
    /// second element of the slice.
    #[test]
    fn the_prerelease_test_is_reading_the_flag_and_not_the_order() {
        let picked = pick_artefact(&[
            cli_release("cli-v2026.9.0", false, false),
            stable_cli_release_2026_8_0(),
        ]).expect("the newer release");
        assert_eq!(picked.asset_name, "bw-windows-2026.9.0.zip");
    }

    /// Drafts, same shape.
    #[test]
    fn a_draft_cli_release_is_skipped() { /* mirror of the two above */ }

    /// **Fail-closed on a missing digest**, carrying `updater`'s existing
    /// decision that `installer_sha256` is required rather than optional.
    /// An asset with no `sha256:` digest is refused, not installed unchecked.
    #[test]
    fn an_asset_with_no_digest_is_refused() {
        let mut release = stable_cli_release_2026_8_0();
        release.assets.iter_mut().for_each(|a| a.digest = None);
        assert!(matches!(pick_artefact(&[release]), Err(AcquireRefusal::NoArtefact(_))));
    }

    /// The digest that comes back is the WINDOWS asset's, not some other
    /// asset's. The failure this catches — right file, wrong hash — makes
    /// every download fail forever with a mismatch nobody can explain.
    #[test]
    fn the_digest_belongs_to_the_asset_that_was_picked() {
        let picked = pick_artefact(&[stable_cli_release_2026_8_0()]).expect("an asset");
        assert_eq!(picked.digest, parse_asset_digest(WINDOWS_ASSET_DIGEST).expect("a digest"));
        // Positive control: the OSS asset's digest is a DIFFERENT value, so
        // the assertion above would fail if the wrong one were carried.
        assert_ne!(WINDOWS_ASSET_DIGEST, OSS_WINDOWS_ASSET_DIGEST);
    }
```

The fixture `stable_cli_release_2026_8_0()` carries the real thirteen asset
names and the real digests recorded in the design, so it is a fixture of
something that exists rather than of something convenient.

Run: all fail — `pick_artefact` does not exist.

- [ ] **Step 2: Make them pass**

Implement `pick_artefact`: filter `tag_name` on a `cli-v` **prefix** (anchored,
not `contains`), reject `prerelease`/`draft`, take the newest remaining, then
find the asset whose name **starts with `bw-windows-` and ends with `.zip`** —
anchored at the front, which is what excludes `bw-oss-windows-`. Read its
`digest` through `updater::parse_asset_digest`; `None` is `NoArtefact`.

`resolve_artefact` is `pick_artefact` behind one `TotalBounded::get` against
`https://api.github.com/repos/bitwarden/clients/releases?per_page=50`, with the
`User-Agent` the crate already sends. Every transport error maps to
`AcquireRefusal::Offline`.

Run all eight. Green.

- [ ] **Step 3: Verify, then commit**

---

### Task 3: The download, and the digest that stops a corrupted one

**Files:** `deskwarden/src/bw_acquire.rs`

**Interfaces**

- *Consumes:* `crate::http_agent::StallBounded`, `crate::updater::{file_sha256, copy_reporting}`.
- *Produces:* `download_artefact`.

- [ ] **Step 1: Write the failing tests**

```rust
    /// A wrong digest is refused, the file is deleted, and NOTHING reaches
    /// the install path. Three assertions, because "returned Err" is not the
    /// same as "left nothing behind".
    #[test]
    fn a_zip_whose_digest_is_wrong_is_deleted_and_installs_nothing() { /* ... */ }

    /// **Positive control**: the identical bytes, with the digest that
    /// actually matches them, are kept. Without this the test above passes
    /// against an implementation that rejects everything.
    #[test]
    fn the_same_bytes_with_the_right_digest_are_kept() { /* ... */ }

    /// Progress is reported and ends on the byte count actually written —
    /// the contract `updater::the_download_reports_progress_and_ends_on_the_
    /// byte_count_it_wrote` already pins for the updater, asserted again
    /// here because this is a second caller of `copy_reporting`.
    #[test]
    fn the_download_reports_progress_ending_on_what_it_wrote() { /* ... */ }

    /// A stalled transfer aborts rather than hanging. Driven through the
    /// seam's `download` slot; `http_agent` already has the real
    /// `TcpStream`-level test for `bounded_stall` itself.
    #[test]
    fn a_stalled_transfer_becomes_an_offline_refusal() { /* ... */ }
```

- [ ] **Step 2: Make it pass**

`download_artefact` streams through `StallBounded::get` and
`updater::copy_reporting` into a temp file under the same cache directory
`UpdateEnv::download_dir` names, hashes it with `updater::file_sha256`, compares,
and on mismatch deletes and returns `DigestMismatch` — the shape
`updater::discard_rejected_installer` already uses.

- [ ] **Step 3: Verify, then commit**

---

### Task 4: Extract, verify, and only then install

**Files:** `deskwarden/src/bw_acquire.rs`

**Interfaces**

- *Consumes:* `crate::signature::{verify_authenticode, is_trusted_organization}`, `crate::bw_path` for the destination.
- *Produces:* `extract_bw_exe`, `verify_is_bitwardens`, `install_at_the_resolver_path`.

**Ordering is the whole task: extract to temp, verify there, copy only after.**
A verified-after-install implementation passes a test that checks the final
file's signature and still leaves an unverified binary at the path the process
already trusts, for however long the check takes.

- [ ] **Step 1: Write the failing tests**

```rust
    /// **The destination is `bw_path`'s own, not a second spelling.**
    /// The `OnceLock` in `bw_path` recorded a path at startup for a file that
    /// did not exist; acquisition makes that path correct by putting the file
    /// under it. A different spelling leaves the process trusting one path
    /// while the file sits at another.
    #[test]
    fn the_install_destination_is_the_path_the_resolver_already_recorded() {
        let exe_dir = std::path::Path::new(r"C:\deskwarden-test\app");
        assert_eq!(install_destination(exe_dir), exe_dir.join("bin").join("bw.exe"));
        // Positive control: this is the same value `bw_path` computes for a
        // process whose exe lives there — asserted against the resolver
        // rather than against the string above.
        assert_eq!(install_destination(exe_dir), crate::bw_path::install_bin_candidate_for(exe_dir));
    }

    /// An unsigned binary is refused and nothing is copied.
    #[test]
    fn an_unsigned_binary_is_refused_and_nothing_is_installed() { /* ... */ }

    /// A validly signed binary whose `O=` is somebody else is refused —
    /// three spellings that a substring check would wave through.
    #[test]
    fn a_binary_signed_by_someone_else_is_refused() {
        for subject in [
            r#"CN=Not Bitwarden, O=Not Bitwarden Ltd, C=US"#,
            r#"CN=Bitwarden Inc., O="Bitwarden Solutions LLC", C=US"#,
            r#"CN=x, OU=bitwarden-integration, O=Someone Else, C=US"#,
        ] {
            let info = SignatureInfo { valid: true, thumbprint: None, subject_dn: Some(subject.into()) };
            assert!(
                matches!(judge(&Ok(info)), Err(AcquireRefusal::NotBitwardenSigned { .. })),
                "{subject} was accepted"
            );
        }
    }

    /// **Positive control**: the real subject measured on 2026-08-10 IS
    /// accepted. Without this, the test above passes against a `judge` that
    /// refuses everything, and the feature would never install anything.
    #[test]
    fn the_real_bitwarden_subject_is_accepted() {
        let info = SignatureInfo {
            valid: true,
            thumbprint: Some("80375A0C9630A51ECB7EC79B37A8174C8DACCCED".into()),
            subject_dn: Some(r#"CN=Bitwarden Inc., O=Bitwarden Inc., C=US"#.into()),
        };
        assert!(judge(&Ok(info)).is_ok());
    }

    /// An invalid signature is refused even when the `O=` is right — the
    /// tampered-with case, which the org check alone would accept.
    #[test]
    fn a_tampered_binary_with_the_right_name_is_still_refused() { /* valid: false */ }

    /// A check that could not run at all is `Unverifiable`, a DIFFERENT
    /// refusal from `NotBitwardenSigned`, because one is retryable and one
    /// is not.
    #[test]
    fn a_check_that_could_not_run_is_its_own_refusal() { /* ... */ }

    /// **Acquisition reads `main`'s list and does not carry its own.**
    /// A second list is how the two come to disagree; the org strings must
    /// come from one place.
    #[test]
    fn the_trusted_organizations_are_not_duplicated_in_this_module() {
        let source = include_str!("bw_acquire.rs");
        let production = &source[..source.find(BELOW_CUT_MARKER).expect("the cut")];
        assert!(!production.contains("8bit Solutions"), "the org list is duplicated here");
        // Positive control: the module DOES name the shared constant, so a
        // module that consulted no list at all cannot pass.
        assert!(production.contains("TRUSTED_BW_SIGNER_ORGANIZATIONS"));
    }
```

The `TRUSTED_BW_SIGNER_ORGANIZATIONS` const currently lives in `main.rs` and is
private to the binary. Moving it to `signature.rs` as a `pub const`, with
`main.rs` reading it from there, is the smallest change that lets both callers
share one list — and it is the *only* edit to `signature.rs` this plan allows.
**If it turns out to need more than that, stop and report.**

- [ ] **Step 2: Make it pass**

Extract with `zip` into a temp directory, find `bw.exe` by name, run
`verify_authenticode` + `is_trusted_organization` on it **there**, and only on
`Ok` copy to `install_destination`. Then write `<InstallDir>\bin` to
`HKCU\Environment` `Path` if absent, and broadcast `WM_SETTINGCHANGE` — the same
two effects `bootstrap-bw.ps1` had, so an existing install's PATH entry keeps
pointing at a real file. **`WM_SETTINGCHANGE` and `HWND_BROADCAST` come from the
`windows` crate's named constants; no numeric literal.**

- [ ] **Step 3: Verify, then commit**

---

### Task 5: The window says what is required, then says it is fetching it

**Files:** `deskwarden/src/login_ui.rs`, `deskwarden/src/job_object.rs`

**Interfaces**

- *Consumes:* `bw_acquire::{acquire_if_needed, this_sign_in_needs_the_cli, BwAcquireEnv, AcquireRefusal}`, `crate::update_panel::download_fraction`.
- *Produces:* the gate at 2985/2986, an `acquire_rx` channel beside `auth_rx`, and a `setup_in_progress` flag beside `auth_in_progress`.

**Guards expected to move, and why each is a re-pin:**

| Guard | What happens | Why it is not a loosening |
| --- | --- | --- |
| `job_object::the_thread_spawn_census_is_exact` | `login_ui.rs`'s count rises by one | The census is exact in both directions; a new `std::thread::spawn` must be declared. The budget goes **up by exactly one** and the interleaved comment says which spawn it is. Nothing else in the file gains a spawn. |
| `login_ui.rs`'s `bw_*` spelling guards | Untouched | Acquisition names no `bw_command`; it goes through `bw_acquire`. If one of these trips, a spawn leaked into the window. |

- [ ] **Step 1: Write the failing tests**

```rust
    /// **Selecting an official server states the requirement and downloads
    /// nothing.** Both halves together, because they are the two ways this
    /// moment goes wrong: a notice that arrives after the download started is
    /// not a disclosure, and a disclosure costing 37 MB to read is not one
    /// either.
    #[test]
    fn choosing_an_official_server_states_the_requirement_and_makes_no_request() {
        let painted = paint_with(ServerChoice::UsCloud, /* bw present */ false);
        assert!(painted.strings().iter().any(|s| s.contains("requires the Bitwarden CLI")));
        assert_eq!(painted.seam_calls, 0, "selecting a server must not fetch anything");
    }

    /// Control for the test above: `SelfHosted` paints no notice, so that
    /// test is reading the choice rather than always finding the string.
    #[test]
    fn choosing_self_hosted_states_no_requirement() {
        let painted = paint_with(ServerChoice::SelfHosted, false);
        assert!(!painted.strings().iter().any(|s| s.contains("Bitwarden CLI")));
    }

    /// And control for the notice itself: a user who already HAS a trusted
    /// binary is told nothing, because there is no requirement left to state.
    #[test]
    fn an_existing_trusted_binary_states_no_requirement() {
        let painted = paint_with(ServerChoice::UsCloud, /* bw present */ true);
        assert!(!painted.strings().iter().any(|s| s.contains("Bitwarden CLI")));
    }

    /// **Positive control**: pressing Continue on the same form DOES reach
    /// the seam. Without this, the test above passes against a build where
    /// acquisition is wired to nothing at all.
    #[test]
    fn continue_on_an_official_server_reaches_the_seam() { /* assert calls == 1 */ }

    /// **The requirement is stated before the first byte.** The one property
    /// the owner's ruling turns on, and it is invisible to every other test
    /// here: the notice is painted at a frame strictly earlier than the first
    /// `resolve` call. A build that painted it from inside the worker would
    /// pass all three tests above and still be a retroactive explanation.
    #[test]
    fn the_requirement_is_painted_before_the_resolver_is_ever_called() {
        let run = drive_frames(ServerChoice::UsCloud, /* submit on frame */ 3);
        assert!(run.first_notice_frame < run.first_resolve_frame);
        // Control: both actually happened, or `usize::MAX < usize::MAX` and
        // two never-set sentinels would satisfy the line above.
        assert!(run.first_notice_frame < usize::MAX, "the notice was never painted");
        assert!(run.first_resolve_frame < usize::MAX, "the resolver was never called");
    }

    /// Self-hosted never reaches it, on either value of the toggle that
    /// matters.
    #[test]
    fn continue_on_a_self_hosted_server_makes_no_request() { /* ... */ }

    /// **An already-present, Bitwarden-signed binary short-circuits before
    /// any network call.** The common case, and the one a user with a slow
    /// connection notices immediately if it is wrong.
    #[test]
    fn an_existing_trusted_binary_short_circuits_before_the_resolver() {
        // `already_present` returns Some; `resolve` is a seam that panics.
        // Reaching it is the failure.
    }

    /// The refusal reaches `form.error`, the password buffer is NOT wiped,
    /// and `auth_in_progress` is never set — so Continue is live again with
    /// the form still filled.
    #[test]
    fn a_refusal_leaves_the_form_usable_and_the_password_intact() { /* ... */ }

    /// Progress becomes a fraction through the panel's existing helper, and
    /// an unknown total stays unknown rather than becoming zero.
    #[test]
    fn setup_progress_is_a_fraction_and_an_unknown_total_stays_unknown() { /* ... */ }
```

- [ ] **Step 2: Make it pass**

**Two edits, and the first must land before the second.**

*The notice, in the card.* Where the card draws its subtitle
(`login_ui.rs:1604`), add one line shown when
`this_sign_in_needs_the_cli(form.server_choice.config_url(), settings.use_official_bw_crypto)`
**and** `bw_path::verified_bw_exe()` names no existing trusted file. It is a
pure function of the form and that one filesystem answer — no worker, no
channel, nothing spawned — which is what makes it impossible for the notice to
arrive after the download. It names the chosen server, the Bitwarden CLI, and
the size.

*The acquisition, at Submit.* Insert between `login_ui.rs:2985` and `:2986`. On
the same condition, spawn the acquisition worker with an `mpsc` progress
channel, set `setup_in_progress`, and **return without setting
`auth_in_progress`** — the credentials have not gone anywhere. The status line
names each stage as it happens ("Downloading the Bitwarden CLI from Bitwarden…",
then "Checking it is signed by Bitwarden…"), never a bare spinner. On `Ok`, fall
through into the existing `configure_server_in` path. On `Err`,
`form.error = refusal.message()`.

**Do not reuse `form.error`'s styling for the notice.** The notice is not an
error — it is a statement about the choice the user just made, and painting it
red would read as a refusal to proceed.

Add `("bw_acquire.rs", 0)`-shaped bookkeeping only if the worker lives there;
otherwise bump `login_ui.rs`'s entry in `THREAD_SPAWN_SITES` by one with a
comment naming this spawn.

- [ ] **Step 3: Verify, then commit**

---

### Task 6: The pin that says nothing is ever executed

**Files:** `deskwarden/src/bw_acquire.rs`

**Interfaces**

- *Consumes:* `crate::below_cut`'s marker idiom, `job_object::CHILD_STARTERS`'s reasoning.
- *Produces:* the module's own source guards.

This is the strongest single statement the feature makes and it is nearly free,
because the scanner already exists.

- [ ] **Step 1: Write the failing tests**

```rust
    /// **This module downloads and verifies. It never runs anything.**
    /// Not even `bw --version` to "check it works" — a binary that has not
    /// been proven to be Bitwarden's must not execute, and one that has
    /// does not need a smoke test. Scanned over the production half only,
    /// with literals erased so a message mentioning the word is free.
    #[test]
    fn nothing_in_this_module_starts_a_process() {
        let production = production_slice(include_str!("bw_acquire.rs"));
        let code = code_without_literals(&production);
        for forbidden in ["Command", ".spawn(", ".output(", ".status("] {
            assert!(!code.contains(forbidden), "{forbidden} appears in bw_acquire's production code");
        }
    }

    /// **Positive control for the scanner**, not for the module. A scanner
    /// that returned "" would pass the test above against any code at all.
    #[test]
    fn the_scanner_would_catch_a_process_start_if_one_were_added() {
        let sneaked = format!("{}\nfn zz() {{ Command::new(\"x\"); }}\n", production_slice(include_str!("bw_acquire.rs")));
        let code = code_without_literals(&sneaked);
        assert!(code.contains("Command"), "control: the scanner did not see a Command it was handed");
    }

    /// Bare `ureq` never appears here — `http_agent`'s own guard covers the
    /// crate, and this restates it at the one new call site so a reviewer
    /// reading this file alone can see it.
    #[test]
    fn every_request_goes_through_a_bounded_agent() { /* ... */ }

    /// The standard cut guard, same as `signature.rs` and `updater.rs`.
    #[test]
    fn nothing_but_gated_test_modules_lives_below_the_guards_cut() { /* ... */ }
```

- [ ] **Step 2: Make it pass**

If the tests fail, the fix is to remove the process start, never to loosen the
scanner. **A failure here is a design violation, not a test problem.**

- [ ] **Step 3: Verify the whole suite, then commit**

Full `cargo test`, `RUSTFLAGS="-D warnings" cargo test --no-run`, then commit.

---

### Task 7: The words that changed

**Files:** `README.md`, `PRIVACY.md`, `deskwarden/README.md`, `CHANGELOG.md`, `deskwarden/src/main.rs`

- [ ] **Step 1: Write the failing test**

There is no test for prose. The check is a read, and it is listed as a task so
it is not skipped:

- `main.rs`'s `fatal_startup_error` on the missing-CLI arm says *"reinstall
  Deskwarden (its installer downloads a signed copy for you)"*. Rewrite to point
  at signing in to an official server, which is now what fetches it.
- `deskwarden/README.md:26` lists the CLI under Requirements. It is no longer a
  requirement of installing. Delete the bullet.
- `PRIVACY.md:14` and `:67` assert the CLI handles credentials. Still true on
  `bw serve`, false on direct-REST. Qualify, do not delete.
- `README.md:102` — *"Signing in uses the CLI either way, today"* — is already
  stale (`authenticate_then_wipe`'s direct arm). Fix it here.
- `CHANGELOG.md` gets a **new entry**. Existing entries are history.

- [ ] **Step 2: Commit**

Explicit paths, `-F` a message file.

## Status

Plan. Not executed. Blocks `2026-08-31-the-installer-forgets-the-cli.md`.
