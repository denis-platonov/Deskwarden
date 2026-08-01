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
/// Bounded on purpose. This used to be a bare `ureq::get(url).call()` with no
/// agent and no timeouts of any kind; because it runs on a detached thread it
/// never froze the UI, but an unreachable icon host leaked that thread and its
/// socket permanently, one per icon. Three such stuck connections to the icon
/// CDN were found alive in a hung v0.3.0 process.
pub fn fetch_icon_bytes(url: &str) -> Option<Vec<u8>> {
    // One shared agent, not one per call: icons are fetched in bursts against
    // a single host, and a fresh agent per call would throw away connection
    // reuse (and open a new TCP+TLS handshake for every icon in the list).
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    let agent =
        AGENT.get_or_init(|| crate::http_agent::bounded_total(CONNECT_TIMEOUT, REQUEST_DEADLINE));
    let response = agent.get(url).call().ok()?;
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

    Some(resample_for_display(width, height, rgba))
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
fn box_downscale(src: &[u8], src_w: usize, src_h: usize, dst_w: usize, dst_h: usize) -> Vec<u8> {
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
