//! Point `deskwarden/src/rest/` at a **real** server, once, and report what
//! came back.
//!
//! # Why this exists
//!
//! Every test under `rest/` drives `mockito` or a published vector. That was
//! deliberate -- no test may reach the network -- and it leaves exactly one
//! thing unchecked: whether the server this backend was written for answers
//! the way the fixtures say. The module header already names the risk ("treat
//! the API as a subset"; "a mapper that unwraps where a fixture was generous
//! panics on the real payload"), and an example is the only place that can
//! settle it without putting the network inside `cargo test`.
//!
//! # What it does, and what it will not do
//!
//! Prelogin, the password grant, one `GET /api/sync`, and the decrypt. **No
//! write of any kind** -- no create, no update, no trash, no archive, no
//! folder. The write path replaces a whole cipher and had a data-loss defect
//! in it once; it is not something to try first against a live vault, and
//! this probe is the read half that has to pass before that question is even
//! asked.
//!
//! # Nothing secret is printed, and nothing secret is on the command line
//!
//! The master password is read from **stdin**, because a command line is
//! readable by every process on the machine -- the same rule
//! `docs/superpowers/specs/2026-08-23-daemon-and-ui-processes-design.md`
//! states for the UI process, applied here.
//!
//! The output is counts, type tallies and decryption *failures*. No item
//! name, no username, no URI and no password is ever printed: a probe that
//! dumped a vault to a terminal scrollback would be a worse leak than the one
//! it is checking for. Names are what a human wants to see to believe the
//! decrypt worked, so the stand-in is `--sample`, which prints the first
//! three characters of a name and the count of the rest.
//!
//! # Running it
//!
//! ```text
//! cargo run --example rest_probe -- https://vault.example.com me@example.com
//! ```
//!
//! It prompts for the master password on stdin. Add `--sample` for the
//! truncated names.

use deskwarden::rest::api::{Device, RestClient};
use deskwarden::rest::sync::decrypt_vault;
use std::collections::BTreeMap;
use std::io::{IsTerminal, Write};
use zeroize::Zeroizing;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let sample = args.iter().any(|a| a == "--sample");
    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    if positional.len() != 2 {
        eprintln!("usage: rest_probe <server-url> <email> [--sample]");
        eprintln!("  the master password is read from stdin, never from the command line");
        std::process::exit(2);
    }
    let (base, email) = (positional[0].as_str(), positional[1].as_str());

    // Zeroizing from the first read, not from the first use: the plaintext
    // exists between those two points either way.
    let password = match read_password() {
        Ok(p) if !p.is_empty() => p,
        Ok(_) => {
            eprintln!("no password on stdin; nothing to try");
            std::process::exit(2);
        }
        Err(e) => {
            eprintln!("could not read the password: {e}");
            std::process::exit(2);
        }
    };

    let client = RestClient::new(base);

    // Prelogin on its own first, and reported on its own, because it is the
    // one call whose answer decides how the master key is derived. A wrong
    // KDF here fails later as "invalid password", which is the least useful
    // possible symptom -- `KdfMemory` being MiB rather than KiB was found
    // exactly this way, by a vector rather than by a login.
    println!("== prelogin ==");
    let kdf = match client.prelogin(email) {
        Ok(kdf) => {
            println!("  {kdf:?}");
            kdf
        }
        Err(e) => {
            println!("  FAILED: {e}");
            std::process::exit(1);
        }
    };
    let _ = kdf;

    println!("== password grant ==");
    // A stable identifier so repeated runs are one device to the server
    // rather than a new one each time. The name is this probe's, so a device
    // list on the server says what made the entry.
    let device = Device::windows_desktop("deskwarden-rest-probe", "Deskwarden REST probe");
    let mut authed = match client.authenticate(email, password.as_bytes(), &device) {
        Ok(a) => {
            println!("  ok -- {:?}", a.session);
            a
        }
        Err(e) => {
            println!("  FAILED: {e}");
            std::process::exit(1);
        }
    };
    drop(password);

    println!("== GET /api/sync ==");
    let response = match client.sync(&authed.session) {
        Ok(r) => {
            println!(
                "  ok -- {} ciphers, {} folders, profile {}, {} organisations",
                r.ciphers.len(),
                r.folders.len(),
                if r.profile.is_some() { "present" } else { "ABSENT" },
                r.organizations.len(),
            );
            r
        }
        Err(e) => {
            println!("  FAILED: {e}");
            std::process::exit(1);
        }
    };

    println!("== decrypt ==");
    let vault = match decrypt_vault(&response, &authed.master_key) {
        Ok(v) => v,
        Err(e) => {
            println!("  FAILED: {e}");
            std::process::exit(1);
        }
    };
    println!("  {} items, {} folders", vault.items.len(), vault.folders.len());

    // The whole point of the run. A sync that returns 1,663 ciphers and
    // decrypts 1,663 items with no failures is the answer; anything in this
    // list is a real field of a real item that this crate could not read, and
    // is named by the same path the write path uses.
    if vault.failures.is_empty() {
        println!("  no decryption failures");
    } else {
        println!("  {} DECRYPTION FAILURES:", vault.failures.len());
        for failure in &vault.failures {
            println!("    {failure}");
        }
    }

    // A cipher the server sent that the mapper dropped entirely is invisible
    // in the counts above unless they are compared, so compare them here
    // rather than leaving it to whoever reads the output.
    if vault.items.len() != response.ciphers.len() {
        println!(
            "  NOTE: {} of {} ciphers produced no item at all",
            response.ciphers.len() - vault.items.len(),
            response.ciphers.len(),
        );
    }

    println!("== what came back ==");
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut with_username = 0usize;
    let mut with_password = 0usize;
    let mut with_totp = 0usize;
    let mut with_uri = 0usize;
    for decrypted in &vault.items {
        let item = &decrypted.item;
        *kinds.entry(format!("type {:?}", item.item_type)).or_default() += 1;
        if let Some(login) = item.login.as_ref() {
            if login.username.as_deref().is_some_and(|u| !u.is_empty()) {
                with_username += 1;
            }
            if login.password.as_deref().is_some_and(|p| !p.is_empty()) {
                with_password += 1;
            }
            if login.totp.as_deref().is_some_and(|t| !t.is_empty()) {
                with_totp += 1;
            }
            if !login.uris.is_empty() {
                with_uri += 1;
            }
        }
    }
    for (kind, count) in &kinds {
        println!("  {kind}: {count}");
    }
    println!(
        "  logins with a username {with_username}, a password {with_password}, \
         a TOTP seed {with_totp}, at least one website {with_uri}"
    );

    if sample {
        // Truncated on purpose -- see the module header. Three characters is
        // enough for a human to recognise their own vault and not enough to
        // be a listing of it.
        println!("== a sample, truncated ==");
        for decrypted in vault.items.iter().take(10) {
            let name = decrypted.item.name.as_str();
            let head: String = name.chars().take(3).collect();
            let rest = name.chars().count().saturating_sub(head.chars().count());
            println!("  {head}... (+{rest} more characters)");
        }
    }

    // Not a write, and the only other thing the session can do. Worth
    // exercising because an expiring session is what the `bw` path has and
    // this path's refresh has only ever been driven against mockito.
    println!("== refresh ==");
    if authed.session.can_refresh() {
        match client.refresh(&mut authed.session) {
            Ok(()) => println!("  ok -- {:?}", authed.session),
            Err(e) => println!("  FAILED: {e}"),
        }
    } else {
        println!("  the grant returned no refresh token, so there is nothing to refresh");
    }

    println!();
    println!("read-only probe finished. Nothing was written to the vault.");
}

/// The master password, from stdin.
///
/// **No echo suppression**, and that is stated rather than hidden: this reads
/// a line like any other. Piping the password in (`... < file`, or from a
/// password manager's own CLI) is the way to run it without the characters
/// appearing on screen, and the prompt below says so when stdin is a
/// terminal. Writing a terminal-mode toggle here would be a second, worse
/// copy of something this crate does not otherwise do.
fn read_password() -> std::io::Result<Zeroizing<String>> {
    if std::io::stdin().is_terminal() {
        print!("master password (it will be visible as you type): ");
        std::io::stdout().flush()?;
    }
    let mut line = Zeroizing::new(String::new());
    std::io::stdin().read_line(&mut line)?;
    let trimmed = line.trim_end_matches(['\r', '\n']).to_string();
    Ok(Zeroizing::new(trimmed))
}
