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
//! By default: prelogin, the password grant, one `GET /api/sync`, the
//! decrypt, and a token refresh. **No write of any kind.** That is the read
//! half, and it has to pass before the write question is even asked.
//!
//! With `--write` it then exercises the write path -- but only against **one
//! item it created itself**, and it hard-deletes that item at the end. It
//! never touches an item that was already in the vault. That restriction is
//! the whole safety argument: `PUT /api/ciphers/{id}` replaces a whole
//! cipher, and this path once stripped every modelled field it could not
//! decrypt and overwrote `name` with an encryption of `""` -- data destroyed
//! behind something that looked correct, fired by an ordinary autofill.
//!
//! The write pass drives [`deskwarden::vault_backend::VaultBackend`] rather
//! than [`RestClient`] directly, because that trait is the surface the app
//! actually calls and is where the integration defects were. Its centre is
//! the `set_app_match` check: an item is created carrying a TOTP seed, notes
//! and a URI, an app-match is written onto it, and every one of those values
//! is read back and compared. That is the exact call that used to destroy
//! them.
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
//! truncated names, and `--write` for the write pass described above.

use deskwarden::app_match::AppMatch;
use deskwarden::rest::api::{Device, RestClient};
use deskwarden::rest::backend::RestBackend;
use deskwarden::rest::sync::decrypt_vault;
use deskwarden::vault_backend::VaultBackend;
use deskwarden::vault_bridge::{NewItem, VaultItem};
use std::collections::BTreeMap;
use std::io::{IsTerminal, Write};
use zeroize::Zeroizing;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let sample = args.iter().any(|a| a == "--sample");
    let write = args.iter().any(|a| a == "--write");
    let cleanup = args.iter().any(|a| a == "--cleanup");
    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    if positional.len() != 2 {
        eprintln!("usage: rest_probe <server-url> <email> [--sample] [--write] [--cleanup]");
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
                r.profile.as_ref().map_or(0, |p| p.organizations.len()),
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

    if cleanup {
        cleanup_pass(RestBackend::new(client, authed), &vault);
        return;
    }

    if !write {
        println!();
        println!("read-only probe finished. Nothing was written to the vault.");
        return;
    }

    write_pass(RestBackend::new(client, authed));
}

/// The prefix every item this probe creates is named with.
///
/// One constant, used by [`write_pass`] to name what it makes and by
/// [`cleanup_pass`] to find it. Two spellings is how a cleanup comes to miss
/// the thing it is for.
const PROBE_NAME_PREFIX: &str = "Deskwarden write probe";

/// Destroys every item this probe has ever left behind, and nothing else.
///
/// # Why this exists rather than being unnecessary
///
/// It was written because a run of [`write_pass`] reported that it had
/// purged its item and had not: against NodeWarden, `DELETE
/// /api/ciphers/{id}` on a *live* cipher is the compat route, which
/// soft-deletes and answers `200`. Two probe items sat in a real vault
/// behind a line that said "the probe item is gone".
///
/// The route is fixed, so this should find nothing on a healthy run. It is
/// kept anyway: a probe that can create items in a real vault should be able
/// to prove none of its own are left, and "should find nothing" is a claim
/// worth being able to check rather than assert.
///
/// # What it will not touch
///
/// Only items whose name starts with [`PROBE_NAME_PREFIX`]. Every id is
/// printed before it is destroyed, so the output is a record of exactly what
/// was removed.
fn cleanup_pass(backend: RestBackend, vault: &deskwarden::rest::sync::DecryptedVault) {
    println!("== cleanup ==");
    let mine: Vec<&VaultItem> = vault
        .items
        .iter()
        .map(|d| &d.item)
        .filter(|i| i.name.starts_with(PROBE_NAME_PREFIX))
        .collect();

    if mine.is_empty() {
        println!("  no probe items in the vault");
        return;
    }

    println!("  {} probe item(s) to destroy:", mine.len());
    let mut failed = 0usize;
    for item in mine {
        // `purge_item` is unconditional now, so no trash step first: a
        // trash-then-purge would be two chances to fail where one is needed.
        match backend.purge_item(&item.id) {
            Ok(()) => println!("    {} destroyed", item.id),
            Err(e) => {
                println!("    {} FAILED: {e:?}", item.id);
                failed += 1;
            }
        }
    }
    if failed > 0 {
        println!("  {failed} could not be destroyed and are still in the vault");
        std::process::exit(1);
    }
    println!("  all gone");
}

/// The write half: one item, created here, exercised, and hard-deleted.
///
/// # The one rule
///
/// **Every call below addresses an item this function created.** No existing
/// cipher is read back, written, trashed or archived. If any step fails, the
/// function still tries to purge what it made -- an abandoned probe item in
/// a real vault is litter, and litter named "Deskwarden write probe" in a
/// list of 1,683 logins is the kind of thing that gets found months later
/// and mistaken for a real credential.
///
/// # Why `set_app_match` is the centre and not one step among many
///
/// It is the call the app makes during ordinary autofill, and it is the one
/// that used to destroy data: the mapper strips every modelled key out of the
/// catch-all, a field that failed to decrypt is `None` in the model, so the
/// write removed the key and the server forgot the value. A TOTP seed, a
/// card number, an SSH private key. The item created here deliberately
/// carries a TOTP seed, notes and a URI so that the check has something to
/// lose.
fn write_pass(backend: RestBackend) {
    // A name nobody will mistake for a credential, with a nonce so two runs
    // do not produce two identically-named items and so a leftover from a
    // failed run is distinguishable from this one's.
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let name = format!("{PROBE_NAME_PREFIX} {nonce} -- safe to delete");

    // Values chosen so every one of them is a field the write path has to
    // carry across a PUT. The TOTP seed is a real base32 string because a
    // server may validate the shape; it is not a seed for anything.
    const USERNAME: &str = "probe@example.invalid";
    const PASSWORD: &str = "probe-password-do-not-use";
    const TOTP: &str = "JBSWY3DPEHPK3PXP";
    const URI: &str = "https://probe.example.invalid/login";
    const NOTES: &str = "Created by rest_probe --write. If you are reading this, the probe did \
                         not clean up after itself; the item is safe to delete.";

    println!("== create ==");
    let created = match backend.create_item(&NewItem::ImportedRecord {
        name: name.clone(),
        folder_id: None,
        username: Some(USERNAME.to_string()),
        password: Some(Zeroizing::new(PASSWORD.to_string())),
        totp: Some(Zeroizing::new(TOTP.to_string())),
        uri: Some(URI.to_string()),
        notes: Some(Zeroizing::new(NOTES.to_string())),
    }) {
        Ok(item) => {
            println!("  ok -- id {}", item.id);
            item
        }
        Err(e) => {
            println!("  FAILED: {e:?}");
            println!("  nothing was created, so there is nothing to clean up");
            std::process::exit(1);
        }
    };
    let id = created.id.clone();

    // From here on every exit goes through `finish`, which purges.
    let mut failures: Vec<String> = Vec::new();

    println!("== read back ==");
    match backend.get_item(&id) {
        Ok(item) => report_fields("  ", &item, &name, &mut failures),
        Err(e) => failures.push(format!("get_item after create: {e:?}")),
    }

    // The check this whole pass exists for.
    println!("== set_app_match (the call that used to destroy fields) ==");
    let matched = AppMatch {
        process: "rest_probe.exe".to_string(),
        title: String::new(),
        hosted: false,
        path: String::new(),
        args: String::new(),
        sequence: String::new(),
        // `Prompt` rather than `Auto`: this match is written to a real vault
        // for a few seconds, and an `Auto` trigger on an exe name is the one
        // value that could make a running Deskwarden type into something.
        trigger: deskwarden::app_match::TriggerMode::Prompt,
    };
    match backend.set_app_match(&created, &matched) {
        Ok(_) => println!("  ok"),
        Err(e) => failures.push(format!("set_app_match: {e:?}")),
    }
    println!("== read back after set_app_match ==");
    match backend.get_item(&id) {
        Ok(item) => report_fields("  ", &item, &name, &mut failures),
        Err(e) => failures.push(format!("get_item after set_app_match: {e:?}")),
    }

    println!("== archive, then unarchive ==");
    // The per-id routes, which is what nodewarden actually has -- this crate
    // used a bulk route until its handler source was read.
    match backend.archive_item(&id).and_then(|()| backend.unarchive_item(&id)) {
        Ok(()) => println!("  ok"),
        Err(e) => failures.push(format!("archive/unarchive: {e:?}")),
    }

    println!("== trash, then restore ==");
    match backend.delete_item(&id).and_then(|()| backend.restore_item(&id)) {
        Ok(()) => println!("  ok"),
        Err(e) => failures.push(format!("trash/restore: {e:?}")),
    }
    // A restore that reported success and did not happen is the failure worth
    // catching here, so the item is read back rather than trusted.
    println!("== read back after the round trip ==");
    match backend.get_item(&id) {
        Ok(item) => report_fields("  ", &item, &name, &mut failures),
        Err(e) => failures.push(format!("get_item after restore: {e:?}")),
    }

    println!("== purge ==");
    match backend.purge_item(&id) {
        Ok(()) => println!("  ok -- the probe item is gone"),
        Err(e) => {
            println!("  FAILED: {e:?}");
            println!("  LEFTOVER: item {id} is still in the vault and should be deleted by hand");
            failures.push(format!("purge: {e:?}"));
        }
    }

    println!();
    if failures.is_empty() {
        println!("write pass finished with no failures.");
    } else {
        println!("write pass finished with {} FAILURES:", failures.len());
        for failure in &failures {
            println!("  {failure}");
        }
        std::process::exit(1);
    }
}

/// Compares every field the write path has to carry, and says which survived.
///
/// Reports rather than asserts, and pushes onto `failures` rather than
/// panicking, because a panic here would skip the purge and leave the probe
/// item in the vault.
fn report_fields(indent: &str, item: &VaultItem, expected_name: &str, failures: &mut Vec<String>) {
    const USERNAME: &str = "probe@example.invalid";
    const PASSWORD: &str = "probe-password-do-not-use";
    const TOTP: &str = "JBSWY3DPEHPK3PXP";

    let mut check = |field: &str, got: Option<&str>, want: &str| {
        let ok = got == Some(want);
        println!("{indent}{field}: {}", if ok { "intact" } else { "LOST OR CHANGED" });
        if !ok {
            // The field NAME and whether it matched -- never the value, for
            // the same reason nothing else here prints one.
            failures.push(format!(
                "{field} did not survive (present: {})",
                got.is_some_and(|g| !g.is_empty())
            ));
        }
    };

    check("name", Some(item.name.as_str()), expected_name);
    let login = item.login.as_ref();
    check("username", login.and_then(|l| l.username.as_deref()), USERNAME);
    check("password", login.and_then(|l| l.password.as_deref()).map(|p| p.as_str()), PASSWORD);
    check("totp", login.and_then(|l| l.totp.as_deref()).map(|t| t.as_str()), TOTP);

    let uris = login.map_or(0, |l| l.uris.len());
    println!("{indent}uris: {uris}");
    if uris == 0 {
        failures.push("the website was lost".to_string());
    }
    let has_notes = item.notes.as_deref().is_some_and(|n| !n.is_empty());
    println!("{indent}notes: {}", if has_notes { "intact" } else { "LOST" });
    if !has_notes {
        failures.push("the notes were lost".to_string());
    }
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
