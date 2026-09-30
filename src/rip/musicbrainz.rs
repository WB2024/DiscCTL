use std::{
    io::Read,
    sync::Mutex,
    time::{Duration, Instant},
};
use serde::{Deserialize, Serialize};
use crate::error::Error;

pub(crate) const MB_API: &str = "https://musicbrainz.org/ws/2";
pub(crate) const USER_AGENT: &str = concat!("RustyDisc/", env!("CARGO_PKG_VERSION"), " ( https://github.com/WB2024/DiscCTL )");

// ── Rate limiting ─────────────────────────────────────────────────────────────
//
// MusicBrainz allows an average of one request per second per client and answers
// 503 to clients that go faster. Every call to the MusicBrainz API (not the Cover Art
// Archive) goes through `mb_call`, which spaces requests out, queueing concurrent
// callers, and backs off and retries when the server says to slow down.

/// Minimum gap between requests; a little over the 1 request/second limit.
const MIN_INTERVAL: Duration = Duration::from_millis(1100);
/// Longest a caller will queue for a slot before giving up (keeps the web UI responsive).
const MAX_QUEUE_WAIT: Duration = Duration::from_secs(10);
/// Retries after a 429/503 response.
const MAX_RETRIES: u32 = 3;

struct Limiter {
    next_slot: Mutex<Instant>,
    interval: Duration,
}

impl Limiter {
    const fn new(interval: Duration, now: Instant) -> Self {
        Limiter { next_slot: Mutex::new(now), interval }
    }

    /// Claim the next request slot and return how long to wait before using it, or `None`
    /// if the queue is already longer than `max_wait` (no slot is claimed then).
    fn reserve(&self, max_wait: Duration) -> Option<Duration> {
        let mut next = self.next_slot.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let start = (*next).max(now);
        let wait = start - now;
        if wait > max_wait {
            return None;
        }
        *next = start + self.interval;
        Some(wait)
    }
}

static LIMITER: std::sync::LazyLock<Limiter> =
    std::sync::LazyLock::new(|| Limiter::new(MIN_INTERVAL, Instant::now()));

/// How long to back off after the server asked us to slow down.
fn retry_delay(retry_after: Option<&str>, attempt: u32) -> Duration {
    let secs = retry_after
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(2u64 << attempt.min(4)); // 2s, 4s, 8s, ...
    Duration::from_secs(secs.clamp(1, 30))
}

/// Make a MusicBrainz API request politely. `build` is called once per attempt.
pub(crate) fn mb_call(build: impl Fn() -> ureq::Request) -> Result<ureq::Response, ureq::Error> {
    let mut attempt = 0;
    loop {
        match LIMITER.reserve(MAX_QUEUE_WAIT) {
            Some(wait) if !wait.is_zero() => std::thread::sleep(wait),
            Some(_) => {}
            None => {
                return Err(ureq::Error::Status(
                    503,
                    ureq::Response::new(503, "Busy", "too many queued MusicBrainz requests")
                        .expect("static response"),
                ));
            }
        }
        match build().call() {
            Err(ureq::Error::Status(code @ (429 | 503), resp)) if attempt < MAX_RETRIES => {
                let delay = retry_delay(resp.header("Retry-After"), attempt);
                eprintln!("MusicBrainz asked us to slow down (HTTP {code}); waiting {}s before retrying", delay.as_secs());
                std::thread::sleep(delay);
                attempt += 1;
            }
            other => return other,
        }
    }
}

// ── Public types ──────────────────────────────────────────────────────────────

/// Metadata for a release retrieved from MusicBrainz.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
    /// MusicBrainz release group this release belongs to (fanart.tv indexes albums by it).
    #[serde(default)]
    pub mb_release_group_id: Option<String>,
    /// How many releases share this DiscID (useful for logging).
    pub total_releases: usize,

    // Everything below comes from a second, fuller lookup (see `mb_enrich`) and is what Picard
    // would tag a file with. Old rips saved without it still load.
    #[serde(default)] pub album_artist_sort: Option<String>,
    #[serde(default)] pub album_artist_ids: Vec<String>,
    /// The album artist credit split into separate names.
    #[serde(default)] pub album_artists: Vec<String>,
    /// The release's disambiguation comment, e.g. "remastered".
    #[serde(default)] pub release_comment: Option<String>,
    #[serde(default)] pub status: Option<String>,
    /// e.g. "album" or "album; live"
    #[serde(default)] pub release_type: Option<String>,
    #[serde(default)] pub country: Option<String>,
    #[serde(default)] pub label: Option<String>,
    #[serde(default)] pub catalog_number: Option<String>,
    #[serde(default)] pub barcode: Option<String>,
    #[serde(default)] pub asin: Option<String>,
    #[serde(default)] pub script: Option<String>,
    #[serde(default)] pub language: Option<String>,
    /// Format of the disc, e.g. "CD".
    #[serde(default)] pub media_format: Option<String>,
    /// First release date of the release group (Picard's `originaldate`).
    #[serde(default)] pub original_date: Option<String>,
    #[serde(default)] pub disc_number: Option<usize>,
    #[serde(default)] pub disc_total: Option<usize>,
    #[serde(default)] pub disc_title: Option<String>,
    #[serde(default)] pub genres: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MbTrackInfo {
    pub number: usize,
    pub title: String,
    pub artist: Option<String>,
    pub mb_recording_id: Option<String>,
    pub mb_artist_id: Option<String>,

    // From the fuller lookup (see `mb_enrich`).
    /// The track's own ID on this release (Picard's "release track id").
    #[serde(default)] pub mb_release_track_id: Option<String>,
    #[serde(default)] pub artist_sort: Option<String>,
    #[serde(default)] pub artists: Vec<String>,
    #[serde(default)] pub artist_ids: Vec<String>,
    #[serde(default)] pub isrcs: Vec<String>,
    #[serde(default)] pub work_ids: Vec<String>,
    #[serde(default)] pub works: Vec<String>,
    #[serde(default)] pub composers: Vec<String>,
    #[serde(default)] pub lyricists: Vec<String>,
    #[serde(default)] pub writers: Vec<String>,
    #[serde(default)] pub arrangers: Vec<String>,
    #[serde(default)] pub conductors: Vec<String>,
    #[serde(default)] pub producers: Vec<String>,
    #[serde(default)] pub mixers: Vec<String>,
    #[serde(default)] pub engineers: Vec<String>,
    #[serde(default)] pub remixers: Vec<String>,
    /// Performers, with the instrument or role in brackets: "Name (guitar)".
    #[serde(default)] pub performers: Vec<String>,
    #[serde(default)] pub length_ms: Option<u64>,
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
    #[serde(rename = "release-group")]
    release_group: Option<MbReleaseGroup>,
}

#[derive(Deserialize)]
struct MbReleaseGroup {
    id: String,
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
    fetch_cover_art_sized(mb_release_id, None, debug)
}

/// Like [`fetch_cover_art`], optionally asking the Cover Art Archive for a thumbnail
/// (`Some(250)`, `Some(500)` or `Some(1200)` pixels wide).
pub fn fetch_cover_art_sized(mb_release_id: &str, size: Option<u32>, debug: bool) -> Option<(Vec<u8>, &'static str)> {
    // CAA redirects to the actual image — ureq follows redirects automatically.
    let url = match size {
        Some(px) => format!("https://coverartarchive.org/release/{}/front-{}", mb_release_id, px),
        None => format!("https://coverartarchive.org/release/{}/front", mb_release_id),
    };
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
        "{}/discid/{}?inc=recordings+artists+release-groups&fmt=json",
        MB_API, discid
    );

    if debug { eprintln!("MusicBrainz lookup: {}", url); }

    let response = mb_call(|| ureq::get(&url).set("User-Agent", USER_AGENT));

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
    let url = format!("{}/release/{}?inc=recordings+artist-credits+discids+release-groups&fmt=json", MB_API, mbid);
    if debug { eprintln!("MusicBrainz release lookup: {}", url); }

    let release: MbRelease = match mb_call(|| ureq::get(&url).set("User-Agent", USER_AGENT)) {
        Ok(resp) => resp.into_json().map_err(|e| Error::backend(format!("MusicBrainz response parse error: {}", e)))?,
        Err(ureq::Error::Status(404, _)) => {
            return Err(Error::validation(format!("MusicBrainz has no release with ID {}", mbid)));
        }
        Err(ureq::Error::Status(429 | 503, _)) => {
            return Err(Error::backend("MusicBrainz is busy or rate limiting requests — try again in a few seconds"));
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

// ── Searching releases ───────────────────────────────────────────────────────

/// What the user typed into a release search.
#[derive(Debug, Clone, Default)]
pub struct SearchQuery {
    /// Free text: every word must appear in the release title or the artist name.
    pub text: String,
    /// Optional artist that must match as well.
    pub artist: String,
    /// Only releases with a medium of exactly this many tracks.
    pub tracks: Option<usize>,
    /// Only releases on CD.
    pub cd_only: bool,
    pub offset: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchHit {
    pub mb_release_id: String,
    pub title: String,
    pub artist: String,
    pub date: Option<String>,
    pub year: Option<String>,
    pub country: Option<String>,
    pub status: Option<String>,
    pub label: Option<String>,
    pub catalog_number: Option<String>,
    pub barcode: Option<String>,
    pub disambiguation: Option<String>,
    /// e.g. "CD" or "2×CD"
    pub format: String,
    /// Track count per disc, e.g. [12] or [17, 15]
    pub disc_track_counts: Vec<usize>,
    /// MusicBrainz relevance score, 0-100
    pub score: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchResults {
    pub count: usize,
    pub offset: usize,
    pub releases: Vec<SearchHit>,
}

#[derive(Deserialize)]
struct MbSearchResponse {
    #[serde(default)]
    count: usize,
    #[serde(default)]
    offset: usize,
    #[serde(default)]
    releases: Vec<MbSearchRelease>,
}

#[derive(Deserialize)]
struct MbSearchRelease {
    id: String,
    #[serde(default)]
    score: Option<u32>,
    title: String,
    status: Option<String>,
    date: Option<String>,
    country: Option<String>,
    barcode: Option<String>,
    disambiguation: Option<String>,
    #[serde(rename = "artist-credit", default)]
    artist_credit: Vec<MbArtistCredit>,
    #[serde(rename = "label-info", default)]
    label_info: Vec<MbLabelInfo>,
    #[serde(default)]
    media: Vec<MbSearchMedia>,
}

#[derive(Deserialize)]
struct MbLabelInfo {
    #[serde(rename = "catalog-number")]
    catalog_number: Option<String>,
    label: Option<MbLabel>,
}

#[derive(Deserialize)]
struct MbLabel {
    name: Option<String>,
}

#[derive(Deserialize)]
struct MbSearchMedia {
    format: Option<String>,
    #[serde(rename = "track-count", default)]
    track_count: usize,
}

/// Escape characters that mean something to the Lucene query parser.
fn lucene_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if "+-&|!(){}[]^\"~*?:/".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn words(s: &str) -> Vec<String> {
    s.split_whitespace()
        .map(lucene_escape)
        .filter(|w| !w.is_empty())
        .collect()
}

/// Build the MusicBrainz search string. Every word must appear in the release title or the
/// artist, so "junior mafia conspiracy" finds the album whichever field each word is in.
pub fn build_search_query(q: &SearchQuery) -> Option<String> {
    let mut parts: Vec<String> = words(&q.text)
        .iter()
        .map(|w| format!("(release:{w} OR artist:{w} OR artistname:{w})"))
        .collect();
    let artist = words(&q.artist);
    if !artist.is_empty() {
        parts.push(format!("artist:({})", artist.join(" AND ")));
    }
    if parts.is_empty() {
        return None;
    }
    if let Some(n) = q.tracks {
        parts.push(format!("tracksmedium:{n}"));
    }
    if q.cd_only {
        parts.push("format:CD".to_string());
    }
    Some(parts.join(" AND "))
}

/// Search MusicBrainz for releases.
pub fn search_releases(q: &SearchQuery, debug: bool) -> Result<SearchResults, Error> {
    let Some(query) = build_search_query(q) else {
        return Err(Error::validation("Type an album or artist to search for"));
    };
    if debug { eprintln!("MusicBrainz search: {}", query); }

    let search_url = format!("{}/release", MB_API);
    let offset = q.offset.to_string();
    let resp = mb_call(|| {
        ureq::get(&search_url)
            .set("User-Agent", USER_AGENT)
            .query("query", &query)
            .query("fmt", "json")
            .query("limit", "20")
            .query("offset", &offset)
    });

    let parsed: MbSearchResponse = match resp {
        Ok(r) => r.into_json().map_err(|e| Error::backend(format!("MusicBrainz response parse error: {}", e)))?,
        Err(ureq::Error::Status(429 | 503, _)) => {
            return Err(Error::backend("MusicBrainz is busy or rate limiting requests — wait a few seconds and try again"));
        }
        Err(ureq::Error::Status(code, _)) => {
            return Err(Error::backend(format!("MusicBrainz returned HTTP {}", code)));
        }
        Err(e) => return Err(Error::backend(format!("Could not reach MusicBrainz: {}", e))),
    };

    let releases = parsed.releases.iter().map(|r| {
        let date = r.date.clone().filter(|d| !d.is_empty());
        let year = date.as_deref().and_then(|d| d.split('-').next()).filter(|y| y.len() == 4).map(String::from);
        let counts: Vec<usize> = r.media.iter().map(|m| m.track_count).collect();
        let mut formats: Vec<&str> = r.media.iter().map(|m| m.format.as_deref().unwrap_or("?")).collect();
        formats.dedup();
        let format = match (r.media.len(), formats.as_slice()) {
            (0, _) => String::new(),
            (1, [f]) => (*f).to_string(),
            (n, [f]) => format!("{n}×{f}"),
            (n, _) => format!("{n} discs"),
        };
        let li = r.label_info.first();
        SearchHit {
            mb_release_id: r.id.clone(),
            title: r.title.clone(),
            artist: artist_name(&r.artist_credit),
            date,
            year,
            country: r.country.clone().filter(|c| !c.is_empty()),
            status: r.status.clone(),
            label: li.and_then(|l| l.label.as_ref()).and_then(|l| l.name.clone()),
            catalog_number: li.and_then(|l| l.catalog_number.clone()),
            barcode: r.barcode.clone().filter(|b| !b.is_empty()),
            disambiguation: r.disambiguation.clone().filter(|d| !d.is_empty()),
            format,
            disc_track_counts: counts,
            score: r.score.unwrap_or(0),
        }
    }).collect();

    Ok(SearchResults { count: parsed.count, offset: parsed.offset, releases })
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
                ..Default::default()
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
        mb_release_group_id: r.release_group.as_ref().map(|g| g.id.clone()),
        total_releases,
        ..Default::default()
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

    #[test]
    fn builds_search_queries() {
        let q = SearchQuery { text: "junior mafia".into(), ..Default::default() };
        assert_eq!(
            build_search_query(&q).unwrap(),
            "(release:junior OR artist:junior OR artistname:junior) AND (release:mafia OR artist:mafia OR artistname:mafia)"
        );
        let q = SearchQuery { text: "".into(), artist: "The Notorious B.I.G.".into(), tracks: Some(12), cd_only: true, offset: 0 };
        assert_eq!(build_search_query(&q).unwrap(), "artist:(The AND Notorious AND B.I.G.) AND tracksmedium:12 AND format:CD");
        // Query syntax is escaped so user text can't change the search.
        let q = SearchQuery { text: "a+b (c) \"d\" e:f".into(), ..Default::default() };
        let built = build_search_query(&q).unwrap();
        assert!(built.contains("release:a\\+b"));
        assert!(built.contains("release:\\(c\\)"));
        assert!(built.contains("release:e\\:f"));
        assert!(build_search_query(&SearchQuery::default()).is_none());
    }

    /// Hits the real MusicBrainz API; run with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn live_search() {
        let r = search_releases(&SearchQuery { text: "junior mafia conspiracy".into(), cd_only: true, ..Default::default() }, false).unwrap();
        println!("{} results", r.count);
        for h in r.releases.iter().take(5) { println!("{} — {} [{}] {:?} {:?}", h.title, h.artist, h.format, h.disc_track_counts, h.year); }
        assert!(!r.releases.is_empty());
    }

    #[test]
    fn limiter_spaces_requests_and_refuses_long_queues() {
        let l = Limiter::new(Duration::from_millis(200), Instant::now());
        let first = l.reserve(Duration::from_secs(5)).unwrap();
        let second = l.reserve(Duration::from_secs(5)).unwrap();
        let third = l.reserve(Duration::from_secs(5)).unwrap();
        assert!(first < Duration::from_millis(20), "first request goes straight away");
        assert!(second >= Duration::from_millis(150) && second <= Duration::from_millis(210), "{second:?}");
        assert!(third >= Duration::from_millis(350) && third <= Duration::from_millis(410), "{third:?}");
        // A queue longer than the caller will accept is refused without claiming a slot.
        assert!(l.reserve(Duration::from_millis(100)).is_none());
        let fourth = l.reserve(Duration::from_secs(5)).unwrap();
        assert!(fourth >= Duration::from_millis(550), "{fourth:?}");
    }

    #[test]
    fn limiter_recovers_after_idle_time() {
        let l = Limiter::new(Duration::from_millis(50), Instant::now());
        l.reserve(Duration::from_secs(1)).unwrap();
        std::thread::sleep(Duration::from_millis(80));
        assert!(l.reserve(Duration::from_secs(1)).unwrap() < Duration::from_millis(10));
    }

    #[test]
    fn retry_delay_honours_retry_after_and_backs_off() {
        assert_eq!(retry_delay(Some("5"), 0), Duration::from_secs(5));
        assert_eq!(retry_delay(Some(" 3 "), 2), Duration::from_secs(3));
        assert_eq!(retry_delay(Some("9999"), 0), Duration::from_secs(30)); // capped
        assert_eq!(retry_delay(None, 0), Duration::from_secs(2));
        assert_eq!(retry_delay(None, 1), Duration::from_secs(4));
        assert_eq!(retry_delay(None, 2), Duration::from_secs(8));
        assert_eq!(retry_delay(Some("soon"), 1), Duration::from_secs(4)); // unparsable → backoff
    }

    /// Fires several real requests at once; they must be spaced out, not rejected.
    /// Run with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn live_burst_is_throttled() {
        let start = Instant::now();
        let handles: Vec<_> = (0..4).map(|_| std::thread::spawn(|| {
            lookup_release("bc8d517f-6ce0-4e45-b6d8-af0f29cdd1ea", None, None, false).is_ok()
        })).collect();
        let ok = handles.into_iter().map(|h| h.join().unwrap()).filter(|&b| b).count();
        let took = start.elapsed();
        println!("{ok}/4 ok in {took:?}");
        assert_eq!(ok, 4);
        assert!(took >= Duration::from_millis(3200), "4 requests must take >= 3 intervals, took {took:?}");
    }
}
