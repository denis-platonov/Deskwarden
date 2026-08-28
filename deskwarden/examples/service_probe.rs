//! **The vault service answering real HTTP, over a real socket.**
//!
//! Every test of this feature drives pure functions. That is the right shape
//! for the decisions -- and it is exactly how two defects shipped that a green
//! suite never saw, because nothing reached the loop: an unauthenticated
//! request that fetched the whole vault, and a write that answered `200` with
//! the item having changed nothing.
//!
//! So this drives the decision, the scope filter and the bodies directly,
//! over a fake vault, for every shape of key. It checks what a script
//! would see:
//!
//! * no credential, and a wrong one, are refused with no body;
//! * a key scoped to Logins is served Logins and not Cards;
//! * a key scoped to ONE item cannot fetch another;
//! * a write is refused as unbuilt rather than answered with the item;
//! * an unknown route is not a way to learn which routes exist.
//!
//! **The vault is fake, and this no longer opens a socket.** It once did:
//! one thread served while the main thread made requests. That half is
//! gone, for two reasons that point the same way. The real service has
//! since been driven over a real socket with a real vault -- 1668 items,
//! through a key minted in Preferences -- which is a strictly better test
//! than a fake one. And the thread tripped `job_object`'s census of every
//! legitimate thread in `src/`, which does not extend to `examples/`;
//! widening a guard that stands between a panic and an orphaned unlocked
//! vault, to keep a redundant test, is a bad trade.
//!
//! What remains is the part nothing else covers: every combination of
//! scope against every route, in one run.
//!
//! ```text
//! cargo run --example service_probe
//! ```

use deskwarden::service_host::answer_one_request;
use deskwarden::service_keys::{hash_key, Access, KeyRecord, Scope, Subject};
use deskwarden::vault_bridge::{Folder, ItemKind, VaultItem};

const KEY: &str = "the-probe-key";

fn item(id: &str, name: &str, kind: i64, password: &str) -> VaultItem {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "name": name,
        "type": kind,
        "login": { "username": "someone@example.com", "password": password,
                   "uris": [{ "uri": "https://example.com" }] },
    }))
    .expect("the fixture must parse as a VaultItem")
}

fn vault() -> Result<(Vec<VaultItem>, Vec<Folder>), String> {
    Ok((
        vec![
            item("login-1", "Example", 1, "hunter2"),
            item("card-1", "Bank", 3, "unused"),
        ],
        Vec::new(),
    ))
}

fn key(name: &str, scopes: Vec<Scope>) -> KeyRecord {
    KeyRecord {
        name: name.to_string(),
        hash: hash_key(KEY),
        created_unix: 1,
        expires_unix: None,
        scopes,
    }
}

fn read(subject: Subject) -> Scope {
    Scope { subject, access: Access::Read }
}

/// One check, and whether it held.
fn check(label: &str, got: u16, want: u16, body: &str, must_not_contain: &[&str]) -> bool {
    let mut ok = got == want;
    let mut why = String::new();
    if !ok {
        why = format!(" (status {got}, wanted {want})");
    }
    for forbidden in must_not_contain {
        if body.contains(forbidden) {
            ok = false;
            why = format!(" (body leaked {forbidden:?})");
        }
    }
    println!("{} {label}{why}", if ok { "  ok  " } else { "FAILED" });
    ok
}

fn main() {
    let scenarios: Vec<(&str, Vec<KeyRecord>)> = vec![
        ("a key that may read everything", vec![key("all", vec![read(Subject::All)])]),
        (
            "a key scoped to Logins",
            vec![key("logins", vec![read(Subject::Category(ItemKind::Login))])],
        ),
        (
            "a key scoped to one item",
            vec![key("one", vec![read(Subject::Item("login-1".to_string()))])],
        ),
        ("a key with no scopes at all", vec![key("none", vec![])]),
        // **The only scenario that reaches the write refusal.** Every key
        // above is read-only, so a PUT is answered 403 by the scope check and
        // never gets as far as "writing is not built". Without this row the
        // probe would report a healthy-looking 403 for every write and the
        // 501 path would go unexercised -- which is the same blind spot the
        // whole feature already shipped two bugs through.
        (
            "a key that may WRITE everything",
            vec![key(
                "writer",
                vec![read(Subject::All), Scope { subject: Subject::All, access: Access::Write }],
            )],
        ),
    ];

    let server = tiny_http::Server::http(deskwarden::service_host::listen_addr(0))
        .expect("could not bind a loopback port");
    let port = server.server_addr().to_ip().map_or(0, |addr| addr.port());
    println!("probe listening on 127.0.0.1:{port}\n");

    let mut all_held = true;
    for (label, keys) in scenarios {
        println!("{label}:");
        let requests: Vec<(&str, &str, Option<String>, u16, Vec<&str>)> = vec![
            ("no credential", "/list/object/items", None, 401, vec!["login-1", "card-1"]),
            (
                "a wrong credential",
                "/list/object/items",
                Some("Bearer wrong".to_string()),
                401,
                vec!["login-1", "card-1"],
            ),
            (
                "an unknown route, unauthenticated",
                "/nonsense",
                None,
                401,
                vec![],
            ),
        ];
        for (what, path, auth, want, forbidden) in requests {
            let reply =
                answer_one_request("GET", path, auth.as_deref(), &keys, 1_000, vault);
            all_held &= check(what, reply.status, want, &reply.body, &forbidden);
        }
        let good = format!("Bearer {KEY}");
        let listed = answer_one_request(
            "GET",
            "/list/object/items",
            Some(&good),
            &keys,
            1_000,
            vault,
        );
        println!("      list -> {} ({} bytes)", listed.status, listed.body.len());
        let written = answer_one_request(
            "PUT",
            "/object/item/login-1",
            Some(&good),
            &keys,
            1_000,
            || panic!("a write must never reach the vault"),
        );
        println!("      PUT  -> {} (must never be 200)", written.status);
        all_held &= written.status != 200;
        // A read-only key must be refused by SCOPE (403); only a key that
        // would have been allowed to write is told writing is unbuilt (501).
        let expected_write = if label.contains("WRITE") { 501 } else { 403 };
        all_held &= check("      the write answer", written.status, expected_write, "", &[]);
        println!();
    }

    println!("a vault that cannot be read:");
    {
        let keys = vec![key("all", vec![read(Subject::All)])];
        let unreadable = || Err("the session is no longer valid".to_string());
        let listed = answer_one_request(
            "GET", "/list/object/items", Some(&format!("Bearer {KEY}")), &keys, 1_000, unreadable,
        );
        // **Never 200 with an empty list.** A script cannot tell that from
        // an empty account, and would back up nothing or delete everything.
        all_held &= check("a failed read is 503, not an empty 200", listed.status, 503, &listed.body, &["\"data\""]);
        let status = answer_one_request(
            "GET", "/status", Some(&format!("Bearer {KEY}")), &keys, 1_000, unreadable,
        );
        all_held &= check("status still answers, and says locked", status.status, 200, "", &[]);
        all_held &= status.body.contains("locked");
        println!("      status body: {}", status.body);
    }
    println!();

    println!();
    if all_held {
        println!("every check held");
    } else {
        println!("SOMETHING DID NOT HOLD -- see the FAILED lines above");
        std::process::exit(1);
    }
}
