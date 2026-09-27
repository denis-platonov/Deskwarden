//! **A client's line to the vault the daemon owns** -- where it is, the grant
//! that admits this client, and the backend, the sync and the change feed
//! built on them.
//!
//! See `docs/superpowers/specs/2026-09-26-daemon-owns-the-vault-design.md`.
//! The daemon holds the one decrypted vault and answers `crate::vault_api` on
//! loopback; the vault window is the first client of it.
//!
//! # How a window gets one
//!
//! The daemon writes ONE line of JSON down the child's stdin as it starts it
//! -- `{"url": "http://127.0.0.1:<port>", "grant": "<64 hex>"}` -- and closes
//! the pipe. Never on the command line, which every process on the machine
//! can read (`ui_process`'s `no_argument_of_a_ui_spawn_could_be_a_secret`
//! still holds: the argument vector is the same two words), and never in the
//! environment, which is readable too. A process nobody piped anything to
//! reads end-of-file and has no link, and behaves exactly as it did before
//! this existed.
//!
//! # Installed once, read wherever it is needed
//!
//! The window's backend is built from it directly; two things deeper down
//! read it from here -- the Sync the window's heartbeat and Sync pill run
//! (`vault_window`'s `DirectRestTasks::sync`), and the notifications listener
//! (`HubListener::for_this_process`), which follows the daemon's change feed
//! instead of dialling the hub itself.

use std::sync::OnceLock;
use std::time::Duration;

use zeroize::Zeroizing;

use crate::app_match::AppMatch;
use crate::change_feed::{FeedPage, LONG_POLL};
use crate::vault_backend::VaultBackend;
use crate::vault_bridge::{Folder, GenerateRequest, NewItem, VaultBridge, VaultError, VaultItem};
use crate::vault_cache::VaultSnapshot;

/// The connect bound for every request to the daemon: a loopback handshake,
/// and a daemon that is not answering one in this long is not there.
const CONNECT: Duration = Duration::from_secs(3);

/// How long `POST /sync` may take: the daemon re-checking the server, which
/// is a revision question and, when the vault moved, a whole read.
const SYNC_DEADLINE: Duration = Duration::from_secs(120);

/// Where the daemon's vault is, and this client's grant to it.
///
/// **Not `Debug`**: the grant is a credential, and a link that could be
/// printed would be.
pub struct DaemonLink {
    base_url: String,
    grant: Zeroizing<String>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Line {
    url: String,
    grant: String,
}

impl DaemonLink {
    /// The line a daemon writes for a client it starts.
    #[must_use]
    pub fn line(base_url: &str, grant: &str) -> Zeroizing<String> {
        let line = Line { url: base_url.to_string(), grant: grant.to_string() };
        Zeroizing::new(serde_json::to_string(&line).unwrap_or_default() + "\n")
    }

    /// A link out of that line, or `None` for anything that is not one.
    ///
    /// **Loopback or nothing.** The line comes from the process that started
    /// this one, and it is still refused unless it names `127.0.0.1` over
    /// plain HTTP: a link that could name another host is a grant that could
    /// be sent to one.
    #[must_use]
    pub fn from_line(line: &str) -> Option<Self> {
        let Line { url, grant } = serde_json::from_str(line.trim()).ok()?;
        let grant = Zeroizing::new(grant);
        let port = url.strip_prefix("http://127.0.0.1:")?;
        if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) || port.parse::<u16>().is_err() {
            return None;
        }
        if grant.len() != 64 || !grant.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        Some(Self { base_url: url, grant })
    }

    /// The link this process's parent wrote down its stdin, if it wrote one.
    ///
    /// One line and then end-of-file: the daemon closes the pipe as soon as it
    /// has written. A process started any other way has no stdin at all -- the
    /// binary is a windows-subsystem one -- and reads end-of-file at once.
    #[must_use]
    pub fn from_stdin() -> Option<Self> {
        let mut line = Zeroizing::new(String::new());
        std::io::stdin().read_line(&mut line).ok()?;
        Self::from_line(&line)
    }

    /// The daemon's base URL, for a log line. Not a secret: a port on this
    /// machine.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// This client's vault backend: the daemon, asked in `bw serve`'s own
    /// words.
    #[must_use]
    pub fn backend(&self) -> DaemonBackend {
        DaemonBackend { bridge: VaultBridge::with_bearer(self.base_url.clone(), &self.grant) }
    }

    /// Asks the daemon to re-check the server -- the window's Sync, its
    /// heartbeat and its answer to a whole-vault announcement all end here.
    /// Behind the daemon's revision gate, so one row when nothing moved.
    ///
    /// # Errors
    ///
    /// The daemon refused or did not answer, as a sentence for the log and
    /// the Sync pill.
    pub fn sync(&self) -> Result<(), String> {
        let agent = crate::http_agent::bounded_total_with_bearer(CONNECT, SYNC_DEADLINE, self.grant.clone());
        agent
            .post(&format!("{}/sync", self.base_url))
            .call()
            .map(|_| ())
            .map_err(|e| format!("the daemon could not sync the vault: {e}"))
    }

    /// One long-poll of the daemon's change feed, as
    /// [`crate::rest::notifications::HubListener::following`] asks for it.
    #[must_use]
    pub fn feed_poll(&self) -> crate::rest::notifications::FollowPoll {
        // The agent outlives each poll and is bounded above the feed's own
        // long-poll, so a quiet feed answers before the request times out.
        let agent = crate::http_agent::bounded_total_with_bearer(
            CONNECT,
            LONG_POLL + Duration::from_secs(10),
            self.grant.clone(),
        );
        let url = format!("{}/changes", self.base_url);
        Box::new(move |after: Option<u64>, live_seen: bool| -> Result<FeedPage, String> {
            let mut request = agent.get(&url).query("live", if live_seen { "true" } else { "false" });
            if let Some(after) = after {
                request = request.query("after", &after.to_string());
            }
            let body = request
                .call()
                .map_err(|e| e.to_string())?
                .into_string()
                .map_err(|e| e.to_string())?;
            crate::change_feed::page_from_body(&body)
        })
    }
}

static INSTALLED: OnceLock<DaemonLink> = OnceLock::new();

/// Makes `link` this process's line to the daemon. Once per process; a second
/// install is refused and says so.
pub fn install(link: DaemonLink) -> bool {
    INSTALLED.set(link).is_ok()
}

/// This process's line to the daemon, if it was handed one.
#[must_use]
pub fn installed() -> Option<&'static DaemonLink> {
    INSTALLED.get()
}

/// The window's Sync, through the daemon -- or `None` in a process with no
/// link, whose Sync is what it always was.
#[must_use]
pub fn sync_through_the_daemon() -> Option<Result<(), String>> {
    installed().map(DaemonLink::sync)
}

/// A change-feed poll for this process's listener, or `None` with no link.
#[must_use]
pub fn feed_poll() -> Option<crate::rest::notifications::FollowPoll> {
    installed().map(DaemonLink::feed_poll)
}

/// **The window's vault backend when the daemon owns the vault.**
///
/// A [`VaultBridge`] -- `bw serve`'s client, complete and tested -- pointed at
/// the daemon with this client's grant on every request. Every vault
/// operation goes to the daemon, which answers reads out of the vault it holds
/// and makes writes through its own cache; two are answered here instead, and
/// both say why.
pub struct DaemonBackend {
    bridge: VaultBridge,
}

impl VaultBackend for DaemonBackend {
    fn list_items(&self) -> Result<Vec<VaultItem>, VaultError> {
        self.bridge.list_items()
    }
    fn get_item(&self, id: &str) -> Result<VaultItem, VaultError> {
        self.bridge.get_item(id)
    }
    fn list_folders(&self) -> Result<Vec<Folder>, VaultError> {
        self.bridge.list_folders()
    }
    /// Two loopback requests, and no server round trip behind either.
    fn list_vault(&self) -> Result<VaultSnapshot, VaultError> {
        let items = self.bridge.list_items()?;
        let folders = self.bridge.list_folders()?;
        Ok(VaultSnapshot { items, folders })
    }
    fn set_app_match(&self, item: &VaultItem, m: &AppMatch) -> Result<VaultItem, VaultError> {
        self.bridge.set_app_match(item, m)
    }
    fn create_folder(&self, name: &str) -> Result<Folder, VaultError> {
        self.bridge.create_folder(name)
    }
    fn update_folder(&self, id: &str, name: &str) -> Result<Folder, VaultError> {
        self.bridge.update_folder(id, name)
    }
    fn delete_folder(&self, id: &str) -> Result<(), VaultError> {
        self.bridge.delete_folder(id)
    }
    fn create_item(&self, new_item: &NewItem) -> Result<VaultItem, VaultError> {
        self.bridge.create_item(new_item)
    }
    fn update_item(&self, item: &VaultItem) -> Result<VaultItem, VaultError> {
        self.bridge.update_item(item)
    }
    fn move_item_to_folder(
        &self,
        item: &VaultItem,
        folder_id: Option<&str>,
    ) -> Result<VaultItem, VaultError> {
        self.bridge.move_item_to_folder(item, folder_id)
    }
    /// **`true`, and it is the daemon's answer, not `bw serve`'s.** The route
    /// is `bw serve`'s, but what is behind it is the direct-REST backend,
    /// which replaces the whole cipher -- so the explicit `null` folder the
    /// bridge sends for a move out of every folder is honoured. See
    /// `VaultBackend::can_unfile_items`.
    fn can_unfile_items(&self) -> bool {
        true
    }
    fn delete_item(&self, id: &str) -> Result<(), VaultError> {
        self.bridge.delete_item(id)
    }
    fn list_trash(&self) -> Result<Vec<VaultItem>, VaultError> {
        self.bridge.list_trash()
    }
    fn list_archive(&self) -> Result<Vec<VaultItem>, VaultError> {
        self.bridge.list_archive()
    }
    fn archive_item(&self, id: &str) -> Result<(), VaultError> {
        self.bridge.archive_item(id)
    }
    fn unarchive_item(&self, id: &str) -> Result<(), VaultError> {
        self.bridge.unarchive_item(id)
    }
    fn restore_item(&self, id: &str) -> Result<(), VaultError> {
        self.bridge.restore_item(id)
    }
    fn purge_item(&self, id: &str) -> Result<(), VaultError> {
        self.bridge.purge_item(id)
    }
    fn get_totp(&self, id: &str) -> Result<Option<String>, VaultError> {
        self.bridge.get_totp(id)
    }
    /// **Generated here, not at the daemon.** No server has an endpoint for
    /// it and it reads nothing from the vault, so a round trip would carry a
    /// fresh secret over a socket for no reason. The same generator and the
    /// same failure mapping as the direct-REST backend's --
    /// `rest::backend::generate_here`.
    fn generate(&self, request: &GenerateRequest) -> Result<Zeroizing<String>, VaultError> {
        crate::rest::backend::generate_here(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRANT: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    /// **What the daemon writes is what a window reads**, and nothing else is.
    #[test]
    fn a_link_line_reads_back_and_nothing_else_does() {
        let line = DaemonLink::line("http://127.0.0.1:50123", GRANT);
        let link = DaemonLink::from_line(&line).expect("the daemon's own line");
        assert_eq!(link.base_url(), "http://127.0.0.1:50123");

        for refused in [
            // Not loopback: a grant that could be sent to another host.
            DaemonLink::line("http://10.0.0.5:50123", GRANT),
            DaemonLink::line("http://localhost:50123", GRANT),
            DaemonLink::line("https://127.0.0.1:50123", GRANT),
            DaemonLink::line("http://127.0.0.1:50123/path", GRANT),
            DaemonLink::line("http://127.0.0.1:99999", GRANT),
            // Not a grant this daemon mints.
            DaemonLink::line("http://127.0.0.1:50123", "short"),
            DaemonLink::line("http://127.0.0.1:50123", &"z".repeat(64)),
            Zeroizing::new(String::new()),
            Zeroizing::new("not json\n".to_string()),
        ] {
            assert!(DaemonLink::from_line(&refused).is_none(), "{:?} was accepted", refused.as_str());
        }
    }

    /// The daemon's backend can un-file an item, because what answers behind
    /// the `bw serve` route is the direct-REST backend.
    #[test]
    fn the_daemons_backend_can_unfile() {
        let link = DaemonLink::from_line(&DaemonLink::line("http://127.0.0.1:1", GRANT)).expect("link");
        assert!(link.backend().can_unfile_items());
    }
}
