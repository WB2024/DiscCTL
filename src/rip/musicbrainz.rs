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
}

#[derive(Deserialize)]
struct MbTrack {
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
            let info = parse_release(best, total);

            if debug {
                eprintln!("MusicBrainz: matched \"{}\" by \"{}\" ({})",
                    info.album, info.album_artist,
                    info.year.as_deref().unwrap_or("unknown year"));
            }

            Ok(Some(info))
        }
    }
}

// ── Response parsing helpers ──────────────────────────────────────────────────

fn parse_release(r: &MbRelease, total_releases: usize) -> ReleaseInfo {
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
    for medium in &r.media {
        for t in &medium.tracks {
            let num = t.number.as_deref()
                .and_then(|n| n.parse::<usize>().ok())
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
