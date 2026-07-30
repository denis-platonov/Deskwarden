//! Fetches an item's website favicon for the vault window's item list and
//! detail pane. Every failure mode here (no URI, fetch error, decode error)
//! is meant to be swallowed by the caller and fall back to the existing
//! colored-initials monogram (`theme::avatar`) -- a missing icon is not
//! worth an error path, this app already has one perfectly good fallback.

use std::io::Read;

/// Where to fetch icons from: Bitwarden's own icon service for the default
/// cloud, or the self-hosted server's own icon proxy otherwise. Self-hosted
/// Bitwarden servers proxy icon fetches themselves (`{server}/icons/...`)
/// rather than having the client reach out to a third party directly.
pub fn icon_base_url(server_url: Option<&str>) -> String {
    match server_url {
        Some(url) if !url.trim().is_empty() && !url.contains("bitwarden.com") && !url.contains("bitwarden.eu") => {
            format!("{}/icons", url.trim().trim_end_matches('/'))
        }
        _ => "https://icons.bitwarden.net".to_string(),
    }
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
        png::ColorType::Indexed => return None,
    };

    Some((width, height, rgba))
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
}
