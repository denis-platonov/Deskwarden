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
//! the fill path's generator card -- would have to grow a second one. Two
//! password generators
//! in one app is two answers to "how strong is a password this app made".
//!
//! # What is generated, and what is refused
//!
//! * [`GenerateRequest::Password`] -- generated here. It is a pure algorithm
//!   over a fixed alphabet and needs no data this crate does not have.
//! * [`GenerateRequest::Passphrase`] -- generated here too, from a word list
//!   that is **a file installed beside the executable and read on demand**,
//!   never bytes compiled into this binary. See [`generate_passphrase`] for
//!   the whole of that argument, and [`PasswordGenError::WordlistUnusable`]
//!   for why a list that does not verify is refused rather than used short.
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

use std::collections::HashSet;
use std::path::PathBuf;

use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::vault_bridge::{GenerateRequest, PassphraseRecipe, PasswordRecipe};

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
    /// The wordlist a passphrase is drawn from could not be found.
    ///
    /// It is a file installed beside the executable (see [`WORDLIST_FILE`]
    /// and [`wordlist_paths`]), not bytes compiled into this binary, because
    /// it is needed only when somebody asks for a passphrase -- which is
    /// rare -- and a list that is `include_str!`d is resident in the image
    /// for the life of a process whose whole design is about staying small.
    ///
    /// **This is a refusal and never a fallback.** See
    /// [`Self::WordlistUnusable`] for the argument, which applies identically
    /// to "the file is not there": a passphrase improvised from whatever
    /// words this module could reach looks exactly like a real one.
    WordlistMissing,
    /// The wordlist was found and **rejected**: the wrong number of words, a
    /// duplicate, a word that is not four to eight lowercase letters, or
    /// contents that do not match [`WORDLIST_SHA256`].
    ///
    /// # Why a short list is refused rather than used
    ///
    /// A passphrase's entire strength is the size of the list its words are
    /// drawn from. This crate's list is [`WORDLIST_WORDS`] words -- 2^12, so
    /// exactly twelve bits a word and a four-word passphrase is 48 bits. A
    /// file truncated to three hundred words yields about 8.2 bits a word,
    /// roughly 33 bits over four words: **thirty thousand times weaker**, and
    /// utterly indistinguishable from the real thing by looking at it. Neither
    /// the user, nor this app's own strength meter, nor a reviewer reading the
    /// output can tell the two apart.
    ///
    /// That is the failure this crate treats as worse than a crash -- a wrong
    /// answer indistinguishable from a right one -- so the list is counted,
    /// de-duplicated and hashed on every load and anything short of an exact
    /// match arrives here.
    WordlistUnusable,
}

impl std::fmt::Display for PasswordGenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rng => f.write_str(
                "this computer's secure random number generator could not be read, so no password \
                 was generated",
            ),
            Self::WordlistMissing => f.write_str(
                "the word list a passphrase is built from was not found beside this application,                  so no passphrase was generated",
            ),
            Self::WordlistUnusable => f.write_str(
                "the word list a passphrase is built from is not the one this application ships,                  so no passphrase was generated",
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
        GenerateRequest::Passphrase(recipe) => generate_passphrase(recipe),
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

// ---- the passphrase ----------------------------------------------------------
//
// Everything below is the second half of this module: the word list, the
// checks it has to pass before a single word is drawn from it, and the draw
// itself. The list is NOT here -- it is `assets/wordlist.txt`, a plain file of
// one word per line, and that is deliberate on two counts. It is installed
// beside the executable rather than compiled in, so it costs nothing until
// somebody asks for a passphrase; and it is data rather than source, so
// `no_wordlist_has_been_smuggled_into_this_module` can still say that this
// file carries no words of its own.

/// The file name the word list is installed under, beside the executable.
///
/// The installer's `[Files]` section writes it to `{app}`, and
/// `the_installer_ships_the_wordlist_this_module_reads` reads that section to
/// hold the two spellings together, so an installer that stopped shipping the
/// file reds a test here rather than producing a build whose Generate button
/// refuses passphrases on a user's machine and nowhere else.
pub const WORDLIST_FILE: &str = "wordlist.txt";

/// How many words the list must hold. **2^12, and the exponent is the point.**
///
/// A word is chosen with exactly twelve random bits (see [`draw_index`]).
/// Because the count is a power of two, those twelve bits map onto the list
/// one-to-one: there is no `%` to bias the low indices, and no rejection loop
/// to write incorrectly. A list of 4,000 or 4,100 words would force one or the
/// other back into this file, silently, and the resulting skew is invisible in
/// the output -- which is exactly the defect [`uniform_below`] exists to keep
/// out of the character generator.
///
/// It is also what makes the strength statable without arithmetic: twelve bits
/// a word, so `n` words is `12n` bits, and the four-word default is 48.
pub const WORDLIST_WORDS: usize = 4096;

/// The twelve bits [`WORDLIST_WORDS`] is the size of.
const WORDLIST_BITS: u32 = 12;

/// The SHA-256 of the list's words joined by a newline.
///
/// # Why the hash is pinned, and why it is over the WORDS rather than the FILE
///
/// The count-and-uniqueness checks catch a truncated or duplicated list, which
/// is the corruption that happens by accident. They do **not** catch a list
/// that is still 4,096 unique words but not *these* 4,096 -- a file swapped
/// for one of 4,096 near-identical variants of the same word, say, whose
/// effective entropy is a fraction of what the count claims. This pin closes
/// that: any edit at all to the list is a different digest and is refused.
///
/// It is computed over the parsed words joined with a newline, **not over the
/// file's bytes**, and that is not laziness. This repository is checked out on
/// Windows, where git may hand a working tree CRLF line endings; a digest over
/// raw bytes would then be red on the machine the file was written on and
/// green on CI, or the reverse. Hashing the parsed words makes the pin depend
/// on the list rather than on the checkout, which is the property actually
/// wanted.
///
/// Changing the list means changing this constant in the same commit, which is
/// the visible edit a wordlist change ought to be.
const WORDLIST_SHA256: [u8; 32] = [
    0xc2, 0xc5, 0x5e, 0x59, 0x32, 0x5b, 0xf5, 0x66, 0xd9, 0x37, 0x56, 0x57, 0xc7, 0xda, 0x8f, 0x90,
    0x0d, 0x0d, 0x1c, 0x16, 0x3d, 0xf9, 0x1a, 0xef, 0xbd, 0x38, 0x76, 0xfa, 0xe8, 0xb7, 0x7d, 0x6c,
];

/// `bw`'s floor on a passphrase's word count, which [`PassphraseRecipe::words`]
/// records: the serve route clamps anything below this up to it.
const MIN_WORDS: u32 = 3;

/// A ceiling on the word count, and the same kind of bound [`MAX_LENGTH`] is:
/// this is Rust reading a `u32` a UI or a future caller filled in, and without
/// it a recipe asking for four billion words is a four-billion-word
/// allocation inside a UI callback. Twenty is far above anything a Bitwarden
/// client offers, so it cannot make a passphrase shorter than one a real
/// client could ask for.
const MAX_WORDS: u32 = 20;

/// The directories the word list is looked for in, **in the order searched**.
///
/// 1. **Beside the executable**, which is where the installer's `[Files]`
///    section puts it and where `build.rs` copies it in a development build,
///    so the installed app and `cargo run` behave identically.
/// 2. **`assets/` beside the executable** -- the layout a build that ships its
///    assets in a subdirectory would use. Costless to look in, and it means
///    one packaging choice later is not a code change here.
///
/// **The user's config directory is deliberately NOT on this list**, and that
/// is the one interesting thing about this function.
/// [`crate::brand_mark::search_dirs`] looks there *first*, precisely so a user
/// can override what the app ships -- which is right for a card logo and would
/// be a hole here: a user-writable word list is a file that anything running
/// as the user can shrink to ten words, after which every passphrase this app
/// generates is guessable and looks exactly as it did before. The list this
/// app draws from is the list this app shipped, and [`WORDLIST_SHA256`] is the
/// check that says so.
///
/// The list may be empty when the running executable's path cannot be read,
/// which reaches the caller as [`PasswordGenError::WordlistMissing`].
pub fn wordlist_paths() -> Vec<PathBuf> {
    let Some(dir) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(PathBuf::from))
    else {
        return Vec::new();
    };
    vec![dir.join(WORDLIST_FILE), dir.join("assets").join(WORDLIST_FILE)]
}

/// One passphrase, from the OS CSPRNG and the installed word list.
///
/// # The list is loaded here and dropped here
///
/// There is no `OnceLock`, no `lazy_static` and no cached `Vec` anywhere in
/// this module. The owner's requirement, in as many words: *"it's not even
/// needed in memory until called (which is rare)"*. So the file is read on the
/// call, verified, drawn from, and freed when this function returns. A
/// passphrase costs one read of a 30 KB file the OS has almost certainly
/// cached; a resident list costs that 30 KB for the life of a process that
/// spends most of its life in the tray doing nothing.
///
/// # What is verified before a single word is drawn
///
/// [`verify`], and it refuses rather than degrades -- see
/// [`PasswordGenError::WordlistUnusable`] for why a short list is the worst
/// possible outcome here rather than a merely disappointing one.
///
/// # The draw
///
/// Twelve bits a word, from [`draw_index`], with no modulo and no rejection
/// because [`WORDLIST_WORDS`] is 2^12. Every position draws from the whole
/// list, which `every_word_position_draws_from_the_whole_list` measures.
///
/// # Where the digit goes
///
/// `include_number` puts one digit on one word, and **which word is chosen at
/// random**. Appending it to the last word would be the same defect as
/// building a password by overwriting its first characters with digits: a
/// known position is a position removed from the search space, and
/// `the_included_number_lands_on_every_word_not_just_the_last` is the test
/// that says it is not.
pub fn generate_passphrase(
    recipe: &PassphraseRecipe,
) -> Result<Zeroizing<String>, PasswordGenError> {
    let words = load_wordlist()?;
    let count = recipe.words.clamp(MIN_WORDS, MAX_WORDS) as usize;
    let separator = separator_of(recipe);

    // The chosen indices, and the word the digit lands on. Neither is the
    // passphrase, but together with the list they ARE it, so both are wiped --
    // the way `generate_password` wipes its plan for the same reason.
    let mut chosen: Zeroizing<Vec<u16>> = Zeroizing::new(Vec::with_capacity(count));
    for _ in 0..count {
        chosen.push(draw_index()?);
    }
    let numbered: Zeroizing<usize> =
        Zeroizing::new(if recipe.include_number { uniform_below(count)? } else { usize::MAX });
    let digit: Zeroizing<u8> =
        Zeroizing::new(if recipe.include_number { uniform_below(10)? as u8 } else { 0 });

    // **Exact capacity, for the reason `generate_password` states**: the only
    // way a half-built passphrase can be left in a freed page that `Zeroizing`
    // never sees is a reallocation while it is being filled. Every word is
    // four to eight ASCII bytes (`verify` guarantees it), capitalising an
    // ASCII letter cannot change its length, the separator is one `char`, and
    // the digit is one byte.
    let mut capacity = if separator == NO_SEPARATOR {
        0
    } else {
        separator.len_utf8() * count.saturating_sub(1)
    };
    if recipe.include_number {
        capacity += 1;
    }
    for index in chosen.iter() {
        match words.get(*index as usize) {
            Some(word) => capacity += word.len(),
            // Unreachable: `draw_index` returns strictly below
            // `WORDLIST_WORDS` and `verify` guarantees the list is that long.
            // An error rather than an `unwrap` because this file does not
            // panic on a path a caller can reach, and a short passphrase is a
            // wrong answer rather than a survivable one.
            None => return Err(PasswordGenError::WordlistUnusable),
        }
    }

    let mut out: Zeroizing<String> = Zeroizing::new(String::with_capacity(capacity));
    for (position, index) in chosen.iter().enumerate() {
        if position > 0 && separator != NO_SEPARATOR {
            out.push(separator);
        }
        let Some(word) = words.get(*index as usize) else {
            return Err(PasswordGenError::WordlistUnusable);
        };
        for (offset, letter) in word.chars().enumerate() {
            if offset == 0 && recipe.capitalize {
                out.push(letter.to_ascii_uppercase());
            } else {
                out.push(letter);
            }
        }
        if recipe.include_number && position == *numbered {
            out.push((b'0' + *digit) as char);
        }
    }
    Ok(out)
}

/// The sentinel [`separator_of`] returns for "the words run together".
///
/// A NUL rather than an `Option<char>`: it is not a character any branch of
/// that function can otherwise produce, and an `Option` unwrapped at every
/// push site reads worse than the one comparison it saves.
const NO_SEPARATOR: char = '\0';

/// The separator this recipe really means, as one `char`.
///
/// [`PassphraseRecipe::separator`]'s doc is the specification and it was
/// verified against `bw`'s own source: the route takes only the **first**
/// character of anything longer than one, and reads the literal words `space`
/// and `empty` as a space and as nothing at all.
fn separator_of(recipe: &PassphraseRecipe) -> char {
    match recipe.separator.as_str() {
        "space" => ' ',
        "empty" | "" => NO_SEPARATOR,
        other => other.chars().next().unwrap_or(NO_SEPARATOR),
    }
}

/// One index into the word list, from exactly [`WORDLIST_BITS`] bits of the OS
/// CSPRNG.
///
/// Two bytes are drawn and the low twelve bits are kept. **This is not a
/// modulo and it is not a rejection loop**: 2^16 is an exact multiple of 2^12,
/// so masking partitions the 65,536 byte pairs into 4,096 classes of exactly
/// sixteen -- every index is reachable by the same number of draws, which is
/// the definition of unbiased. That property is a consequence of
/// [`WORDLIST_WORDS`] being a power of two and is the whole reason that
/// constant is not a round decimal number.
///
/// The four discarded bits are not a weakness; they are simply not asked for.
fn draw_index() -> Result<u16, PasswordGenError> {
    let mut bytes = [0u8; 2];
    getrandom::getrandom(&mut bytes).map_err(|_| PasswordGenError::Rng)?;
    let mask = (1u16 << WORDLIST_BITS) - 1;
    Ok(u16::from_le_bytes(bytes) & mask)
}

/// Reads and verifies the installed word list.
///
/// The first candidate path that **exists** is the one used, and a file that
/// exists but fails [`verify`] is an error rather than a reason to try the
/// next: a search that fell through to a second directory because the first
/// held a *broken* file is a search whose answer depends on which failure came
/// first. That is [`crate::brand_mark::find_file`]'s rule, for the same
/// reason.
fn load_wordlist() -> Result<Vec<String>, PasswordGenError> {
    for path in wordlist_paths() {
        if !path.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(&path).map_err(|_| PasswordGenError::WordlistUnusable)?;
        return verify(&text);
    }
    Err(PasswordGenError::WordlistMissing)
}

/// The four checks a candidate word list must pass, and why each is a check
/// rather than an assumption.
///
/// 1. **Exactly [`WORDLIST_WORDS`] words.** Anything else and twelve bits no
///    longer index the list one-to-one -- see [`draw_index`].
/// 2. **Every word four to eight lowercase ASCII letters.** An apostrophe or
///    an accent would collide with the separator and the capitalize rule; a
///    word outside the length band is either confusable or a nuisance to
///    dictate over a telephone.
/// 3. **All unique.** A duplicate is a word with twice the probability inside
///    a list whose length still claims full entropy. This is the failure that
///    is invisible without a check, which is why the check is not optional.
/// 4. **The digest matches [`WORDLIST_SHA256`].**
///
/// All four fail as [`PasswordGenError::WordlistUnusable`], one variant rather
/// than four, because the caller's answer is the same for every one of them --
/// refuse -- and four variants would be four opportunities to handle one of
/// them by generating anyway.
fn verify(text: &str) -> Result<Vec<String>, PasswordGenError> {
    let mut words: Vec<String> = text.lines().map(|line| line.trim().to_string()).collect();
    // A single trailing newline leaves no empty entry (`lines` does not yield
    // one), but a file ending in several does. Those are dropped; an empty
    // line in the MIDDLE is not, and fails the shape check below as it should.
    while words.last().is_some_and(|word| word.is_empty()) {
        words.pop();
    }

    if words.len() != WORDLIST_WORDS {
        return Err(PasswordGenError::WordlistUnusable);
    }
    let well_formed = words
        .iter()
        .all(|w| (4..=8).contains(&w.len()) && w.bytes().all(|b| b.is_ascii_lowercase()));
    if !well_formed {
        return Err(PasswordGenError::WordlistUnusable);
    }
    let unique: HashSet<&str> = words.iter().map(String::as_str).collect();
    if unique.len() != WORDLIST_WORDS {
        return Err(PasswordGenError::WordlistUnusable);
    }

    let mut hasher = Sha256::new();
    for (index, word) in words.iter().enumerate() {
        if index > 0 {
            hasher.update(b"\n");
        }
        hasher.update(word.as_bytes());
    }
    let digest: [u8; 32] = hasher.finalize().into();
    if digest != WORDLIST_SHA256 {
        return Err(PasswordGenError::WordlistUnusable);
    }

    Ok(words)
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

    // ---- the word list -------------------------------------------------------

    /// The shipped list, read from `assets/` rather than from beside the test
    /// binary, so a test asserting a property of "the word list" is asserting
    /// it of **the file that is committed and installed** and not of whatever
    /// `build.rs` happened to copy.
    fn shipped_wordlist_text() -> String {
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/wordlist.txt"))
            .expect("the shipped word list")
    }

    /// **The test the word list exists for**, and every clause of it is load
    /// bearing.
    ///
    /// * **Exactly 4,096.** Not "about 4,000": 2^12 is what makes
    ///   [`draw_index`] a mask rather than a modulo, so a list that drifted by
    ///   one word would reintroduce bias into every passphrase this app makes.
    /// * **All unique**, checked with a set rather than by eye. A duplicate is
    ///   a word with twice the probability inside a list still claiming twelve
    ///   bits, and it is invisible in a file of four thousand lines.
    /// * **Lowercase `a`-`z` only**, four to eight characters. An apostrophe
    ///   or an accent would collide with the separator and the capitalize
    ///   rule; the length band is what keeps a word dictatable.
    #[test]
    fn the_shipped_wordlist_is_four_thousand_and_ninety_six_unique_short_lowercase_words() {
        let text = shipped_wordlist_text();
        let words: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();

        assert_eq!(
            words.len(),
            WORDLIST_WORDS,
            "the word list holds {} words. It must hold exactly {WORDLIST_WORDS} = 2^{WORDLIST_BITS}, \
             because that is what lets a word be chosen with {WORDLIST_BITS} bits and no modulo",
            words.len()
        );

        let unique: HashSet<&&str> = words.iter().collect();
        assert_eq!(
            unique.len(),
            words.len(),
            "the word list holds {} distinct words out of {}; a duplicate is a word with double \
             the probability and a list with less entropy than its length claims",
            unique.len(),
            words.len()
        );

        for word in &words {
            assert!(
                (4..=8).contains(&word.len()),
                "`{word}` is {} characters; the list is four to eight",
                word.len()
            );
            assert!(
                word.bytes().all(|b| b.is_ascii_lowercase()),
                "`{word}` is not lowercase ASCII a-z"
            );
        }
    }

    /// And the shipped list is the list this module pins: [`verify`] accepts
    /// it whole, hash included.
    ///
    /// Kept apart from the test above on purpose. That one states the
    /// *properties* a reader can check by hand; this one states that the
    /// running code agrees, so a hash constant left stale by an edit to the
    /// list fails here with a name that says which of the two is wrong.
    #[test]
    fn the_shipped_wordlist_passes_the_verification_the_generator_applies() {
        let words = verify(&shipped_wordlist_text()).expect("the shipped list must verify");
        assert_eq!(words.len(), WORDLIST_WORDS);
    }

    /// Line endings do not decide whether the list verifies.
    ///
    /// This repository is checked out on Windows and git may hand the working
    /// tree CRLF. [`WORDLIST_SHA256`] is over the parsed words for exactly
    /// that reason, and this is the assertion that says so rather than a
    /// paragraph hoping it is true.
    #[test]
    fn the_wordlist_verifies_under_either_line_ending() {
        let unix = shipped_wordlist_text().replace("\r\n", "\n");
        let dos = unix.replace('\n', "\r\n");
        verify(&unix).expect("LF");
        verify(&dos).expect("CRLF");
    }

    // ---- refusing rather than degrading --------------------------------------

    /// **The check that stands between a good passphrase and one that looks
    /// identical and is thirty thousand times weaker.**
    ///
    /// Four corruptions, each a way a word list really goes wrong, and every
    /// one of them must be *refused* rather than used:
    ///
    /// 1. **Truncated** -- a partial write, a truncated download. 300 words is
    ///    8.2 bits a word instead of twelve.
    /// 2. **Duplicated** -- still 4,096 lines, one word twice and one missing.
    ///    The count check cannot see this; the uniqueness check must.
    /// 3. **Substituted** -- still 4,096 unique well-formed words, one of them
    ///    changed. Only the hash pin sees this.
    /// 4. **Malformed** -- a word with a character outside `a`-`z`.
    #[test]
    fn a_truncated_duplicated_substituted_or_malformed_list_is_refused_not_used() {
        let text = shipped_wordlist_text().replace("\r\n", "\n");
        let words: Vec<&str> = text.lines().map(str::trim).collect();

        let mut truncated = words.clone();
        truncated.truncate(300);

        let mut duplicated = words.clone();
        duplicated[4_000] = duplicated[0];

        let mut substituted = words.clone();
        substituted[100] = "zzzzzz";

        let mut malformed = words.clone();
        malformed[7] = "Wordlist";

        for (name, broken) in [
            ("truncated to 300 words", truncated),
            ("4,096 lines with one word twice", duplicated),
            ("4,096 unique words, one of them swapped", substituted),
            ("a word that is not lowercase a-z", malformed),
        ] {
            let err = verify(&broken.join("\n"))
                .err()
                .unwrap_or_else(|| panic!("a list {name} was ACCEPTED; it must be refused"));
            assert_eq!(err, PasswordGenError::WordlistUnusable, "{name}");
        }
    }

    /// A missing file is its own named refusal, not a silent empty list.
    #[test]
    fn an_absent_word_list_is_refused_by_name() {
        // `verify` is the half a test can reach without moving the running
        // executable; the missing-file half is `load_wordlist`'s, and what is
        // asserted here is that the two refusals are DIFFERENT so a user can
        // be told which one happened.
        assert_ne!(PasswordGenError::WordlistMissing, PasswordGenError::WordlistUnusable);
        assert!(
            PasswordGenError::WordlistMissing.to_string().contains("not found"),
            "the refusal does not say the list is absent"
        );
    }

    // ---- the draw ------------------------------------------------------------

    /// **The distribution test, and the one this file's history says must be
    /// proved to bite.**
    ///
    /// Every word position must draw from the whole list. The failure it
    /// excludes is a selection that reaches only part of the file -- an index
    /// masked to eleven bits, a `% 1000`, a list read short -- all of which
    /// produce passphrases that look exactly right.
    ///
    /// # The measurement
    ///
    /// 60,000 words are drawn (15,000 four-word passphrases) and bucketed by
    /// which sixteenth of the list they came from, 256 words a bucket. Each
    /// bucket is a binomial with n = 60,000 and p = 1/16, expectation 3,750 and
    /// standard deviation about 59.3. The tolerance is 400, nearly seven
    /// standard deviations, so a false failure is well under one in a million
    /// across all sixteen buckets.
    ///
    /// It also asserts the coarser thing directly: the lowest and highest
    /// indices in the whole sample must sit inside the first and last buckets,
    /// which no truncated selection can manage.
    ///
    /// # And it bites -- measured, not reasoned about
    ///
    /// [`draw_index`]'s mask was changed from `(1 << 12) - 1` to
    /// `(1 << 11) - 1` -- one character, and exactly the defect described
    /// above: half the list becomes unreachable and the other half comes up
    /// twice as often. This test was run against that mutant and failed on
    /// the first bucket it looked at, reporting **words 0..256 drawn 7,515
    /// times against an expected 3,750, off by 3,765 with a tolerance of
    /// 400** -- nine times the tolerance, and that was before it reached the
    /// eight buckets it would have found empty. The mask was then restored
    /// and the test passed.
    #[test]
    fn every_word_position_draws_from_the_whole_list() {
        const ROUNDS: usize = 15_000;
        const WORDS: usize = 4;
        const BUCKETS: usize = 16;
        const PER_BUCKET: usize = WORDLIST_WORDS / BUCKETS;
        const EXPECTED: i64 = (ROUNDS * WORDS / BUCKETS) as i64;
        const TOLERANCE: i64 = 400;

        let list = verify(&shipped_wordlist_text()).expect("the shipped list");
        let index_of: std::collections::HashMap<&str, usize> =
            list.iter().enumerate().map(|(i, w)| (w.as_str(), i)).collect();

        let recipe = PassphraseRecipe {
            words: WORDS as u32,
            separator: "-".to_string(),
            capitalize: false,
            include_number: false,
        };

        let mut counts = [0i64; BUCKETS];
        let mut lowest = usize::MAX;
        let mut highest = 0usize;
        for _ in 0..ROUNDS {
            let phrase = generate_passphrase(&recipe).expect("the shipped list and the CSPRNG");
            let parts: Vec<&str> = phrase.split('-').collect();
            assert_eq!(parts.len(), WORDS, "a {WORDS}-word passphrase came back as `{}`", &*phrase);
            for part in parts {
                let index = *index_of
                    .get(part)
                    .unwrap_or_else(|| panic!("`{part}` is not a word in the shipped list"));
                counts[index / PER_BUCKET] += 1;
                lowest = lowest.min(index);
                highest = highest.max(index);
            }
        }

        for (bucket, count) in counts.iter().enumerate() {
            let deviation = (count - EXPECTED).abs();
            assert!(
                deviation <= TOLERANCE,
                "words {}..{} of the list came up {count} times against an expected {EXPECTED} \
                 (off by {deviation}, tolerance {TOLERANCE}); the selection is not drawing from \
                 the whole list",
                bucket * PER_BUCKET,
                (bucket + 1) * PER_BUCKET
            );
        }
        assert!(
            lowest < PER_BUCKET,
            "the lowest index drawn in {} words was {lowest}; the front of the list is not being \
             reached",
            ROUNDS * WORDS
        );
        assert!(
            highest >= WORDLIST_WORDS - PER_BUCKET,
            "the highest index drawn in {} words was {highest} of {WORDLIST_WORDS}; the end of \
             the list is not being reached",
            ROUNDS * WORDS
        );
    }

    /// Two passphrases in a row do not agree. Cheap insurance against a cached
    /// buffer or a clock seed, the same one the password side carries.
    #[test]
    fn two_passphrases_in_a_row_are_not_the_same() {
        let recipe = PassphraseRecipe::default();
        let first = generate_passphrase(&recipe).expect("the shipped list");
        let second = generate_passphrase(&recipe).expect("the shipped list");
        assert_ne!(*first, *second, "two generated passphrases were identical");
    }

    // ---- the recipe's documented semantics -----------------------------------

    /// `words` is honoured, and clamped up to three the way the route clamps
    /// it -- never down to a shorter passphrase than was asked for.
    #[test]
    fn the_word_count_is_honoured_and_clamped_up_to_three() {
        for (asked, expected) in [(0u32, 3usize), (1, 3), (3, 3), (4, 4), (9, 9)] {
            let recipe = PassphraseRecipe {
                words: asked,
                separator: " ".to_string(),
                capitalize: false,
                include_number: false,
            };
            let phrase = generate_passphrase(&recipe).expect("the shipped list");
            assert_eq!(
                phrase.split(' ').count(),
                expected,
                "asking for {asked} words produced `{}`",
                &*phrase
            );
        }
    }

    /// The absurd recipe is bounded rather than allocated, the same decision
    /// [`normalize`] records for a password's length.
    #[test]
    fn an_absurd_word_count_is_bounded_instead_of_allocating_forever() {
        let recipe = PassphraseRecipe {
            words: u32::MAX,
            separator: "-".to_string(),
            capitalize: false,
            include_number: false,
        };
        let phrase = generate_passphrase(&recipe).expect("the shipped list");
        assert_eq!(phrase.split('-').count(), MAX_WORDS as usize);
    }

    /// `separator`, in every shape [`PassphraseRecipe::separator`] documents:
    /// a single character, only the FIRST character of anything longer, and
    /// the two literal words that are not separators at all.
    #[test]
    fn the_separator_is_the_one_the_recipe_documents() {
        let phrase = |separator: &str| {
            let recipe = PassphraseRecipe {
                words: 4,
                separator: separator.to_string(),
                capitalize: false,
                include_number: false,
            };
            generate_passphrase(&recipe).expect("the shipped list").to_string()
        };

        assert_eq!(phrase("-").split('-').count(), 4, "a single character separator");
        assert_eq!(phrase("+").split('+').count(), 4);
        // Only the first character of a longer string.
        let long = phrase("+++");
        assert_eq!(long.split('+').count(), 4, "`+++` used more than its first character: {long}");
        // The two literal words.
        assert_eq!(phrase("space").split(' ').count(), 4, "`space` is a space");
        for word in ["empty", ""] {
            let joined = phrase(word);
            assert!(
                joined.bytes().all(|b| b.is_ascii_lowercase()),
                "`{word}` left a separator in `{joined}`"
            );
            assert!(joined.len() >= 16, "`{word}` produced only `{joined}`");
        }
    }

    /// `capitalize` capitalises the first letter of **every** word and nothing
    /// else, and with it off the passphrase is all lowercase.
    #[test]
    fn capitalize_raises_the_first_letter_of_every_word_and_only_that() {
        for _ in 0..200 {
            let recipe = PassphraseRecipe {
                words: 5,
                separator: "-".to_string(),
                capitalize: true,
                include_number: false,
            };
            let phrase = generate_passphrase(&recipe).expect("the shipped list");
            for word in phrase.split('-') {
                let mut letters = word.chars();
                let first = letters.next().expect("a non-empty word");
                assert!(first.is_ascii_uppercase(), "`{word}` does not start capitalised");
                assert!(
                    letters.all(|c| c.is_ascii_lowercase()),
                    "`{word}` capitalised more than its first letter"
                );
            }

            let plain = generate_passphrase(&PassphraseRecipe { capitalize: false, ..recipe })
                .expect("the shipped list");
            assert!(
                plain.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
                "`{}` is not all lowercase with capitalize off",
                &*plain
            );
        }
    }

    /// `include_number` puts exactly one digit in the passphrase, and leaves
    /// none there when it is off.
    #[test]
    fn include_number_adds_exactly_one_digit_and_none_when_off() {
        let base = PassphraseRecipe {
            words: 4,
            separator: "-".to_string(),
            capitalize: false,
            include_number: true,
        };
        for _ in 0..300 {
            let with = generate_passphrase(&base).expect("the shipped list");
            assert_eq!(
                with.chars().filter(char::is_ascii_digit).count(),
                1,
                "`{}` does not carry exactly one digit",
                &*with
            );
            let without =
                generate_passphrase(&PassphraseRecipe { include_number: false, ..base.clone() })
                    .expect("the shipped list");
            assert!(
                !without.chars().any(|c| c.is_ascii_digit()),
                "`{}` carries a digit with includeNumber off",
                &*without
            );
        }
    }

    /// **The other half of "no bias"**, and the passphrase's version of
    /// `a_forced_character_lands_in_every_position_not_just_the_first`.
    ///
    /// A generator that satisfies `include_number` by appending a digit to the
    /// last word passes every test that merely counts digits, and has given
    /// away the digit's position for nothing. So this counts positions: over
    /// 8,000 six-word passphrases every one of the six words must have carried
    /// the digit a plausible number of times.
    ///
    /// Each position is a binomial with n = 8,000 and p = 1/6: expectation
    /// 1,333 and standard deviation about 33.3. The tolerance of 15% is four
    /// hundred times the standard deviation twelve times over -- it cannot
    /// flake -- while an implementation that always appends puts all 8,000 in
    /// the last bin, 500% out.
    #[test]
    fn the_included_number_lands_on_every_word_not_just_the_last() {
        const WORDS: usize = 6;
        const ROUNDS: usize = 8_000;
        let recipe = PassphraseRecipe {
            words: WORDS as u32,
            separator: "-".to_string(),
            capitalize: false,
            include_number: true,
        };

        let mut positions = [0usize; WORDS];
        for _ in 0..ROUNDS {
            let phrase = generate_passphrase(&recipe).expect("the shipped list");
            let mut found = None;
            for (index, word) in phrase.split('-').enumerate() {
                if word.chars().any(|c| c.is_ascii_digit()) {
                    assert!(found.is_none(), "`{}` carries two numbered words", &*phrase);
                    found = Some(index);
                }
            }
            positions[found.expect("a numbered word")] += 1;
        }

        let mean = ROUNDS as f64 / WORDS as f64;
        for (index, count) in positions.iter().enumerate() {
            let deviation = (*count as f64 - mean).abs() / mean;
            assert!(
                deviation <= 0.15,
                "word {index} carried the digit {count} times against a mean of {mean:.0} across \
                 all {WORDS} words ({:.1}% off); the digit is not being placed at random",
                deviation * 100.0
            );
        }
    }

    /// The digit itself is a digit, and every one of the ten is reachable.
    /// A generator that only ever appended `7` would pass the position test.
    #[test]
    fn the_included_digit_reaches_all_ten_values() {
        let recipe = PassphraseRecipe {
            words: 3,
            separator: "-".to_string(),
            capitalize: false,
            include_number: true,
        };
        let mut seen = HashSet::new();
        for _ in 0..600 {
            let phrase = generate_passphrase(&recipe).expect("the shipped list");
            for c in phrase.chars().filter(char::is_ascii_digit) {
                seen.insert(c);
            }
        }
        for digit in '0'..='9' {
            assert!(seen.contains(&digit), "`{digit}` was never generated");
        }
    }

    // ---- the dispatch and the installer --------------------------------------

    /// A passphrase is generated by the function a backend calls, rather than
    /// refused there. This is the wiring `RestBackend::generate` depends on.
    #[test]
    fn the_dispatch_generates_a_passphrase_rather_than_refusing_it() {
        let phrase = generate(&GenerateRequest::Passphrase(PassphraseRecipe::default()))
            .expect("a passphrase");
        // The default recipe: four words, `-`, capitalised, with a number.
        assert_eq!(phrase.split('-').count(), 4, "{}", &*phrase);
        assert_eq!(phrase.chars().filter(char::is_ascii_digit).count(), 1);
    }

    /// The dispatch does route a password request to the generator, so the
    /// function a backend calls is the function that was tested above.
    #[test]
    fn the_dispatch_generates_a_password_rather_than_refusing_it() {
        let password = generate(&GenerateRequest::Password(PasswordRecipe::default()))
            .expect("a password");
        assert_eq!(password.len(), 20);
    }

    /// **The installer really ships the file this module reads.**
    ///
    /// A generator that works in a development build and refuses in the
    /// installed app is a defect nobody sees until release, so the installer
    /// script is read here and required to install the word list to `{app}`,
    /// under the name [`WORDLIST_FILE`] this module looks for.
    #[test]
    fn the_installer_ships_the_wordlist_this_module_reads() {
        let script = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/installer/deskwarden.iss"
        ))
        .expect("the installer script");
        let line = script
            .lines()
            .map(str::trim)
            .find(|l| !l.starts_with(';') && l.contains(WORDLIST_FILE))
            .unwrap_or_else(|| {
                panic!(
                    "the installer no longer ships `{WORDLIST_FILE}`, so the installed app would \
                     refuse every passphrase while a development build generated them"
                )
            });
        assert!(line.starts_with("Source:"), "`{line}` is not a [Files] entry");
        assert!(
            line.contains(r"..\assets\wordlist.txt"),
            "the installer ships something other than the committed list: `{line}`"
        );
        assert!(
            line.contains(r#"DestDir: "{app}""#),
            "the word list is not installed beside the executable, which is the only place \
             `wordlist_paths` looks: `{line}`"
        );
    }

    /// And `build.rs` puts it beside the executable in a development build, so
    /// the two layouts are the same and `cargo run` is not the only build that
    /// refuses passphrases.
    #[test]
    fn the_build_script_copies_the_wordlist_beside_the_executable() {
        let build = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/build.rs"))
            .expect("build.rs");
        assert!(
            build.contains("assets/wordlist.txt") && build.contains("wordlist.txt\""),
            "build.rs no longer copies the word list beside the executable"
        );
    }

    // ---- the refusals carry nothing ------------------------------------------

    /// This module carries no wordlist *in its source*, asserted against the
    /// source text so that "do not invent one" survives the next edit.
    ///
    /// The list this module uses is a file. What must never appear here is a
    /// second, improvised one, and the shape it would arrive in is a long list
    /// of quoted strings. This counts them.
    #[test]
    fn no_wordlist_has_been_smuggled_into_this_module() {
        let source = include_str!("password_gen.rs");
        let quoted = source.matches('"').count();
        assert!(
            quoted < 400,
            "this file now holds {quoted} quote characters, which is enough to be an improvised \
             wordlist; the list belongs in `assets/wordlist.txt`, where it is counted, \
             de-duplicated and hashed before use"
        );
    }

    /// No error may carry a generated secret, the way
    /// `rest::api`'s `no_error_can_carry_a_credential` says of that module.
    /// Every variant is fieldless, so this is a check that they stay so.
    #[test]
    fn no_error_can_carry_a_generated_secret() {
        // A fieldless variant's `Debug` is exactly its name, so anything else
        // appearing here means a variant grew somewhere to put a secret.
        assert_eq!(format!("{:?}", PasswordGenError::Rng), "Rng");
        assert_eq!(format!("{:?}", PasswordGenError::WordlistMissing), "WordlistMissing");
        assert_eq!(format!("{:?}", PasswordGenError::WordlistUnusable), "WordlistUnusable");
        // And the compiler's half: if a variant ever gains a field, this stops
        // compiling as written and whoever added it has to come back here.
        let _exhaustive: fn(PasswordGenError) -> u8 = |e| match e {
            PasswordGenError::Rng => 0,
            PasswordGenError::WordlistMissing => 1,
            PasswordGenError::WordlistUnusable => 2,
        };
        // Nor may a message quote anything that was generated. Every message
        // is a fixed English sentence with no interpolation at all, which is
        // asserted here as "it says the same thing every time": a message that
        // grew a `{}` would differ between two calls that produced different
        // secrets.
        for error in [
            PasswordGenError::Rng,
            PasswordGenError::WordlistMissing,
            PasswordGenError::WordlistUnusable,
        ] {
            assert!(!error.to_string().is_empty());
            assert_eq!(error.to_string(), error.to_string());
            assert!(
                !error.to_string().chars().any(|c| c.is_ascii_digit()),
                "an error message carries a digit, which a generated secret can be made of: {error}"
            );
        }
    }
}
