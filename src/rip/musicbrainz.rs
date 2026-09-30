use std::io::Read;
use serde::{Deserialize, Serialize};
use crate::error::Error;

const MB_API: &str = "https://musicbrainz.org/ws/2";
const USER_AGENT: &str = "RustyDisc/0.1 ( https://github.com/WB2024/DiscCTL )";

// ── Public types ──────────────────────────────────────────────────────────────

/// Metadata for a release retrieved from MusicBrainz.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseInfo {
    pub mb_release_id: String,
    pub album: String,
    pub album_artist: String,
    pub mb_artist_id: Option<String>,
    /// Release date string as returned by MB, e.g. "1991-11-04" or "1991".
    pub date: Option<String>,
    /// 4-digit year extracted from `date`.
    pub year: Option<String>,
    pub tracks: Vec<MbTrackInfo>,
    /// How many releases share this DiscID (useful for logging).
    pub total_releases: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MbTrackInfo {
    pub number: usize,
    pub title: String,
    pub artist: Option<String>,
    pub mb_recording_id: Option<String>,
    pub mb_artist_id: Option<String>,
}

// ── API response types (private, only used for deserialisation) ───────────────

#[derive(Deserialize)]
struct MbDiscResponse {
    releases: Option<Vec<MbRelease>>,
}

#[derive(Deserialize)]
struct MbRelease {
    id: String,
    title: String,
    date: Option<String>,
    #[serde(rename = "artist-credit", default)]
    artist_credit: Vec<MbArtistCredit>,
    #[serde(default)]
    media: Vec<MbMedia>,
}

#[derive(Deserialize)]
struct MbArtistCredit {
    name: Option<String>,
    artist: Option<MbArtistRef>,
    #[serde(default)]
    joinphrase: String,
}

#[derive(Deserialize)]
struct MbArtistRef {
    id: String,
    name: String,
}

#[derive(Deserialize, Default)]
struct MbMedia {
    #[serde(default)]
    tracks: Vec<MbTrack>,
    /// Discs (DiscIDs) attached to this medium; only present when requested with `inc=discids`.
    #[serde(default)]
    discs: Vec<MbDisc>,
}

#[derive(Deserialize)]
struct MbDisc {
    id: String,
}

#[derive(Deserialize)]
struct MbTrack {
    position: Option<usize>,
    number: Option<String>,
    title: String,
    #[serde(rename = "artist-credit", default)]
    artist_credit: Vec<MbArtistCredit>,
    recording: Option<MbRecording>,
}

#[derive(Deserialize)]
struct MbRecording {
    id: String,
    #[serde(rename = "artist-credit", default)]
    artist_credit: Vec<MbArtistCredit>,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Fetch the front cover art for a release from the Cover Art Archive.
///
/// Tries full-size first, falls back to 500px thumbnail.
/// Returns the raw image bytes and a file extension ("jpg" or "png").
pub fn fetch_cover_art(mb_release_id: &str, debug: bool) -> Option<(Vec<u8>, &'static str)> {
    // CAA redirects to the actual image — ureq follows redirects automatically.
    let url = format!("https://coverartarchive.org/release/{}/front", mb_release_id);
    if debug { eprintln!("Cover Art Archive: {}", url); }

    let resp = ureq::get(&url)
        .set("User-Agent", USER_AGENT)
        .call();

    match resp {
        Ok(r) => {
            let content_type = r.header("Content-Type").unwrap_or("").to_lowercase();
            let ext = if content_type.contains("png") { "png" } else { "jpg" };
            let mut bytes = Vec::new();
            r.into_reader().read_to_end(&mut bytes).ok()?;
            if bytes.is_empty() { return None; }
            if debug { eprintln!("Cover art: {} bytes ({})", bytes.len(), ext); }
            Some((bytes, ext))
        }
        Err(ureq::Error::Status(404, _)) => {
            if debug { eprintln!("Cover Art Archive: no image for release {}", mb_release_id); }
            None
        }
        Err(e) => {
            if debug { eprintln!("Cover Art Archive: network error — {}", e); }
            None
        }
    }
}

/// Look up a disc by its MusicBrainz DiscID.
///
/// Returns `Ok(None)` if the disc is not in the MusicBrainz database.
/// Returns `Ok(Some(info))` on a successful match (using the first/best release).
/// Network or parse errors are returned as `Err`.
pub fn lookup(discid: &str, debug: bool) -> Result<Option<ReleaseInfo>, Error> {
    let url = format!(
        "{}/discid/{}?inc=recordings+artists&fmt=json",
        MB_API, discid
    );

    if debug { eprintln!("MusicBrainz lookup: {}", url); }

    let response = ureq::get(&url)
        .set("User-Agent", USER_AGENT)
        .call();

    match response {
        Err(ureq::Error::Status(404, _)) => {
            if debug { eprintln!("MusicBrainz: disc not found (404)"); }
            return Ok(None);
        }
        Err(ureq::Error::Status(code, resp)) => {
            if debug {
                let body = resp.into_string().unwrap_or_default();
                eprintln!("MusicBrainz: HTTP {} — {}", code, body.trim());
            }
            return Ok(None); // non-fatal: fall back to CD-Text
        }
        Err(e) => {
            if debug { eprintln!("MusicBrainz: network error — {}", e); }
            return Ok(None); // non-fatal
        }
        Ok(resp) => {
            let parsed: MbDiscResponse = resp.into_json().map_err(|e| {
                Error::backend(format!("MusicBrainz response parse error: {}", e))
            })?;

            let releases = match parsed.releases {
                Some(r) if !r.is_empty() => r,
                _ => {
                    if debug { eprintln!("MusicBrainz: response contained no releases"); }
                    return Ok(None);
                }
            };

            let total = releases.len();
            if debug && total > 1 {
                eprintln!("MusicBrainz: {} releases match this DiscID — using first", total);
                for (i, r) in releases.iter().enumerate() {
                    eprintln!("  [{}] {} — {} ({})", i + 1, r.title,
                        artist_name(&r.artist_credit),
                        r.date.as_deref().unwrap_or("no date"));
                }
            }

            // Pick the first release (MB returns them in relevance order).
            let best = &releases[0];
            let info = parse_release(best, &best.media, total);

            if debug {
                eprintln!("MusicBrainz: matched \"{}\" by \"{}\" ({})",
                    info.album, info.album_artist,
                    info.year.as_deref().unwrap_or("unknown year"));
            }

            Ok(Some(info))
        }
    }
}

/// Extract a release MBID from a bare UUID or a MusicBrainz release URL
/// (e.g. `https://musicbrainz.org/release/bc8d517f-6ce0-4e45-b6d8-af0f29cdd1ea`).
pub fn parse_release_id(input: &str) -> Result<String, Error> {
    let s = input.trim();
    let candidate = match s.find("/release/") {
        Some(i) => s[i + "/release/".len()..].split(['/', '?', '#']).next().unwrap_or(""),
        None => s,
    };
    let c = candidate.to_ascii_lowercase();
    let b = c.as_bytes();
    let shape_ok = b.len() == 36
        && [8usize, 13, 18, 23].iter().all(|&i| b[i] == b'-')
        && b.iter().enumerate().all(|(i, ch)| [8usize, 13, 18, 23].contains(&i) || ch.is_ascii_hexdigit());
    if shape_ok {
        Ok(c)
    } else {
        Err(Error::validation(format!(
            "'{}' is not a MusicBrainz release ID or URL (expected e.g. bc8d517f-6ce0-4e45-b6d8-af0f29cdd1ea)",
            s
        )))
    }
}

/// Fetch a specific release by MBID, for when the DiscID lookup finds nothing (or the
/// wrong release).
///
/// A release can hold several discs. The medium used for the track list is the one
/// carrying `discid`, otherwise the one whose track count equals `audio_tracks`.
/// Returns the release plus an optional warning (e.g. the track count doesn't match).
pub fn lookup_release(
    mbid: &str,
    discid: Option<&str>,
    audio_tracks: Option<usize>,
    debug: bool,
) -> Result<(ReleaseInfo, Option<String>), Error> {
    let url = format!("{}/release/{}?inc=recordings+artist-credits+discids&fmt=json", MB_API, mbid);
    if debug { eprintln!("MusicBrainz release lookup: {}", url); }

    let release: MbRelease = match ureq::get(&url).set("User-Agent", USER_AGENT).call() {
        Ok(resp) => resp.into_json().map_err(|e| Error::backend(format!("MusicBrainz response parse error: {}", e)))?,
        Err(ureq::Error::Status(404, _)) => {
            return Err(Error::validation(format!("MusicBrainz has no release with ID {}", mbid)));
        }
        Err(ureq::Error::Status(code, _)) => {
            return Err(Error::backend(format!("MusicBrainz returned HTTP {} for release {}", code, mbid)));
        }
        Err(e) => return Err(Error::backend(format!("Could not reach MusicBrainz: {}", e))),
    };

    if release.media.is_empty() {
        return Err(Error::validation("That MusicBrainz release has no track list"));
    }

    let by_discid = discid.and_then(|d| release.media.iter().position(|m| m.discs.iter().any(|x| x.id == d)));
    let by_count = audio_tracks.and_then(|n| release.media.iter().position(|m| m.tracks.len() == n));

    let (idx, warning) = match (by_discid, by_count) {
        (Some(i), _) => (i, None),
        (None, Some(i)) => (i, None),
        (None, None) if release.media.len() == 1 => {
            let have = release.media[0].tracks.len();
            let warning = audio_tracks.filter(|&n| n != have).map(|n| format!(
                "This release lists {have} tracks but the disc has {n} audio tracks — check it is the right release."
            ));
            (0, warning)
        }
        (None, None) => {
            let counts: Vec<String> = release.media.iter().map(|m| m.tracks.len().to_string()).collect();
            return Err(Error::validation(format!(
                "That release has {} discs with {} tracks, but the disc has {} audio tracks — none of them match",
                release.media.len(), counts.join("/"), audio_tracks.unwrap_or(0)
            )));
        }
    };

    let mut info = parse_release(&release, std::slice::from_ref(&release.media[idx]), 1);
    if release.media.len() > 1 && debug {
        eprintln!("MusicBrainz: release has {} discs — using disc {}", release.media.len(), idx + 1);
    }
    info.total_releases = 1;
    Ok((info, warning))
}

// ── Response parsing helpers ──────────────────────────────────────────────────

fn parse_release(r: &MbRelease, media: &[MbMedia], total_releases: usize) -> ReleaseInfo {
    let album_artist = artist_name(&r.artist_credit);
    let mb_artist_id = r.artist_credit.first()
        .and_then(|c| c.artist.as_ref())
        .map(|a| a.id.clone());

    let year = r.date.as_deref().and_then(|d| {
        // Date can be "1991", "1991-11", or "1991-11-04"
        let y = d.split('-').next()?;
        if y.len() == 4 { Some(y.to_string()) } else { None }
    });

    // Flatten all tracks from all media (for a single-disc release there's one medium).
    let mut tracks: Vec<MbTrackInfo> = Vec::new();
    for medium in media {
        for t in &medium.tracks {
            let num = t.position
                .or_else(|| t.number.as_deref().and_then(|n| n.parse::<usize>().ok()))
                .unwrap_or(tracks.len() + 1);

            // Track-level artist: prefer recording artist-credit, then track-level, then album artist.
            let track_artist_credits = t.recording.as_ref()
                .map(|rec| &rec.artist_credit)
                .filter(|c| !c.is_empty())
                .unwrap_or(&t.artist_credit);

            let track_artist_str = artist_name(track_artist_credits);
            let track_artist = if track_artist_str.is_empty() || track_artist_str == artist_name(&r.artist_credit) {
                None // same as album artist — no need to duplicate
            } else {
                Some(track_artist_str)
            };

            let mb_recording_id = t.recording.as_ref().map(|rec| rec.id.clone());
            let mb_artist_id = track_artist_credits.first()
                .and_then(|c| c.artist.as_ref())
                .map(|a| a.id.clone());

            tracks.push(MbTrackInfo {
                number: num,
                title: t.title.clone(),
                artist: track_artist,
                mb_recording_id,
                mb_artist_id,
            });
        }
    }

    ReleaseInfo {
        mb_release_id: r.id.clone(),
        album: r.title.clone(),
        album_artist,
        mb_artist_id,
        date: r.date.clone(),
        year,
        tracks,
        total_releases,
    }
}

/// Join artist credit entries (artist name + joinphrase) into a display string.
fn artist_name(credits: &[MbArtistCredit]) -> String {
    credits.iter().map(|c| {
        let name = c.name.as_deref()
            .or_else(|| c.artist.as_ref().map(|a| a.name.as_str()))
            .unwrap_or("");
        format!("{}{}", name, c.joinphrase)
    }).collect::<String>().trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_release_ids_and_urls() {
        let id = "bc8d517f-6ce0-4e45-b6d8-af0f29cdd1ea";
        assert_eq!(parse_release_id(id).unwrap(), id);
        assert_eq!(parse_release_id(&format!("  {}  ", id.to_uppercase())).unwrap(), id);
        assert_eq!(parse_release_id(&format!("https://musicbrainz.org/release/{id}")).unwrap(), id);
        assert_eq!(parse_release_id(&format!("https://musicbrainz.org/release/{id}/disc/1?x=1#y")).unwrap(), id);
        assert!(parse_release_id("https://musicbrainz.org/release-group/bc8d517f-6ce0-4e45-b6d8-af0f29cdd1ea").is_err());
        assert!(parse_release_id("not-an-id").is_err());
        assert!(parse_release_id("bc8d517f-6ce0-4e45-b6d8-af0f29cdd1eZ").is_err());
    }

    /// Hits the real MusicBrainz API; run with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn live_release_lookup() {
        let (r, warning) = lookup_release("bc8d517f-6ce0-4e45-b6d8-af0f29cdd1ea", None, None, false).unwrap();
        println!("{} — {} ({:?}) {} tracks, warning {:?}", r.album, r.album_artist, r.year, r.tracks.len(), warning);
        assert!(!r.tracks.is_empty());
        assert_eq!(r.tracks[0].number, 1);
    }
}
