//! Bitwarden's per-item **master password re-prompt**, enforced.
//!
//! An item can carry `reprompt: 1`, which every other Bitwarden client reads
//! as "ask the user to prove themselves again before revealing or using this
//! item's secrets". This crate used to preserve that flag faithfully and
//! ignore it completely, which is worse than not supporting it: the user ticks
//! a box in the web vault, believes the item is protected, and this app
//! reveals, copies, fills and sends it like any other.
//!
//! # What counts as proof, and what does not
//!
//! The obvious mechanism -- ask for the master password and check it -- **is
//! not available to this app**, and the investigation that settled that is
//! recorded here rather than in a commit message because every future attempt
//! to add one will start by re-asking it:
//!
//! * `bw unlock <password>` is the only `bw` subcommand that takes a master
//!   password, and its own `--help` states: *"After unlocking, any previous
//!   session keys will no longer be valid."* Deskwarden's `bw serve` child is
//!   holding one of those previous session keys ([`crate::bw_serve`]), so
//!   using `unlock` to check a typed password would log this app out of the
//!   vault as a side effect of confirming who the user is. That is fatal, not
//!   inconvenient.
//! * `bw unlock --check` reports lock status and takes no password at all.
//!   `bw serve`'s REST surface exposes the same `unlock` and therefore the
//!   same invalidation. There is no subcommand and no endpoint that validates
//!   a master password without minting a session.
//! * There is no local password hash to compare against: this crate stores a
//!   session token ([`crate::session_store`]) and, for enrolled users, a
//!   sealed copy of the password ([`crate::hello`]) -- and reading the sealed
//!   copy already requires the gesture below, so comparing a typed password
//!   against it would be a second, weaker spelling of the same proof.
//!
//! What *is* available is **Windows Hello**. [`crate::hello::unlock_password_for`]
//! succeeds only after the OS has verified the user's face, fingerprint or
//! PIN, and only for the account that enrolled. That gesture is a proof of
//! presence taken by the operating system, and it is the proof this module
//! uses. The master password it hands back is dropped immediately -- it is the
//! *gesture* that is the evidence, not the value.
//!
//! # The user who cannot prove
//!
//! For an account with no Hello enrollment there is **no safe mechanism at
//! all**, and this module says so instead of inventing one. [`Need::Cannot`]
//! is a refusal: the operation does not happen, and the UI explains that the
//! item asks for the master password and that quick unlock is how this app can
//! ask for it. A prompt that can be satisfied without proving anything would
//! be worse than none, because it would let this app claim the feature.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::accounts::AccountId;

/// How long one proof of presence covers.
///
/// **Sixty seconds, and per-window rather than per-action.** Per-action is
/// safest and was the starting position; it was rejected because the units a
/// user actually works in are not single actions. Copying a username and then
/// a password is two actions. Revealing a card's number to read it and then
/// copying the security code is two more. A Hello dialog between each is not
/// security theatre exactly, but it is the kind of friction that gets a
/// feature turned off, and this feature's whole value is that it is on.
///
/// Sixty is the same order as [`crate::clipboard::CLEAR_AFTER`]'s forty-five,
/// deliberately: this app already has a considered answer to "how long may a
/// secret stay exposed after the user asked for it", and a re-prompt window
/// that outlived the clipboard's would be the longer of two answers to one
/// question. It is short enough that a user who walks away from an unlocked
/// machine has not left the protected item open behind them for any
/// meaningful time.
///
/// It is a value rather than a literal at the comparison so a test can say
/// "one tick before" and "one tick after" without re-spelling the number.
pub const PROOF_LASTS: Duration = Duration::from_secs(60);

/// Whether a proof taken at `taken_at` still covers `now`.
///
/// `None` is not covered -- no proof was ever taken. A `now` that is *before*
/// `taken_at` is also not covered: [`Instant::checked_duration_since`] answers
/// `None` for it, and treating an incoherent clock as "still proven" is the
/// one direction this must never fail in.
fn proof_covers(taken_at: Option<Instant>, now: Instant) -> bool {
    taken_at
        .and_then(|at| now.checked_duration_since(at))
        .is_some_and(|age| age < PROOF_LASTS)
}

/// What must happen before a protected item's secrets may be exposed.
///
/// Three arms and not a `bool`, because "the user must be asked" and "the user
/// cannot be asked" lead to completely different outcomes -- a dialog and a
/// refusal -- and a `bool` would collapse them into whichever one the first
/// call site happened to write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    /// Go ahead. Either the item is not protected, or a live proof covers it.
    Nothing,
    /// Take a proof of presence first. The operation happens only if it
    /// succeeds.
    Prove,
    /// Refuse. The item is protected, no live proof covers it, and this
    /// account has no way to take one. See the module doc.
    Cannot,
}

/// **The whole re-prompt decision, as a pure function.**
///
/// Every call site -- reveal, every copy row, every copy chord, the fill path,
/// the fill hotkey and the Send builder -- asks this and nothing else. The
/// alternative, which this crate has lost to repeatedly, is `if
/// reprompt_protected(item)` written out at each of them: two enumerations
/// that must agree, and a sixth surface added later that agrees with neither.
///
/// `protected` comes from [`crate::vault_bridge::reprompt_protected`],
/// `can_prove` from whether this account has a Windows Hello enrollment, and
/// `taken_at`/`now` from the caller's [`Proof`]. All four are values, so both
/// refusing arms are reachable from a unit test with no vault, no Hello and no
/// clock.
pub fn need(
    protected: bool,
    can_prove: bool,
    taken_at: Option<Instant>,
    now: Instant,
) -> Need {
    if !protected || proof_covers(taken_at, now) {
        return Need::Nothing;
    }
    if can_prove {
        Need::Prove
    } else {
        Need::Cannot
    }
}

/// The most recent proof of presence, held by whoever owns the screen the
/// protected item is on.
///
/// **Cleared on lock**, by [`Self::forget`]. That is the same rule
/// `vault_window::ends_a_copied_secrets_life` establishes for a copied secret:
/// locking the vault ends the life of everything the unlock bought, and a
/// re-prompt proof is exactly such a thing. A proof that survived a lock would
/// mean locking and unlocking left a protected item unprotected for the
/// remainder of the minute.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Proof {
    taken_at: Option<Instant>,
}

impl Proof {
    /// A proof taken at `now` -- what a successful Hello gesture produces.
    pub fn taken_at(now: Instant) -> Self {
        Self { taken_at: Some(now) }
    }

    /// Records a fresh proof, extending the window from `now`.
    pub fn record(&mut self, now: Instant) {
        self.taken_at = Some(now);
    }

    /// Forgets the proof. Called when the vault locks, when the active
    /// account changes, and when the app quits -- the three events that end a
    /// copied secret's life.
    pub fn forget(&mut self) {
        self.taken_at = None;
    }

    /// [`need`] for this proof.
    pub fn need(&self, protected: bool, can_prove: bool, now: Instant) -> Need {
        need(protected, can_prove, self.taken_at, now)
    }
}

/// What a gated operation did, or did not do.
///
/// [`Self::Cannot`] and [`Self::Refused`] are separate arms rather than one
/// "didn't happen", because they say different things to the user: one is
/// "this build cannot ask you", the other is "you said no".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome<T> {
    /// The operation ran. Either it needed no proof or the user gave one.
    Done(T),
    /// The proof was cancelled or failed. **The operation did not run.**
    Refused,
    /// No proof could be asked for. **The operation did not run.**
    Cannot,
}

impl<T> Outcome<T> {
    /// Whether the guarded operation actually ran.
    ///
    /// Exists so a caller that only needs the yes/no does not have to
    /// re-spell the match and get it subtly different.
    pub fn happened(&self) -> bool {
        matches!(self, Self::Done(_))
    }
}

/// Where a proof can be taken: the config directory and the account whose
/// Hello enrollment would be verified.
///
/// Owned rather than borrowed because the gate outlives any one frame, and
/// `vault_window`'s event loop has no lifetime to lend it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    config_dir: PathBuf,
    account: AccountId,
}

impl Scope {
    pub fn new(config_dir: PathBuf, account: AccountId) -> Self {
        Self { config_dir, account }
    }
}

/// The production prover: a Windows Hello gesture.
///
/// The master password it releases is **dropped immediately**. Only the
/// gesture is evidence; binding the value here would give the plaintext a
/// second home for no gain. `Zeroizing` wipes it as this function returns.
///
/// A cancelled gesture returns `Err` and, critically, does **not** unenroll:
/// `hello::open_blob` deletes the sealed blob only when the blob itself cannot
/// be opened, never when the key closure refuses. Cancelling a re-prompt must
/// not cost the user their quick unlock.
fn prove_by_hello(config_dir: &Path, account: &AccountId) -> Result<(), String> {
    crate::hello::unlock_password_for(config_dir, account).map(|_| ())
}

/// The gate for `scope`, given whether that account is Hello-enrolled.
///
/// The pure half of [`gate_for_account`]: `enrolled` is the one thing that
/// call has to ask the operating system, so it is a parameter here and both
/// answers are reachable from a test.
///
/// An account with no scope -- no resolvable config directory, or no account
/// at all -- is [`RepromptGate::unprovable`] for the same reason an
/// un-enrolled one is: there is nowhere to look for the enrollment that would
/// prove anything.
pub fn gate_from(scope: Option<Scope>, enrolled: bool) -> RepromptGate {
    match scope {
        Some(scope) if enrolled => RepromptGate::production(scope),
        _ => RepromptGate::unprovable(),
    }
}

/// Where this app's per-account files live, or `None` on a platform with no
/// resolvable config directory.
///
/// Derived from [`crate::settings::default_path`]'s parent rather than
/// resolving `ProjectDirs` a second time: two spellings of "the config
/// directory" is exactly how one of them ends up pointing somewhere else.
pub fn config_dir() -> Option<PathBuf> {
    crate::settings::default_path()
        .as_deref()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
}

/// The gate for the account this window is showing.
///
/// Probes Windows Hello **once**, when the window opens, rather than per
/// frame: `hello::state_for` is a WinRT round trip and the answer cannot
/// change while one vault window is up (enrolling happens on the login
/// window, which is not on screen at the same time).
pub fn gate_for_account(account: Option<&AccountId>) -> RepromptGate {
    let scope = match (config_dir(), account) {
        (Some(dir), Some(id)) => Some(Scope::new(dir, id.clone())),
        _ => None,
    };
    let enrolled = match &scope {
        Some(s) => crate::hello::state_for(&s.config_dir, &s.account).enrolled,
        None => false,
    };
    gate_from(scope, enrolled)
}

/// The one place a protected item's secrets may be exposed, **and it is a
/// gating function rather than an `if` at each call site**.
///
/// Modelled on [`crate::vault_window::preflight::SendGate`] and for the reason
/// recorded there in full: a pin on a pure decision cannot see whether the
/// decision is in a gating position, and on this crate neutralising such a
/// gate to `let _ = decision(..);` has been measured to survive the entire
/// suite at zero warnings. So the prover lives behind a seam a test can drive
/// end to end, and the assertions are about ROUTING -- that the guarded
/// operation is NOT reached when the proof is refused and IS reached when it
/// is given.
pub struct RepromptGate {
    /// `Some` when this account has a Windows Hello enrollment, which is the
    /// only thing that makes a proof possible. `None` is
    /// [`Need::Cannot`]'s source.
    scope: Option<Scope>,
    /// [`prove_by_hello`] in production, a fixture in a test.
    prove: fn(&Path, &AccountId) -> Result<(), String>,
}

impl RepromptGate {
    /// The gate for an account that can be asked. Callers build this only
    /// when [`crate::hello::state_for`] reports the account enrolled.
    pub fn production(scope: Scope) -> Self {
        Self { scope: Some(scope), prove: prove_by_hello }
    }

    /// The gate for an account that cannot be asked at all: no Hello, no
    /// enrollment, no proof.
    ///
    /// Its prover is still the real one, so that "which prover" and "is there
    /// a scope" stay independent -- a test cannot accidentally pass because
    /// this variant quietly carried a prover that always says yes.
    pub fn unprovable() -> Self {
        Self { scope: None, prove: prove_by_hello }
    }

    /// A gate whose prover is `prove`, for tests.
    ///
    /// `pub(crate)` and test-only: the surfaces that must be driven through a
    /// refused and an allowed proof -- the copy rows, the copy chords, the
    /// reveal, the fill and the Send builder -- live in other modules, and a
    /// test there cannot pop a real Hello dialog. Not reachable from
    /// production code at all, so no shipped path can hand itself a prover
    /// that always says yes.
    #[cfg(test)]
    pub(crate) fn with_prover(
        scope: Option<Scope>,
        prove: fn(&Path, &AccountId) -> Result<(), String>,
    ) -> Self {
        Self { scope, prove }
    }

    /// A gate that always allows, for tests of surfaces that are not about
    /// the re-prompt: driving an ordinary copy through a window whose gate
    /// refuses everything would make every unrelated test a re-prompt test.
    #[cfg(test)]
    pub(crate) fn allowing_for_test() -> Self {
        fn yes(_: &Path, _: &AccountId) -> Result<(), String> {
            Ok(())
        }
        Self {
            scope: Some(Scope::new(
                PathBuf::from("C:/nowhere"),
                AccountId::parse("0123456789abcdef0123456789abcdef")
                    .expect("a 32-char lowercase hex id"),
            )),
            prove: yes,
        }
    }

    /// Whether a proof can be taken at all -- [`need`]'s `can_prove`.
    pub fn can_prove(&self) -> bool {
        self.scope.is_some()
    }
}

/// Runs `act` **only** when the item needs no proof, or when one is given now.
///
/// `act` is `FnOnce` so it cannot be run twice and cannot be run at all
/// without being consumed: a refusal drops it unused, which is a state the
/// compiler makes visible. `proof` is `&mut` because a gesture given here
/// covers the next [`PROOF_LASTS`] for every other call site.
///
/// This is the function every exposing path calls. It is deliberately the
/// *only* public way to combine [`need`] with an action, so that a new surface
/// added later cannot reach a secret by asking the decision and then ignoring
/// it.
pub fn permit<T>(
    gate: &RepromptGate,
    protected: bool,
    proof: &mut Proof,
    now: Instant,
    act: impl FnOnce() -> T,
) -> Outcome<T> {
    match proof.need(protected, gate.can_prove(), now) {
        Need::Nothing => Outcome::Done(act()),
        Need::Cannot => Outcome::Cannot,
        Need::Prove => {
            let Some(scope) = gate.scope.as_ref() else {
                // Unreachable: `can_prove` is `scope.is_some()`, so `Prove`
                // implies a scope. Stated rather than unwrapped -- the safe
                // answer to "the two disagreed" is to refuse.
                return Outcome::Cannot;
            };
            match (gate.prove)(&scope.config_dir, &scope.account) {
                Ok(()) => {
                    proof.record(now);
                    Outcome::Done(act())
                }
                Err(e) => {
                    log::info!("a master-password re-prompt was not satisfied: {e}");
                    Outcome::Refused
                }
            }
        }
    }
}

/// What the UI says when a protected item's secrets are withheld.
///
/// One string per refusal, here rather than at each surface, so that four
/// screens cannot word the same refusal four ways -- and so that neither
/// wording can drift into implying the operation happened.
pub fn refusal_text(outcome_was_cannot: bool) -> &'static str {
    if outcome_was_cannot {
        "This item asks for the master password. Turn on Windows Hello quick unlock to confirm it."
    } else {
        "Cancelled. This item asks for the master password, so nothing was revealed or copied."
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn scope() -> Scope {
        Scope::new(
            PathBuf::from("C:/nowhere"),
            AccountId::parse("0123456789abcdef0123456789abcdef").expect("a 32-char hex id"),
        )
    }

    fn allows(_: &Path, _: &AccountId) -> Result<(), String> {
        Ok(())
    }

    fn refuses(_: &Path, _: &AccountId) -> Result<(), String> {
        Err("the user cancelled the Windows Hello prompt".to_string())
    }

    /// A clock a test owns. `Instant` cannot be constructed from a number, so
    /// "now" is one real reading and every other moment is an offset from it.
    fn t0() -> Instant {
        Instant::now()
    }

    #[test]
    fn an_unprotected_item_never_asks_for_anything() {
        let now = t0();
        assert_eq!(need(false, true, None, now), Need::Nothing);
        assert_eq!(
            need(false, false, None, now),
            Need::Nothing,
            "an unprotected item was refused because the account cannot prove -- that would \
             break every ordinary item in the vault for a user with no Hello enrollment"
        );
    }

    #[test]
    fn a_protected_item_asks_when_there_is_no_proof_and_refuses_when_it_cannot_ask() {
        let now = t0();
        assert_eq!(need(true, true, None, now), Need::Prove);
        assert_eq!(need(true, false, None, now), Need::Cannot);
    }

    #[test]
    fn a_proof_covers_the_window_and_stops_covering_at_its_end() {
        let taken = t0();
        // The positive control: inside the window, a protected item is let
        // straight through. Without this the expiry assertion below is
        // satisfied by a function that never covers anything.
        assert_eq!(
            need(true, true, Some(taken), taken),
            Need::Nothing,
            "a proof did not cover the instant it was taken"
        );
        assert_eq!(
            need(true, true, Some(taken), taken + PROOF_LASTS - Duration::from_millis(1)),
            Need::Nothing,
            "a proof stopped covering before its window was up"
        );
        assert_eq!(
            need(true, true, Some(taken), taken + PROOF_LASTS),
            Need::Prove,
            "a proof still covered at exactly its expiry -- the window is half-open"
        );
        assert_eq!(
            need(true, true, Some(taken), taken + PROOF_LASTS + Duration::from_secs(600)),
            Need::Prove,
            "a ten-minute-old proof still covered a protected item"
        );
    }

    #[test]
    fn a_proof_from_the_future_covers_nothing() {
        let now = t0();
        assert_eq!(
            need(true, true, Some(now + Duration::from_secs(1)), now),
            Need::Prove,
            "a proof timestamped after `now` was treated as live; an incoherent clock must \
             fail towards asking, never towards allowing"
        );
    }

    #[test]
    fn an_expired_proof_still_refuses_when_the_account_cannot_prove() {
        let taken = t0();
        assert_eq!(
            need(true, false, Some(taken), taken + PROOF_LASTS),
            Need::Cannot,
            "an expired proof fell back to allowing rather than to refusing"
        );
    }

    #[test]
    fn locking_forgets_the_proof() {
        let now = t0();
        let mut proof = Proof::taken_at(now);
        assert_eq!(
            proof.need(true, true, now),
            Need::Nothing,
            "the premise: this proof covers `now` before the lock"
        );
        proof.forget();
        assert_eq!(
            proof.need(true, true, now),
            Need::Prove,
            "a proof survived a lock, so locking and unlocking would leave a protected item \
             open for the rest of its window"
        );
    }

    #[test]
    fn a_default_proof_has_proven_nothing() {
        assert_eq!(Proof::default().need(true, true, t0()), Need::Prove);
    }

    // -- the gate, in the position that gates ------------------------------

    // Every test below counts how many times the guarded operation actually
    // ran. That count is the whole point: these are ROUTING assertions, about
    // whether `act` is reached, not about what `need` answers.

    #[test]
    fn an_unprotected_item_reaches_the_operation_without_a_prompt() {
        let ran = Cell::new(0u32);
        // A prover that would panic if called: an unprotected item must not
        // pop a Hello dialog.
        fn boom(_: &Path, _: &AccountId) -> Result<(), String> {
            panic!("an unprotected item asked for a proof of presence");
        }
        let gate = RepromptGate::with_prover(Some(scope()), boom);
        let mut proof = Proof::default();
        let out = permit(&gate, false, &mut proof, t0(), || {
            ran.set(ran.get() + 1);
            "secret"
        });
        assert_eq!(out, Outcome::Done("secret"));
        assert_eq!(ran.get(), 1);
    }

    #[test]
    fn a_given_proof_lets_the_operation_run_and_covers_the_next_one() {
        let ran = Cell::new(0u32);
        let gate = RepromptGate::with_prover(Some(scope()), allows);
        let mut proof = Proof::default();
        let now = t0();

        let out = permit(&gate, true, &mut proof, now, || ran.set(ran.get() + 1));
        assert!(out.happened(), "a satisfied proof did not run the operation");
        assert_eq!(ran.get(), 1);

        // The second call inside the window must not need the prover at all,
        // which is what `PROOF_LASTS` is for. A prover that panics proves it.
        fn boom(_: &Path, _: &AccountId) -> Result<(), String> {
            panic!("a second action inside the proof window asked again");
        }
        let covered = RepromptGate::with_prover(Some(scope()), boom);
        let out = permit(&covered, true, &mut proof, now + Duration::from_secs(1), || {
            ran.set(ran.get() + 1)
        });
        assert!(out.happened());
        assert_eq!(ran.get(), 2);
    }

    /// **Step 4's pairing.** A cancelled prompt must not fall through to the
    /// unprotected path, and the only way to see that is to watch whether the
    /// operation was reached.
    #[test]
    fn a_cancelled_proof_does_not_run_the_operation() {
        let ran = Cell::new(0u32);
        let gate = RepromptGate::with_prover(Some(scope()), refuses);
        let mut proof = Proof::default();
        let now = t0();

        let out = permit(&gate, true, &mut proof, now, || ran.set(ran.get() + 1));
        assert_eq!(out, Outcome::Refused);
        assert_eq!(
            ran.get(),
            0,
            "a cancelled re-prompt still ran the operation -- the secret was revealed, copied \
             or typed anyway"
        );
        assert!(!out.happened());

        // And it left no proof behind, so the next attempt asks again rather
        // than inheriting a window a refusal opened.
        assert_eq!(proof.need(true, true, now), Need::Prove);

        // THE POSITIVE CONTROL on the same gate shape: with a prover that
        // says yes, this identical call does run. Without it, "0" above is
        // also what a `permit` that never runs anything would produce.
        let allowed = RepromptGate::with_prover(Some(scope()), allows);
        let mut fresh = Proof::default();
        let out = permit(&allowed, true, &mut fresh, now, || ran.set(ran.get() + 1));
        assert!(out.happened());
        assert_eq!(ran.get(), 1);
    }

    #[test]
    fn an_account_that_cannot_prove_does_not_run_the_operation_either() {
        let ran = Cell::new(0u32);
        let gate = RepromptGate::with_prover(None, allows);
        assert!(!gate.can_prove());
        let mut proof = Proof::default();

        let out = permit(&gate, true, &mut proof, t0(), || ran.set(ran.get() + 1));
        assert_eq!(out, Outcome::Cannot);
        assert_eq!(
            ran.get(),
            0,
            "an item that could not be gated at all was exposed anyway"
        );

        // The control: the same unprovable gate lets an UNPROTECTED item
        // straight through, so `Cannot` is about the flag and not about the
        // gate being inert.
        let out = permit(&gate, false, &mut proof, t0(), || ran.set(ran.get() + 1));
        assert!(out.happened());
        assert_eq!(ran.get(), 1);
    }

    #[test]
    fn the_production_gate_holds_the_real_hello_prover() {
        // The seam is only worth having if production goes through the real
        // thing. Compared by address, as `SendGate`'s own guard does: a
        // wrapper, a forwarder or a flag-gated no-op is a different address.
        let gate = RepromptGate::production(scope());
        assert!(
            std::ptr::fn_addr_eq(gate.prove, prove_by_hello as fn(&Path, &AccountId) -> Result<(), String>),
            "the production re-prompt gate is not wired to the Windows Hello prover"
        );
        assert!(gate.can_prove(), "a production gate with a scope cannot prove");
        assert!(
            std::ptr::fn_addr_eq(
                RepromptGate::unprovable().prove,
                prove_by_hello as fn(&Path, &AccountId) -> Result<(), String>
            ),
            "the unprovable gate quietly carries a different prover, so `can_prove` and \
             `prove` are not independent"
        );
        assert!(!RepromptGate::unprovable().can_prove());
    }

    #[test]
    fn only_an_enrolled_account_with_somewhere_to_look_can_be_asked() {
        assert!(
            gate_from(Some(scope()), true).can_prove(),
            "an enrolled account was given a gate that can never ask, so every protected item \
             would be refused outright"
        );
        assert!(!gate_from(Some(scope()), false).can_prove());
        assert!(!gate_from(None, true).can_prove());
        assert!(!gate_from(None, false).can_prove());
    }

    #[test]
    fn the_two_refusals_are_worded_differently_and_neither_claims_success() {
        let cannot = refusal_text(true);
        let cancelled = refusal_text(false);
        assert_ne!(cannot, cancelled);
        for text in [cannot, cancelled] {
            assert!(
                !text.is_empty(),
                "a refusal with no words is a refusal the user cannot see"
            );
        }
        assert!(
            cannot.contains("Windows Hello"),
            "the refusal a user can act on does not say what to do: {cannot}"
        );
        assert!(
            cancelled.contains("nothing was"),
            "the cancellation does not say that nothing happened: {cancelled}"
        );
    }
}
