//! This crate's own password generator: the one operation of
//! [`crate::vault_backend::VaultBackend`] that no server can answer.
//!
//! # Why this module exists at all, and why it is not inside `rest/`
//!
//! [`crate::vault_backend`]'s docs say it plainly: of the twenty vault
//! operations, `generate` is the one with **no server endpoint anywhere**.
//! `bw serve`'s `GET /generate` is not a Bitwarden API route at all -- it is
//! the `bw` CLI running its own generator in-process and handing the result
//! back over loopback. A backend without `bw` therefore has to generate the
//! string itself, and [`crate::rest::backend::RestBackend`] refused to,
//! recording the reason: choosing an alphabet and an entropy source is a
//! decision for the owner of the crate rather than something slipped into the
//! last function of a backend implementation.
//!
//! That decision has now been taken, and this is where it is written down.
//! **It is a module of its own, beside the backends rather than inside one**,
//! because the requirement was that the generator be *reused*: a generator
//! living in `rest/` would be reachable only by the REST backend, and the
//! next backend -- or the UI, which already builds a [`PasswordRecipe`] in
//! `overlay_ui` -- would have to grow a second one. Two password generators
//! in one app is two answers to "how strong is a password this app made".
//!
//! # What is generated, and what is refused
//!
//! * [`GenerateRequest::Password`] -- generated here. It is a pure algorithm
//!   over a fixed alphabet and needs no data this crate does not have.
//! * [`GenerateRequest::Passphrase`] -- **refused by name**, and see
//!   [`PasswordGenError::NoWordlist`] for why that refusal is the safe answer
//!   rather than the lazy one.
//!
//! # The rules this file is written under
//!
//! Four, and each is load-bearing rather than decorative:
//!
//! 1. **The OS CSPRNG and nothing else.** Every random bit here comes from
//!    [`getrandom`], which is `BCryptGenRandom` on Windows and `getrandom(2)`
//!    on Linux -- the same source [`crate::rest::crypto`] draws its IVs from
//!    and the same one [`crate::vault_disk_cache`] draws its content keys
//!    from. There is no seeded PRNG in this file, no clock, no hash used as
//!    a stretcher, and no `rand` thread RNG.
//! 2. **An RNG failure is an error, never a fallback.** Every draw is
//!    fallible and every failure propagates as [`PasswordGenError::Rng`]. A
//!    generator that quietly returns something when the CSPRNG is unavailable
//!    is a generator that returns something predictable.
//! 3. **No modulo bias.** See [`uniform_below`], which is the whole of the
//!    argument and is sampled byte-wise with rejection precisely so that the
//!    classic defect in this function is both possible to write and possible
//!    to *detect* -- `a_uniform_draw_is_uniform_and_a_naive_modulo_would_not_be`
//!    is the test that would catch it.
//! 4. **Nothing holds the password in the clear.** The result is built once,
//!    into a [`Zeroizing<String>`] whose capacity is exact so that no push
//!    can reallocate and strand a copy in a freed page. There is no
//!    intermediate `String` and no intermediate `Vec<u8>` of characters.
//!
//! # Where this can differ from `bw`, and where it cannot
//!
//! [`PasswordRecipe`]'s own doc is the specification, and it was verified
//! against `bw`'s source rather than its help text -- including the fact that
//! [`PasswordRecipe::avoid_ambiguous`] is named the opposite way round from
//! the `ambiguous` wire key on purpose. [`normalize`] below mirrors
//! `bw`'s option normalisation step by step and its doc names, one by one,
//! the two places this implementation deliberately bounds an input that the
//! JavaScript route would not. Those two are the whole of the difference that
//! is known; they are also in the implementation report on this branch.

use zeroize::Zeroizing;

use crate::vault_bridge::{GenerateRequest, PasswordRecipe};

// ---- the alphabets -----------------------------------------------------------
//
// These four constants ARE the security decision this module exists to make,
// so they are written out as literals a reviewer can count rather than built
// by filtering a range at runtime.
//
// They are Bitwarden's own sets, and the shape worth noticing is that
// "avoid ambiguous" is spelled here as *the default set is already the
// narrow one* and the ambiguous characters are added back when they are
// allowed. That is `bw`'s own direction (its generator's `ambiguous: true`
// means "allow"), and writing it the same way round means a reader comparing
// the two files is not also inverting a condition in their head.

/// Lowercase, with the ambiguous `l` held back. Note that `i` and `o` are
/// **not** held back: that is Bitwarden's choice, not an oversight here --
/// its lowercase set removes only `l`, and matching it matters more than
/// being tidier than it.
const LOWERCASE: &str = "abcdefghijkmnopqrstuvwxyz";
/// The `l` that [`LOWERCASE`] holds back.
const LOWERCASE_AMBIGUOUS: &str = "l";

/// Uppercase, with the ambiguous `I` and `O` held back.
const UPPERCASE: &str = "ABCDEFGHJKLMNPQRSTUVWXYZ";
/// The `I` and `O` that [`UPPERCASE`] holds back.
const UPPERCASE_AMBIGUOUS: &str = "IO";

/// Digits, with the ambiguous `0` and `1` held back.
const NUMBER: &str = "23456789";
/// The `0` and `1` that [`NUMBER`] holds back.
const NUMBER_AMBIGUOUS: &str = "01";

/// The special characters, which have **no** ambiguous variant: Bitwarden's
/// special set is these eight whether ambiguous characters are allowed or
/// not, so there is nothing to add back.
const SPECIAL: &str = "!@#$%^&*";

/// `bw`'s own floor on a generated password's length, which
/// [`PasswordRecipe::length`]'s doc records: the serve route clamps anything
/// below this up to it.
const MIN_LENGTH: u32 = 5;

/// A ceiling on the generated length. **This is one of the two places this
/// implementation is deliberately stricter than the JavaScript route** -- see
/// [`normalize`].
const MAX_LENGTH: u32 = 128;

/// A ceiling on `minNumber` and `minSpecial`, and the second of the two
/// places this implementation is stricter than the route -- see [`normalize`].
const MAX_MINIMUM: u32 = 9;

/// What can go wrong, which is deliberately almost nothing.
///
/// # Nothing in here is a secret, and there is nothing for one to hide in
///
/// Both variants are fieldless. There is no arm carrying a generated
/// password, a partial one, a recipe or a byte of randomness, so a `Debug` of
/// this type in a log line cannot leak anything -- which is what
/// `no_error_can_carry_a_generated_secret` asserts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasswordGenError {
    /// The operating system's CSPRNG could not be read.
    ///
    /// **This is returned and never worked around.** There is no fallback
    /// path in this module: no clock, no counter, no "good enough" source. A
    /// password produced without the CSPRNG is a password whose strength
    /// nobody can state, and a caller that sees this error can retry or tell
    /// the user, both of which are better than being handed a guessable
    /// string it has no way to recognise as one.
    Rng,
    /// A passphrase was asked for, and this crate has no wordlist.
    ///
    /// # Why this is a refusal rather than a small wordlist
    ///
    /// A passphrase's entire strength is the size of the list the words are
    /// drawn from: Bitwarden uses the EFF long list, 7,776 words, which is
    /// 12.9 bits per word -- a four-word passphrase is about 51 bits. A
    /// hand-written or improvised list of a few hundred words looks exactly
    /// like a real passphrase to the user and to this app's own strength
    /// meter while carrying **less than half** the entropy, and neither the
    /// user nor a reviewer can see the difference by looking at the output.
    /// That is the failure this crate treats as worse than a crash: a wrong
    /// answer indistinguishable from a right one.
    ///
    /// Shipping the EFF list is a data and licensing decision for the owner
    /// of this crate and it has not been taken. Until it is, the honest
    /// answer is this variant, and it says what is missing so that the person
    /// reading the refusal knows exactly what would resolve it.
    NoWordlist,
}

impl std::fmt::Display for PasswordGenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rng => f.write_str(
                "this computer's secure random number generator could not be read, so no password \
                 was generated",
            ),
            Self::NoWordlist => f.write_str(
                "generating a passphrase needs a wordlist, and this app does not carry one yet",
            ),
        }
    }
}

impl std::error::Error for PasswordGenError {}

/// The whole of what a caller asks for, answered or refused by name.
///
/// This is the function a [`crate::vault_backend::VaultBackend`] that is not
/// `bw serve` should call: it takes the same [`GenerateRequest`] the trait
/// method takes, so a backend's `generate` is one line and the split between
/// what is implemented and what is refused lives here rather than being
/// re-decided per backend.
pub fn generate(request: &GenerateRequest) -> Result<Zeroizing<String>, PasswordGenError> {
    match request {
        GenerateRequest::Password(recipe) => generate_password(recipe),
        // See `PasswordGenError::NoWordlist`. This arm must not grow an
        // improvised list of words.
        GenerateRequest::Passphrase(_) => Err(PasswordGenError::NoWordlist),
    }
}

/// One character password, from the OS CSPRNG.
///
/// # The shape of the algorithm, and why it is this shape
///
/// It is `bw`'s, and it is worth stating why rather than just asserting it.
/// The naive way to honour `minNumber` is to generate a password and then
/// overwrite its first *n* characters with digits; the result is a password
/// whose digits are always at the front, which is a real and well-known
/// weakness -- it removes the positions of the forced characters from the
/// search space entirely.
///
/// So instead a **plan** is built first: a list of class tags, one per
/// character, holding the required minimums and filled out with "any enabled
/// class". That list is then **shuffled** with a Fisher-Yates whose swap
/// indices come from the same unbiased draw as the characters, and only then
/// is each position filled from its class's alphabet. The forced characters
/// land in uniformly random positions, which is the property
/// `a_forced_character_lands_in_every_position_not_just_the_first` asserts.
pub fn generate_password(recipe: &PasswordRecipe) -> Result<Zeroizing<String>, PasswordGenError> {
    let plan = normalize(recipe);
    let alphabets = Alphabets::for_plan(&plan);

    // The class tag per position, before shuffling. This is not the password,
    // but it is the password's *structure* -- "positions 3 and 11 are the
    // digits" is worth something to an attacker who obtained it -- so it is
    // wiped like the password is.
    let length = plan.length as usize;
    let mut positions: Zeroizing<Vec<Class>> = Zeroizing::new(Vec::with_capacity(length));
    for _ in 0..plan.min_lowercase {
        positions.push(Class::Lowercase);
    }
    for _ in 0..plan.min_uppercase {
        positions.push(Class::Uppercase);
    }
    for _ in 0..plan.min_number {
        positions.push(Class::Number);
    }
    for _ in 0..plan.min_special {
        positions.push(Class::Special);
    }
    // `normalize` guarantees the minimums fit, so this only ever extends.
    while positions.len() < length {
        positions.push(Class::Any);
    }

    shuffle(&mut positions)?;

    // **Exact capacity, on purpose.** Every character below is one ASCII
    // byte, so a `String` with `length` bytes of capacity cannot reallocate
    // while it is filled -- which is the only way a partially built password
    // could be left behind in a freed allocation that `Zeroizing` never sees.
    let mut out: Zeroizing<String> = Zeroizing::new(String::with_capacity(length));
    for class in positions.iter() {
        let set = alphabets.set_for(*class);
        // `set` is one of the four constants above or their concatenation,
        // all of which are ASCII, so indexing by byte is indexing by
        // character. `normalize` guarantees a non-empty set for every class
        // that can appear in `positions`.
        let index = uniform_below(set.len())?;
        match set.as_bytes().get(index) {
            Some(&byte) => out.push(byte as char),
            // Unreachable: `uniform_below(n)` returns strictly less than `n`.
            // Written as an error rather than an `unwrap` because this file
            // must not be able to panic on a path a caller can reach, and a
            // short password is a wrong answer rather than a survivable one.
            None => return Err(PasswordGenError::Rng),
        }
    }
    Ok(out)
}

// ---- the unbiased draw -------------------------------------------------------

/// A uniformly distributed `usize` in `0..bound`, from the OS CSPRNG.
///
/// # The bias this avoids, and why it is sampled a byte at a time
///
/// The classic defect in a password generator is `byte % alphabet.len()`.
/// One byte has 256 values; an alphabet of 70 divides 256 three times with 46
/// left over, so the first 46 letters of the alphabet come up **four** times
/// per 256 draws and the remaining 24 come up three -- a 33% excess on more
/// than half the alphabet, silently, in a function whose output looks
/// perfectly random to a human reading it.
///
/// The fix is **rejection sampling**: the top of the byte range that does not
/// divide evenly is thrown away rather than folded, so every accepted value
/// maps to exactly one residue and the residues are equally likely. `limit`
/// below is the largest multiple of `bound` that fits in a byte, and any draw
/// at or above it is discarded and redrawn.
///
/// # Why a byte and not a `u32`
///
/// A four-byte draw would make even the naive `%` biased by only about one
/// part in 2^26 -- undetectable by any test that could run here. That sounds
/// like an argument for the wider draw, and it is exactly backwards for this
/// crate: it would mean the *defect this module is required to exclude* could
/// be reintroduced by an editor with no test able to notice. Sampling a byte
/// keeps the failure mode large enough to be measured, and
/// `a_uniform_draw_is_uniform_and_a_naive_modulo_would_not_be` measures it.
/// Every alphabet in this file is far below 256, so a byte is the natural
/// width regardless.
///
/// # Termination
///
/// The worst acceptance rate over the bounds this module uses is a little
/// over one half, so the loop's expected cost is under two draws. It is
/// nevertheless **bounded**: a CSPRNG that answered with the same rejected
/// byte forever is a broken machine, and this returns
/// [`PasswordGenError::Rng`] rather than hanging the UI thread that asked for
/// a password.
fn uniform_below(bound: usize) -> Result<usize, PasswordGenError> {
    // Both are caller invariants rather than user input -- every call site
    // passes an alphabet length or a password length, and `normalize` bounds
    // the second. They are checked rather than asserted because this file
    // does not panic.
    if bound == 0 || bound > 256 {
        return Err(PasswordGenError::Rng);
    }
    let limit = 256 - (256 % bound);
    // Generous: at the worst acceptance rate this module can reach, the
    // chance of exhausting these draws legitimately is below one in 2^100.
    for _ in 0..128 {
        let mut byte = [0u8; 1];
        getrandom::getrandom(&mut byte).map_err(|_| PasswordGenError::Rng)?;
        let value = byte[0] as usize;
        if value < limit {
            return Ok(value % bound);
        }
    }
    Err(PasswordGenError::Rng)
}

/// Fisher-Yates, with [`uniform_below`] supplying the swap indices.
///
/// The standard loop, downward, choosing from `0..=i` -- and it is written
/// out rather than reached for from a crate because the off-by-one that turns
/// it into Sattolo's algorithm (choosing from `0..i` instead) silently
/// removes every fixed point from the permutation, which here would mean a
/// forced digit could never stay in the slot it started in.
fn shuffle(items: &mut [Class]) -> Result<(), PasswordGenError> {
    if items.len() < 2 {
        return Ok(());
    }
    for i in (1..items.len()).rev() {
        let j = uniform_below(i + 1)?;
        items.swap(i, j);
    }
    Ok(())
}

// ---- the plan ----------------------------------------------------------------

/// Which alphabet one position draws from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Lowercase,
    Uppercase,
    Number,
    Special,
    /// Every enabled class at once, which is what a position with no minimum
    /// to satisfy draws from.
    Any,
}

/// Hand-written so that the plan can live in a [`Zeroizing`].
///
/// The plan is not the password, but it is the password's shape -- "position
/// 3 is the digit" -- and this module wipes that for the same reason it wipes
/// the string. `zeroize` has a blanket impl for `Vec<Z: Zeroize>` but none for
/// a crate-local enum, so the one line it needs is written here rather than
/// dropping the wipe or encoding the enum as a byte and losing the exhaustive
/// match in [`Alphabets::set_for`].
///
/// [`Class::Any`] is the value overwritten with because it is the one that
/// carries no information: every position of a plan reduced to `Any` says
/// only "some enabled class", which is what an unfilled plan already says.
impl zeroize::Zeroize for Class {
    fn zeroize(&mut self) {
        *self = Self::Any;
    }
}

/// A [`PasswordRecipe`] with every ambiguity resolved: the classes that are
/// really on, the minimums that are really required, and a length that is
/// really achievable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Plan {
    length: u32,
    lowercase: bool,
    uppercase: bool,
    number: bool,
    special: bool,
    min_lowercase: u32,
    min_uppercase: u32,
    min_number: u32,
    min_special: u32,
    avoid_ambiguous: bool,
}

/// `bw`'s option normalisation, step for step -- and the two places this is
/// deliberately not `bw`.
///
/// # What is mirrored
///
/// * **All four classes off becomes lowercase, uppercase and number.** This
///   is `bw`'s silent substitution, which [`GenerateRequest`]'s own doc
///   already warns callers about in as many words. It is mirrored rather than
///   refused because refusing would make this generator answer differently
///   from the one the user had yesterday for a recipe the UI can produce.
/// * **An enabled class has a minimum of at least one.** `bw` raises a zero
///   minimum to one for every class that is on, so a four-class password
///   always contains all four kinds. A disabled class's minimum is forced to
///   zero, so a `min_special` on a recipe with `special: false` cannot demand
///   a character from an alphabet that is not in play.
/// * **The length floor is five**, which [`PasswordRecipe::length`] records,
///   **and it is raised again if the minimums do not fit.** Asking for four
///   digits in a three-character password is answered by lengthening the
///   password, not by dropping a requirement -- again `bw`'s behaviour, and
///   the one that never returns a weaker password than was asked for.
///
/// # The two places this is stricter, both of them bounds
///
/// Both exist because this is Rust reading a struct that a UI or a future
/// caller fills in, not JavaScript reading a query string a human typed:
///
/// 1. **`min_number` and `min_special` are capped at [`MAX_MINIMUM`]**, nine,
///    which is the maximum Bitwarden's own clients offer for these options.
/// 2. **The final length is capped at [`MAX_LENGTH`]**, 128, which is
///    Bitwarden's maximum generated length.
///
/// Without them a recipe asking for four billion special characters is a
/// four-billion-character allocation inside a UI callback. The cap on the
/// minimums is applied *before* the length is raised to fit them, so the two
/// cannot fight: four capped minimums sum to at most 9 + 9 + 1 + 1 = 20,
/// comfortably inside 128, and the length floor can therefore always be
/// satisfied. Neither cap can make a password *shorter* than `bw` would while
/// the recipe stays inside the range a Bitwarden client can express.
fn normalize(recipe: &PasswordRecipe) -> Plan {
    let (lowercase, uppercase, number, special) =
        if recipe.lowercase || recipe.uppercase || recipe.number || recipe.special {
            (recipe.lowercase, recipe.uppercase, recipe.number, recipe.special)
        } else {
            // `bw`'s substitution when a request turns everything off.
            (true, true, true, false)
        };

    let min_lowercase = u32::from(lowercase);
    let min_uppercase = u32::from(uppercase);
    let min_number =
        if number { recipe.min_number.clamp(1, MAX_MINIMUM) } else { 0 };
    let min_special =
        if special { recipe.min_special.clamp(1, MAX_MINIMUM) } else { 0 };

    let required = min_lowercase + min_uppercase + min_number + min_special;
    let length = recipe.length.max(MIN_LENGTH).max(required).min(MAX_LENGTH);

    Plan {
        length,
        lowercase,
        uppercase,
        number,
        special,
        min_lowercase,
        min_uppercase,
        min_number,
        min_special,
        avoid_ambiguous: recipe.avoid_ambiguous,
    }
}

/// The four alphabets a [`Plan`] actually draws from, plus their union.
///
/// Built once per password rather than per character: the union is a `String`
/// and building it inside the loop would be one allocation per character, all
/// of them holding the alphabet rather than the password -- harmless but
/// pointless.
struct Alphabets {
    lowercase: String,
    uppercase: String,
    number: String,
    special: String,
    any: String,
}

impl Alphabets {
    fn for_plan(plan: &Plan) -> Self {
        let widen = |narrow: &str, extra: &str| {
            let mut set = String::with_capacity(narrow.len() + extra.len());
            set.push_str(narrow);
            if !plan.avoid_ambiguous {
                set.push_str(extra);
            }
            set
        };
        let lowercase = widen(LOWERCASE, LOWERCASE_AMBIGUOUS);
        let uppercase = widen(UPPERCASE, UPPERCASE_AMBIGUOUS);
        let number = widen(NUMBER, NUMBER_AMBIGUOUS);
        let special = SPECIAL.to_string();

        let mut any = String::new();
        if plan.lowercase {
            any.push_str(&lowercase);
        }
        if plan.uppercase {
            any.push_str(&uppercase);
        }
        if plan.number {
            any.push_str(&number);
        }
        if plan.special {
            any.push_str(&special);
        }
        Self { lowercase, uppercase, number, special, any }
    }

    fn set_for(&self, class: Class) -> &str {
        match class {
            Class::Lowercase => &self.lowercase,
            Class::Uppercase => &self.uppercase,
            Class::Number => &self.number,
            Class::Special => &self.special,
            Class::Any => &self.any,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault_bridge::PassphraseRecipe;
    use std::collections::HashMap;

    /// A recipe with exactly one class on, so a test can assert the alphabet
    /// a password was drawn from without the other three in the way.
    fn only(lowercase: bool, uppercase: bool, number: bool, special: bool) -> PasswordRecipe {
        PasswordRecipe {
            length: 40,
            lowercase,
            uppercase,
            number,
            special,
            min_number: 0,
            min_special: 0,
            avoid_ambiguous: false,
        }
    }

    // ---- the bias tests, which are the point of this file ------------------

    /// **The test the whole sampler exists for.**
    ///
    /// [`uniform_below`] is drawn 210,000 times over a bound of 70 -- the
    /// size of this module's widest alphabet, all four classes with ambiguous
    /// characters allowed -- and every residue must come up within a
    /// tolerance of its expected 3,000.
    ///
    /// # Why the tolerance is where it is
    ///
    /// Each bin is a binomial with n = 210,000 and p = 1/70, so the expected
    /// count is 3,000 and one standard deviation is
    /// `sqrt(210000 * (1/70) * (69/70))`, about 54.4. The tolerance below is
    /// 340, a little over six standard deviations, which puts the chance of
    /// this test failing on correct code at roughly 2e-9 per bin and about
    /// 1.4e-7 over all seventy -- small enough that a failure here is
    /// evidence of a bug rather than of an unlucky afternoon.
    ///
    /// # And why it bites
    ///
    /// This is the half that matters, because a distribution test that cannot
    /// fail is decoration. Replace the body of [`uniform_below`] with the
    /// naive `byte % bound` and the arithmetic is forced: 256 = 3*70 + 46, so
    /// residues 0..=45 are reachable from four bytes each and residues
    /// 46..=69 from only three. The naive sampler's expected counts are
    /// therefore 210000 * 4/256 = 3,281 for the first 46 bins and
    /// 210000 * 3/256 = 2,461 for the last 24 -- deviations of +281 and
    /// **-539** against a tolerance of 340. The low bins miss by more than
    /// half again the tolerance, roughly ten standard deviations out, so the
    /// naive version fails this test essentially every time it is run rather
    /// than occasionally.
    ///
    /// **That was measured, not reasoned about.** `limit` in [`uniform_below`]
    /// was set to a flat 256 -- which is exactly `byte % bound` with the
    /// rejection removed -- and this test was run five times: it failed all
    /// five, reporting deviations of 344 to 390 on the *high* bins alone,
    /// before ever reaching the low bins where the miss is larger still.
    /// `the_naive_modulo_this_test_excludes_is_measurably_skewed` then pins
    /// the same arithmetic without needing the mutation, so a later editor
    /// widening the tolerance is told rather than being trusted to re-derive
    /// this paragraph by hand.
    #[test]
    fn a_uniform_draw_is_uniform_and_a_naive_modulo_would_not_be() {
        const BOUND: usize = 70;
        const PER_BIN: usize = 3_000;
        const DRAWS: usize = BOUND * PER_BIN;
        const TOLERANCE: i64 = 340;

        let mut counts = vec![0i64; BOUND];
        for _ in 0..DRAWS {
            let value = uniform_below(BOUND).expect("the OS CSPRNG");
            assert!(value < BOUND, "a draw of {value} escaped its bound of {BOUND}");
            counts[value] += 1;
        }

        let expected = PER_BIN as i64;
        for (value, count) in counts.iter().enumerate() {
            let deviation = (count - expected).abs();
            assert!(
                deviation <= TOLERANCE,
                "residue {value} came up {count} times against an expected {expected} (off by \
                 {deviation}, tolerance {TOLERANCE}); the draw is not uniform"
            );
        }
    }

    /// The arithmetic behind the previous test's claim that it bites, checked
    /// rather than asserted in prose.
    ///
    /// This does not run a naive sampler -- it counts, exactly, how many of
    /// the 256 byte values fold onto each residue under `byte % 70`, and
    /// requires that the resulting skew is larger than the tolerance the
    /// distribution test allows. If someone widens that tolerance far enough
    /// to let a biased sampler through, this test says so.
    #[test]
    fn the_naive_modulo_this_test_excludes_is_measurably_skewed() {
        const BOUND: usize = 70;
        const PER_BIN: usize = 3_000;
        const DRAWS: i64 = (BOUND * PER_BIN) as i64;
        const TOLERANCE: i64 = 340;

        let mut folded = vec![0i64; BOUND];
        for byte in 0..=255u16 {
            folded[(byte as usize) % BOUND] += 1;
        }

        let expected = PER_BIN as i64;
        let worst = folded
            .iter()
            .map(|ways| ((DRAWS * ways / 256) - expected).abs())
            .max()
            .expect("a non-empty alphabet");
        assert!(
            worst > TOLERANCE,
            "a naive `byte % {BOUND}` would deviate by only {worst}, which the distribution \
             test's tolerance of {TOLERANCE} would accept -- that test no longer excludes the \
             defect it exists to exclude"
        );
    }

    /// The other half of "no bias": a forced character must be able to land
    /// anywhere, not merely somewhere.
    ///
    /// A generator that satisfies `min_special` by appending or prepending is
    /// a real weakness and a common bug, and it passes every test that only
    /// counts characters. So this one counts *positions*: a recipe with
    /// exactly one required special character in a twelve-character password
    /// is generated many times, and every one of the twelve positions must
    /// have held that character a plausible number of times.
    ///
    /// # Why this compares positions to each other rather than to a number
    ///
    /// The first version of this test asserted a fixed count per position and
    /// was wrong, in a way worth recording: a special character can reach a
    /// position two ways, either as the *forced* one or by an unconstrained
    /// position drawing one out of the combined alphabet. The expected rate
    /// is therefore not `1/length` but a mixture, and hard-coding it means
    /// re-deriving the alphabet sizes inside the test.
    ///
    /// What actually matters here is that the rate is the **same for every
    /// position**, whatever it is -- that is precisely the property an
    /// appending implementation violates. So the observed mean is the
    /// baseline and each position must sit within 15% of it. Under a correct
    /// implementation one position's count has a standard deviation near 50
    /// against a mean near 3,400, so 15% is about ten standard deviations and
    /// this will not flake; an implementation that appends the forced
    /// character puts every one of 12,000 into the last bin and misses by
    /// more than 200%.
    #[test]
    fn a_forced_character_lands_in_every_position_not_just_the_first() {
        const LENGTH: usize = 12;
        const ROUNDS: usize = 12_000;
        let recipe = PasswordRecipe {
            length: LENGTH as u32,
            lowercase: true,
            uppercase: false,
            number: false,
            special: true,
            min_number: 0,
            min_special: 1,
            avoid_ambiguous: true,
        };

        let mut positions = [0usize; LENGTH];
        for _ in 0..ROUNDS {
            let password = generate_password(&recipe).expect("the OS CSPRNG");
            assert_eq!(password.len(), LENGTH);
            let specials: Vec<usize> = password
                .char_indices()
                .filter(|(_, c)| SPECIAL.contains(*c))
                .map(|(i, _)| i)
                .collect();
            assert!(!specials.is_empty(), "a required special character was missing");
            for index in specials {
                positions[index] += 1;
            }
        }

        let total: usize = positions.iter().sum();
        let mean = total as f64 / LENGTH as f64;
        for (index, count) in positions.iter().enumerate() {
            let deviation = (*count as f64 - mean).abs() / mean;
            assert!(
                deviation <= 0.15,
                "position {index} held a special character {count} times against a mean of \
                 {mean:.0} across all {LENGTH} positions ({:.1}% off); the forced characters are \
                 not being placed at random",
                deviation * 100.0
            );
        }
    }

    // ---- the recipe's documented semantics ---------------------------------

    /// The default recipe, which is what a user gets for clicking Generate,
    /// produces what it says: twenty characters with all four classes
    /// present.
    #[test]
    fn the_default_recipe_produces_twenty_characters_of_all_four_classes() {
        for _ in 0..200 {
            let password = generate_password(&PasswordRecipe::default()).expect("the CSPRNG");
            assert_eq!(password.len(), 20, "{}", password.len());
            assert!(password.chars().any(|c| c.is_ascii_lowercase()), "no lowercase");
            assert!(password.chars().any(|c| c.is_ascii_uppercase()), "no uppercase");
            assert!(password.chars().any(|c| c.is_ascii_digit()), "no digit");
            assert!(password.chars().any(|c| SPECIAL.contains(c)), "no special");
        }
    }

    /// `avoid_ambiguous` is honoured, and it is honoured the way its doc says
    /// -- the field is the opposite way round from the wire key, so a test
    /// that got this backwards would be asserting the bug.
    ///
    /// `true` means *avoid*: none of `l`, `I`, `O`, `0`, `1` may appear.
    #[test]
    fn avoiding_ambiguous_characters_removes_exactly_the_five_bitwarden_removes() {
        let recipe = PasswordRecipe { avoid_ambiguous: true, ..PasswordRecipe::default() };
        for _ in 0..400 {
            let password = generate_password(&recipe).expect("the CSPRNG");
            for c in password.chars() {
                assert!(
                    !"lIO01".contains(c),
                    "the ambiguous character `{c}` appeared in a password that asked to avoid them"
                );
            }
        }
    }

    /// And the inverse: with ambiguous characters allowed they must actually
    /// be reachable, or "avoid" would be a setting that changed nothing.
    ///
    /// Five specific characters over a large sample: each has probability
    /// about 1/70 per character, so seeing all five somewhere in 400 passwords
    /// of 40 characters is a certainty for any correct implementation.
    #[test]
    fn allowing_ambiguous_characters_actually_reaches_them() {
        let recipe = PasswordRecipe { avoid_ambiguous: false, ..only(true, true, true, false) };
        let mut seen: HashMap<char, usize> = HashMap::new();
        for _ in 0..400 {
            let password = generate_password(&recipe).expect("the CSPRNG");
            for c in password.chars() {
                if "lIO01".contains(c) {
                    *seen.entry(c).or_default() += 1;
                }
            }
        }
        for c in "lIO01".chars() {
            assert!(
                seen.contains_key(&c),
                "`{c}` never appeared with ambiguous characters allowed; the sets are wrong"
            );
        }
    }

    /// A disabled class contributes nothing, for each of the four in turn.
    ///
    /// One test over four single-class recipes rather than four tests, so a
    /// fifth class added later has an obvious place to be listed.
    #[test]
    fn a_class_that_is_off_never_appears() {
        /// A case: what it is called, the recipe, and the predicate that
        /// says a character had no business being there.
        type Case = (&'static str, PasswordRecipe, fn(char) -> bool);
        let cases: [Case; 4] = [
            ("lowercase only", only(true, false, false, false), |c| !c.is_ascii_lowercase()),
            ("uppercase only", only(false, true, false, false), |c| !c.is_ascii_uppercase()),
            ("number only", only(false, false, true, false), |c| !c.is_ascii_digit()),
            ("special only", only(false, false, false, true), |c| !SPECIAL.contains(c)),
        ];
        for (name, recipe, is_wrong) in cases {
            for _ in 0..100 {
                let password = generate_password(&recipe).expect("the CSPRNG");
                assert_eq!(password.len(), 40);
                for c in password.chars() {
                    assert!(!is_wrong(c), "a `{name}` password contained `{c}`");
                }
            }
        }
    }

    /// The length floor `bw` applies: anything under five comes back as five,
    /// including zero.
    #[test]
    fn a_length_below_five_is_raised_to_five_rather_than_honoured() {
        for length in 0..=5u32 {
            let recipe = PasswordRecipe { length, ..only(true, false, false, false) };
            let password = generate_password(&recipe).expect("the CSPRNG");
            assert_eq!(password.len(), 5, "a length of {length} produced {}", password.len());
        }
    }

    /// Minimums that do not fit lengthen the password rather than being
    /// dropped. The failure this excludes is a five-character password that
    /// silently contains fewer digits than were required.
    #[test]
    fn minimums_that_exceed_the_length_lengthen_the_password_rather_than_being_dropped() {
        let recipe = PasswordRecipe {
            length: 5,
            lowercase: true,
            uppercase: true,
            number: true,
            special: true,
            min_number: 6,
            min_special: 5,
            avoid_ambiguous: true,
        };
        let password = generate_password(&recipe).expect("the CSPRNG");
        // 1 lowercase + 1 uppercase + 6 numbers + 5 specials.
        assert_eq!(password.len(), 13, "{}", password.len());
        assert_eq!(password.chars().filter(char::is_ascii_digit).count(), 6);
        assert!(password.chars().filter(|c| SPECIAL.contains(*c)).count() >= 5);
    }

    /// A minimum for a class that is switched off is discarded, not honoured
    /// from an alphabet that is not in play.
    #[test]
    fn a_minimum_for_a_disabled_class_is_discarded() {
        let recipe = PasswordRecipe {
            min_special: 4,
            min_number: 4,
            ..only(true, false, false, false)
        };
        let password = generate_password(&recipe).expect("the CSPRNG");
        assert_eq!(password.len(), 40);
        assert!(password.chars().all(|c| c.is_ascii_lowercase()), "{}", password.len());
    }

    /// The absurd recipe is bounded rather than allocated. This is the
    /// stricter-than-`bw` behaviour [`normalize`] documents, asserted so that
    /// it is a decision rather than an accident.
    #[test]
    fn an_absurd_recipe_is_bounded_instead_of_allocating_forever() {
        let recipe = PasswordRecipe {
            length: u32::MAX,
            min_number: u32::MAX,
            min_special: u32::MAX,
            ..PasswordRecipe::default()
        };
        let password = generate_password(&recipe).expect("the CSPRNG");
        assert_eq!(password.len(), MAX_LENGTH as usize);
        // At *least* the capped minimum: the unconstrained positions draw
        // from every enabled class and contribute digits of their own, so an
        // equality here would be asserting that they do not.
        assert!(
            password.chars().filter(char::is_ascii_digit).count() >= MAX_MINIMUM as usize,
            "the capped minimum was not honoured"
        );
    }

    /// `bw`'s silent substitution when a recipe turns every class off, which
    /// [`GenerateRequest`]'s doc warns about. Mirrored deliberately: this
    /// generator answering differently from the one the user had yesterday
    /// would be the surprise.
    #[test]
    fn a_recipe_with_no_classes_at_all_becomes_the_three_class_password_bw_substitutes() {
        let recipe = only(false, false, false, false);
        let mut saw_lower = false;
        let mut saw_upper = false;
        let mut saw_digit = false;
        for _ in 0..50 {
            let password = generate_password(&recipe).expect("the CSPRNG");
            assert_eq!(password.len(), 40);
            for c in password.chars() {
                assert!(!SPECIAL.contains(c), "the substitution added a special character");
                saw_lower |= c.is_ascii_lowercase();
                saw_upper |= c.is_ascii_uppercase();
                saw_digit |= c.is_ascii_digit();
            }
        }
        assert!(saw_lower && saw_upper && saw_digit, "the substituted classes are not all present");
    }

    /// Two calls do not agree. A generator seeded from a clock, or one that
    /// cached a buffer, would fail this -- and it is cheap insurance against
    /// the worst possible regression in this file.
    #[test]
    fn two_passwords_in_a_row_are_not_the_same() {
        let recipe = PasswordRecipe::default();
        let first = generate_password(&recipe).expect("the CSPRNG");
        let second = generate_password(&recipe).expect("the CSPRNG");
        assert_ne!(*first, *second, "two generated passwords were identical");
    }

    // ---- the refusal --------------------------------------------------------

    /// A passphrase is refused by name, and the refusal is the wordlist one.
    ///
    /// This must stay a refusal until a real wordlist is a decision the owner
    /// has taken. See [`PasswordGenError::NoWordlist`].
    #[test]
    fn a_passphrase_is_refused_because_there_is_no_wordlist() {
        let err = generate(&GenerateRequest::Passphrase(PassphraseRecipe::default()))
            .expect_err("no wordlist");
        assert_eq!(err, PasswordGenError::NoWordlist);
        assert!(
            err.to_string().contains("wordlist"),
            "the refusal does not say what is missing: {err}"
        );
    }

    /// The dispatch does route a password request to the generator, so the
    /// function a backend calls is the function that was tested above.
    #[test]
    fn the_dispatch_generates_a_password_rather_than_refusing_it() {
        let password = generate(&GenerateRequest::Password(PasswordRecipe::default()))
            .expect("a password");
        assert_eq!(password.len(), 20);
    }

    /// This module carries no wordlist, asserted against the source text so
    /// that "do not invent one" survives the next edit.
    ///
    /// A wordlist is a large array of words; the shape it would arrive in is
    /// a long list of quoted strings. This counts them, and a file that grew
    /// hundreds is a file that grew a wordlist.
    #[test]
    fn no_wordlist_has_been_smuggled_into_this_module() {
        let source = include_str!("password_gen.rs");
        let quoted = source.matches('"').count();
        assert!(
            quoted < 400,
            "this file now holds {quoted} quote characters, which is enough to be an improvised \
             wordlist; see `PasswordGenError::NoWordlist` for why one must not be invented here"
        );
    }

    /// No error may carry a generated secret, the way
    /// `rest::api`'s `no_error_can_carry_a_credential` says of that module.
    /// Both variants are fieldless, so this is a check that they stay so.
    #[test]
    fn no_error_can_carry_a_generated_secret() {
        // A fieldless variant's `Debug` is exactly its name, so anything else
        // appearing here means a variant grew somewhere to put a secret.
        assert_eq!(format!("{:?}", PasswordGenError::Rng), "Rng");
        assert_eq!(format!("{:?}", PasswordGenError::NoWordlist), "NoWordlist");
        // And the compiler's half: if a variant ever gains a field, this stops
        // compiling as written and whoever added it has to come back here.
        let _exhaustive: fn(PasswordGenError) -> u8 = |e| match e {
            PasswordGenError::Rng => 0,
            PasswordGenError::NoWordlist => 1,
        };
    }
}
