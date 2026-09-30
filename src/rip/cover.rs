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
