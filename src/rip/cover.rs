//! Album art: where it comes from and what happens to it.
//!
//! Sources are tried in the order the user chose and the first one that returns an image
//! wins. `Fanart` needs a personal API key from fanart.tv and looks the album up by its
//! MusicBrainz *release group*; `CoverArtArchive` is MusicBrainz's own art and is looked up
//! by release.

use std::{io::Read, str::FromStr, time::Duration};

use serde::Deserialize;

use super::musicbrainz;

const FANART_API: &str = "https://webservice.fanart.tv/v3/music/albums";
const USER_AGENT: &str = concat!("RustyDisc/", env!("CARGO_PKG_VERSION"), " ( https://github.com/WB2024/DiscCTL )");
/// Refuse absurdly large downloads.
const MAX_IMAGE_BYTES: u64 = 25 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverSource {
    Fanart,
    CoverArtArchive,
}

impl CoverSource {
    /// Short machine name, as used in settings and on the command line.
    pub fn id(self) -> &'static str {
        match self {
            CoverSource::Fanart => "fanart",
            CoverSource::CoverArtArchive => "caa",
        }
    }

    /// Name shown to people.
    pub fn label(self) -> &'static str {
        match self {
            CoverSource::Fanart => "fanart.tv",
            CoverSource::CoverArtArchive => "Cover Art Archive",
        }
    }
}

impl FromStr for CoverSource {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "fanart" | "fanart.tv" | "fanarttv" => Ok(CoverSource::Fanart),
            "caa" | "coverartarchive" | "cover-art-archive" | "musicbrainz" => Ok(CoverSource::CoverArtArchive),
            other => Err(format!("unknown cover source '{other}' (use fanart or caa)")),
        }
    }
}

/// Parse a comma-separated priority list such as `fanart,caa`.
pub fn parse_sources(list: &str) -> Result<Vec<CoverSource>, String> {
    let mut out: Vec<CoverSource> = Vec::new();
    for part in list.split(',').filter(|p| !p.trim().is_empty()) {
        let s: CoverSource = part.parse()?;
        if !out.contains(&s) {
            out.push(s);
        }
    }
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct CoverOptions {
    /// In priority order; the first source that has an image wins.
    pub sources: Vec<CoverSource>,
    pub fanart_key: Option<String>,
    /// Save `cover.jpg` / `cover.png` next to the tracks.
    pub save_file: bool,
    /// Embed the image in every audio file.
    pub embed: bool,
}

impl Default for CoverOptions {
    fn default() -> Self {
        CoverOptions { sources: vec![CoverSource::CoverArtArchive], fanart_key: None, save_file: true, embed: true }
    }
}

impl CoverOptions {
    /// Whether any cover art needs fetching at all.
    pub fn wanted(&self) -> bool {
        (self.save_file || self.embed) && !self.sources.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct Cover {
    pub bytes: Vec<u8>,
    /// "jpg" or "png"
    pub ext: &'static str,
    pub source: CoverSource,
}

impl Cover {
    pub fn mime(&self) -> &'static str {
        if self.ext == "png" { "image/png" } else { "image/jpeg" }
    }
}

/// "jpg" or "png" if these bytes are one of those images, judged by the file's own header
/// (never by its name).
pub fn sniff_ext(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("png")
    } else {
        None
    }
}

/// What an image is, read from its own header.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ImageInfo {
    pub format: &'static str,
    pub width: u32,
    pub height: u32,
}

/// The format and pixel size of a JPEG or PNG, without decoding it.
pub fn image_info(b: &[u8]) -> Option<ImageInfo> {
    match sniff_ext(b)? {
        "png" => {
            // 8-byte signature, then the IHDR chunk: length, "IHDR", width, height.
            if b.len() < 24 || &b[12..16] != b"IHDR" {
                return None;
            }
            let be = |i: usize| u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
            Some(ImageInfo { format: "PNG", width: be(16), height: be(20) })
        }
        _ => {
            // JPEG: walk the segments until a start-of-frame marker, which holds the size.
            let mut i = 2;
            while i + 9 < b.len() {
                if b[i] != 0xFF {
                    i += 1;
                    continue;
                }
                let marker = b[i + 1];
                if marker == 0xFF {
                    i += 1;
                    continue;
                }
                // Markers with no length: standalone, restart and start/end of image.
                if marker == 0x01 || (0xD0..=0xD9).contains(&marker) {
                    i += 2;
                    continue;
                }
                let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
                // SOF0..SOF15, except DHT (C4), JPG (C8) and DAC (CC).
                if (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
                    let h = u16::from_be_bytes([b[i + 5], b[i + 6]]) as u32;
                    let w = u16::from_be_bytes([b[i + 7], b[i + 8]]) as u32;
                    return (w > 0 && h > 0).then_some(ImageInfo { format: "JPEG", width: w, height: h });
                }
                i += 2 + len.max(2);
            }
            None
        }
    }
}

/// Try each source in order and return the first image found.
pub fn fetch(release_id: &str, release_group_id: Option<&str>, opts: &CoverOptions, debug: bool) -> Option<Cover> {
    for &source in &opts.sources {
        let found = match source {
            CoverSource::Fanart => {
                match (opts.fanart_key.as_deref().filter(|k| !k.is_empty()), release_group_id) {
                    (Some(key), Some(rg)) => fetch_fanart(rg, key, debug),
                    (None, _) => {
                        if debug { eprintln!("Cover art: fanart.tv skipped (no API key set)"); }
                        None
                    }
                    (_, None) => {
                        if debug { eprintln!("Cover art: fanart.tv skipped (release has no release group)"); }
                        None
                    }
                }
            }
            CoverSource::CoverArtArchive => musicbrainz::fetch_cover_art(release_id, debug),
        };
        if let Some((bytes, ext)) = found {
            return Some(Cover { bytes, ext, source });
        }
    }
    None
}

// ── fanart.tv ─────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct FanartAlbums {
    #[serde(default)]
    albums: std::collections::HashMap<String, FanartAlbum>,
}

#[derive(Deserialize, Default)]
struct FanartAlbum {
    #[serde(default)]
    albumcover: Vec<FanartImage>,
}

#[derive(Deserialize)]
struct FanartImage {
    url: String,
    /// fanart.tv sends the like count as a string.
    likes: Option<String>,
}

/// Pick the most-liked album cover out of a fanart.tv response.
fn best_cover_url(resp: &FanartAlbums, release_group_id: &str) -> Option<String> {
    let album = resp.albums.get(release_group_id).or_else(|| resp.albums.values().next())?;
    album
        .albumcover
        .iter()
        .max_by_key(|i| i.likes.as_deref().and_then(|l| l.trim().parse::<u32>().ok()).unwrap_or(0))
        .map(|i| i.url.clone())
}

fn fanart_agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(Duration::from_secs(20)).user_agent(USER_AGENT).build()
}

/// Outcome of asking fanart.tv about an album; used by the settings "Test" button too.
#[derive(Debug, PartialEq, Eq)]
pub enum FanartStatus {
    Ok,
    /// The key was rejected.
    BadKey,
    /// The key works but fanart.tv has nothing for this album.
    NoArtwork,
    Error(String),
}

fn fanart_get(release_group_id: &str, key: &str, debug: bool) -> Result<FanartAlbums, FanartStatus> {
    let url = format!("{}/{}", FANART_API, release_group_id);
    if debug { eprintln!("fanart.tv: GET {} (api key hidden)", url); }
    match fanart_agent().get(&url).query("api_key", key).call() {
        Ok(r) => r.into_json().map_err(|e| FanartStatus::Error(format!("unreadable response: {e}"))),
        Err(ureq::Error::Status(404, _)) => Err(FanartStatus::NoArtwork),
        Err(ureq::Error::Status(401 | 403, _)) => Err(FanartStatus::BadKey),
        Err(ureq::Error::Status(code, _)) => Err(FanartStatus::Error(format!("HTTP {code}"))),
        Err(e) => Err(FanartStatus::Error(e.to_string())),
    }
}

/// Check an API key against fanart.tv using a well-known album.
pub fn test_fanart_key(key: &str) -> FanartStatus {
    // Radiohead — OK Computer (release group)
    match fanart_get("b1392450-e666-3926-a536-22c65f834433", key, false) {
        Ok(_) => FanartStatus::Ok,
        Err(FanartStatus::NoArtwork) => FanartStatus::Ok, // authenticated; just nothing for this album
        Err(other) => other,
    }
}

fn fetch_fanart(release_group_id: &str, key: &str, debug: bool) -> Option<(Vec<u8>, &'static str)> {
    let resp = match fanart_get(release_group_id, key, debug) {
        Ok(r) => r,
        Err(FanartStatus::NoArtwork) => {
            if debug { eprintln!("fanart.tv: no artwork for this album"); }
            return None;
        }
        Err(FanartStatus::BadKey) => {
            eprintln!("fanart.tv: the API key was rejected — check it in Settings");
            return None;
        }
        Err(e) => {
            if debug { eprintln!("fanart.tv: {:?}", e); }
            return None;
        }
    };
    let url = best_cover_url(&resp, release_group_id)?;
    if debug { eprintln!("fanart.tv: downloading {}", url); }

    let r = fanart_agent().get(&url).call().ok()?;
    let content_type = r.header("Content-Type").unwrap_or("").to_lowercase();
    let ext = if content_type.contains("png") || url.to_lowercase().ends_with(".png") { "png" } else { "jpg" };
    let mut bytes = Vec::new();
    r.into_reader().take(MAX_IMAGE_BYTES).read_to_end(&mut bytes).ok()?;
    if bytes.is_empty() { None } else { Some((bytes, ext)) }
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_png_and_jpeg_sizes_from_the_header() {
        // A 1x1 PNG.
        let png = [
            0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13, b'I', b'H', b'D', b'R', 0, 0, 2, 0x58, 0, 0, 1, 0x2C, 8, 2, 0, 0, 0,
        ];
        let i = super::image_info(&png).unwrap();
        assert_eq!((i.format, i.width, i.height), ("PNG", 600, 300));
        // A JPEG with an APP0 segment, then SOF0 for 1400 x 1400.
        let mut jpg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 16, b'J', b'F', b'I', b'F', 0, 1, 1, 0, 0, 1, 0, 1, 0, 0];
        jpg.extend([0xFF, 0xC0, 0, 17, 8, 0x05, 0x78, 0x05, 0x78, 3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]);
        let i = super::image_info(&jpg).unwrap();
        assert_eq!((i.format, i.width, i.height), ("JPEG", 1400, 1400));
        assert!(super::image_info(b"not an image at all, just text").is_none());
        assert!(super::image_info(&[0xFF, 0xD8, 0xFF]).is_none(), "truncated");
    }

    use super::*;

    #[test]
    fn parses_priority_lists() {
        assert_eq!(parse_sources("fanart,caa").unwrap(), vec![CoverSource::Fanart, CoverSource::CoverArtArchive]);
        assert_eq!(parse_sources(" CAA , fanart.tv ,caa").unwrap(), vec![CoverSource::CoverArtArchive, CoverSource::Fanart]);
        assert!(parse_sources("").unwrap().is_empty());
        assert!(parse_sources("fanart,bing").is_err());
    }

    #[test]
    fn picks_the_most_liked_fanart_cover() {
        let resp: FanartAlbums = serde_json::from_str(r#"{
            "name": "X", "mbid_id": "abc",
            "albums": { "abc": { "albumcover": [
                {"id": "1", "url": "https://assets.fanart.tv/a.jpg", "likes": "2"},
                {"id": "2", "url": "https://assets.fanart.tv/b.png", "likes": "11"},
                {"id": "3", "url": "https://assets.fanart.tv/c.jpg", "likes": "0"}
            ], "cdart": [] } }
        }"#).unwrap();
        assert_eq!(best_cover_url(&resp, "abc").unwrap(), "https://assets.fanart.tv/b.png");
        // fanart.tv keys the album by release group; fall back to the only album if the id differs
        assert_eq!(best_cover_url(&resp, "zzz").unwrap(), "https://assets.fanart.tv/b.png");
        let empty: FanartAlbums = serde_json::from_str(r#"{"albums": {"abc": {"cdart": []}}}"#).unwrap();
        assert!(best_cover_url(&empty, "abc").is_none());
    }

    #[test]
    fn nothing_wanted_when_disabled_or_no_sources() {
        let mut o = CoverOptions::default();
        assert!(o.wanted());
        o.save_file = false;
        assert!(o.wanted());
        o.embed = false;
        assert!(!o.wanted());
        let o = CoverOptions { sources: vec![], ..Default::default() };
        assert!(!o.wanted());
    }

    #[test]
    fn skips_fanart_without_key_and_falls_through() {
        // No key: fanart is skipped without any network access. Only fanart is listed, so
        // nothing is found.
        let o = CoverOptions { sources: vec![CoverSource::Fanart], fanart_key: None, ..Default::default() };
        assert!(fetch("rel", Some("rg"), &o, false).is_none());
    }

    #[test]
    fn pictures_are_recognised_by_their_header_not_their_name() {
        assert_eq!(sniff_ext(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0]), Some("jpg"));
        assert_eq!(sniff_ext(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0]), Some("png"));
        assert_eq!(sniff_ext(b"GIF89a"), None);
        assert_eq!(sniff_ext(b"<svg></svg>"), None);
        assert_eq!(sniff_ext(&[]), None);
    }
}
