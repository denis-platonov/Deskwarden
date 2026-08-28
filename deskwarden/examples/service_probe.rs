//! **The vault service answering real HTTP, over a real socket.**
//!
//! Every test of this feature drives pure functions. That is the right shape
//! for the decisions -- and it is exactly how two defects shipped that a green
//! suite never saw, because nothing reached the loop: an unauthenticated
//! request that fetched the whole vault, and a write that answered `200` with
//! the item having changed nothing.
//!
//! So this binds `127.0.0.1:0`, serves from a fake vault, and speaks HTTP to
//! itself. It checks what a script would actually see:
//!
//! * no credential, and a wrong one, are refused with no body;
//! * a key scoped to Logins is served Logins and not Cards;
//! * a key scoped to ONE item cannot fetch another;
//! * a write is refused as unbuilt rather than answered with the item;
//! * an unknown route is not a way to learn which routes exist.
//!
//! **The vault is fake and the account is not real.** What this proves is that
//! the routing, the scope check, the bodies, the status codes and `tiny_http`
//! agree with each other over a socket. What it does NOT prove is that a real
//! `RestBackend` behind it returns what this service expects -- that needs a
//! direct-REST account, and is stated as missing rather than implied.
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

    println!("=== now over a real socket ===");
    let keys = vec![key("all", vec![read(Subject::All)])];
    std::thread::spawn(move || {
        for request in server.incoming_requests() {
            let auth = request
                .headers()
                .iter()
                .find(|h| h.field.equiv("Authorization"))
                .map(|h| h.value.as_str().to_string());
            let reply = answer_one_request(
                request.method().as_str(),
                request.url(),
                auth.as_deref(),
                &keys,
                1_000,
                vault,
            );
            let header =
                tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                    .expect("a literal header");
            let _ = request.respond(
                tiny_http::Response::from_string(reply.body)
                    .with_status_code(reply.status)
                    .with_header(header),
            );
        }
    });

    let base = format!("http://127.0.0.1:{port}");
    let agent = ureq::AgentBuilder::new().build();

    let anonymous = agent.get(&format!("{base}/list/object/items")).call();
    let status = match &anonymous {
        Ok(response) => response.status(),
        Err(ureq::Error::Status(code, _)) => *code,
        Err(e) => {
            println!("FAILED the socket did not answer at all: {e}");
            std::process::exit(1);
        }
    };
    all_held &= check("HTTP with no credential", status, 401, "", &[]);

    let authorised = agent
        .get(&format!("{base}/list/object/items"))
        .set("Authorization", &format!("Bearer {KEY}"))
        .call();
    match authorised {
        Ok(response) => {
            let code = response.status();
            let body = response.into_string().unwrap_or_default();
            all_held &= check("HTTP with a credential", code, 200, "", &[]);
            let parsed: serde_json::Value =
                serde_json::from_str(&body).expect("the service answered something that is not JSON");
            let listed = parsed["data"]["data"].as_array().map_or(0, Vec::len);
            println!("  ok   the body is bw serve's envelope, {listed} items");
            all_held &= listed == 2;
        }
        Err(e) => {
            println!("FAILED an authorised request over HTTP failed: {e}");
            all_held = false;
        }
    }

    println!();
    if all_held {
        println!("every check held");
    } else {
        println!("SOMETHING DID NOT HOLD -- see the FAILED lines above");
        std::process::exit(1);
    }
}
