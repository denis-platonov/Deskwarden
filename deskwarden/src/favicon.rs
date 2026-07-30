//! Fetches an item's website favicon for the vault window's item list and
//! detail pane. Every failure mode here (no URI, fetch error, decode error)
//! is meant to be swallowed by the caller and fall back to the existing
//! colored-initials monogram (`theme::avatar`) -- a missing icon is not
//! worth an error path, this app already has one perfectly good fallback.

use std::io::Read;
use std::path::{Path, PathBuf};

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
fn host_from_url(url: &str) -> String {
    let stripped = url
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let host = stripped.split(['/', '?', '#']).next().unwrap_or(stripped);
    let host = host.split(':').next().unwrap_or(host);
    host.to_string()
}

/// True only for `bitwarden.com`/`bitwarden.eu` themselves or a subdomain of
/// one of them -- not for unrelated domains that merely contain those
/// strings as a substring (e.g. `vault.bitwarden.community`).
fn is_bitwarden_cloud_host(host: &str) -> bool {
    host == "bitwarden.com"
        || host.ends_with(".bitwarden.com")
        || host == "bitwarden.eu"
        || host.ends_with(".bitwarden.eu")
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

/// Blocking GET for the icon's raw bytes. Call only from a background
/// thread -- see `vault_window::favicon_loader` for the async wrapper the UI
/// actually uses.
pub fn fetch_icon_bytes(url: &str) -> Option<Vec<u8>> {
    let response = ureq::get(url).call().ok()?;
    let mut bytes = Vec::new();
    response.into_reader().read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

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
