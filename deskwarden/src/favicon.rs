//! Fetches an item's website favicon for the vault window's item list and
//! detail pane. Every failure mode here (no URI, fetch error, decode error)
//! is meant to be swallowed by the caller and fall back to the existing
//! colored-initials monogram (`theme::avatar`) -- a missing icon is not
//! worth an error path, this app already has one perfectly good fallback.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

/// Where to fetch icons from: Bitwarden's own icon service for the default
/// cloud, or the self-hosted server's own icon proxy otherwise. Self-hosted
/// Bitwarden servers proxy icon fetches themselves (`{server}/icons/...`)
/// rather than having the client reach out to a third party directly.
///
/// Detection is by exact host / host-suffix match (the same normalization
/// `login_ui::server_host` uses), not a naive substring check -- a substring
/// check would misclassify an unrelated self-hosted domain like
/// `vault.bitwarden.community` as the default cloud (it contains the
/// substring `bitwarden.com`), silently leaking that user's icon requests to
/// Bitwarden's third-party icon service instead of proxying through their
/// own server.
pub fn icon_base_url(server_url: Option<&str>) -> String {
    match server_url {
        Some(url) if !url.trim().is_empty() && !is_bitwarden_cloud_host(&host_from_url(url)) => {
            format!("{}/icons", url.trim().trim_end_matches('/'))
        }
        _ => "https://icons.bitwarden.net".to_string(),
    }
}

/// Extracts just the host (no scheme, path, query, fragment, or port) from a
/// server URL, e.g. `https://vault.example.com:8443/api` -> `vault.example.com`.
pub fn host_from_url(url: &str) -> String {
    let stripped = url
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let host = stripped.split(['/', '?', '#']).next().unwrap_or(stripped);
    let host = host.split(':').next().unwrap_or(host);
    host.to_string()
}

/// **Which** of Bitwarden's own clouds `host` belongs to -- `"bitwarden.com"`
/// or `"bitwarden.eu"` -- or `None` for anything else, including a self-hosted
/// server.
///
/// The answer is the region's own name rather than the host that was asked
/// about, because `vault.bitwarden.eu` and `bitwarden.eu` are one answer to
/// "which of the three is this account on?" -- which is what the vault window's
/// account menu paints under the address. There are exactly three cases a user
/// can be in (a self-hosted URL, `bitwarden.com`, `bitwarden.eu`) and this
/// tells the last two apart.
///
/// Matched by exact host or host-suffix, never a substring: a substring check
/// classifies the unrelated self-hosted domain `vault.bitwarden.community` as
/// the default cloud, which in [`icon_base_url`] means silently leaking that
/// user's icon requests to a third party and in the account menu means telling
/// them their vault is somewhere it is not. **One host test, two callers** --
/// a second copy is how the two come to disagree, and only one of them has a
/// visible symptom.
pub fn bitwarden_cloud(host: &str) -> Option<&'static str> {
    if host == "bitwarden.com" || host.ends_with(".bitwarden.com") {
        Some("bitwarden.com")
    } else if host == "bitwarden.eu" || host.ends_with(".bitwarden.eu") {
        Some("bitwarden.eu")
    } else {
        None
    }
}

/// True only for `bitwarden.com`/`bitwarden.eu` themselves or a subdomain of
/// one of them -- not for unrelated domains that merely contain those
/// strings as a substring (e.g. `vault.bitwarden.community`).
fn is_bitwarden_cloud_host(host: &str) -> bool {
    bitwarden_cloud(host).is_some()
}

/// Extracts a bare domain (`vault.example.com`, no scheme/path/port) from a
/// login item's stored URI, the same normalization `login_ui::server_host`
/// already does for the login window's server footer.
pub fn domain_from_uri(uri: &str) -> Option<String> {
    let stripped = uri
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let host = stripped.split(['/', '?', '#']).next().unwrap_or(stripped);
    let host = host.split(':').next().unwrap_or(host);
    if host.is_empty() || !host.contains('.') {
        None
    } else {
        Some(host.to_string())
    }
}

/// Extracts a login item's icon target **with its port kept** --
/// `192.168.68.95:8080`, `vault.example.com:8443`, plain `github.com` for a
/// URI that names no port.
///
/// **The port is the whole point, and it is why this is not
/// [`domain_from_uri`].** A service on a home network is almost never on
/// port 80 or 443: `http://192.168.68.95:8080/` is a qBittorrent web UI, and
/// the icon it serves lives at `192.168.68.95:8080`. Drop the port and the
/// fetch goes to whatever answers on port 80 of that address -- for a home
/// router, the router's own admin page -- so the request is not merely less
/// precise, it is aimed at a different service.
///
/// **[`domain_from_uri`] keeps dropping the port, and the two are not a
/// duplicate of each other.** That one answers "which *domain* is this item
/// about", which is what the icon proxy is keyed on -- `{server}/icons/
/// {domain}/icon.png` takes a domain and always has -- and what
/// `app_candidates` matches an executable's publisher against. This one
/// answers "where would this app connect to, to fetch that icon itself",
/// which stopped being the same question the moment the answer became a
/// socket rather than a name. See [`icon_source_for`] for which of the two
/// each path uses.
///
/// **Only `http://` and `https://` are stripped**, exactly as
/// [`domain_from_uri`] strips them. A `androidapp://com.example` or
/// `ftp://files.example.com` URI keeps its scheme text, fails the host check
/// below, and answers `None` -- so no scheme this app does not speak can
/// ever become a host it dials.
///
/// A `:` with no digits after it is **not** treated as a port: it stays part
/// of the host and is then rejected by the same check. That is what keeps
/// `androidapp://com.example` (whose first path-free run is `androidapp:`)
/// from parsing as a host named `androidapp`.
///
/// **The dotted-host rule is [`domain_from_uri`]'s, unchanged, and a bare
/// `localhost` is still rejected here.** Widening it was tried and undone:
/// [`is_private_host`] does answer `true` for `localhost`, so a URI of
/// `localhost` *could* be fetched directly and correctly, but accepting it
/// reverses a pin (`a_login_still_reaches_the_icon_path_through_its_uri`)
/// that says a login with no dotted host gets no icon, and it is not what the
/// port fix is for. A `http://localhost:3000` entry keeps its monogram. That
/// is a real gap and a deliberate one; widening it is a change to which items
/// disclose anything and belongs in its own commit with its own argument, not
/// as a side effect of keeping a port.
pub fn authority_from_uri(uri: &str) -> Option<String> {
    let stripped = uri
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let authority = stripped.split(['/', '?', '#']).next().unwrap_or(stripped);
    let (host, _) = split_authority(authority);
    if host.is_empty() || !host.contains('.') {
        None
    } else {
        Some(authority.to_string())
    }
}

/// Splits `host:port` into its two halves, or `(authority, None)` when there
/// is no port. Only a run of ASCII digits after the **last** `:` counts as a
/// port, so a bare `androidapp:` and an IPv6 literal alike keep their colons
/// and are handled as hosts by the caller.
fn split_authority(authority: &str) -> (&str, Option<&str>) {
    match authority.rsplit_once(':') {
        Some((host, port))
            if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) =>
        {
            (host, Some(port))
        }
        _ => (authority, None),
    }
}

/// The host half of an authority, with any port dropped -- the form the icon
/// **proxy** is keyed on.
pub fn host_of_authority(authority: &str) -> &str {
    split_authority(authority).0
}

/// True for an address that only this machine's own network can reach:
/// RFC1918 private space (`10/8`, `172.16/12`, `192.168/16`), loopback
/// (`127/8`, `localhost`, `::1`), and link-local (`169.254/16`, `fe80::/10`),
/// plus IPv6 unique-local (`fc00::/7`).
///
/// **This is a routing fact, not a preference**, and everything about how the
/// icon for such a host is fetched follows from it. An icon proxy running on
/// the public internet -- Bitwarden's, or a self-hosted server on a Cloudflare
/// Worker -- cannot open a connection to `192.168.68.95`. Not "should not":
/// cannot. There is no configuration of that proxy under which it succeeds,
/// because the address means something different on its network than on this
/// one. So for these hosts the choice is not "proxy or direct", it is "direct
/// or no icon, ever", which is why [`icon_source_for`] answers
/// [`IconSource::Direct`] for them with no setting consulted.
///
/// The ranges are checked with `std`'s own predicates rather than by
/// comparing octets by hand, because the boundaries are exactly where a
/// hand-rolled check goes wrong: `172.32.0.1` is public and `172.16.0.1` is
/// not, `192.169.0.1` is public and `192.168.0.1` is not, and a `starts_with`
/// on the text `"172.16"` also swallows `172.160.0.1`. The tests walk the
/// neighbour on both sides of every boundary.
pub fn is_private_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") || {
        let lower = host.to_ascii_lowercase();
        lower.ends_with(".localhost")
    } {
        return true;
    }
    if let Ok(v4) = host.parse::<std::net::Ipv4Addr>() {
        return v4.is_private() || v4.is_loopback() || v4.is_link_local();
    }
    // An IPv6 literal arrives from a URI wrapped in brackets; strip them so
    // `[::1]` is recognised as the loopback it is rather than failing to
    // parse and being treated as a public name.
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(v6) = bare.parse::<std::net::Ipv6Addr>() {
        let first = v6.segments()[0];
        // `is_unique_local` and `is_unicast_link_local` are still unstable,
        // so the two prefixes are spelled out: `fc00::/7` and `fe80::/10`.
        return v6.is_loopback() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80;
    }
    false
}

/// Where one item's icon is fetched from, and by whom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IconSource {
    /// Ask the icon service -- Bitwarden's, or the self-hosted server's own
    /// `/icons` proxy -- for the icon at this one URL. The proxy makes the
    /// outbound request; this app talks only to a host it already talks to.
    Proxy(String),
    /// Fetch it from the site itself, trying these URLs in order and keeping
    /// the first that decodes. Nothing is proxied and the icon service is not
    /// contacted at all.
    Direct(Vec<String>),
}

/// The paths a direct fetch tries, in order.
///
/// `/favicon.ico` is first because it is the one location every web server
/// answers whether or not anybody configured it -- it is where the qBittorrent
/// web UI this feature exists for keeps its icon. The two PNG paths follow for
/// sites that serve a modern icon and leave `/favicon.ico` as a 404.
///
/// **The list is short on purpose.** Each entry is a request that leaves this
/// machine, and the honest way to find a site's declared icon -- fetch the
/// page, parse its `<link rel="icon">` -- means fetching the page, which
/// discloses considerably more than asking for a fixed path and is a fetch of
/// somebody's HTML by a password manager. Three fixed paths, or nothing.
const DIRECT_ICON_PATHS: [&str; 3] = ["favicon.ico", "favicon.png", "apple-touch-icon.png"];

/// Decides where `authority`'s icon comes from.
///
/// **The two rules, and they are deliberately not the same rule:**
///
/// * A **private** address ([`is_private_host`]) is always fetched directly,
///   with `direct_for_all_hosts` never consulted. The proxy cannot reach it,
///   so there is no second option for a setting to choose between, and the
///   request does not leave the network the user is already on.
/// * Every **other** host is proxied, unless `direct_for_all_hosts`
///   ([`crate::settings::Settings::fetch_icons_direct`]) is on, in which case
///   it too is fetched directly and the proxy is not used at all.
///
/// **Scheme is chosen here rather than taken from the item's URI, and only
/// ever `http` or `https`.** A public host is asked over `https` and is never
/// downgraded: a plaintext request to a host on the internet announces which
/// domain is being asked for to every hop in between, which is the disclosure
/// the setting is already the user's decision about, and doubling it silently
/// is not. A private host is tried over `http` first -- LAN services are
/// overwhelmingly plain HTTP, and that traffic stays on the user's own
/// segment -- and then over `https`, so a NAS or a router that only speaks
/// TLS still gets an icon.
pub fn icon_source_for(
    authority: &str,
    server_url: Option<&str>,
    direct_for_all_hosts: bool,
) -> IconSource {
    let host = host_of_authority(authority);
    let private = is_private_host(host);
    if !private && !direct_for_all_hosts {
        let base = icon_base_url(server_url);
        return IconSource::Proxy(format!("{base}/{host}/icon.png"));
    }
    let schemes: &[&str] = if private { &["http", "https"] } else { &["https"] };
    IconSource::Direct(
        schemes
            .iter()
            .flat_map(|scheme| {
                DIRECT_ICON_PATHS.iter().map(move |path| format!("{scheme}://{authority}/{path}"))
            })
            .collect(),
    )
}

/// The custom field a card's bank domain is stored on.
///
/// Namespaced the way `app_match::APP_MATCH_FIELD_NAME` is, and declared
/// **once**: the reader below and the picker that writes it are in different
/// modules, and two spellings of a field name that must match is a defect
/// that shows up only as a silently blank tile.
pub const BANK_DOMAIN_FIELD: &str = "deskwarden:bank-domain";

/// The domain whose icon represents `item`, or `None` for an item that has no
/// icon of its own and should fall back to the colored-initials monogram.
///
/// **This is the seam between two differently-keyed things.** The UI's icon
/// cache is keyed by *item id*; everything in this module is keyed by
/// *domain*. The only thing standing between them is the question this
/// function answers, which used to be answered inline in the loader for
/// logins and therefore could only ever be answered for logins.
///
/// With the question lifted out, the loader and the item list need no
/// knowledge of cards at all: they keep asking the same question and start
/// getting an answer for a kind of item that previously had none. Fetching,
/// the on-disk cache, the on-screen prefetch window and the monogram fallback
/// all come along for free, because those are properties of the machinery
/// rather than of logins.
///
/// A login answers with [`domain_from_uri`] of its first URI -- exactly what
/// the loader did itself. A card answers with its [`BANK_DOMAIN_FIELD`], if
/// set. Everything else answers `None`.
pub fn icon_domain_for(item: &crate::vault_bridge::VaultItem) -> Option<String> {
    match crate::vault_bridge::ItemKind::of(item) {
        crate::vault_bridge::ItemKind::Login => item
            .login
            .as_ref()
            .and_then(|l| l.uris.first())
            .and_then(|u| u.uri.as_deref())
            .and_then(domain_from_uri),
        crate::vault_bridge::ItemKind::Card => item
            .fields
            .iter()
            .find(|f| f.name.as_deref() == Some(BANK_DOMAIN_FIELD))
            .and_then(|f| f.value.as_deref())
            .map(|v| v.trim())
            .filter(|d| !d.is_empty())
            .map(str::to_string),
        // Listed rather than caught by a `_`, as `ItemKind`'s own doc
        // requires: a type Bitwarden ships later must arrive here as
        // "no icon", not as whatever the arm above it happens to do.
        crate::vault_bridge::ItemKind::SecureNote
        | crate::vault_bridge::ItemKind::Identity
        | crate::vault_bridge::ItemKind::SshKey
        | crate::vault_bridge::ItemKind::Unknown(_) => None,
    }
}

/// [`icon_domain_for`], but answering with the **authority** -- host and port
/// -- for the one kind of item that can carry a port.
///
/// This is what the icon loader keys its cache on and hands to
/// [`icon_source_for`]; [`icon_domain_for`] remains the answer for everything
/// that wants a domain, and the proxy URL that `icon_source_for` builds drops
/// the port back off through [`host_of_authority`]. Two answers to two
/// questions, from one place, rather than a port that survives to one caller
/// by accident.
///
/// A **login** answers with [`authority_from_uri`] of its first URI, so
/// `http://192.168.68.95:8080/` keeps its `:8080`. Every other kind defers to
/// [`icon_domain_for`] verbatim: a card's bank domain is typed into a field
/// by hand and is a domain by construction, and the kinds that have no icon
/// still have none. The arms are listed rather than caught by a `_` for the
/// reason [`icon_domain_for`]'s own are.
pub fn icon_authority_for(item: &crate::vault_bridge::VaultItem) -> Option<String> {
    match crate::vault_bridge::ItemKind::of(item) {
        crate::vault_bridge::ItemKind::Login => item
            .login
            .as_ref()
            .and_then(|l| l.uris.first())
            .and_then(|u| u.uri.as_deref())
            .and_then(authority_from_uri),
        crate::vault_bridge::ItemKind::Card
        | crate::vault_bridge::ItemKind::SecureNote
        | crate::vault_bridge::ItemKind::Identity
        | crate::vault_bridge::ItemKind::SshKey
        | crate::vault_bridge::ItemKind::Unknown(_) => icon_domain_for(item),
    }
}

/// How long to wait for the icon host's TCP handshake.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Total-time bound for one icon fetch.
///
/// An icon is a few kilobytes, so total elapsed time is the right quantity:
/// there is nothing here to stream and no reason to distinguish "slow" from
/// "stalled". The caller's fallback for a failed fetch is the monogram it is
/// already drawing, so waiting longer than this buys nothing.
///
/// It also has to be a *total* bound rather than a per-read one for a second
/// reason: icons are fetched repeatedly against a single host, so pooled
/// connections are the normal case here, and a per-read timeout does not
/// survive pooling. See [`crate::http_agent`].
const REQUEST_DEADLINE: Duration = Duration::from_secs(10);

/// Blocking GET for the icon's raw bytes. Call only from a background
/// thread -- see `vault_window::favicon_loader` for the async wrapper the UI
/// actually uses.
///
/// Bounded on purpose. This used to be one of ureq's bare free functions, with
/// no agent and no timeouts of any kind; because it runs on a detached thread it
/// never froze the UI, but an unreachable icon host leaked that thread and its
/// socket permanently, one per icon. Three such stuck connections to the icon
/// CDN were found alive in a hung v0.3.0 process.
pub fn fetch_icon_bytes(url: &str) -> Option<Vec<u8>> {
    // One shared agent, not one per call: icons are fetched in bursts against
    // a single host, and a fresh agent per call would throw away connection
    // reuse (and open a new TCP+TLS handshake for every icon in the list).
    static AGENT: OnceLock<crate::http_agent::TotalBounded> = OnceLock::new();
    let agent =
        AGENT.get_or_init(|| crate::http_agent::bounded_total(CONNECT_TIMEOUT, REQUEST_DEADLINE));
    let response = agent.get(url).call().ok()?;
    let mut bytes = Vec::new();
    response.into_reader().read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

/// The `User-Agent` every direct icon request carries, and the only thing in
/// that request that this app chose to put there.
///
/// A constant, and deliberately a **bare** one. It names the program and
/// nothing else: not the version, not the operating system, not the HTTP
/// library, not a build id -- so it is byte-identical for every Deskwarden
/// user and adds no bit to what a site can tell about the person asking.
/// ureq's default `User-Agent` names the library and its exact version, which
/// is a fingerprint bit and an inventory of this app's dependencies, handed to
/// every site somebody holds an entry for.
///
/// Naming the app at all is a choice rather than an oversight: a site's
/// operator seeing an unexplained hit on `/favicon.ico` is owed the ability to
/// find out what made it, and `pinned by
/// the_direct_request_head_carries_nothing_beyond_the_allowlist` is what keeps
/// this from growing a version number later.
pub const DIRECT_USER_AGENT: &str = "Deskwarden";

/// How long a direct icon fetch waits for a TCP handshake.
///
/// Shorter than [`CONNECT_TIMEOUT`], and the difference is what is on the
/// other end. The proxy is one known, reachable host. A direct fetch dials a
/// host that may not exist, may not be listening on the scheme being tried,
/// and -- on the private path -- is on the user's own LAN, where a machine
/// that is up answers in milliseconds and one that is not should not hold a
/// thread for five seconds while [`icon_source_for`] still has candidates
/// left to try.
const DIRECT_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// Total-time bound for one direct icon fetch, per candidate URL.
///
/// Shorter than [`REQUEST_DEADLINE`] for the same reason, and because there
/// are up to six candidates: the bound that matters is the one on the whole
/// walk, and six times ten seconds is a thread alive for a minute over an
/// icon.
const DIRECT_REQUEST_DEADLINE: Duration = Duration::from_secs(5);

/// Fetches `source`'s icon bytes -- from the proxy, or from the site itself.
///
/// The two arms are not symmetric, and the asymmetry is the point:
///
/// * [`IconSource::Proxy`] is one URL and the bytes are returned as they
///   arrive, exactly as [`fetch_icon_bytes`] has always returned them. The
///   caller's decode step is the only judge, as before.
/// * [`IconSource::Direct`] is a **list**, and choosing between candidates is
///   a decision only a decode can make: a web server that answers `/favicon.
///   ico` with a `200` and an HTML error page is the ordinary case, not the
///   exotic one, and taking those bytes would cache a page as an icon and
///   stop the walk one candidate short of the real one. So a candidate counts
///   only if it decodes, and the first that does wins.
pub fn fetch_icon_for(source: &IconSource) -> Option<Vec<u8>> {
    match source {
        IconSource::Proxy(url) => fetch_icon_bytes(url),
        IconSource::Direct(urls) => urls.iter().find_map(|url| {
            let bytes = fetch_icon_direct(url)?;
            decode_rgba_unscaled(&bytes).is_some().then_some(bytes)
        }),
    }
}

/// Blocking GET of one direct candidate URL. Call only from a background
/// thread.
///
/// **A separate agent from [`fetch_icon_bytes`]'s, on purpose.** That one
/// talks to a single known host and follows redirects; this one talks to
/// whatever hosts are in the user's vault and must not -- see
/// [`crate::http_agent::bounded_total_plain`] for both refusals. Sharing one
/// agent would mean the icon service's connection settings governing requests
/// to strangers, which is exactly backwards.
///
/// Still **one** shared agent rather than one per call, for the reason the
/// other has: a vault's worth of icons is a burst, and several items commonly
/// share a host.
fn fetch_icon_direct(url: &str) -> Option<Vec<u8>> {
    static AGENT: OnceLock<crate::http_agent::TotalBounded> = OnceLock::new();
    let agent = AGENT.get_or_init(|| {
        crate::http_agent::bounded_total_plain(
            DIRECT_CONNECT_TIMEOUT,
            DIRECT_REQUEST_DEADLINE,
            DIRECT_USER_AGENT,
        )
    });
    let response = agent.get(url).call().ok()?;
    let mut bytes = Vec::new();
    // Bounded by the same rule the on-disk mark reader uses: an icon is a few
    // kilobytes, and a host that answers `/favicon.ico` with a gigabyte is a
    // host that must not be allowed to fill this process's memory.
    response.into_reader().take(MAX_DIRECT_ICON_BYTES).read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

/// The most bytes a direct icon fetch will read from a host it does not
/// know. A 256x256 32-bit ICO is about 270 KB, so this is comfortable room
/// for any real favicon and no room at all for a response that is not one.
const MAX_DIRECT_ICON_BYTES: u64 = 2 * 1024 * 1024;

/// Decodes PNG bytes to (width, height, RGBA8 pixels), normalizing whatever
/// color type/bit depth the source used (indexed, grayscale, RGB without
/// alpha, ...) to straight 8-bit RGBA via `png`'s built-in transformations,
/// so the caller never has to branch on source format.
///
/// This includes indexed-color (palette) PNGs, an extremely common format
/// for small icons/favicons: `Transformations::normalize_to_color8()`
/// includes `EXPAND`, which the `png` crate applies while decoding the
/// frame, so by the time `next_frame` returns, an indexed source has
/// already been expanded to full RGB/RGBA. `info.color_type` below can
/// never actually observe `Indexed` -- see the comment on that match arm.
pub fn decode_rgba(png_bytes: &[u8]) -> Option<(usize, usize, Vec<u8>)> {
    let (width, height, rgba) = decode_rgba_unscaled(png_bytes)?;
    Some(resample_for_display(width, height, rgba))
}

/// The decode half of [`decode_rgba`], **without** the display resampling.
///
/// Split out for [`crate::card_mark`]'s brand marks, which must see the
/// source's own pixels: [`resample_for_display`] letterboxes a non-square
/// image onto a transparent square, and a mark is classified by whether its
/// border is transparent -- so a mark run through the icon path would arrive
/// with a transparent border this app had just added, and a full-bleed logo
/// would be mistaken for an isolated one and cropped.
///
/// One decoder, two callers, and deliberately not a second one: the colour
/// types, bit depths and the indexed-palette expansion are the same problem
/// for a file off disk as for a file off the wire, and the arm-by-arm
/// reasoning above is the part that must not be duplicated. The *bounds* are
/// not shared, because they are not the same question -- see
/// `card_mark::MAX_MARK_BYTES`.
pub fn decode_rgba_unscaled(png_bytes: &[u8]) -> Option<(usize, usize, Vec<u8>)> {
    // **ICO first, because the direct path made it reachable.** The icon
    // proxy has always answered `icon.png` with a PNG, so until now every
    // caller here held one. A direct fetch asks a web server for
    // `/favicon.ico`, and what comes back is a real Windows icon file more
    // often than not -- the qBittorrent web UI this path exists for serves
    // one. A decoder that refused it would have made the whole direct fetch
    // decorative: the request would succeed and the item would still wear a
    // monogram.
    //
    // Dispatched on the file's own magic rather than on the URL's extension,
    // so a `/favicon.ico` that is actually a PNG (common) and a
    // `/favicon.png` that is actually an ICO (less common, still real) both
    // land in the right decoder.
    if let Some(inner) = ico_best_image(png_bytes) {
        return match inner {
            IcoImage::Png(bytes) => decode_rgba_unscaled(bytes),
            IcoImage::Dib { width, height, rgba } => Some((width, height, rgba)),
        };
    }
    let mut decoder = png::Decoder::new(png_bytes);
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    let (width, height) = (info.width as usize, info.height as usize);

    let rgba = match info.color_type {
        png::ColorType::Rgba => buf[..info.buffer_size()].to_vec(),
        png::ColorType::Rgb => buf[..info.buffer_size()]
            .chunks_exact(3)
            .flat_map(|rgb| [rgb[0], rgb[1], rgb[2], 255])
            .collect(),
        png::ColorType::Grayscale => buf[..info.buffer_size()]
            .iter()
            .flat_map(|&g| [g, g, g, 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => buf[..info.buffer_size()]
            .chunks_exact(2)
            .flat_map(|ga| [ga[0], ga[0], ga[0], ga[1]])
            .collect(),
        // Unreachable in practice: `EXPAND` (part of `normalize_to_color8()`
        // set above) makes the `png` crate expand indexed-color frames to
        // RGB/RGBA during decode, so `info.color_type` is never `Indexed`
        // here. Kept only so this match on `png::ColorType` stays
        // exhaustive -- not a real rejection path, and not something to
        // "fix" by adding pre-transformation indexed-PNG detection.
        png::ColorType::Indexed => return None,
    };

    Some((width, height, rgba))
}

/// The one image picked out of a Windows `.ico` container: either a PNG
/// payload to hand back to the PNG decoder, or a decoded Windows DIB.
enum IcoImage<'a> {
    Png(&'a [u8]),
    Dib { width: usize, height: usize, rgba: Vec<u8> },
}

fn u16le(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u32le(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// Reads a `.ico` container and returns its **largest** image, or `None` for
/// bytes that are not an icon file at all.
///
/// `None` is the answer for anything unrecognised rather than an error,
/// because that is what lets [`decode_rgba_unscaled`] dispatch on magic: a
/// PNG, a JPEG, an HTML error page and a truncated download all answer `None`
/// here and fall through to the PNG decoder, which rejects them itself.
///
/// **Type 1 only.** A `.cur` cursor file has the identical layout with a `2`
/// in the type field and hotspot coordinates where the colour planes and bit
/// count go, so decoding one as an icon reads two invented numbers. It is
/// rejected rather than tolerated.
///
/// Every offset and length in the directory is bounds-checked against the
/// buffer before it is used: this parses a file fetched from a host in
/// somebody's vault, so a malformed directory has to be an unfetched icon and
/// never an index out of range.
fn ico_best_image(bytes: &[u8]) -> Option<IcoImage<'_>> {
    if bytes.len() < 6 || u16le(bytes, 0) != 0 || u16le(bytes, 2) != 1 {
        return None;
    }
    let count = u16le(bytes, 4) as usize;
    if count == 0 || bytes.len() < 6 + count * 16 {
        return None;
    }

    // Largest by pixel area, ties broken by colour depth: a 32x32 32-bit
    // entry beside a 32x32 4-bit one is the one worth drawing. A width or
    // height byte of 0 means 256, which is the format's way of fitting 256
    // into a byte and reads as the *smallest* entry if taken literally.
    let mut best: Option<(u64, usize, usize)> = None;
    for i in 0..count {
        let entry = 6 + i * 16;
        let width = if bytes[entry] == 0 { 256u64 } else { bytes[entry] as u64 };
        let height = if bytes[entry + 1] == 0 { 256u64 } else { bytes[entry + 1] as u64 };
        let depth = u16le(bytes, entry + 6) as u64;
        let len = u32le(bytes, entry + 8) as usize;
        let offset = u32le(bytes, entry + 12) as usize;
        if len == 0 || offset.saturating_add(len) > bytes.len() {
            continue;
        }
        let score = width * height * 256 + depth;
        if best.is_none_or(|(best_score, _, _)| score > best_score) {
            best = Some((score, offset, len));
        }
    }

    let (_, offset, len) = best?;
    let payload = &bytes[offset..offset + len];
    // A Vista-era `.ico` stores its larger sizes as whole PNG files inside
    // the container, so this is not an exotic case -- it is the common one
    // for anything 256x256.
    if payload.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        return Some(IcoImage::Png(payload));
    }
    decode_ico_dib(payload)
}

/// Decodes the classic (pre-PNG) icon payload: a `BITMAPINFOHEADER`, an
/// optional palette, bottom-up XOR pixels, then a 1-bit AND transparency
/// mask.
///
/// Supports 32, 24, 8, 4 and 1 bits per pixel, uncompressed. That is every
/// depth a real favicon has ever been written at; RLE-compressed and
/// `BITMAPV5HEADER` payloads answer `None` and the item keeps its monogram,
/// which is the same outcome an unreachable host gets.
///
/// **The AND mask is not optional decoration.** A 24-bit payload has no
/// alpha channel at all, and a 32-bit one is routinely written with every
/// alpha byte zero by tools that expect the mask to be honoured. Ignoring it
/// gives, respectively, an icon with an opaque rectangle of background
/// around it and an icon that is entirely invisible.
fn decode_ico_dib(payload: &[u8]) -> Option<IcoImage<'static>> {
    if payload.len() < 40 || u32le(payload, 0) != 40 || u32le(payload, 16) != 0 {
        return None;
    }
    let width = i32::from_le_bytes(payload[4..8].try_into().ok()?);
    let doubled = i32::from_le_bytes(payload[8..12].try_into().ok()?);
    let bpp = u16le(payload, 14) as usize;
    // `biHeight` in an icon payload counts the XOR rows and the AND rows
    // together, so it is always twice the real height and always even.
    if width <= 0 || width > 256 || doubled <= 0 || doubled % 2 != 0 || doubled > 512 {
        return None;
    }
    let (width, height) = (width as usize, (doubled / 2) as usize);

    let palette_entries = match bpp {
        1 | 4 | 8 => {
            let declared = u32le(payload, 32) as usize;
            if declared == 0 { 1usize << bpp } else { declared.min(1usize << bpp) }
        }
        24 | 32 => 0,
        _ => return None,
    };
    let palette_at = 40;
    let xor_at = palette_at + palette_entries * 4;
    let xor_stride = (width * bpp).div_ceil(32) * 4;
    let mask_stride = width.div_ceil(32) * 4;
    let mask_at = xor_at + xor_stride * height;
    if payload.len() < mask_at {
        return None;
    }
    // A payload that stops before its AND mask is still usable -- the mask is
    // then treated as all-opaque -- but one that stops inside the XOR pixels
    // is not, which is what the check above is.
    let mask = payload.get(mask_at..mask_at + mask_stride * height);

    let mut rgba = vec![0u8; width * height * 4];
    let mut any_alpha = false;
    for y in 0..height {
        // Bottom-up: the first row in the file is the bottom row on screen.
        let row = &payload[xor_at + (height - 1 - y) * xor_stride..][..xor_stride];
        for x in 0..width {
            let (blue, green, red, alpha) = match bpp {
                32 => (row[x * 4], row[x * 4 + 1], row[x * 4 + 2], row[x * 4 + 3]),
                24 => (row[x * 3], row[x * 3 + 1], row[x * 3 + 2], 255),
                _ => {
                    let index = match bpp {
                        8 => row[x] as usize,
                        4 => (row[x / 2] >> if x % 2 == 0 { 4 } else { 0 }) as usize & 0x0f,
                        _ => (row[x / 8] >> (7 - x % 8)) as usize & 0x01,
                    };
                    if index >= palette_entries {
                        return None;
                    }
                    let at = palette_at + index * 4;
                    (payload[at], payload[at + 1], payload[at + 2], 255)
                }
            };
            any_alpha |= alpha != 0;
            let out = (y * width + x) * 4;
            rgba[out] = red;
            rgba[out + 1] = green;
            rgba[out + 2] = blue;
            rgba[out + 3] = alpha;
        }
    }

    // The mask is applied when it is the only transparency information there
    // is (every depth but 32) and when a 32-bit payload's alpha channel is
    // uniformly zero, which means its author left transparency to the mask.
    if bpp != 32 || !any_alpha {
        if let Some(mask) = mask {
            for y in 0..height {
                let row = &mask[(height - 1 - y) * mask_stride..][..mask_stride];
                for x in 0..width {
                    // A set bit means "AND the screen through", i.e. transparent.
                    let transparent = (row[x / 8] >> (7 - x % 8)) & 1 == 1;
                    rgba[(y * width + x) * 4 + 3] = if transparent { 0 } else { 255 };
                }
            }
        } else if bpp == 32 {
            // No mask and no alpha at all: an entirely invisible image is
            // never what was meant, so read the payload as opaque.
            for pixel in rgba.chunks_exact_mut(4) {
                pixel[3] = 255;
            }
        }
    }

    Some(IcoImage::Dib { width, height, rgba })
}

/// The longest edge, in pixels, a decoded icon is reduced to fit inside.
///
/// The item list draws icons at 32 *logical* px. Windows commonly runs at
/// 125%-200% display scaling, so 32 logical px is up to 64 physical px, and
/// 64 is therefore the smallest target that never asks the renderer to
/// magnify on a real monitor. It is deliberately a FIXED constant rather
/// than something derived from the live DPI: the decoded result is shared
/// across every monitor the window can be dragged to, and this module has no
/// `egui` context to read a scale factor from anyway.
const ICON_TARGET_PX: usize = 64;

/// Brings a decoded icon close to the size it is actually drawn at, so the
/// texture the GPU samples is not wildly larger (or smaller) than its
/// on-screen footprint.
///
/// Three rules, each deliberate:
///
/// * **Downscale only.** Sources longer than [`ICON_TARGET_PX`] are reduced
///   by area-averaging ([`box_downscale`]). The real icon cache holds
///   512x512 and 548x548 files drawn at 32 logical px; minifying that far in
///   the renderer's single bilinear tap, with no mipmaps, samples a sparse
///   scattering of source pixels and aliases -- which is what "not sharp"
///   looks like.
/// * **Never upscale.** A 16x16 or 21x21 source is returned byte-for-byte
///   unchanged. Magnifying it here would invent detail and bake in a blur
///   that the renderer would then resample a second time; letting the draw
///   site scale it once is strictly better.
/// * **Pad, do not stretch.** Non-square sources (the cache has a 1121x256
///   and a 32x34) keep their aspect ratio and are centred on a transparent
///   square canvas. Letterboxing is chosen over returning a non-square
///   texture because the draw site fits icons to an exact square: a
///   non-square texture handed to it is *stretched*, and staying correct
///   would require the draw site to cooperate. A square canvas cannot look
///   wrong under either a square fit or an aspect-preserving one.
///
/// Returns straight (non-premultiplied) RGBA, matching what
/// `egui::ColorImage::from_rgba_unmultiplied` expects at every call site.
fn resample_for_display(width: usize, height: usize, rgba: Vec<u8>) -> (usize, usize, Vec<u8>) {
    if width == 0 || height == 0 {
        return (width, height, rgba);
    }

    let longest = width.max(height);
    let (dst_w, dst_h) = if longest > ICON_TARGET_PX {
        let scale = ICON_TARGET_PX as f64 / longest as f64;
        (
            ((width as f64 * scale).round() as usize).max(1),
            ((height as f64 * scale).round() as usize).max(1),
        )
    } else {
        (width, height)
    };

    // A square source at or below the target takes this branch with the
    // original buffer moved straight through: no filtering, no copy.
    let scaled = if (dst_w, dst_h) == (width, height) {
        rgba
    } else {
        box_downscale(&rgba, width, height, dst_w, dst_h)
    };
    if dst_w == dst_h {
        return (dst_w, dst_h, scaled);
    }

    let side = dst_w.max(dst_h);
    (side, side, letterbox(&scaled, dst_w, dst_h, side))
}

/// Area-averaging (box) reduction: every destination pixel is the mean of
/// the whole source rectangle it covers, each source pixel weighted by how
/// much of it falls inside that rectangle. Unlike nearest-neighbour or a
/// single bilinear tap, this reads *every* source pixel exactly once, which
/// is what removes the aliasing on a 512 -> 64 reduction.
///
/// **The averaging is done premultiplied.** A fully transparent pixel still
/// carries RGB bytes -- almost always black -- and averaging straight RGBA
/// across a transparent edge folds that black into the neighbouring opaque
/// colour, ringing the artwork with a dark fringe. Favicons are mostly
/// transparent edge, so this is the failure mode that matters. Colour is
/// therefore accumulated weighted by alpha and divided back out by the alpha
/// sum, which makes transparent pixels contribute to the result's *alpha*
/// and to nothing else. The output is un-premultiplied again on the way out,
/// because that is what the `from_rgba_unmultiplied` call sites want.
///
/// Callers guarantee `dst_w <= src_w` and `dst_h <= src_h`.
///
/// `pub(crate)` for [`crate::card_mark`], which reduces a brand mark to the
/// size it is drawn at for exactly the reason above and must not grow a
/// second, worse reduction of its own.
pub(crate) fn box_downscale(src: &[u8], src_w: usize, src_h: usize, dst_w: usize, dst_h: usize) -> Vec<u8> {
    let mut out = vec![0u8; dst_w * dst_h * 4];
    let x_ratio = src_w as f64 / dst_w as f64;
    let y_ratio = src_h as f64 / dst_h as f64;

    for dy in 0..dst_h {
        let (y0, y1) = (dy as f64 * y_ratio, (dy as f64 + 1.0) * y_ratio);
        for dx in 0..dst_w {
            let (x0, x1) = (dx as f64 * x_ratio, (dx as f64 + 1.0) * x_ratio);
            let (mut weight, mut alpha) = (0.0f64, 0.0f64);
            let (mut red, mut green, mut blue) = (0.0f64, 0.0f64, 0.0f64);

            for sy in y0 as usize..(y1.ceil() as usize).min(src_h) {
                let cover_y = (y1.min(sy as f64 + 1.0) - y0.max(sy as f64)).max(0.0);
                for sx in x0 as usize..(x1.ceil() as usize).min(src_w) {
                    let cover = cover_y * (x1.min(sx as f64 + 1.0) - x0.max(sx as f64)).max(0.0);
                    let i = (sy * src_w + sx) * 4;
                    let a = cover * (src[i + 3] as f64 / 255.0);
                    weight += cover;
                    alpha += a;
                    red += a * src[i] as f64;
                    green += a * src[i + 1] as f64;
                    blue += a * src[i + 2] as f64;
                }
            }

            // An entirely transparent destination pixel has no colour to
            // recover -- leaving the quad at zero is the honest answer, and
            // dividing by `alpha` there would be a division by zero.
            if alpha > 0.0 && weight > 0.0 {
                let o = (dy * dst_w + dx) * 4;
                out[o] = (red / alpha).round().clamp(0.0, 255.0) as u8;
                out[o + 1] = (green / alpha).round().clamp(0.0, 255.0) as u8;
                out[o + 2] = (blue / alpha).round().clamp(0.0, 255.0) as u8;
                out[o + 3] = (alpha / weight * 255.0).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

/// Centres a `width` x `height` image on a fully transparent `side` x `side`
/// canvas. The padding is `[0, 0, 0, 0]` and is never averaged with the
/// content -- it is written around an already-resampled image, not filtered
/// across -- so it cannot introduce the dark fringe `box_downscale` guards
/// against.
fn letterbox(content: &[u8], width: usize, height: usize, side: usize) -> Vec<u8> {
    let mut out = vec![0u8; side * side * 4];
    let (off_x, off_y) = ((side - width) / 2, (side - height) / 2);
    for y in 0..height {
        let from = y * width * 4;
        let to = ((y + off_y) * side + off_x) * 4;
        out[to..to + width * 4].copy_from_slice(&content[from..from + width * 4]);
    }
    out
}

/// Reads a previously-cached icon for `domain` from disk, if one exists.
/// `cache_dir` is a directory the caller owns (created if needed by
/// `write_cached_icon`) -- this function does not create it, just reads.
pub fn read_cached_icon(cache_dir: &Path, domain: &str) -> Option<Vec<u8>> {
    std::fs::read(icon_cache_path(cache_dir, domain)).ok()
}

/// Writes `png_bytes` to the on-disk cache for `domain`. Best-effort: a
/// failure to persist (permissions, disk full, whatever) just means this
/// domain gets re-fetched next time rather than being treated as fatal --
/// this is a cache, not a source of truth.
pub fn write_cached_icon(cache_dir: &Path, domain: &str, png_bytes: &[u8]) {
    let path = icon_cache_path(cache_dir, domain);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, png_bytes);
}

/// Builds a cache-file path for `domain`, sanitizing it into a safe file
/// name first. `domain_from_uri` already strips scheme/path/port, so this
/// is normally just alphanumerics/dots/hyphens already -- but treat that as
/// a property to defend, not a guarantee, since this ends up as an actual
/// file path on disk.
fn icon_cache_path(cache_dir: &Path, domain: &str) -> PathBuf {
    let safe: String = domain
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' })
        .collect();
    cache_dir.join(format!("{safe}.png"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_base_defaults_to_bitwardens_cloud_service() {
        assert_eq!(icon_base_url(None), "https://icons.bitwarden.net");
        assert_eq!(icon_base_url(Some("")), "https://icons.bitwarden.net");
        assert_eq!(
            icon_base_url(Some("https://vault.bitwarden.com")),
            "https://icons.bitwarden.net"
        );
    }

    #[test]
    fn icon_base_proxies_through_a_self_hosted_server() {
        assert_eq!(
            icon_base_url(Some("https://vault.example.eu/")),
            "https://vault.example.eu/icons"
        );
    }

    #[test]
    fn icon_base_does_not_mistake_a_substring_match_for_the_bitwarden_cloud() {
        // "vault.bitwarden.community" contains the substring "bitwarden.com"
        // (from "...bitwarden.COMmunity") but is an unrelated self-hosted
        // domain, not a Bitwarden cloud subdomain -- it must be proxied
        // through its own server, not routed to icons.bitwarden.net.
        assert_eq!(
            icon_base_url(Some("https://vault.bitwarden.community")),
            "https://vault.bitwarden.community/icons"
        );
    }

    #[test]
    fn domain_strips_scheme_path_and_port() {
        assert_eq!(
            domain_from_uri("https://app.ledgerline.com/login?x=1"),
            Some("app.ledgerline.com".to_string())
        );
    }

    #[test]
    fn domain_strips_a_port_when_present() {
        // A bare IP with no dot would be rejected by the dotted-host check,
        // so this uses a real hostname with a port instead.
        assert_eq!(
            domain_from_uri("https://vault.example.com:8443/x"),
            Some("vault.example.com".to_string())
        );
    }

    #[test]
    fn domain_rejects_uris_with_no_dotted_host() {
        assert_eq!(domain_from_uri("localhost"), None);
        assert_eq!(domain_from_uri(""), None);
    }

    /// Builds a `VaultItem` from wire JSON, so these fixtures exercise the
    /// same deserialization the real vault snapshot goes through rather than
    /// a hand-assembled struct that could drift from it.
    fn item_from_json(json: &str) -> crate::vault_bridge::VaultItem {
        serde_json::from_str(json).expect("fixture item parses")
    }

    fn login_with_uri(uri: &str) -> crate::vault_bridge::VaultItem {
        item_from_json(&format!(
            r#"{{"id":"i1","name":"Login","type":1,"login":{{"uris":[{{"uri":"{uri}"}}]}}}}"#
        ))
    }

    /// A `type: 3` card carrying a real number and one custom field.
    fn card_with_field(name: &str, value: &str) -> crate::vault_bridge::VaultItem {
        item_from_json(&format!(
            r#"{{"id":"c1","name":"Card","type":3,"card":{{"number":"4111111111111111"}},
                 "fields":[{{"name":"{name}","value":"{value}"}}]}}"#
        ))
    }

    fn plain_card() -> crate::vault_bridge::VaultItem {
        item_from_json(
            r#"{"id":"c2","name":"Card","type":3,"card":{"number":"4111111111111111"}}"#,
        )
    }

    fn secure_note() -> crate::vault_bridge::VaultItem {
        item_from_json(r#"{"id":"n1","name":"Note","type":2}"#)
    }

    #[test]
    fn a_login_answers_with_its_uris_domain_exactly_as_before() {
        let item = login_with_uri("https://github.com/login");
        assert_eq!(icon_domain_for(&item).as_deref(), Some("github.com"));
        // The loader used to call `domain_from_uri` on this URI itself; the
        // lifted function must be the same answer, not merely *an* answer.
        assert_eq!(
            icon_domain_for(&item),
            domain_from_uri("https://github.com/login")
        );
    }

    #[test]
    fn a_login_with_no_usable_uri_still_answers_nothing() {
        // Both of the loader's old bail-outs: no URI at all, and a URI with
        // no dotted host. Neither may start producing a domain.
        assert_eq!(icon_domain_for(&item_from_json(r#"{"id":"l","name":"L","type":1}"#)), None);
        assert_eq!(icon_domain_for(&login_with_uri("localhost")), None);
    }

    #[test]
    fn a_card_answers_with_its_bank_domain_field() {
        let item = card_with_field(BANK_DOMAIN_FIELD, "chase.com");
        assert_eq!(icon_domain_for(&item).as_deref(), Some("chase.com"));
    }

    #[test]
    fn a_card_without_the_field_has_no_icon_of_its_own() {
        let item = plain_card();
        assert_eq!(icon_domain_for(&item), None);
        // Control: the fixture is a real card with a number, so `None` is
        // about the missing field and not about an empty item.
        assert!(item.card.as_ref().expect("fixture is a card").number.is_some());
    }

    #[test]
    fn a_card_whose_bank_domain_is_blank_is_the_same_as_not_having_one() {
        // The picker writing an empty string, or `bw` round-tripping one, is
        // a card with no bank -- not a fetch of `https://.../ /icon.png`.
        let item = card_with_field(BANK_DOMAIN_FIELD, "   ");
        assert_eq!(icon_domain_for(&item), None);
        // Control: the same fixture shape with a real value does answer, so
        // this is about the blank and not about the fixture.
        assert_eq!(
            icon_domain_for(&card_with_field(BANK_DOMAIN_FIELD, "chase.com")).as_deref(),
            Some("chase.com")
        );
    }

    #[test]
    fn a_card_reads_only_its_own_field_name() {
        // A card carrying some *other* custom field must not have that value
        // treated as a domain -- a hidden field's value would then be fetched
        // as a URL path segment.
        let item = card_with_field("Security question", "chase.com");
        assert_eq!(icon_domain_for(&item), None);
        assert_eq!(BANK_DOMAIN_FIELD, "deskwarden:bank-domain");
    }

    #[test]
    fn a_secure_note_has_no_icon() {
        assert_eq!(icon_domain_for(&secure_note()), None);
    }

    #[test]
    fn the_kinds_that_have_no_icon_of_their_own_all_answer_none() {
        // Identity, SSH key and a type Bitwarden has not shipped yet. Driven
        // as a list so a new kind cannot quietly inherit the login arm.
        for ty in [2, 4, 5, 6] {
            let item = item_from_json(&format!(r#"{{"id":"x","name":"X","type":{ty}}}"#));
            assert_eq!(icon_domain_for(&item), None, "type {ty}");
        }
        // Control: the loop is not passing because every item answers None.
        assert!(icon_domain_for(&login_with_uri("https://chase.com")).is_some());
    }

    /// Encodes straight (non-premultiplied) RGBA8 pixels as a PNG, so the
    /// resampling tests below can drive the real `decode_rgba` entry point
    /// rather than reaching into a private helper.
    fn rgba_png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("png header");
            writer.write_image_data(rgba).expect("png pixel data");
        }
        out
    }

    /// The RGBA quad at `(x, y)` of a `width`-wide straight-RGBA buffer.
    fn pixel_at(rgba: &[u8], width: usize, x: usize, y: usize) -> [u8; 4] {
        let i = (y * width + x) * 4;
        [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
    }

    #[test]
    fn a_large_icon_is_reduced_to_the_display_target() {
        // The real cache holds 512x512, 548x548 and 540x540 sources, all
        // drawn at 32 logical px. Minifying that far in one bilinear tap at
        // draw time is what makes them shimmer; the reduction happens here.
        let src = vec![200u8; 512 * 512 * 4];
        let (w, h, out) = decode_rgba(&rgba_png(512, 512, &src)).expect("decodes");

        assert_eq!((w, h), (64, 64));
        assert_eq!(out.len(), 64 * 64 * 4);
    }

    #[test]
    fn a_square_icon_at_or_below_the_target_is_left_completely_untouched() {
        // Upscaling invents detail. A 32x32 or 16x16 source is handed to the
        // renderer exactly as decoded, byte for byte.
        let src: Vec<u8> = (0..32 * 32 * 4).map(|i| (i % 251) as u8).collect();
        let (w, h, out) = decode_rgba(&rgba_png(32, 32, &src)).expect("decodes");
        assert_eq!((w, h), (32, 32));
        assert_eq!(out, src, "a 32x32 source must survive decode unchanged");

        let small: Vec<u8> = (0..16 * 16 * 4).map(|i| (i % 253) as u8).collect();
        let (w, h, out) = decode_rgba(&rgba_png(16, 16, &small)).expect("decodes");
        assert_eq!((w, h), (16, 16));
        assert_eq!(out, small, "a 16x16 source must not be magnified here");
    }

    #[test]
    fn a_wide_icon_is_letterboxed_into_a_square_rather_than_stretched() {
        // 1121x256 is a real file in the user's cache. The draw site fits the
        // texture to an exact square, so anything non-square arriving there
        // is stretched -- padding to a square here makes that fit correct
        // without the draw site having to change.
        let src = vec![255u8; 1121 * 256 * 4];
        let (w, h, out) = decode_rgba(&rgba_png(1121, 256, &src)).expect("decodes");

        assert_eq!((w, h), (64, 64), "a wide source must land on a square canvas");
        // 256 * 64/1121 rounds to 15 content rows, centred at offset 24.
        assert_eq!(pixel_at(&out, 64, 32, 0)[3], 0, "top padding must be transparent");
        assert_eq!(pixel_at(&out, 64, 32, 63)[3], 0, "bottom padding must be transparent");
        assert_eq!(pixel_at(&out, 64, 32, 31)[3], 255, "the content band must survive");
    }

    #[test]
    fn a_non_square_icon_below_the_target_is_padded_rather_than_stretched() {
        // 32x34 is also a real cache file: too small to downscale, but still
        // non-square, so it still needs the square canvas.
        let src = vec![255u8; 32 * 34 * 4];
        let (w, h, out) = decode_rgba(&rgba_png(32, 34, &src)).expect("decodes");

        assert_eq!((w, h), (34, 34), "the canvas is the longer side, not the target");
        assert_eq!(pixel_at(&out, 34, 0, 17)[3], 0, "left padding must be transparent");
        assert_eq!(pixel_at(&out, 34, 17, 17)[3], 255, "the content must survive");
    }

    #[test]
    fn averaging_across_a_transparent_edge_does_not_tint_the_opaque_pixels() {
        // Favicons are full of transparent edges. Averaging STRAIGHT RGBA
        // across such an edge pulls in the colour stored under fully
        // transparent pixels -- usually black -- and rings the artwork with a
        // dark fringe. The average has to happen premultiplied.
        //
        // 128 -> 64 halves exactly, so each destination pixel covers two
        // source columns. Making the split 63 columns wide (not 64) puts the
        // edge INSIDE destination column 31, which therefore averages one
        // opaque red pixel with one fully transparent black one.
        let mut src = vec![0u8; 128 * 128 * 4];
        for y in 0..128 {
            for x in 0..63 {
                let i = (y * 128 + x) * 4;
                src[i] = 255;
                src[i + 3] = 255;
            }
        }
        let (w, h, out) = decode_rgba(&rgba_png(128, 128, &src)).expect("decodes");
        assert_eq!((w, h), (64, 64));

        let interior = pixel_at(&out, 64, 0, 32);
        assert_eq!(interior, [255, 0, 0, 255], "a fully covered pixel must stay pure red");

        let edge = pixel_at(&out, 64, 31, 32);
        assert_eq!(
            [edge[0], edge[1], edge[2]],
            [255, 0, 0],
            "the edge pixel's colour was pulled toward the transparent side: {edge:?} -- \
             this is the straight-RGBA average mistake; average premultiplied instead"
        );
        assert!(
            (100..=155).contains(&edge[3]),
            "the edge pixel should be about half transparent, got alpha {}",
            edge[3]
        );
    }

    fn unique_cache_dir(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "deskwarden-test-favicon-cache-{label}-{}",
            std::process::id()
        ))
    }

    #[test]
    fn cached_icon_round_trips_through_disk() {
        let dir = unique_cache_dir("round-trip");
        let bytes = vec![1u8, 2, 3, 4, 5];

        write_cached_icon(&dir, "app.ledgerline.com", &bytes);
        assert_eq!(read_cached_icon(&dir, "app.ledgerline.com"), Some(bytes));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn reading_an_uncached_domain_returns_none() {
        let dir = unique_cache_dir("miss");
        assert_eq!(read_cached_icon(&dir, "never-written.example.com"), None);

        std::fs::remove_dir_all(&dir).ok();
    }

    // -----------------------------------------------------------------
    // The port, and where it does and does not survive
    // -----------------------------------------------------------------

    /// **The bug this whole change exists for**, as the smallest test that
    /// shows it: the owner's qBittorrent web UI is on `192.168.68.95:8080`
    /// and Deskwarden drew no icon for it.
    #[test]
    fn a_lan_uri_keeps_its_port_all_the_way_into_the_fetch_url() {
        assert_eq!(
            authority_from_uri("http://192.168.68.95:8080/"),
            Some("192.168.68.95:8080".to_string())
        );
        let source = icon_source_for("192.168.68.95:8080", None, false);
        let IconSource::Direct(urls) = &source else {
            panic!("a private address must be fetched directly, got {source:?}");
        };
        assert!(
            urls.iter().all(|u| u.contains("192.168.68.95:8080")),
            "a candidate URL dropped the port, so the fetch goes to port 80 of that address \
             -- a different service entirely: {urls:?}"
        );
        // The positive control for the assertion above: it is checking a
        // non-empty list, and the port really is the thing under test rather
        // than a substring that would be there anyway.
        assert!(!urls.is_empty(), "no candidate URLs at all");
        assert!(
            !urls.iter().any(|u| u.contains("192.168.68.95/")),
            "a candidate URL is aimed at the bare address as well: {urls:?}"
        );
    }

    /// **And where the port must NOT survive.** The icon proxy takes a bare
    /// domain -- `{server}/icons/{domain}/icon.png` always has -- so the
    /// proxied path drops it again. Both halves in one test, because "kept
    /// here, dropped there" is the claim and either half alone is half a
    /// claim.
    #[test]
    fn the_proxy_is_still_asked_for_the_bare_host_with_no_port() {
        let source = icon_source_for("vault.example.com:8443", Some("https://vault.example.eu"), false);
        assert_eq!(
            source,
            IconSource::Proxy("https://vault.example.eu/icons/vault.example.com/icon.png".to_string()),
            "the proxy URL carried a port; the icon service has never taken one"
        );
        // The control: the same authority, fetched directly, DOES keep it --
        // so the assertion above is about the proxy path rather than about
        // `host_of_authority` having eaten the port for everybody.
        let direct = icon_source_for("vault.example.com:8443", Some("https://vault.example.eu"), true);
        let IconSource::Direct(urls) = &direct else { panic!("expected direct, got {direct:?}") };
        assert!(
            urls.iter().all(|u| u.contains("vault.example.com:8443")),
            "the direct path dropped the port too, so this test is not about the proxy: {urls:?}"
        );
    }

    /// `host_from_url` and `domain_from_uri` are deliberately unchanged: they
    /// answer "which domain", and their callers -- the cloud check, the
    /// `send_link` origin comparison, `app_candidates`' executable matching --
    /// all want a bare host. Pinned so a later "fix the port everywhere" pass
    /// has to read this rather than discover it.
    #[test]
    fn the_two_domain_functions_still_drop_the_port_on_purpose() {
        assert_eq!(host_from_url("https://vault.example.com:8443/api"), "vault.example.com");
        assert_eq!(
            domain_from_uri("http://192.168.68.95:8080/"),
            Some("192.168.68.95".to_string()),
            "`domain_from_uri` started keeping the port, which changes what the icon PROXY is \
             asked for and what `app_candidates` matches an executable against"
        );
    }

    #[test]
    fn an_authority_without_a_port_is_exactly_the_domain() {
        // The overwhelmingly common case, and the one that keeps every
        // existing user's on-disk icon cache valid: the loader keys the cache
        // on this string, so a `github.com` that started answering
        // `https://github.com` would re-fetch every icon anybody has.
        assert_eq!(authority_from_uri("https://github.com/login"), Some("github.com".to_string()));
        assert_eq!(authority_from_uri("https://github.com/login"), domain_from_uri("https://github.com/login"));
    }

    #[test]
    fn a_scheme_this_app_does_not_speak_never_becomes_a_host() {
        // Only `http://` and `https://` are stripped, so the scheme text of
        // anything else stays in the host and is rejected. An
        // `androidapp://com.example` URI is ordinary in a real vault.
        assert_eq!(authority_from_uri("androidapp://com.example.app"), None);
        assert_eq!(authority_from_uri("ftp://files.example.com/x"), None);
        assert_eq!(authority_from_uri(""), None);
        // The dotted-host rule is `domain_from_uri`'s, kept: a bare
        // `localhost` URI is still no icon, although `is_private_host` would
        // route one directly if it got that far. See `authority_from_uri`'s
        // own doc for why that gap is deliberate.
        assert_eq!(authority_from_uri("localhost"), None);
        assert_eq!(authority_from_uri("http://localhost:3000/"), None);
        assert!(is_private_host("localhost"), "the control: the predicate does know it");
        // The control: the same shape WITH a scheme this app does speak is
        // accepted, so the rejections above are about the scheme.
        assert_eq!(
            authority_from_uri("https://files.example.com/x"),
            Some("files.example.com".to_string())
        );
    }

    #[test]
    fn a_colon_that_is_not_a_port_is_not_treated_as_one() {
        assert_eq!(split_authority("example.com:8080"), ("example.com", Some("8080")));
        assert_eq!(split_authority("example.com"), ("example.com", None));
        assert_eq!(split_authority("example.com:"), ("example.com:", None));
        assert_eq!(split_authority("example.com:https"), ("example.com:https", None));
        assert_eq!(host_of_authority("192.168.68.95:8080"), "192.168.68.95");
        assert_eq!(host_of_authority("192.168.68.95"), "192.168.68.95");
    }

    // -----------------------------------------------------------------
    // Private-address detection, at every boundary
    // -----------------------------------------------------------------

    /// **The off-by-one neighbours are the test.** Every one of these ranges
    /// has a public address immediately outside it, and every hand-rolled
    /// version of this check gets at least one of them wrong -- most often
    /// `172.32.0.0`, which a `starts_with("172.")` swallows, and
    /// `172.160.0.1`, which a `starts_with("172.16")` swallows.
    #[test]
    fn every_private_range_is_matched_and_its_public_neighbours_are_not() {
        let private = [
            // 10/8
            "10.0.0.0", "10.255.255.255", "10.0.0.1",
            // 172.16/12
            "172.16.0.0", "172.31.255.255", "172.16.0.1",
            // 192.168/16
            "192.168.0.0", "192.168.255.255", "192.168.68.95",
            // 127/8 and its name
            "127.0.0.1", "127.255.255.255", "localhost", "LOCALHOST", "box.localhost",
            // 169.254/16
            "169.254.0.0", "169.254.255.255", "169.254.1.1",
            // and the IPv6 spellings of the same three ideas
            "::1", "[::1]", "fc00::1", "fe80::1",
        ];
        for host in private {
            assert!(is_private_host(host), "{host} is on this machine's own network and was not matched");
        }

        let public = [
            // The neighbour on each side of each boundary, which is what a
            // sloppy prefix check gets wrong.
            "9.255.255.255", "11.0.0.1",
            "172.15.255.255", "172.32.0.1", "172.160.0.1",
            "192.167.255.255", "192.169.0.1", "192.1680.0.1",
            "126.255.255.255", "128.0.0.1",
            "169.253.255.255", "169.255.0.1",
            // and ordinary public hosts, including two whose TEXT contains a
            // private range's text.
            "github.com", "icons.bitwarden.net", "10.0.0.1.example.com",
            "localhost.example.com", "notlocalhost", "192.168.0.1.nip.io",
            "2001:4860:4860::8888",
        ];
        for host in public {
            assert!(!is_private_host(host), "{host} is a public address and was matched as private");
        }

        // The instrument: both lists were non-empty and the predicate is not
        // a constant. Without this, a function that answered `true` for
        // everything would fail the second loop -- but one that answered
        // nothing at all would need the first loop to have run.
        assert!(private.len() >= 20 && public.len() >= 15);
    }

    // -----------------------------------------------------------------
    // Which of the two paths a host takes
    // -----------------------------------------------------------------

    /// **The positive control for the whole switch.** With the switch off, a
    /// public host must still go to the proxy -- otherwise a test that says
    /// "private goes direct" proves nothing, because everything would.
    #[test]
    fn with_the_switch_off_a_public_host_still_goes_to_the_proxy() {
        assert_eq!(
            icon_source_for("github.com", None, false),
            IconSource::Proxy("https://icons.bitwarden.net/github.com/icon.png".to_string()),
            "a public host stopped being proxied with the switch OFF, which is the behaviour \
             every existing user has and did not ask to change"
        );
        assert_eq!(
            icon_source_for("github.com", Some("https://vault.example.eu/"), false),
            IconSource::Proxy("https://vault.example.eu/icons/github.com/icon.png".to_string()),
            "a self-hosted account stopped proxying through its own server"
        );
        // The control: the SAME host with the switch on does leave the proxy,
        // so the two assertions above are about the switch and not about
        // `icon_source_for` being unable to answer `Direct` at all.
        assert!(matches!(
            icon_source_for("github.com", None, true),
            IconSource::Direct(_)
        ));
    }

    /// A private address is fetched directly **with the switch off**, because
    /// the proxy cannot reach it however it is configured.
    #[test]
    fn a_private_address_is_fetched_directly_with_the_switch_off() {
        for authority in ["192.168.68.95:8080", "10.1.2.3", "127.0.0.1:8080", "localhost"] {
            assert!(
                matches!(icon_source_for(authority, None, false), IconSource::Direct(_)),
                "{authority} was sent to the icon proxy, which has no route to it -- so the \
                 item gets no icon, ever"
            );
            // ... and the switch makes no difference to it, which is the
            // other half of the claim: this is not the switch defaulting on.
            assert_eq!(
                icon_source_for(authority, None, false),
                icon_source_for(authority, None, true),
                "the direct-fetch switch changed what happens to a private address"
            );
        }
    }

    /// The scheme is chosen here, not taken from the item's URI, and a public
    /// host is never asked over plaintext.
    #[test]
    fn a_public_direct_fetch_is_https_only_and_a_private_one_tries_http_first() {
        let IconSource::Direct(public) = icon_source_for("github.com", None, true) else {
            panic!("expected direct")
        };
        assert!(!public.is_empty());
        assert!(
            public.iter().all(|u| u.starts_with("https://")),
            "a public host was asked over plaintext, which announces the domain to every hop \
             in between: {public:?}"
        );

        let IconSource::Direct(private) = icon_source_for("192.168.68.95:8080", None, false) else {
            panic!("expected direct")
        };
        assert!(
            private[0].starts_with("http://192.168.68.95:8080/"),
            "a LAN service is tried over plaintext first; got {:?}",
            private[0]
        );
        assert!(
            private.iter().any(|u| u.starts_with("https://")),
            "a LAN box that only speaks TLS gets no icon at all: {private:?}"
        );
        // Every candidate, on either path, is `http` or `https` and nothing
        // else -- the blunt form of "the scheme is ours".
        for url in public.iter().chain(private.iter()) {
            assert!(
                url.starts_with("http://") || url.starts_with("https://"),
                "a candidate URL is not an http(s) URL: {url:?}"
            );
        }
    }

    #[test]
    fn a_login_answers_with_its_port_and_a_card_answers_exactly_as_before() {
        let lan = login_with_uri("http://192.168.68.95:8080/");
        assert_eq!(icon_authority_for(&lan).as_deref(), Some("192.168.68.95:8080"));
        // The card path is untouched and must stay so: a bank domain is typed
        // into a field by hand and is a domain, not a socket.
        let card = card_with_field(BANK_DOMAIN_FIELD, "chase.com");
        assert_eq!(icon_authority_for(&card), icon_domain_for(&card));
        assert_eq!(icon_authority_for(&card).as_deref(), Some("chase.com"));
        assert_eq!(icon_authority_for(&secure_note()), None);
        assert_eq!(icon_authority_for(&plain_card()), None);
        // The control for the first assertion: `icon_domain_for` on the same
        // login gives the port-less answer, so the two really are different
        // questions and the login arm really did change.
        assert_eq!(icon_domain_for(&lan).as_deref(), Some("192.168.68.95"));
    }

    // -----------------------------------------------------------------
    // ICO decoding -- what a direct `/favicon.ico` actually returns
    // -----------------------------------------------------------------

    /// Wraps `payload` in a single-entry `.ico` container.
    fn ico_of(width: u8, height: u8, bpp: u16, payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&0u16.to_le_bytes()); // reserved
        out.extend_from_slice(&1u16.to_le_bytes()); // type: icon
        out.extend_from_slice(&1u16.to_le_bytes()); // one entry
        out.push(width);
        out.push(height);
        out.push(0); // palette size
        out.push(0); // reserved
        out.extend_from_slice(&1u16.to_le_bytes()); // colour planes
        out.extend_from_slice(&bpp.to_le_bytes());
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(&22u32.to_le_bytes()); // offset: right after this entry
        out.extend_from_slice(payload);
        out
    }

    /// A 2x2 32-bit uncompressed icon payload: header, bottom-up BGRA, then
    /// an all-clear AND mask.
    fn dib_2x2_bgra(pixels: [[u8; 4]; 4]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&40u32.to_le_bytes()); // biSize
        out.extend_from_slice(&2i32.to_le_bytes()); // biWidth
        out.extend_from_slice(&4i32.to_le_bytes()); // biHeight: 2x the real height
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bit count
        out.extend_from_slice(&0u32.to_le_bytes()); // compression: none
        out.extend_from_slice(&[0u8; 20]); // sizes, resolutions, palette counts
        for pixel in pixels {
            out.extend_from_slice(&pixel);
        }
        out.extend_from_slice(&[0u8; 8]); // AND mask: two rows, 4 bytes each
        out
    }

    /// The direct path asks for `/favicon.ico` first, and what a web server
    /// answers with is a Windows icon file. A decoder that took only PNGs
    /// would have made the whole direct fetch decorative: the request would
    /// succeed and the row would still wear a monogram.
    #[test]
    fn a_windows_icon_file_decodes_to_the_pixels_it_carries() {
        // Bottom-up in the file, so the first two entries are the BOTTOM row.
        let bytes = ico_of(
            2,
            2,
            32,
            &dib_2x2_bgra([
                [0x00, 0x00, 0xff, 0xff], // bottom-left:  red
                [0x00, 0xff, 0x00, 0xff], // bottom-right: green
                [0xff, 0x00, 0x00, 0xff], // top-left:     blue
                [0xff, 0xff, 0xff, 0xff], // top-right:    white
            ]),
        );
        let (width, height, rgba) = decode_rgba_unscaled(&bytes).expect("the ICO decodes");
        assert_eq!((width, height), (2, 2));
        assert_eq!(pixel_at(&rgba, 2, 0, 0), [0, 0, 255, 255], "top-left is not the blue pixel -- the rows were not flipped");
        assert_eq!(pixel_at(&rgba, 2, 1, 0), [255, 255, 255, 255]);
        assert_eq!(pixel_at(&rgba, 2, 0, 1), [255, 0, 0, 255], "bottom-left is not the red pixel");
        assert_eq!(pixel_at(&rgba, 2, 1, 1), [0, 255, 0, 255]);
    }

    /// A modern `.ico` stores its larger sizes as whole PNG files inside the
    /// container, so the container is a wrapper and the PNG decoder does the
    /// work. Both shapes, so neither can be the only one that works.
    #[test]
    fn an_ico_that_wraps_a_png_decodes_as_that_png() {
        let png = rgba_png(2, 2, &[9u8; 16]);
        let wrapped = ico_of(2, 2, 32, &png);
        assert_eq!(
            decode_rgba_unscaled(&wrapped),
            decode_rgba_unscaled(&png),
            "an ICO wrapping a PNG did not decode to the same thing as that PNG alone"
        );
        // The control: the wrapper really was a wrapper and not the PNG
        // itself, so the assertion above went through the ICO path.
        assert_ne!(wrapped, png);
        assert!(decode_rgba_unscaled(&png).is_some(), "the fixture PNG does not decode");
    }

    /// Everything that is not an icon file falls through to the PNG decoder
    /// untouched, which is what lets the dispatch be on magic bytes.
    #[test]
    fn bytes_that_are_not_an_icon_file_are_not_taken_for_one() {
        let png = rgba_png(4, 4, &[7u8; 64]);
        assert!(decode_rgba_unscaled(&png).is_some(), "an ordinary PNG stopped decoding");
        for not_an_icon in [
            b"<!doctype html><title>404</title>".to_vec(),
            Vec::new(),
            vec![0u8, 0, 1, 0],           // a truncated ICO header
            vec![0u8, 0, 2, 0, 1, 0],     // a CUR file: same layout, type 2
            vec![0u8, 0, 1, 0, 0, 0],     // an ICO claiming zero entries
        ] {
            assert!(
                decode_rgba_unscaled(&not_an_icon).is_none(),
                "these bytes decoded to an image: {not_an_icon:?}"
            );
        }
    }

    /// A directory entry pointing outside the buffer is an unfetched icon,
    /// never a panic. This parses bytes from a host in somebody's vault.
    #[test]
    fn a_malformed_icon_directory_answers_none_rather_than_panicking() {
        let mut bytes = ico_of(2, 2, 32, &dib_2x2_bgra([[0xff; 4]; 4]));
        // Point the single entry's data at an offset well past the end.
        bytes[18..22].copy_from_slice(&9_000_000u32.to_le_bytes());
        assert_eq!(decode_rgba_unscaled(&bytes), None);
        // The control: the same container with its offset intact does decode,
        // so the `None` above is the bounds check and not a broken fixture.
        let intact = ico_of(2, 2, 32, &dib_2x2_bgra([[0xff; 4]; 4]));
        assert!(decode_rgba_unscaled(&intact).is_some());
    }

    // -----------------------------------------------------------------
    // What a direct request actually puts on the wire
    // -----------------------------------------------------------------

    /// **The privacy claim for the direct path, as a test**, in the shape
    /// `breach::the_request_head_carries_nothing_beyond_the_allowlist`
    /// established: read off the literal bytes this crate put on a socket,
    /// and asserted as an **exact allowlist** rather than as a hunt for
    /// something bad.
    ///
    /// It **fails closed**. A header that is not on the list fails this test
    /// even if it carries nothing interesting, because this request goes to a
    /// host the app has no other relationship with -- every byte in the head
    /// is a byte handed to a stranger, and a new one has to be added here
    /// deliberately by somebody reading this comment.
    ///
    /// The server is on `127.0.0.1`, which is a private address, so this also
    /// pins the end-to-end claim the loopback case is about: the app fetched
    /// the icon **itself**, with the setting off.
    #[test]
    fn the_direct_request_head_carries_nothing_beyond_the_allowlist() {
        let mut server = crate::test_http::server();
        let port = server.socket_address().port();
        let captured: std::sync::Arc<std::sync::Mutex<Option<String>>> =
            std::sync::Arc::new(std::sync::Mutex::new(None));
        let sink = std::sync::Arc::clone(&captured);
        let icon = rgba_png(2, 2, &[0x40u8; 16]);
        let body = icon.clone();
        let mock = server
            .mock("GET", "/favicon.ico")
            .with_status(200)
            .with_body_from_request(move |request| {
                *sink.lock().expect("the head sink") = Some(request.raw_head().to_string());
                body.clone()
            })
            .expect(1)
            .create();

        // The setting is OFF, and the address is loopback: this is the
        // private-address rule, not the switch.
        let authority = format!("127.0.0.1:{port}");
        let source = icon_source_for(&authority, None, false);
        assert!(
            matches!(source, IconSource::Direct(_)),
            "loopback was not routed to the direct path, so nothing below is under test"
        );
        let fetched = fetch_icon_for(&source).expect("the direct fetch returned nothing");
        assert_eq!(fetched, icon, "the bytes that came back are not the ones served");
        // The request really did reach the route this test is about.
        mock.assert();

        let head = captured
            .lock()
            .expect("the head sink")
            .clone()
            .expect("the mock was never asked, so no head was captured");

        // Controls first: a real, complete request head.
        assert!(head.starts_with("GET "), "captured head is not a request head: {head:?}");
        assert!(head.ends_with("\r\n\r\n"), "the captured head is truncated: {head:?}");

        let mut lines = head.trim_end_matches("\r\n\r\n").split("\r\n");
        assert_eq!(
            lines.next().expect("a request head has a request line"),
            "GET /favicon.ico HTTP/1.1",
            "the request line is not exactly `GET /favicon.ico HTTP/1.1` -- a query string, an \
             extra path segment or a changed method all land here"
        );

        let allowed: Vec<(&str, String)> = vec![
            ("host", format!("127.0.0.1:{port}")),
            ("accept", "*/*".to_string()),
            // A fixed string, identical for every user of this app: no
            // version, no OS, no build id, no HTTP-library name.
            ("user-agent", DIRECT_USER_AGENT.to_string()),
            ("accept-encoding", "gzip".to_string()),
        ];

        let mut seen: Vec<String> = Vec::new();
        for line in lines {
            assert!(!line.is_empty(), "a blank line inside the head: {head:?}");
            let Some((name, value)) = line.split_once(':') else {
                panic!("header line is not `Name: value`: {line:?}");
            };
            let name = name.trim().to_ascii_lowercase();
            let Some((_, expected)) = allowed.iter().find(|(n, _)| *n == name) else {
                panic!(
                    "the direct icon request carried a header that is not on the allowlist: \
                     {line:?}\nFull head: {head:?}\n\
                     This request goes to a host in the user's vault that this app has no other \
                     relationship with. If this header is meant to be sent, add it to the \
                     allowlist above and say why."
                );
            };
            assert_eq!(
                value.trim(), expected,
                "header {name:?} carried {value:?}, not the one value it is allowed to carry"
            );
            seen.push(name);
        }

        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), seen.len(), "a header name was sent twice: {seen:?}");
        let mut expected_names: Vec<String> = allowed.iter().map(|(n, _)| (*n).to_string()).collect();
        expected_names.sort();
        assert_eq!(
            sorted, expected_names,
            "the set of headers sent is not the allowlist: {head:?}"
        );

        // Redundant and cheap: no cookie, no referer, no authorization, under
        // any casing. The allowlist above already forbids them; this is the
        // claim in its bluntest form.
        let lower = head.to_ascii_lowercase();
        for forbidden in ["cookie", "referer", "authorization", "x-"] {
            assert!(!lower.contains(forbidden), "the head carries {forbidden:?}: {head:?}");
        }
    }

    /// A candidate that answers `200` with something that is not an image --
    /// a web server's HTML 404 page, which is the ordinary case rather than
    /// the exotic one -- must not end the walk. The next candidate is tried,
    /// and it is the one whose bytes are kept.
    #[test]
    fn a_candidate_that_answers_with_a_page_does_not_end_the_walk() {
        let mut server = crate::test_http::server();
        let port = server.socket_address().port();
        let icon = rgba_png(2, 2, &[0x11u8; 16]);
        let html = server
            .mock("GET", "/favicon.ico")
            .with_status(200)
            .with_body("<!doctype html><title>Not found</title>")
            .expect(1)
            .create();
        let png = server
            .mock("GET", "/favicon.png")
            .with_status(200)
            .with_body(icon.clone())
            .expect(1)
            .create();

        let source = icon_source_for(&format!("127.0.0.1:{port}"), None, false);
        assert_eq!(
            fetch_icon_for(&source),
            Some(icon),
            "the walk kept the HTML page's bytes, so a page would have been cached as this \
             site's icon and the real one never asked for"
        );
        // Both routes were actually visited, in that order.
        html.assert();
        png.assert();
    }

    #[test]
    fn a_domain_with_unsafe_filename_characters_still_round_trips() {
        let dir = unique_cache_dir("unsafe-chars");
        let domain = "evil/../../etc:passwd";
        let bytes = vec![9u8, 8, 7];

        write_cached_icon(&dir, domain, &bytes);
        assert_eq!(read_cached_icon(&dir, domain), Some(bytes));

        std::fs::remove_dir_all(&dir).ok();
    }
}
