pub mod send_input;
pub mod sequence;
pub mod ui_automation;

use crate::fill_stats::FillOutcome;
use sequence::Plan;
use std::sync::atomic::{AtomicBool, Ordering};

/// Where a fill's **real** outcome goes, once something knows it.
///
/// A boxed `FnOnce` and not a channel. The outcome is produced on the typing
/// thread and there is nobody left on the UI thread waiting for it, so a
/// channel would need a receiver polled from the message pump -- one more
/// place to forget, and a fill whose outcome arrives after the app closes
/// would simply be lost. Worse, a channel invites a `recv()`, and the one
/// thing this design may never do is make the UI thread wait on the typing
/// thread; that is the entire reason the thread exists.
///
/// A closure that owns everything it needs instead (see
/// `app::fill_outcome_sink`: a `FillStats`, which is a `PathBuf`, and a copy
/// of the item id) reports from the thread that knows, borrows nothing owned
/// by the UI, and blocks nobody. `Send + 'static` is not decoration -- it is
/// the compiler enforcing that second property.
pub type OutcomeSink = Box<dyn FnOnce(FillOutcome) + Send + 'static>;

pub trait UiAutomationFiller {
    fn fill(&self, hwnd: isize, user: &str, pass: &str) -> Result<bool, String>;
}

/// Whether a typing run is in flight. See [`SequenceGuard`].
static SEQUENCE_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// **What a caller is told when [`SequenceGuard::acquire`] says no.**
///
/// One constant and not two literals, because both fill paths now refuse
/// through it -- [`Injector::fill`] and [`Injector::fill_sequence`] -- and a
/// user who pressed the hotkey has no idea which of the two they were on. The
/// sentence has to name the state ("something is already typing"), the way out
/// of it (wait, or switch away), and the retry, because a refusal that only
/// says "no" is indistinguishable from a hotkey that never registered.
pub const ALREADY_TYPING: &str = "an auto-type sequence is already being typed; wait for it to \
     finish, or switch away to stop it, then press the hotkey again";

/// Proof that the holder, and nobody else, may synthesise keystrokes right now.
///
/// # What goes wrong without it
///
/// [`SendInputFiller::fill_sequence`] performs a sequence on a fresh thread,
/// because a `{DELAY 2000}` performed on the caller's thread would freeze the
/// app. Nothing stopped a second hotkey press *during* that delay from
/// starting a second thread. Both threads then see the same foreground window
/// and both pass every check they make -- the checks are individually correct
/// and jointly useless -- so the two sequences interleave their keystrokes
/// into the same field. Each thread's [`Plan`] still wipes on drop, so this
/// corrupts a login rather than leaking a password, but a half-typed
/// interleaving of two passwords is not a state a user can diagnose.
///
/// # Refuse, rather than queue
///
/// A queued sequence types into whatever is in front when its turn arrives,
/// which by then may be a window the user has navigated away from. That is
/// precisely the hazard the whole `sequence` module is built to prevent, so
/// queueing would reintroduce it at the one layer that cannot check for it:
/// the plan was resolved against an `hwnd` chosen a fill ago. Refusing loses
/// nothing a second hotkey press cannot recover, and the refusal reaches the
/// user (`app::fill_from_vault` passes it to the notifier), so a fill that
/// declined to happen is never silent.
///
/// # Scope, release and liveness
///
/// The flag is **process-global, not per-[`Injector`]**. `Injector` is `Clone`
/// and is cloned onto worker threads; what is being contended for is not an
/// injector but the machine's one keyboard and one foreground window, so two
/// clones typing at once is exactly the case this exists to stop.
///
/// Released by `Drop`, which runs on a panicking typing thread too -- this
/// crate unwinds -- so a panic mid-sequence cannot strand the flag set and
/// wedge auto-type for the rest of the session. **Nothing ever blocks on it**:
/// a caller that cannot have it is told so and gives up immediately, so there
/// is no lock here to deadlock on and no waiter to strand.
///
/// # It also carries the outcome back
///
/// The guard is already the one thing that is moved onto the typing thread
/// and whose `Drop` runs there on every exit including a panic. That makes it
/// the right place to hang "what did this fill actually do?" on: a separate
/// reporting channel would have to duplicate the same move and the same
/// drop-on-unwind guarantee, and could be forgotten on a path the guard
/// cannot be. See [`OutcomeSink`] and [`SequenceGuard::report`].
pub struct SequenceGuard {
    /// Where to report what the typing did. `None` for a guard nobody is
    /// counting -- the tests that contend for the flag and nothing else.
    sink: Option<OutcomeSink>,
    /// What will be reported if nothing says otherwise.
    ///
    /// **`NotTyped` by default, on purpose.** A typing thread that panics
    /// part-way never reaches its `report` call, and unwinding still runs
    /// this `Drop`, so the count stays where it was rather than crediting a
    /// fill nobody can vouch for. The failure mode of the default is an
    /// undercount, which is recoverable by filling again; the failure mode of
    /// defaulting the other way is a password that was never typed climbing
    /// the picker.
    outcome: FillOutcome,
}

impl SequenceGuard {
    /// `None` if a sequence is already running.
    pub fn acquire() -> Option<Self> {
        SEQUENCE_IN_FLIGHT
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self { sink: None, outcome: FillOutcome::NotTyped })
    }

    /// Attaches the callback that will be handed this guard's outcome, exactly
    /// once, when it drops -- on whichever thread that turns out to be.
    pub fn reports_to(&mut self, sink: OutcomeSink) {
        self.sink = Some(sink);
    }

    /// Records what the typing actually did. The last call before the drop is
    /// the one that counts; no call at all means [`FillOutcome::NotTyped`].
    pub fn report(&mut self, outcome: FillOutcome) {
        self.outcome = outcome;
    }
}

impl Drop for SequenceGuard {
    fn drop(&mut self) {
        // The flag first, unconditionally, before anything that could go
        // wrong: releasing the right to type must not be contingent on a
        // bookkeeping callback behaving.
        SEQUENCE_IN_FLIGHT.store(false, Ordering::Release);
        if let Some(sink) = self.sink.take() {
            let outcome = self.outcome;
            // A `Drop` that panics while the thread is *already* unwinding
            // aborts the process. The sink writes a small JSON file and
            // swallows its own errors by design, so it should never panic --
            // and "should never" is not a thing to bet a process abort on.
            let _ =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || sink(outcome)));
        }
    }
}

/// Serialises every test that passes through [`Injector::fill_sequence`].
///
/// [`SEQUENCE_IN_FLIGHT`] is process-global, which is the correct scope for
/// the machine's one keyboard and the wrong scope for a suite running fifteen
/// hundred tests across a thread pool: two unrelated tests inside
/// `fill_sequence` at the same instant would leave one of them holding the
/// flag while the other was told "already typing", and it would fail at
/// random. Taking this first means the only contention a test ever observes is
/// the contention it arranged itself.
#[cfg(test)]
static SEQUENCE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) fn sequence_test_lock() -> std::sync::MutexGuard<'static, ()> {
    // A test that panicked while holding this poisons it. The protected data
    // is `()`, so there is no invariant left broken; propagating the poison
    // would turn one genuine failure into a cascade of unrelated ones that
    // hides it.
    SEQUENCE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

pub trait SendInputFiller {
    /// `hwnd` is the window the caller *intends* to fill. Implementations must
    /// verify it actually has foreground before typing: `SendInput` goes to
    /// whatever holds keyboard focus, which is not necessarily the window we
    /// matched (see `send_input::ensure_foreground`).
    fn fill(&self, hwnd: isize, user: &str, pass: &str) -> Result<(), String>;

    /// Performs a planned auto-type sequence against `hwnd`.
    ///
    /// Takes the [`Plan`] **by value** so it can be moved onto whichever
    /// thread performs it: a sequence contains `{DELAY}`s, and performing one
    /// on the caller's thread would freeze the app for as long as the user
    /// asked it to wait. Owning it also means the plan's `Drop` -- the wipe --
    /// runs wherever it finishes, rather than leaving a plaintext password on
    /// a caller's stack for the duration.
    ///
    /// **Required, with no default body.** It used to default to a refusal,
    /// on the argument that a filler which had not opted in must not look like
    /// one that had. The argument was right and the mechanism was wrong:
    /// replacing that default with `Ok(())` left the entire suite green, and
    /// it silently re-pointed `main.rs`'s `NeverTypes` -- which overrides
    /// `fill` with a panic precisely to prove the fallback is unreachable, but
    /// inherited the default here -- at a body that succeeds and records a
    /// fill. A defaulted method cannot be pinned by a test, because there is
    /// no implementor whose absence anything can notice. This trait has
    /// exactly two real implementors; requiring the method costs them one line
    /// each and makes forgetting it a compile error.
    ///
    /// `guard` is the process-wide permission to type, taken by
    /// [`Injector::fill_sequence`] before it calls this. It is passed by value
    /// rather than acquired here so that an implementation which spawns a
    /// thread must *move* it onto that thread, tying the flag's release to the
    /// end of the typing rather than to the return of this call -- which
    /// happens almost immediately and would make the guard meaningless.
    ///
    /// **It is also how the outcome gets home.** `Ok(())` from this method
    /// means "started", so an implementation must call
    /// [`SequenceGuard::report`] with what the typing actually did before the
    /// guard drops. Saying nothing is not a bug, it is a claim:
    /// [`FillOutcome::NotTyped`], which is what a panicking thread reports by
    /// default and the only safe thing to assume of a run that never spoke.
    fn fill_sequence(&self, hwnd: isize, plan: Plan, guard: SequenceGuard)
        -> Result<(), String>;
}

#[derive(Clone)]
pub struct Injector<A: UiAutomationFiller, B: SendInputFiller> {
    pub ui: A,
    pub fallback: B,
}

impl<A: UiAutomationFiller, B: SendInputFiller> Injector<A, B> {
    /// The default fill: UI Automation first, keystrokes if that finds nothing.
    ///
    /// # It takes the same permission to type that a sequence does
    ///
    /// [`SequenceGuard::acquire`] used to be called only from
    /// [`Self::fill_sequence`], so this path synthesised keystrokes with **no
    /// reference to `SEQUENCE_IN_FLIGHT` at all**. With a guard held --
    /// i.e. with a sequence sitting in the middle of a `{DELAY 2000}` -- a
    /// hotkey press or an `Auto` dispatch for a second item reached here and
    /// really typed, and the two runs interleaved their keystrokes into one
    /// field. That is precisely the failure [`SequenceGuard`]'s doc claims to
    /// prevent, and the guard's own words for it applied to itself: the checks
    /// were individually correct and jointly useless.
    ///
    /// The acquisition wraps the **whole** dispatch and not just the fallback,
    /// which is deliberate. UI Automation synthesises no keystrokes, so it is
    /// not contending for the keyboard -- but it *is* contending for the target
    /// window's fields, and setting a password into a control while another
    /// thread is typing a different password into the same window is the same
    /// unrecoverable mess by a different route. Holding it across the match
    /// also means there is one acquisition on this path rather than one per
    /// arm, so a future third arm cannot be added without it.
    ///
    /// # It stays synchronous, and it refuses rather than queues
    ///
    /// Nothing here spawns a thread. A [`Plan`] built by [`default_plan`]
    /// contains no `Wait` step -- there is no `{DELAY}` in a default fill -- so
    /// the only time it costs the UI thread is the typing itself, which is what
    /// this path has always cost it. Staying synchronous keeps this method's
    /// `Result` meaning what it has always meant, which is what lets
    /// `app::fill_from_vault` turn it straight into a [`FillOutcome`] instead of
    /// needing a sink and a thread.
    ///
    /// So a refusal is simply an `Err` the caller already handles. See
    /// [`ALREADY_TYPING`] for why it is a sentence and not a silent `Ok(())`.
    pub fn fill(&self, hwnd: isize, user: &str, pass: &str) -> Result<(), String> {
        // Named, not `_`: a bare `_` pattern drops immediately and would
        // release the right to type before a single character was sent, which
        // is the one way to write this that compiles and guards nothing.
        let Some(_holds_the_keyboard) = SequenceGuard::acquire() else {
            return Err(ALREADY_TYPING.to_string());
        };
        match self.ui.fill(hwnd, user, pass) {
            Ok(true) => Ok(()),
            Ok(false) => self.fallback.fill(hwnd, user, pass),
            Err(e) => {
                log::warn!("UI Automation fill failed for hwnd {hwnd} ({e}); using SendInput");
                self.fallback.fill(hwnd, user, pass)
            }
        }
    }

    /// The sequence path. **It does not try UI Automation first**, and that is
    /// the answer to whether the default fill is "just a sequence".
    ///
    /// It is not. UI Automation fills *named fields*: it walks the window's
    /// automation tree, finds the control whose type says password, and sets
    /// its value -- without depending on focus, without synthesising a single
    /// keystroke, and without caring what the tab order is. A sequence types
    /// *keystrokes at whatever has focus*. Those are different acts with
    /// different failure modes, and the UIA one is strictly safer where it
    /// works, which is why the default fill still starts there.
    ///
    /// Collapsing them would have been elegant and wrong twice over. It would
    /// have deleted the UIA path for every existing item in every existing
    /// vault (all of which store no sequence), turning a fill that needs no
    /// foreground into one that does. And it could not have worked anyway:
    /// UIA has no way to express `{ENTER}`, a `{DELAY 2000}`, or a second
    /// screen. `key_sequence::DEFAULT_SEQUENCE` is an honest description of
    /// what the *SendInput fallback* does -- which is what the preview should
    /// show a user with no sequence -- but the default fill is a different
    /// act, so it keeps a different path.
    /// Refuses outright if a sequence is already being typed -- see
    /// [`SequenceGuard`] for why refusing beats queueing.
    ///
    /// `on_outcome` is handed what the fill actually did, once something
    /// knows -- which for the sequence path is later, on the typing thread.
    /// **Every** exit reports exactly once, including this one, so a caller
    /// that counts fills never has to infer anything from the return value.
    pub fn fill_sequence(
        &self,
        hwnd: isize,
        plan: Plan,
        on_outcome: OutcomeSink,
    ) -> Result<(), String> {
        let Some(mut guard) = SequenceGuard::acquire() else {
            // Explicit, and not merely "let it fall out of scope": this is the
            // one exit from this function that holds a resolved password and
            // builds no `Plan`-owning thread to wipe it later.
            drop(plan);
            // Equally explicit. A refused fill typed nothing, and saying so
            // out loud beats leaving `on_outcome` to be dropped uncalled and
            // relying on a reader to work out that a sink never invoked and a
            // sink invoked with `NotTyped` happen to mean the same thing.
            on_outcome(FillOutcome::NotTyped);
            return Err(ALREADY_TYPING.to_string());
        };
        guard.reports_to(on_outcome);
        self.fallback.fill_sequence(hwnd, plan, guard)
    }
}

#[derive(Clone, Copy)]
pub struct RealUiAutomation;
impl UiAutomationFiller for RealUiAutomation {
    fn fill(&self, hwnd: isize, user: &str, pass: &str) -> Result<bool, String> {
        ui_automation::fill_via_ui_automation(hwnd, user, pass).map_err(|e| e.to_string())
    }
}

/// **What a finished [`sequence::run`] meant for the fill count**, as a pure
/// function.
///
/// One line, extracted, because the body it came out of runs on a typing
/// thread behind `SendInput` and cannot be entered from a test -- and it is
/// the line that decides whether a password the user interrupted counts as
/// typed. Left inline it would have been exactly the shape this crate keeps
/// getting caught by: a value at a call site that nothing can read back.
///
/// `Err` covers both an abandoned run (the foreground re-check refused before
/// a step) and a failed keystroke. Both are [`FillOutcome::Partial`]: typing
/// had begun, and it did not finish. Neither counts -- see
/// [`crate::fill_stats::counts_as_a_fill`].
fn outcome_of_a_run(result: &Result<(), String>) -> FillOutcome {
    match result {
        Ok(()) => FillOutcome::Typed,
        Err(_) => FillOutcome::Partial,
    }
}

/// **The typing thread's entire body, as a function a test can call.**
///
/// It used to be the closure inside `RealSendInput::fill_sequence`'s
/// `thread::spawn`, which sits behind `SendInput` and cannot be entered from a
/// test. One line in it -- `guard.report(outcome_of_a_run(&result))` -- was the
/// whole of `b14b6b2` in production, and nothing could read it back: replacing
/// it with a constant `FillOutcome::Typed` left the entire suite green,
/// silently reinstating the pre-`b14b6b2` defect in which an abandoned
/// sequence records a fill and climbs the picker's MRU. Everything *around*
/// that line was exhaustively tested; the line itself was not pinned anywhere.
///
/// Generic over [`sequence::Keyboard`] and [`sequence::Notifier`] rather than
/// hard-wired to the real ones, so `sequence::run_tests::FakeKeyboard` -- which
/// answers `holds_foreground` however a test says and **sends no real input**
/// -- can drive it, and a recording notifier can stand in for the message box.
///
/// # Two things this is **not** covered for, stated so nobody assumes it is
///
/// The allocator probe in `login_ui::password_lifetime_tests` is
/// `thread_local!`, and the real `fill_sequence` runs this on a thread it
/// spawns. So no `!leaked` assertion anywhere can see the production drop of a
/// sequence's [`Plan`] -- what is covered is this function called directly, on
/// the test's own thread. The wipe is a property of `Drop for Plan` and is
/// pinned there; the claim that it happens *on the typing thread* is not
/// something the instrument can reach.
///
/// And `sequence::run_tests::FakeKeyboard::holds_foreground` ignores its
/// `hwnd` and answers from a script, so a caller that passed the *wrong*
/// window here would be invisible to every test that drives this. The `hwnd`
/// hand-off from `fill_sequence` to `perform` is unpinned.
///
/// # The order of the last three acts is the contract
///
/// **The plan is wiped first**, and only then is the flag released and the
/// sink run. That was not what the code did: `let mut guard = guard` moved the
/// guard out of the closure's capture, making it a local that dropped *before*
/// the still-captured `plan`. So the "already typing" flag went back and the
/// sink ran -- writing a JSON file synchronously -- while a plaintext password
/// was still sitting on this thread's stack, the exact reverse of what the
/// comment underneath claimed. The explicit `drop(plan)` is what makes the
/// code and the comment agree, and it costs nothing.
fn perform<K: sequence::Keyboard, N: sequence::Notifier>(
    kb: &K,
    hwnd: isize,
    plan: Plan,
    mut guard: SequenceGuard,
    notifier: &N,
) {
    let result = sequence::run(kb, hwnd, &plan);
    // **First.** Wiping the password is not allowed to wait behind a
    // bookkeeping callback, and nothing that observes this run's end may
    // observe it while the plaintext is still here.
    drop(plan);
    guard.report(outcome_of_a_run(&result));
    if let Err(e) = result {
        sequence::Notifier::refused(notifier, &e);
    }
    // `guard` drops here: the flag is released and the outcome delivered,
    // after the wipe. On a panic anywhere above, unwinding still drops both,
    // and the outcome reported is the default `NotTyped` rather than a fill
    // nothing can vouch for.
}

/// **The sequence template the default fill is, spelled out.**
///
/// [`crate::key_sequence::DEFAULT_SEQUENCE`] is `{USERNAME}{TAB}{PASSWORD}`
/// and that is what a fill with both credentials types. The two elisions are
/// not a shortcut, they are the only way to keep the old behaviour: the old
/// body called `type_text(user)` unconditionally, and typing an **empty**
/// string sends no keystrokes -- whereas [`sequence::plan`] refuses a
/// `{USERNAME}` it cannot resolve with [`sequence::Refusal::Unresolved`]. Left
/// in, a login that stores only a password -- which the vault permits, and
/// which filled perfectly well yesterday -- would stop filling at all.
///
/// **The `{TAB}` is never elided, including when both are empty.** It looks
/// like a case worth tidying and it is not ours to tidy: `fill_via_send_input`
/// pressed Tab between two `type_text` calls with no reference to whether
/// either had anything to type, so a password-only item has always had a Tab
/// pressed *before* its password. That may well be wrong, but this change is
/// about bounding the gap between foreground checks, and quietly deleting a
/// keystroke from every password-only fill on the way past is exactly the kind
/// of unrelated behaviour change a security fix must not smuggle in. It is
/// recorded, not fixed.
///
/// A template and not a plan: the string returned here holds placeholders and
/// never a secret. The one copy of the password is made by [`sequence::plan`],
/// into a [`Plan`], which wipes it.
fn default_sequence_for(user: &str, pass: &str) -> String {
    let mut template = String::new();
    if !user.is_empty() {
        template.push_str("{USERNAME}");
    }
    template.push_str("{TAB}");
    if !pass.is_empty() {
        template.push_str("{PASSWORD}");
    }
    template
}

/// The default fill as a [`Plan`], so it runs through the same machinery a
/// stored sequence does.
///
/// Built by parsing a template and planning it rather than by assembling
/// [`sequence::Step`]s here, so that the [`MAX_BURST`](sequence::MAX_BURST)
/// chunking, the UTF-16 projection, the [`MIN_RATE`](sequence::MIN_RATE) floor
/// and the [`MAX_SEQUENCE`](sequence::MAX_SEQUENCE) bound are the *same*
/// tested code and not a second implementation of each that has to be kept in
/// step.
fn default_plan(user: &str, pass: &str) -> Result<Plan, String> {
    sequence::plan(
        &crate::key_sequence::parse(&default_sequence_for(user, pass)),
        &sequence::Resolved { username: user, password: pass, totp: None, custom: vec![] },
    )
    .map_err(|refusal| refusal.message())
}

/// **The default fill's typing, as a function a test can call** -- the same
/// move [`perform`] is, for the same reason.
///
/// Everything in [`RealSendInput::fill`] except the one
/// [`send_input::ensure_foreground`] call, which is a live Win32 round trip
/// with sleeps in it and cannot be entered from a test. What is left is
/// generic over [`sequence::Keyboard`], so `sequence::run_tests::FakeKeyboard`
/// -- which answers `holds_foreground` however a test says and **sends no real
/// input** -- can drive the whole default fill and a test can watch it abandon
/// a password half-typed when the window goes away. Left inline behind
/// `SendInput` it would have been the shape this crate keeps getting caught
/// by: a guarantee at a call site that nothing can read back.
fn fill_by_typing<K: sequence::Keyboard>(
    kb: &K,
    hwnd: isize,
    user: &str,
    pass: &str,
) -> Result<(), String> {
    let plan = default_plan(user, pass)?;
    let result = sequence::run(kb, hwnd, &plan);
    // Explicit and first, for the reason `perform` drops its plan first:
    // nothing that observes this fill's end -- the caller's `FillOutcome`, the
    // log line, the `SequenceGuard` release one frame up -- may observe it
    // while the plaintext is still on this stack.
    drop(plan);
    result
}

#[derive(Clone, Copy)]
pub struct RealSendInput;
impl SendInputFiller for RealSendInput {
    /// **Types the default fill as a plan, not as a straight line.**
    ///
    /// This used to be `ensure_foreground(hwnd)?` followed by
    /// `fill_via_send_input`, which typed the username, pressed Tab and typed
    /// the password at 3ms per UTF-16 unit with **no further verification of
    /// anything**. One check, then roughly 120ms of unchecked typing for a
    /// forty-character credential pair, and proportionally more for a
    /// passphrase -- while the sequence path a few lines down re-checks the
    /// foreground before every step and chops any burst longer than
    /// [`sequence::MAX_BURST`] so that it can. Every item in every existing
    /// vault stores no sequence and therefore takes *this* path, so the
    /// 250ms guarantee -- corrected once for UTF-16 and once for `{DELAY=0}` --
    /// existed only on the minority one.
    ///
    /// The fix is to stop having two implementations. [`default_plan`] turns
    /// the fill into the plan it always was, and [`sequence::run`] performs it,
    /// which inherits the per-step foreground re-check, the burst chop, the
    /// rate floor and the zeroizing `Drop for Plan` in a form that cannot drift
    /// from the sequence path because it *is* the sequence path.
    ///
    /// **What is deliberately not collapsed is [`Injector::fill`]'s dispatch.**
    /// UI Automation fills *named fields* without focus and without a
    /// keystroke; a plan types at whatever has focus. Those stay two different
    /// acts, and the UIA-first order is untouched -- see
    /// [`Injector::fill_sequence`]'s doc, which makes that argument at length.
    /// What changed is only how the SendInput fallback types once UIA has
    /// declined, which is the one place the two paths were doing the same act
    /// by two different means.
    ///
    /// Still synchronous, and still one [`send_input::ensure_foreground`]: that
    /// first call *restores* focus, which is right exactly once, right after
    /// our own overlay closes. Every check after it is
    /// [`send_input::RealKeyboard::holds_foreground`], which is passive and
    /// stops rather than yanking the user's window back.
    fn fill(&self, hwnd: isize, user: &str, pass: &str) -> Result<(), String> {
        // Verify (and if necessary restore) foreground before typing anything.
        // On mismatch this returns Err and nothing is typed, rather than
        // blasting a password into an unverified window.
        send_input::ensure_foreground(hwnd)?;
        fill_by_typing(&send_input::RealKeyboard, hwnd, user, pass)
    }

    /// Restores foreground once, then hands the plan to a thread.
    ///
    /// The one [`send_input::ensure_foreground`] call is the *same* one the
    /// default fill makes and is there for the same reason: right after our
    /// own overlay closes, Windows has not necessarily handed focus back yet,
    /// and refusing on that transient would refuse most Prompt-mode fills.
    /// After that single restore, every further check is
    /// `RealKeyboard::holds_foreground`, which is passive -- see its doc for
    /// why re-stealing focus mid-sequence would be exactly backwards.
    ///
    /// The thread is what keeps a `{DELAY 2000}` from freezing the app. It
    /// means `Ok(())` here means "started", not "typed"; the outcome the user
    /// needs to know about is an abort, and that is reported by the notifier
    /// from inside the thread rather than through a return value nobody is
    /// still waiting on -- and, since the count of fills must mean "typed" and
    /// not "started" either, through `guard.report` on the same thread.
    fn fill_sequence(&self, hwnd: isize, plan: Plan, guard: SequenceGuard) -> Result<(), String> {
        let mut guard = guard;
        if let Err(e) = send_input::ensure_foreground(hwnd) {
            // The one exit that returns before a thread exists. Nothing was
            // typed, and this says so rather than leaning on the default.
            guard.report(FillOutcome::NotTyped);
            return Err(e);
        }
        std::thread::spawn(move || {
            // `guard` and `plan` are moved onto this thread so the "already
            // typing" flag stays set for as long as this thread is really
            // typing, and not merely until the spawn returns. Everything the
            // thread then does is [`perform`], which is a function precisely
            // so that something other than a live `SendInput` can call it.
            perform(&send_input::RealKeyboard, hwnd, plan, guard, &sequence::RealNotifier);
        });
        Ok(())
    }
}

#[cfg(test)]
mod orchestration_tests {
    use super::*;
    use std::cell::RefCell;
    use std::sync::{Arc, Mutex};

    /// A sink that remembers every outcome reported through it.
    ///
    /// A `Vec<FillOutcome>` and not a counter: the assertions below care
    /// *which* outcome arrived and *how many times*, and a counter would let
    /// `Typed` and `NotTyped` stand in for one another -- which is precisely
    /// the confusion this whole change exists to remove.
    #[derive(Clone, Default)]
    struct Reported(Arc<Mutex<Vec<FillOutcome>>>);

    impl Reported {
        fn sink(&self) -> OutcomeSink {
            let seen = self.0.clone();
            Box::new(move |outcome| seen.lock().unwrap().push(outcome))
        }
        fn seen(&self) -> Vec<FillOutcome> {
            self.0.lock().unwrap().clone()
        }
    }

    /// For the tests that are about the flag and not about counting.
    fn ignored() -> OutcomeSink {
        Box::new(|_| {})
    }

    struct FakeUi {
        result: Result<bool, String>,
        calls: RefCell<u32>,
    }
    impl UiAutomationFiller for FakeUi {
        fn fill(&self, _hwnd: isize, _user: &str, _pass: &str) -> Result<bool, String> {
            *self.calls.borrow_mut() += 1;
            self.result.clone()
        }
    }

    struct FakeFallback {
        calls: RefCell<u32>,
        last_hwnd: RefCell<Option<isize>>,
        result: Result<(), String>,
    }
    impl FakeFallback {
        fn new() -> Self {
            Self {
                calls: RefCell::new(0),
                last_hwnd: RefCell::new(None),
                result: Ok(()),
            }
        }
        fn failing() -> Self {
            Self {
                calls: RefCell::new(0),
                last_hwnd: RefCell::new(None),
                result: Err("target window is not foreground".into()),
            }
        }
    }
    impl SendInputFiller for FakeFallback {
        fn fill(&self, hwnd: isize, _user: &str, _pass: &str) -> Result<(), String> {
            *self.calls.borrow_mut() += 1;
            *self.last_hwnd.borrow_mut() = Some(hwnd);
            self.result.clone()
        }

        /// These tests are about the *default* fill's fallback chain. Reaching
        /// the sequence path here would mean `Injector::fill` had dispatched
        /// somewhere it must never dispatch, so it fails loudly rather than
        /// recording a call some assertion might mistake for the real thing.
        fn fill_sequence(&self, _: isize, _: Plan, _: SequenceGuard) -> Result<(), String> {
            panic!("the default fill must not reach the sequence path")
        }
    }

    // -- one sequence at a time ---------------------------------------------

    fn a_plan() -> Plan {
        sequence::plan(
            &crate::key_sequence::parse("{USERNAME}{TAB}{PASSWORD}"),
            &sequence::Resolved {
                username: "work.account@contoso.com",
                password: "Zq7-tremulous-BADGER",
                totp: None,
                custom: vec![],
            },
        )
        .expect("plans")
    }

    /// A notifier that records instead of opening a window.
    ///
    /// Deliberately **not** `RealNotifier`: that one is silent under test only
    /// because of a `cfg(test)` gate, and the bin links the lib without
    /// `cfg(test)`, so a test that leans on the gate is one fixture away from
    /// a real message box on a real desktop.
    #[derive(Default)]
    struct RecordingNotifier(RefCell<Vec<String>>);
    impl sequence::Notifier for RecordingNotifier {
        fn refused(&self, detail: &str) {
            self.0.borrow_mut().push(detail.to_string());
        }
    }

    /// **`b14b6b2`, pinned at the one place production runs it.**
    ///
    /// The line that decides whether an interrupted password counts as a fill
    /// lived inside a `thread::spawn` closure behind `SendInput`, where no test
    /// could reach it: replacing it with a hard-wired `FillOutcome::Typed` left
    /// the whole suite green, quietly reinstating the defect where an abandoned
    /// sequence records a fill and climbs the picker's MRU. Now that the body is
    /// [`perform`], a `FakeKeyboard` can drive it.
    ///
    /// The two arms must not be able to stand in for one another, so they
    /// differ in the keyboard, in the reported outcome, **and** in whether the
    /// user was told -- and the second `acquire` doubles as proof that the
    /// first run released the flag on its way out.
    #[test]
    fn the_typing_thread_reports_what_the_run_actually_did() {
        use sequence::run_tests::FakeKeyboard;
        let _lock = sequence_test_lock();
        const HWND: isize = 0x4321;

        // Not `assert_ne!(Typed, Partial)`, which was what stood here: two
        // distinct variants of a `PartialEq` enum can never be equal, so it
        // read like the crate's two-inputs-agree guard while pinning nothing.
        // What the two arms below actually have to differ in is the answer
        // this suite cares about, so that is what is asserted.
        assert_ne!(
            crate::fill_stats::counts_as_a_fill(FillOutcome::Typed),
            crate::fill_stats::counts_as_a_fill(FillOutcome::Partial),
            "the fixture cannot tell a completed run from an abandoned one"
        );

        // A run that finishes reports `Typed`, and has nothing to warn about.
        let finished = Reported::default();
        let mut guard = SequenceGuard::acquire().expect("the flag is free");
        guard.reports_to(finished.sink());
        let told = RecordingNotifier::default();
        perform(&FakeKeyboard::new(), HWND, a_plan(), guard, &told);
        assert_eq!(
            finished.seen(),
            vec![FillOutcome::Typed],
            "a completed run must count as a fill"
        );
        assert!(
            told.0.borrow().is_empty(),
            "a completed run has nothing to tell the user"
        );

        // The same call with the foreground gone before the first step reports
        // `Partial`, which `fill_stats::counts_as_a_fill` does not count.
        let abandoned = Reported::default();
        let mut guard =
            SequenceGuard::acquire().expect("the finished run released the flag on its way out");
        guard.reports_to(abandoned.sink());
        let told = RecordingNotifier::default();
        perform(&FakeKeyboard::loses_foreground_after(0), HWND, a_plan(), guard, &told);
        assert_eq!(
            abandoned.seen(),
            vec![FillOutcome::Partial],
            "an abandoned sequence must not record a fill"
        );
        assert_eq!(
            told.0.borrow().len(),
            1,
            "an abandoned sequence must tell the user why it stopped"
        );
    }

    /// A filler that **keeps** the guard instead of releasing it: what the
    /// real one looks like from the outside while its thread is still typing.
    struct StillTyping {
        held: RefCell<Vec<SequenceGuard>>,
        calls: RefCell<u32>,
    }
    impl StillTyping {
        fn new() -> Self {
            Self { held: RefCell::new(Vec::new()), calls: RefCell::new(0) }
        }
        /// Ends the "typing" run without saying what it did -- a thread that
        /// died, or one that simply never reported.
        fn finish(&self) {
            self.held.borrow_mut().clear();
        }

        /// Ends the "typing" run the way the real filler's thread does:
        /// reporting the outcome, then dropping the guard.
        fn finish_reporting(&self, outcome: FillOutcome) {
            let mut held = self.held.borrow_mut();
            for guard in held.iter_mut() {
                guard.report(outcome);
            }
            held.clear();
        }
    }
    impl SendInputFiller for StillTyping {
        fn fill(&self, _: isize, _: &str, _: &str) -> Result<(), String> {
            panic!("this test is about the sequence path")
        }
        fn fill_sequence(&self, _: isize, _: Plan, guard: SequenceGuard) -> Result<(), String> {
            *self.calls.borrow_mut() += 1;
            self.held.borrow_mut().push(guard);
            Ok(())
        }
    }

    /// **Two hotkey presses during one `{DELAY}` must not interleave two
    /// passwords into the same field.**
    ///
    /// Nothing guarded this: `fill_sequence` spawned a thread per call, both
    /// threads saw the same foreground window, and both passed every check.
    #[test]
    fn a_second_sequence_is_refused_while_the_first_is_still_typing() {
        let _serialised = sequence_test_lock();
        let injector = Injector { ui: FakeUi { result: Ok(false), calls: RefCell::new(0) }, fallback: StillTyping::new() };

        injector.fill_sequence(7, a_plan(), ignored()).expect("the first fill starts");

        let err = injector
            .fill_sequence(7, a_plan(), ignored())
            .expect_err("the second fill must be refused, not started");
        assert!(err.contains("already being typed"), "got: {err}");
        assert_eq!(
            *injector.fallback.calls.borrow(),
            1,
            "the refused fill still reached the filler"
        );
    }

    /// The other half: the refusal is temporary, not a one-shot latch. Without
    /// this, a guard that was never released would pass the test above.
    #[test]
    fn a_sequence_is_allowed_again_once_the_previous_one_finishes() {
        let _serialised = sequence_test_lock();
        let injector = Injector { ui: FakeUi { result: Ok(false), calls: RefCell::new(0) }, fallback: StillTyping::new() };

        injector.fill_sequence(7, a_plan(), ignored()).expect("the first fill starts");
        assert!(
            injector.fill_sequence(7, a_plan(), ignored()).is_err(),
            "expected a refusal"
        );

        injector.fallback.finish();

        injector
            .fill_sequence(7, a_plan(), ignored())
            .expect("a fill after the first finished");
        assert_eq!(*injector.fallback.calls.borrow(), 2, "the third fill did not reach the filler");
    }

    /// A typing thread that panics must not wedge auto-type for the session.
    /// `Drop` runs while unwinding, so the flag is released on the way out.
    #[test]
    fn a_panicking_sequence_releases_the_guard() {
        let _serialised = sequence_test_lock();

        // The panic below is the fixture, not a failure. Muting the hook for
        // its duration keeps a deliberate panic from printing a backtrace that
        // reads like a broken test in the suite's output.
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let panicked = std::panic::catch_unwind(|| {
            let guard = SequenceGuard::acquire().expect("nothing else holds it");
            let _moved_onto_the_doomed_thread = guard;
            panic!("the typing thread died");
        });
        std::panic::set_hook(previous);
        assert!(panicked.is_err(), "the fixture did not actually panic");

        assert!(
            SequenceGuard::acquire().is_some(),
            "a panic left the guard held and auto-type wedged"
        );
    }

    // -- what the fill actually did -----------------------------------------

    /// **A run that performed every step typed.** The positive control for
    /// the two below: without it `outcome_of_a_run` could answer `Partial`
    /// unconditionally and no sequence would ever be counted again.
    #[test]
    fn a_run_that_finished_counts_as_typed() {
        assert_eq!(outcome_of_a_run(&Ok(())), FillOutcome::Typed);
    }

    /// **A run that stopped part-way did not type.**
    ///
    /// The user alt-tabbed after the username: `run` returns the "no longer in
    /// front, nothing further was typed" error, and the password never
    /// arrived. Answering `Typed` here is the defect this change exists to
    /// remove, transplanted one layer down.
    #[test]
    fn a_run_that_was_abandoned_counts_as_partial() {
        let abandoned = Err(
            "auto-type stopped: window 7 is no longer in front (after 2 of 5 steps).              Nothing further was typed."
                .to_string(),
        );
        assert_eq!(outcome_of_a_run(&abandoned), FillOutcome::Partial);
    }

    /// A keystroke that failed outright takes the same answer -- typing had
    /// begun and did not finish, whatever stopped it.
    #[test]
    fn a_run_whose_keystroke_failed_also_counts_as_partial() {
        assert_eq!(
            outcome_of_a_run(&Err("SendInput refused the keystroke".to_string())),
            FillOutcome::Partial
        );
    }

    /// Neither answer a run can give is one that a count may be taken from
    /// without the decision in [`crate::fill_stats::counts_as_a_fill`] --
    /// which is to say: finishing is the only thing that counts.
    #[test]
    fn only_a_finished_run_is_countable_as_a_fill() {
        use crate::fill_stats::counts_as_a_fill;
        assert!(counts_as_a_fill(outcome_of_a_run(&Ok(()))));
        assert!(!counts_as_a_fill(outcome_of_a_run(&Err("stopped".to_string()))));
    }

    /// **Silence means nothing was typed.**
    ///
    /// A guard that drops without a `report` is the shape a panicking typing
    /// thread leaves behind, and the shape of any future path that forgets.
    /// Defaulting the other way would credit a fill nobody can vouch for.
    #[test]
    fn a_guard_that_reports_nothing_says_nothing_was_typed() {
        let _serialised = sequence_test_lock();
        let reported = Reported::default();

        let mut guard = SequenceGuard::acquire().expect("nothing else holds it");
        guard.reports_to(reported.sink());
        assert!(reported.seen().is_empty(), "reported before the guard was released");
        drop(guard);

        assert_eq!(reported.seen(), vec![FillOutcome::NotTyped]);
    }

    /// The positive control for the test above: a guard that *was* told
    /// something reports that, and reports it once.
    #[test]
    fn a_guard_reports_what_it_was_told_exactly_once() {
        let _serialised = sequence_test_lock();
        let reported = Reported::default();

        let mut guard = SequenceGuard::acquire().expect("nothing else holds it");
        guard.reports_to(reported.sink());
        guard.report(FillOutcome::Typed);
        drop(guard);

        assert_eq!(reported.seen(), vec![FillOutcome::Typed]);
    }

    /// A run that started typing and then stopped reports the stop, not the
    /// start. Without this, a filler could report `Typed` the moment it began
    /// and nothing would notice -- which is the exact defect being fixed.
    #[test]
    fn a_later_report_replaces_an_earlier_one() {
        let _serialised = sequence_test_lock();
        let reported = Reported::default();

        let mut guard = SequenceGuard::acquire().expect("nothing else holds it");
        guard.reports_to(reported.sink());
        guard.report(FillOutcome::Typed);
        guard.report(FillOutcome::Partial);
        drop(guard);

        assert_eq!(reported.seen(), vec![FillOutcome::Partial]);
    }

    /// **A typing thread that panics part-way must not report a fill.**
    ///
    /// `panic = unwind`, so `Drop` runs on the way out and the sink is called;
    /// what it is called with is the default, because the `report` the thread
    /// was going to make never happened.
    #[test]
    fn a_panicking_typing_thread_reports_that_nothing_was_typed() {
        let _serialised = sequence_test_lock();
        let reported = Reported::default();

        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let sink = reported.sink();
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let mut guard = SequenceGuard::acquire().expect("nothing else holds it");
            guard.reports_to(sink);
            let _moved_onto_the_doomed_thread = guard;
            panic!("the typing thread died");
        }));
        std::panic::set_hook(previous);
        assert!(panicked.is_err(), "the fixture did not actually panic");

        assert_eq!(
            reported.seen(),
            vec![FillOutcome::NotTyped],
            "a panicking run reported something other than 'nothing was typed'"
        );
        assert!(
            SequenceGuard::acquire().is_some(),
            "a panic left the guard held and auto-type wedged"
        );
    }

    /// **A sequence refused because another is already typing typed nothing**,
    /// and says so. This is one of the two cases the old `Ok(()) =>
    /// record_fill` arm got wrong: the refusal never reaches a keyboard, and
    /// it used to be indistinguishable from a fill.
    #[test]
    fn a_sequence_refused_while_another_is_typing_reports_that_nothing_was_typed() {
        let _serialised = sequence_test_lock();
        let injector = Injector {
            ui: FakeUi { result: Ok(false), calls: RefCell::new(0) },
            fallback: StillTyping::new(),
        };
        let first = Reported::default();
        let second = Reported::default();

        injector.fill_sequence(7, a_plan(), first.sink()).expect("the first fill starts");
        injector
            .fill_sequence(7, a_plan(), second.sink())
            .expect_err("the second fill must be refused");

        assert_eq!(
            second.seen(),
            vec![FillOutcome::NotTyped],
            "the refused fill did not report that it typed nothing"
        );
    }

    /// **The UI thread does not learn the outcome, and does not wait for it.**
    ///
    /// `fill_sequence` has already returned `Ok(())` while the filler is still
    /// holding the guard -- exactly as the real one returns while its thread is
    /// still typing -- and nothing has been reported yet. The report arrives
    /// when the typing ends, from wherever it ended.
    #[test]
    fn the_outcome_arrives_when_the_typing_ends_not_when_the_call_returns() {
        let _serialised = sequence_test_lock();
        let injector = Injector {
            ui: FakeUi { result: Ok(false), calls: RefCell::new(0) },
            fallback: StillTyping::new(),
        };
        let reported = Reported::default();

        injector.fill_sequence(7, a_plan(), reported.sink()).expect("the fill starts");
        assert!(
            reported.seen().is_empty(),
            "the outcome was reported before the typing finished, so `Ok(())` still \
             means 'started' to whoever is counting"
        );

        injector.fallback.finish_reporting(FillOutcome::Typed);

        assert_eq!(reported.seen(), vec![FillOutcome::Typed]);
    }

    /// The other half of the test above with the outcome that must not count:
    /// a run abandoned mid-way reports `Partial`, and the same wiring carries
    /// it. Substituting `Typed` here is what the fixed defect looked like.
    #[test]
    fn an_abandoned_sequence_reports_that_it_only_partly_typed() {
        let _serialised = sequence_test_lock();
        let injector = Injector {
            ui: FakeUi { result: Ok(false), calls: RefCell::new(0) },
            fallback: StillTyping::new(),
        };
        let reported = Reported::default();

        injector.fill_sequence(7, a_plan(), reported.sink()).expect("the fill starts");
        injector.fallback.finish_reporting(FillOutcome::Partial);

        assert_eq!(reported.seen(), vec![FillOutcome::Partial]);
    }

    // -- the default fill is a plan too ------------------------------------

    /// The two must never be able to stand in for one another: a fixture whose
    /// user name and password agree cannot tell a fill from a swap, and cannot
    /// tell "typed the username twice" from "typed both". Asserted, not merely
    /// written differently, so that editing one of them back into the other is
    /// a failure rather than a silently weaker suite.
    const FILL_USER: &str = "hedge.sparrow@contoso.example";
    const FILL_PASS: &str = "Vv4-quixotic-LANTERN-7";

    #[test]
    fn the_fill_fixture_can_tell_the_username_from_the_password() {
        assert_ne!(FILL_USER, FILL_PASS, "the fixture cannot tell a swap from a fill");
    }

    /// **The default fill types the same three things it always did.**
    ///
    /// The behaviour `fill_via_send_input` had, now expressed as a plan. If the
    /// template ever loses a piece or reorders them, this is what says so.
    #[test]
    fn the_default_fill_types_the_username_then_tab_then_the_password() {
        use sequence::run_tests::FakeKeyboard;
        let kb = FakeKeyboard::new();

        fill_by_typing(&kb, 0x99, FILL_USER, FILL_PASS).expect("a default fill types");

        assert_eq!(
            kb.transcript(),
            vec![
                format!("type {FILL_USER}"),
                "press TAB".to_string(),
                format!("type {FILL_PASS}"),
            ]
        );
    }

    /// **The gap between foreground checks, which is the whole finding.**
    ///
    /// The old body asked once and then typed the username, a Tab and the
    /// password with nothing in between -- ~120ms unchecked for a short
    /// credential pair, proportionally more for a passphrase. Now every step is
    /// preceded by its own check, exactly as a stored sequence's is.
    #[test]
    fn the_default_fill_re_checks_the_foreground_before_every_step() {
        use sequence::run_tests::FakeKeyboard;
        let kb = FakeKeyboard::new();

        fill_by_typing(&kb, 0x99, FILL_USER, FILL_PASS).expect("a default fill types");

        let plan = default_plan(FILL_USER, FILL_PASS).expect("the same plan it just ran");
        assert!(plan.len() > 1, "a one-step plan would make this assertion meaningless");
        assert_eq!(
            kb.foreground_checks() as usize,
            plan.len(),
            "the default fill did not re-check the foreground once per step"
        );
    }

    /// **A long credential is chopped, so no single burst outruns
    /// [`sequence::MAX_BURST`].**
    ///
    /// The 250ms guarantee the sequence path is built around, on the path every
    /// item in every existing vault actually takes. A passphrase used to be one
    /// unbroken burst of `SendInput` calls with no check anywhere inside it;
    /// the projection of the whole plan below is many times the burst bound,
    /// and not one step is.
    #[test]
    fn a_long_credential_is_chopped_so_no_burst_outruns_the_bound() {
        // 500 units at the 3ms default rate is ~1.5s of typing: six bursts, not
        // one. A real passphrase or a generated 128-character password lands in
        // the same territory.
        let long_password = "q".repeat(500);
        let plan = default_plan(FILL_USER, &long_password).expect("plans");

        assert!(
            plan.projected() > sequence::MAX_BURST * 4,
            "the fixture is too short to need chopping, so this proves nothing"
        );
        let bursts = plan
            .steps()
            .iter()
            .filter(|s| matches!(s, sequence::Step::Text { .. }))
            .count();
        assert!(
            bursts > 2,
            "a long password was not split into bursts (got {bursts} text steps)"
        );
        for step in plan.steps() {
            assert!(
                step.projected() <= sequence::MAX_BURST,
                "a single burst is projected at {:?}, over the {:?} bound",
                step.projected(),
                sequence::MAX_BURST
            );
        }
    }

    /// **A default fill whose window goes away stops, and the password never
    /// arrives.**
    ///
    /// The behaviour that did not exist at all before: the old body would have
    /// typed the password into whatever had taken the foreground. The fake
    /// keyboard sends no real input, so this exercises the abandonment without
    /// a window and without a keystroke.
    #[test]
    fn a_default_fill_stops_when_the_window_goes_away_before_the_password() {
        use sequence::run_tests::FakeKeyboard;
        // In front for the username, gone by the Tab.
        let kb = FakeKeyboard::loses_foreground_after(1);

        let err = fill_by_typing(&kb, 0x99, FILL_USER, FILL_PASS)
            .expect_err("a fill into a window that went away must not report success");

        assert!(err.contains("no longer in front"), "got: {err}");
        assert_eq!(
            kb.transcript(),
            vec![format!("type {FILL_USER}")],
            "the fill kept typing after the window it was aimed at had gone"
        );
        assert!(
            !kb.transcript().iter().any(|line| line.contains(FILL_PASS)),
            "the password was typed into a window that was no longer ours"
        );
    }

    /// **A login with no user name still fills.**
    ///
    /// `plan` refuses an unresolvable `{USERNAME}`, and the old straight line
    /// simply typed an empty string and sent no keystrokes. Leaving
    /// `{USERNAME}` in the template unconditionally would have turned every
    /// password-only item in the vault -- which the vault permits -- into a
    /// fill that refuses outright.
    #[test]
    fn a_login_with_no_username_still_fills_its_password() {
        use sequence::run_tests::FakeKeyboard;
        let kb = FakeKeyboard::new();

        fill_by_typing(&kb, 0x99, "", FILL_PASS).expect("a password-only login still fills");

        assert_eq!(
            kb.transcript(),
            vec!["press TAB".to_string(), format!("type {FILL_PASS}")],
            "a password-only fill must type exactly what the straight line did"
        );
    }

    /// The other half, and the reason the `{TAB}` is unconditional: the old
    /// body pressed Tab between two `type_text` calls whatever either
    /// contained, so a username-only fill has always ended on a Tab.
    #[test]
    fn a_login_with_no_password_still_fills_its_username() {
        use sequence::run_tests::FakeKeyboard;
        let kb = FakeKeyboard::new();

        fill_by_typing(&kb, 0x99, FILL_USER, "").expect("a username-only login still fills");

        assert_eq!(
            kb.transcript(),
            vec![format!("type {FILL_USER}"), "press TAB".to_string()],
        );
    }

    /// The template, pinned directly, so a change to it is visible as a change
    /// to it and not only as a change to four transcripts.
    #[test]
    fn the_default_template_elides_only_what_it_cannot_resolve() {
        assert_eq!(default_sequence_for(FILL_USER, FILL_PASS), "{USERNAME}{TAB}{PASSWORD}");
        assert_eq!(default_sequence_for("", FILL_PASS), "{TAB}{PASSWORD}");
        assert_eq!(default_sequence_for(FILL_USER, ""), "{USERNAME}{TAB}");
        // Reached only if a caller skips `app::fill_from_vault`'s empty-
        // credentials warning; it is the straight line's own behaviour, which
        // pressed Tab and typed nothing either side of it.
        assert_eq!(default_sequence_for("", ""), "{TAB}");
    }

    /// The template it builds really is the crate's stated default, rather
    /// than a second spelling of it that could drift.
    #[test]
    fn the_default_template_is_the_crates_default_sequence() {
        assert_eq!(
            default_sequence_for(FILL_USER, FILL_PASS),
            crate::key_sequence::DEFAULT_SEQUENCE
        );
    }

    // -- one fill at a time, on the default path too -------------------------

    /// A fallback that reports whether anything else could have typed while it
    /// was running -- i.e. whether the default path really holds the guard for
    /// the duration, rather than acquiring and dropping it on the way past.
    struct ChecksTheFlag {
        free_during_the_fill: RefCell<Option<bool>>,
    }
    impl SendInputFiller for ChecksTheFlag {
        fn fill(&self, _: isize, _: &str, _: &str) -> Result<(), String> {
            *self.free_during_the_fill.borrow_mut() = Some(SequenceGuard::acquire().is_some());
            Ok(())
        }
        fn fill_sequence(&self, _: isize, _: Plan, _: SequenceGuard) -> Result<(), String> {
            panic!("this test is about the default path")
        }
    }

    /// **The finding: a default fill used to type while a sequence was mid
    /// `{DELAY}`.**
    ///
    /// With a guard held, `Injector::fill` succeeded and the fallback really
    /// typed -- two runs interleaving keystrokes into one field, which is
    /// exactly what `SequenceGuard`'s doc says it prevents. Neither the UIA arm
    /// nor the keystroke arm may be reached now, so the refusal is checked
    /// before the dispatch and not inside one branch of it.
    #[test]
    fn a_default_fill_is_refused_while_something_else_is_typing() {
        let _serialised = sequence_test_lock();
        let injector = Injector {
            ui: FakeUi { result: Ok(false), calls: RefCell::new(0) },
            fallback: FakeFallback::new(),
        };

        let held = SequenceGuard::acquire().expect("nothing else holds it");
        let err = injector
            .fill(7, FILL_USER, FILL_PASS)
            .expect_err("a default fill during a sequence must be refused, not typed");

        assert!(err.contains("already being typed"), "got: {err}");
        assert_eq!(
            *injector.fallback.calls.borrow(),
            0,
            "the refused fill still reached the keyboard"
        );
        assert_eq!(
            *injector.ui.calls.borrow(),
            0,
            "the refused fill still reached UI Automation, which sets the same fields"
        );

        // The positive control: the refusal is a state, not a latch. Without
        // this, a `fill` hard-wired to `Err` would pass everything above.
        drop(held);
        injector.fill(7, FILL_USER, FILL_PASS).expect("a fill once the other run has finished");
        assert_eq!(*injector.fallback.calls.borrow(), 1);
    }

    /// The other direction: while a default fill is running, nothing else may
    /// start. Acquiring the guard and dropping it before the typing -- the
    /// `let Some(_) = ...` spelling, which compiles -- passes the test above
    /// and fails here.
    #[test]
    fn nothing_else_may_type_while_a_default_fill_is_running() {
        let _serialised = sequence_test_lock();
        let injector = Injector {
            ui: FakeUi { result: Ok(false), calls: RefCell::new(0) },
            fallback: ChecksTheFlag { free_during_the_fill: RefCell::new(None) },
        };

        injector.fill(7, FILL_USER, FILL_PASS).expect("the fill runs");

        assert_eq!(
            *injector.fallback.free_during_the_fill.borrow(),
            Some(false),
            "a second run could have started typing in the middle of a default fill"
        );
    }

    /// A default fill that failed must not wedge auto-type for the session:
    /// the guard is released on the error path as well as the happy one.
    #[test]
    fn a_failed_default_fill_still_releases_the_keyboard() {
        let _serialised = sequence_test_lock();
        let injector = Injector {
            ui: FakeUi { result: Ok(false), calls: RefCell::new(0) },
            fallback: FakeFallback::failing(),
        };

        injector.fill(7, FILL_USER, FILL_PASS).expect_err("the fallback refuses");

        assert!(
            SequenceGuard::acquire().is_some(),
            "a failed default fill left the guard held and auto-type wedged"
        );
    }

    /// **A sequence is refused while a default fill is running**, which is the
    /// same flag seen from the other side. Both paths refusing each other is
    /// the property; either one alone is not.
    #[test]
    fn a_sequence_is_refused_while_a_default_fill_holds_the_keyboard() {
        let _serialised = sequence_test_lock();
        let injector = Injector {
            ui: FakeUi { result: Ok(false), calls: RefCell::new(0) },
            fallback: StillTyping::new(),
        };

        // Stand in for a default fill in progress: `Injector::fill` holds
        // exactly this, for exactly as long as it is typing.
        let typing_a_default_fill = SequenceGuard::acquire().expect("nothing else holds it");
        let reported = Reported::default();

        let err = injector
            .fill_sequence(7, a_plan(), reported.sink())
            .expect_err("a sequence during a default fill must be refused");

        assert!(err.contains("already being typed"), "got: {err}");
        assert_eq!(*injector.fallback.calls.borrow(), 0, "the refused sequence reached the filler");
        assert_eq!(
            reported.seen(),
            vec![FillOutcome::NotTyped],
            "the refused sequence did not report that it typed nothing"
        );
        drop(typing_a_default_fill);
    }

    /// Both refusals say the same sentence, because a user who pressed a
    /// hotkey does not know which path they were on. Two literals could drift
    /// into one path explaining itself and the other saying something else.
    #[test]
    fn both_paths_refuse_with_the_same_sentence() {
        let _serialised = sequence_test_lock();
        let injector = Injector {
            ui: FakeUi { result: Ok(false), calls: RefCell::new(0) },
            fallback: FakeFallback::new(),
        };
        let held = SequenceGuard::acquire().expect("nothing else holds it");

        let default_refusal = injector.fill(7, FILL_USER, FILL_PASS).unwrap_err();
        let sequence_refusal =
            injector.fill_sequence(7, a_plan(), ignored()).unwrap_err();
        drop(held);

        assert_eq!(default_refusal, sequence_refusal);
        assert_eq!(default_refusal, ALREADY_TYPING);
        // The sentence has to be actionable: a bare "no" is indistinguishable
        // from a hotkey that never registered.
        assert!(default_refusal.contains("press the hotkey again"), "got: {default_refusal}");
    }

    #[test]
    fn does_not_fall_back_when_ui_automation_succeeds() {
        let _serialised = sequence_test_lock();
        let ui = FakeUi { result: Ok(true), calls: RefCell::new(0) };
        let injector = Injector { ui, fallback: FakeFallback::new() };

        injector.fill(1, "u", "p").unwrap();

        assert_eq!(*injector.ui.calls.borrow(), 1);
        assert_eq!(*injector.fallback.calls.borrow(), 0);
    }

    #[test]
    fn falls_back_when_ui_automation_finds_no_fields() {
        let _serialised = sequence_test_lock();
        let ui = FakeUi { result: Ok(false), calls: RefCell::new(0) };
        let injector = Injector { ui, fallback: FakeFallback::new() };

        injector.fill(1, "u", "p").unwrap();

        assert_eq!(*injector.fallback.calls.borrow(), 1);
    }

    #[test]
    fn falls_back_when_ui_automation_errors() {
        let _serialised = sequence_test_lock();
        let ui = FakeUi { result: Err("com failure".into()), calls: RefCell::new(0) };
        let injector = Injector { ui, fallback: FakeFallback::new() };

        injector.fill(1, "u", "p").unwrap();

        assert_eq!(*injector.fallback.calls.borrow(), 1);
    }

    #[test]
    fn passes_the_target_hwnd_to_the_fallback() {
        let _serialised = sequence_test_lock();
        // The fallback has to know which window it's meant to be typing into
        // so it can verify foreground; before this it typed blind.
        let ui = FakeUi { result: Ok(false), calls: RefCell::new(0) };
        let injector = Injector { ui, fallback: FakeFallback::new() };

        injector.fill(4242, "u", "p").unwrap();

        assert_eq!(*injector.fallback.last_hwnd.borrow(), Some(4242));
    }

    #[test]
    fn surfaces_a_fallback_refusal_as_an_error() {
        let _serialised = sequence_test_lock();
        // If the fallback refuses because the target isn't foreground, that
        // must reach the caller (which logs it), not be swallowed.
        let ui = FakeUi { result: Ok(false), calls: RefCell::new(0) };
        let injector = Injector { ui, fallback: FakeFallback::failing() };

        let err = injector.fill(1, "u", "p").unwrap_err();
        assert!(err.contains("not foreground"), "got: {err}");
    }
}
