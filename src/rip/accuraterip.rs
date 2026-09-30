//! AccurateRip verification.
//!
//! After cdparanoia has extracted the tracks (and before they are encoded), the
//! rip is compared with the AccurateRip database: the disc is looked up by IDs
//! derived from its table of contents, each track gets a v1 and v2 checksum, and
//! the checksums are matched against what other people ripped from the same
//! pressing.
//!
//! cdparanoia does not correct for the drive's read offset, so a rip is often
//! shifted by a few hundred samples compared with the reference. Tracks are
//! therefore also matched at every shift of up to +/-2939 samples, using prefix
//! sums so the search is cheap. Matching at a non-zero shift still proves the
//! audio is identical; it also reveals the drive's offset. The audio itself is
//! not modified.
//!
//! Everything here is best-effort: any failure is reported and the rip carries on.

use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    time::Duration,
};

use serde::Serialize;

use super::engine::{emit_step, emit_progress};
use crate::{analyzer::{DiscInfo, TrackKind}, error::Error};

const SAMPLES_PER_SECTOR: usize = 588;
/// The first track's first 5 sectors and the last track's last 5 sectors are excluded
/// from the checksum, because drive offsets make those samples unreliable.
const EDGE_WORDS: usize = 5 * SAMPLES_PER_SECTOR;
/// Largest sample shift that can be tested without touching the excluded edges.
pub const MAX_SHIFT: usize = EDGE_WORDS - 1;

const AR_BASE: &str = "http://www.accuraterip.com/accuraterip";
const USER_AGENT: &str = "RustyDisc/0.1 ( https://github.com/WB2024/DiscCTL )";

// ── Table of contents and disc IDs ───────────────────────────────────────────

/// The parts of the TOC that AccurateRip needs. All offsets are LSNs (track 1 at 0
/// for a disc without a pregap), which is what `DiscInfo` stores.
#[derive(Debug, Clone)]
pub struct Toc {
    /// (CD track number, start LSN) of every audio track, in order.
    pub audio: Vec<(usize, u32)>,
    /// Start of every track, audio and data, in order (needed for the CDDB ID).
    pub all_offsets: Vec<u32>,
    pub leadout: u32,
}

impl Toc {
    pub fn from_info(info: &DiscInfo) -> Option<Toc> {
        let mut tracks: Vec<_> = info.sessions.iter().flat_map(|s| s.tracks.iter()).collect();
        tracks.sort_by_key(|t| t.number);
        let last = tracks.last()?;
        // If the lead-out could not be read, lba_end is a placeholder just past the start.
        if last.lba_end < last.lba_start + 300 {
            return None;
        }
        let audio: Vec<(usize, u32)> = tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Audio)
            .map(|t| (t.number, t.lba_start))
            .collect();
        if audio.is_empty() {
            return None;
        }
        Some(Toc {
            audio,
            all_offsets: tracks.iter().map(|t| t.lba_start).collect(),
            leadout: last.lba_end,
        })
    }

    /// AccurateRip disc IDs 1 and 2.
    pub fn ids(&self) -> (u32, u32) {
        let mut id1 = 0u32;
        let mut id2 = 0u32;
        for (i, &(_, off)) in self.audio.iter().enumerate() {
            id1 = id1.wrapping_add(off);
            id2 = id2.wrapping_add(off.max(1).wrapping_mul(i as u32 + 1));
        }
        id1 = id1.wrapping_add(self.leadout);
        id2 = id2.wrapping_add(self.leadout.wrapping_mul(self.audio.len() as u32 + 1));
        (id1, id2)
    }

    /// FreeDB / CDDB disc ID. Unlike the two IDs above it counts data tracks too.
    pub fn cddb_id(&self) -> u32 {
        fn digit_sum(mut n: u32) -> u32 {
            let mut s = 0;
            while n > 0 {
                s += n % 10;
                n /= 10;
            }
            s
        }
        let n: u32 = self.all_offsets.iter().map(|o| digit_sum((o + 150) / 75)).sum();
        let first = self.all_offsets.first().copied().unwrap_or(0);
        let t = (self.leadout / 75).saturating_sub(first / 75);
        ((n % 255) << 24) | (t << 8) | self.all_offsets.len() as u32
    }

    /// Path of this disc's entry in the database, relative to the base URL.
    pub fn db_path(&self) -> String {
        let (id1, id2) = self.ids();
        let h = format!("{:08x}", id1);
        let b = h.as_bytes();
        format!(
            "{}/{}/{}/dBAR-{:03}-{:08x}-{:08x}-{:08x}.bin",
            b[7] as char, b[6] as char, b[5] as char,
            self.audio.len(), id1, id2, self.cddb_id()
        )
    }
}

// ── Database response ────────────────────────────────────────────────────────

/// One pressing's entry: a (confidence, checksum) pair per audio track.
#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    pub tracks: Vec<(u8, u32)>,
}

/// Parse the binary response. Entries whose track count differs from `expected_tracks`
/// (or that are cut short) are ignored.
pub fn parse_responses(mut data: &[u8], expected_tracks: usize) -> Vec<Response> {
    let mut out = Vec::new();
    while let Some(&n) = data.first() {
        let n = n as usize;
        let len = 13 + n * 9;
        if data.len() < len {
            break;
        }
        if n == expected_tracks {
            let tracks = (0..n)
                .map(|i| {
                    let p = 13 + i * 9;
                    let crc = u32::from_le_bytes([data[p + 1], data[p + 2], data[p + 3], data[p + 4]]);
                    (data[p], crc)
                })
                .collect();
            out.push(Response { tracks });
        }
        data = &data[len..];
    }
    out
}

/// Fetch the disc's database entry. `Ok(None)` means the disc is not in the database.
pub fn lookup(toc: &Toc, debug: bool) -> Result<Option<(String, Vec<Response>)>, Error> {
    let url = format!("{}/{}", AR_BASE, toc.db_path());
    if debug {
        eprintln!("AccurateRip: GET {}", url);
    }
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(15))
        .user_agent(USER_AGENT)
        .build();
    match agent.get(&url).call() {
        Ok(resp) => {
            let mut buf = Vec::new();
            resp.into_reader().take(4 << 20).read_to_end(&mut buf)?;
            let responses = parse_responses(&buf, toc.audio.len());
            Ok(if responses.is_empty() { None } else { Some((url, responses)) })
        }
        Err(ureq::Error::Status(404, _)) => Ok(None),
        Err(e) => Err(Error::backend(format!("AccurateRip lookup failed: {e}"))),
    }
}

// ── Checksums ────────────────────────────────────────────────────────────────

/// Half-open range of word indices that take part in the checksum.
fn window(n: usize, first: bool, last: bool) -> (usize, usize) {
    let a = if first { EDGE_WORDS - 1 } else { 0 };
    let b = if last { n.saturating_sub(EDGE_WORDS) } else { n };
    (a, b.max(a))
}

/// AccurateRip v1 checksum of a track's stereo samples (one `u32` per L/R pair).
pub fn checksum_v1(words: &[u32], first: bool, last: bool) -> u32 {
    let (a, b) = window(words.len(), first, last);
    let mut crc = 0u32;
    for i in a..b {
        crc = crc.wrapping_add((i as u32 + 1).wrapping_mul(words[i]));
    }
    crc
}

/// AccurateRip v2 checksum: like v1, but the high half of each 64-bit product is kept.
pub fn checksum_v2(words: &[u32], first: bool, last: bool) -> u32 {
    let (a, b) = window(words.len(), first, last);
    let mut crc = 0u32;
    for i in a..b {
        let p = words[i] as u64 * (i as u64 + 1);
        crc = crc.wrapping_add(p as u32).wrapping_add((p >> 32) as u32);
    }
    crc
}

/// v1 checksums of a track at every shift in -MAX_SHIFT..=MAX_SHIFT.
///
/// `y` is the track with MAX_SHIFT extra samples on each side (neighbouring audio, or
/// silence past the ends of the disc). Element `s + MAX_SHIFT` of the result is the
/// checksum of the track as it would be after shifting by `s` samples.
fn checksum_v1_all_shifts(y: &[u32], first: bool, last: bool) -> Vec<u32> {
    let r = MAX_SHIFT;
    let n = y.len() - 2 * r;
    let (a, b) = window(n, first, last);
    if a >= b {
        return vec![0; 2 * r + 1];
    }

    // Prefix sums P0[k] = sum(y[j], j<k) and P1[k] = sum(j*y[j], j<k), needed only in
    // two narrow bands, so they are computed on the fly instead of for the whole track.
    let band = |kmin: usize| -> Vec<(u32, u32)> {
        let (mut p0, mut p1) = (0u32, 0u32);
        for (j, &v) in y[..kmin].iter().enumerate() {
            p0 = p0.wrapping_add(v);
            p1 = p1.wrapping_add((j as u32).wrapping_mul(v));
        }
        let mut out = Vec::with_capacity(2 * r + 1);
        out.push((p0, p1));
        for k in kmin..kmin + 2 * r {
            p0 = p0.wrapping_add(y[k]);
            p1 = p1.wrapping_add((k as u32).wrapping_mul(y[k]));
            out.push((p0, p1));
        }
        out
    };
    let lo = band(a);
    let hi = band(b);

    (0..=2 * r)
        .map(|idx| {
            let s = idx as i64 - r as i64;
            let sum0 = hi[idx].0.wrapping_sub(lo[idx].0);
            let sum1 = hi[idx].1.wrapping_sub(lo[idx].1);
            // multiplier of y[j] is j - (R + s - 1)
            let c = (r as i64 + s - 1) as u32;
            sum1.wrapping_sub(c.wrapping_mul(sum0))
        })
        .collect()
}

// ── Per-track verification ───────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    pub confidence: u8,
    pub version: u8,
    pub shift: i32,
}

fn best_match(entries: &[(u8, u32)], v1: u32, v2: u32, shift: i32) -> Option<Match> {
    entries
        .iter()
        .filter_map(|&(conf, crc)| {
            if crc == v2 {
                Some(Match { confidence: conf, version: 2, shift })
            } else if crc == v1 {
                Some(Match { confidence: conf, version: 1, shift })
            } else {
                None
            }
        })
        .max_by_key(|m| m.confidence)
}

/// Verify one track. `y` is the track with MAX_SHIFT samples of context on each side.
fn verify_track(y: &[u32], first: bool, last: bool, entries: &[(u8, u32)]) -> (u32, u32, Option<Match>) {
    let r = MAX_SHIFT;
    let n = y.len() - 2 * r;
    let track = &y[r..r + n];
    let (v1, v2) = (checksum_v1(track, first, last), checksum_v2(track, first, last));

    if let Some(m) = best_match(entries, v1, v2, 0) {
        return (v1, v2, Some(m));
    }
    if entries.is_empty() {
        return (v1, v2, None);
    }

    let fast = checksum_v1_all_shifts(y, first, last);
    for d in 1..=r as i32 {
        for s in [d, -d] {
            let candidate = fast[(s + r as i32) as usize];
            if entries.iter().any(|&(_, crc)| crc == candidate) {
                let start = (r as i32 + s) as usize;
                let z = &y[start..start + n];
                if let Some(m) = best_match(entries, checksum_v1(z, first, last), checksum_v2(z, first, last), s) {
                    return (v1, v2, Some(m));
                }
            }
        }
    }
    (v1, v2, None)
}

/// Find the shift at which a track matches a v2 database checksum.
///
/// v2 can't be searched with prefix sums like v1, so shifts are tried one by one, nearest
/// to zero first (real drive offsets are usually small) and spread over several threads.
/// `y` is the track with MAX_SHIFT samples of context on each side.
fn find_shift_v2(y: &[u32], first: bool, last: bool, targets: &[u32]) -> Option<i32> {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let r = MAX_SHIFT;
    let n = y.len() - 2 * r;
    let order: Vec<i32> = (1..=r as i32).flat_map(|d| [d, -d]).collect();
    let next = AtomicUsize::new(0);
    let best = AtomicUsize::new(usize::MAX);
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).min(8);
    // Long tracks make every shift expensive; don't let a hopeless search hold up the rip.
    let deadline = std::time::Instant::now() + Duration::from_secs(45);

    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= order.len() || i >= best.load(Ordering::Relaxed) || std::time::Instant::now() > deadline {
                    break;
                }
                let start = (r as i32 + order[i]) as usize;
                if targets.contains(&checksum_v2(&y[start..start + n], first, last)) {
                    best.fetch_min(i, Ordering::Relaxed);
                }
            });
        }
    });

    let i = best.load(Ordering::Relaxed);
    order.get(i).copied()
}

// ── WAV access ───────────────────────────────────────────────────────────────

struct Wav {
    path: String,
    data_start: u64,
    words: usize,
}

fn open_wav(path: &str) -> Result<Wav, Error> {
    let mut f = File::open(path)?;
    let mut riff = [0u8; 12];
    f.read_exact(&mut riff)?;
    if &riff[0..4] != b"RIFF" || &riff[8..12] != b"WAVE" {
        return Err(Error::backend(format!("{path} is not a WAV file")));
    }
    let file_len = f.metadata()?.len();
    loop {
        let mut ch = [0u8; 8];
        f.read_exact(&mut ch)
            .map_err(|_| Error::backend(format!("{path} has no audio data")))?;
        let size = u32::from_le_bytes([ch[4], ch[5], ch[6], ch[7]]) as u64;
        if &ch[0..4] == b"data" {
            let start = f.stream_position()?;
            let avail = file_len.saturating_sub(start);
            return Ok(Wav { path: path.to_string(), data_start: start, words: (size.min(avail) / 4) as usize });
        }
        f.seek(SeekFrom::Current((size + (size & 1)) as i64))?;
    }
}

impl Wav {
    fn read(&self, from: usize, count: usize) -> Result<Vec<u32>, Error> {
        let count = count.min(self.words.saturating_sub(from));
        let mut f = File::open(&self.path)?;
        f.seek(SeekFrom::Start(self.data_start + from as u64 * 4))?;
        let mut out = Vec::with_capacity(count);
        let mut buf = vec![0u8; 1 << 20];
        let mut left = count * 4;
        while left > 0 {
            let want = left.min(buf.len());
            f.read_exact(&mut buf[..want])?;
            out.extend(buf[..want].chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])));
            left -= want;
        }
        Ok(out)
    }
}

// ── Report ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct TrackReport {
    pub track: usize,
    /// "verified", "not_verified" or "skipped"
    pub status: String,
    pub confidence: Option<u8>,
    /// Which checksum version matched (1 or 2)
    pub version: Option<u8>,
    /// Sample shift at which the rip matched the database (0 = exact position)
    pub shift_samples: Option<i32>,
    pub v1: String,
    pub v2: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    /// False if the disc has no entry in the database
    pub found: bool,
    pub database_url: String,
    pub pressings: usize,
    pub verified: usize,
    pub total: usize,
    /// Set when every verified track matched at the same non-zero shift, which suggests
    /// the drive's read offset (the audio is not corrected for it).
    pub detected_shift_samples: Option<i32>,
    pub tracks: Vec<TrackReport>,
}

fn detected_shift(tracks: &[TrackReport]) -> Option<i32> {
    let mut shifts = tracks.iter().filter(|t| t.status == "verified").filter_map(|t| t.shift_samples);
    let first = shifts.next()?;
    (first != 0 && shifts.all(|s| s == first)).then_some(first)
}

/// The most common non-zero shift among verified tracks.
fn consensus_shift(tracks: &[TrackReport]) -> Option<i32> {
    let mut counts: Vec<(i32, usize)> = Vec::new();
    for s in tracks.iter().filter(|t| t.status == "verified").filter_map(|t| t.shift_samples).filter(|s| *s != 0) {
        match counts.iter_mut().find(|(v, _)| *v == s) {
            Some((_, c)) => *c += 1,
            None => counts.push((s, 1)),
        }
    }
    counts.into_iter().max_by_key(|&(s, c)| (c, std::cmp::Reverse(s.abs()))).map(|(s, _)| s)
}

fn describe(t: &TrackReport) -> String {
    match t.status.as_str() {
        "verified" => {
            let shift = match t.shift_samples {
                Some(s) if s != 0 => format!(", matched at a {s:+} sample shift"),
                _ => String::new(),
            };
            format!(
                "Track {:02}: accurately ripped (v{}, confidence {}{})",
                t.track, t.version.unwrap_or(0), t.confidence.unwrap_or(0), shift
            )
        }
        "skipped" => format!("Track {:02}: not checked (no audio file)", t.track),
        _ => format!("Track {:02}: no match in the database (a damaged read, a different pressing, or an uncorrected drive offset)", t.track),
    }
}

/// Verify every audio track of a rip against the database responses.
fn verify_disc(toc: &Toc, responses: &[Response], wavs: &[(usize, String)], debug: bool) -> Vec<TrackReport> {
    let wav_for = |num: usize| wavs.iter().find(|(n, _)| *n == num).map(|(_, p)| p.as_str());
    let total = toc.audio.len();

    // A track with MAX_SHIFT samples of neighbouring audio on each side, so it can be
    // examined at any small shift. Past the ends of the disc the context is silence.
    let load_context = |idx: usize| -> Result<Option<Vec<u32>>, Error> {
        let Some(path) = wav_for(toc.audio[idx].0) else { return Ok(None) };
        let this = open_wav(path)?;
        let r = MAX_SHIFT;
        let mut y = vec![0u32; r];
        if idx > 0 {
            if let Some(prev) = wav_for(toc.audio[idx - 1].0).map(open_wav).transpose()? {
                let tail = prev.read(prev.words.saturating_sub(r), r)?;
                let at = r - tail.len();
                y[at..].copy_from_slice(&tail);
            }
        }
        y.extend(this.read(0, this.words)?);
        let mut after = vec![0u32; r];
        if idx + 1 < total {
            if let Some(next) = wav_for(toc.audio[idx + 1].0).map(open_wav).transpose()? {
                let head = next.read(0, r)?;
                after[..head.len()].copy_from_slice(&head);
            }
        }
        y.extend(after);
        Ok(Some(y))
    };
    let entries_for = |idx: usize| -> Vec<(u8, u32)> { responses.iter().map(|resp| resp.tracks[idx]).collect() };

    let mut tracks = Vec::with_capacity(total);
    for idx in 0..total {
        let num = toc.audio[idx].0;
        let skipped = |why: &str| {
            if debug { eprintln!("AccurateRip: track {num}: {why}"); }
            TrackReport { track: idx + 1, status: "skipped".into(), confidence: None, version: None, shift_samples: None, v1: String::new(), v2: String::new() }
        };
        tracks.push(match load_context(idx) {
            Ok(Some(y)) => {
                let (v1, v2, m) = verify_track(&y, idx == 0, idx + 1 == total, &entries_for(idx));
                TrackReport {
                    track: idx + 1,
                    status: if m.is_some() { "verified" } else { "not_verified" }.into(),
                    confidence: m.as_ref().map(|m| m.confidence),
                    version: m.as_ref().map(|m| m.version),
                    shift_samples: m.as_ref().map(|m| m.shift),
                    v1: format!("{v1:08x}"),
                    v2: format!("{v2:08x}"),
                }
            }
            Ok(None) => skipped("no WAV"),
            Err(e) => skipped(&e.to_string()),
        });
    }

    // The drive's read offset is the same for the whole disc, so a shift found on some
    // tracks is tried on the others. If no track matched (typically a database entry that
    // holds v2 checksums, which can't be searched cheaply), look for the shift on one or
    // two short tracks first.
    let mut shift_hint = consensus_shift(&tracks);
    if shift_hint.is_none() && tracks.iter().all(|t| t.status != "verified") {
        let mut candidates: Vec<usize> = (0..total).filter(|&i| tracks[i].status == "not_verified").collect();
        if total > 2 {
            candidates.retain(|&i| i != 0 && i + 1 != total); // edges are partly excluded from the checksum
        }
        let len_of = |i: usize| wav_for(toc.audio[i].0).and_then(|p| open_wav(p).ok()).map(|w| w.words).unwrap_or(usize::MAX);
        candidates.sort_by_key(|&i| len_of(i));
        for idx in candidates.into_iter().take(2) {
            if let Ok(Some(y)) = load_context(idx) {
                let targets: Vec<u32> = entries_for(idx).iter().map(|e| e.1).collect();
                if let Some(s) = find_shift_v2(&y, idx == 0, idx + 1 == total, &targets) {
                    if debug { eprintln!("AccurateRip: track {} matched a v2 checksum at a {:+} sample shift", idx + 1, s); }
                    shift_hint = Some(s);
                    break;
                }
            }
        }
    }
    if let Some(shift) = shift_hint {
        for idx in 0..total {
            if tracks[idx].status != "not_verified" {
                continue;
            }
            if let Ok(Some(y)) = load_context(idx) {
                let (first, last) = (idx == 0, idx + 1 == total);
                let n = y.len() - 2 * MAX_SHIFT;
                let start = (MAX_SHIFT as i32 + shift) as usize;
                let z = &y[start..start + n];
                if let Some(m) = best_match(&entries_for(idx), checksum_v1(z, first, last), checksum_v2(z, first, last), shift) {
                    let t = &mut tracks[idx];
                    t.status = "verified".into();
                    t.confidence = Some(m.confidence);
                    t.version = Some(m.version);
                    t.shift_samples = Some(m.shift);
                }
            }
        }
    }

    tracks
}

// ── Entry point ──────────────────────────────────────────────────────────────

/// Check a finished rip against AccurateRip. Never fails the rip: problems are
/// reported and `None` is returned.
pub fn check(info: &DiscInfo, wavs: &[(usize, String)], debug: bool, progress_json: bool) -> Option<Report> {
    let say = |msg: &str| {
        if progress_json { emit_step(msg) } else { eprintln!("{msg}") }
    };

    let Some(toc) = Toc::from_info(info) else {
        say("AccurateRip: skipped (table of contents incomplete)");
        return None;
    };
    say("Checking rip against the AccurateRip database...");

    let (url, responses) = match lookup(&toc, debug) {
        Ok(Some(found)) => found,
        Ok(None) => {
            say("AccurateRip: this disc is not in the database");
            return Some(Report {
                found: false,
                database_url: format!("{}/{}", AR_BASE, toc.db_path()),
                pressings: 0,
                verified: 0,
                total: toc.audio.len(),
                detected_shift_samples: None,
                tracks: Vec::new(),
            });
        }
        Err(e) => {
            say(&format!("AccurateRip: {e} (continuing without it)"));
            return None;
        }
    };

    let total = toc.audio.len();
    let tracks = verify_disc(&toc, &responses, wavs, debug);
    let verified = tracks.iter().filter(|t| t.status == "verified").count();
    let report = Report {
        found: true,
        database_url: url,
        pressings: responses.len(),
        verified,
        total,
        detected_shift_samples: detected_shift(&tracks),
        tracks,
    };

    if progress_json {
        emit_step(&format!("AccurateRip: {verified} of {total} tracks verified"));
        if let Ok(json) = serde_json::to_string(&serde_json::json!({"type": "accuraterip", "report": &report})) {
            println!("{json}");
        }
        emit_progress(85.0);
    } else {
        eprintln!("AccurateRip: {verified} of {total} tracks verified");
        for t in &report.tracks {
            eprintln!("  {}", describe(t));
        }
    }
    if let Some(s) = report.detected_shift_samples {
        say(&format!("AccurateRip: every verified track matched at a {s:+} sample shift — this looks like your drive's read offset (audio not corrected)"));
    }
    Some(report)
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic pseudo-random audio.
    fn noise(n: usize, seed: u32) -> Vec<u32> {
        let mut x = seed;
        (0..n)
            .map(|_| {
                x = x.wrapping_mul(1664525).wrapping_add(1013904223);
                x
            })
            .collect()
    }

    fn toc_from_lengths(lens: &[u32], pregap: u32, data: u32) -> Toc {
        let mut offsets = Vec::new();
        let mut pos = pregap;
        for l in lens {
            offsets.push(pos);
            pos += l;
        }
        let audio = offsets.iter().enumerate().map(|(i, &o)| (i + 1, o)).collect();
        let mut all = offsets.clone();
        let mut leadout = pos;
        if data > 0 {
            let d = pos + 11400; // gap between the audio and data sessions
            all.push(d);
            leadout = d + data;
        }
        Toc { audio, all_offsets: all, leadout }
    }

    // Disc IDs and database paths from real discs (ARver's test suite).
    #[test]
    fn disc_ids_match_known_discs() {
        let cases: &[(&[u32], u32, u32, &str)] = &[
            (&[279037], 0, 0, "001-000441fd-000883fb-020e8801"),
            (&[75258, 54815, 205880], 0, 0, "003-00084264-001cc184-19117f03"),
            (&[107450, 71470, 105737, 71600], 33, 0, "004-000e26d9-00380804-3e128e04"),
            (&[143963], 32, 0, "001-0002329b-00046516-02077f01"),
            (&[12617, 27720, 22738, 30185, 24705, 33750, 32475, 30920, 32195, 22880], 0, 52066, "010-00164419-00b9f6e2-9e11600b"),
        ];
        for (lens, pregap, data, expected) in cases {
            let toc = toc_from_lengths(lens, *pregap, *data);
            let (id1, id2) = toc.ids();
            assert_eq!(format!("{:03}-{:08x}-{:08x}-{:08x}", toc.audio.len(), id1, id2, toc.cddb_id()), *expected);
        }
    }

    #[test]
    fn db_path_uses_reversed_id_digits() {
        let toc = toc_from_lengths(&[279037], 0, 0);
        assert_eq!(toc.db_path(), "d/f/1/dBAR-001-000441fd-000883fb-020e8801.bin");
    }

    // Expected values produced by the reference C implementation (accuraterip-checksum).
    #[test]
    fn checksums_match_reference_implementation() {
        let d = noise(20000, 12345);
        let cases = [
            (true, true, 0x9842b73c, 0x9c6f49bb),
            (true, false, 0x2e1e669c, 0x33ec3862),
            (false, false, 0xb6354940, 0xbc23a670),
            (false, true, 0x205999e0, 0x24a6b7c9),
        ];
        for (first, last, v1, v2) in cases {
            assert_eq!(checksum_v1(&d, first, last), v1, "v1 first={first} last={last}");
            assert_eq!(checksum_v2(&d, first, last), v2, "v2 first={first} last={last}");
        }
    }

    #[test]
    fn parses_multiple_pressings_and_ignores_truncation() {
        let mut data = Vec::new();
        for (conf, crc) in [(5u8, 0x11223344u32), (9, 0xdeadbeef)] {
            data.extend([2u8]);
            data.extend(1u32.to_le_bytes());
            data.extend(2u32.to_le_bytes());
            data.extend(3u32.to_le_bytes());
            for t in 0..2u32 {
                data.push(conf);
                data.extend((crc + t).to_le_bytes());
                data.extend(0u32.to_le_bytes());
            }
        }
        let r = parse_responses(&data, 2);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].tracks, vec![(5, 0x11223344), (5, 0x11223345)]);
        assert_eq!(r[1].tracks[0], (9, 0xdeadbeef));
        assert_eq!(parse_responses(&data[..data.len() - 3], 2).len(), 1);
        assert!(parse_responses(&data, 3).is_empty());
    }

    // The prefix-sum shift search must agree with computing the checksum directly.
    #[test]
    fn shift_search_matches_direct_computation() {
        let y = noise(9000 + 2 * MAX_SHIFT, 99);
        let n = 9000;
        for (first, last) in [(false, false), (true, false), (false, true), (true, true)] {
            let fast = checksum_v1_all_shifts(&y, first, last);
            for s in [-(MAX_SHIFT as i32), -2000, -1, 0, 1, 37, 1500, MAX_SHIFT as i32] {
                let start = (MAX_SHIFT as i32 + s) as usize;
                let direct = checksum_v1(&y[start..start + n], first, last);
                assert_eq!(fast[(s + MAX_SHIFT as i32) as usize], direct, "shift {s} first={first} last={last}");
            }
        }
    }

    #[test]
    fn verifies_at_exact_position_and_at_a_shift() {
        let y = noise(12000 + 2 * MAX_SHIFT, 7);
        let n = 12000;
        let track = &y[MAX_SHIFT..MAX_SHIFT + n];

        // Reference rip identical to ours.
        let v2 = checksum_v2(track, false, false);
        let (_, _, m) = verify_track(&y, false, false, &[(3, 0x1234), (40, v2)]);
        assert_eq!(m, Some(Match { confidence: 40, version: 2, shift: 0 }));

        // Reference rip is our audio shifted by +37 samples (v1 checksum in the database).
        let start = MAX_SHIFT + 37;
        let reference = checksum_v1(&y[start..start + n], false, false);
        let (_, _, m) = verify_track(&y, false, false, &[(12, reference)]);
        assert_eq!(m, Some(Match { confidence: 12, version: 1, shift: 37 }));

        // ... and a negative shift, with a v2 entry.
        let start = MAX_SHIFT - 600;
        let reference = checksum_v2(&y[start..start + n], false, false);
        let (_, _, m) = verify_track(&y, false, false, &[(2, reference)]);
        // A v2 entry can't be searched across shifts on its own (check() retries other
        // tracks at the disc's consensus shift instead), so it must not be matched here.
        assert!(m.is_none());
    }

    #[test]
    fn unrelated_audio_does_not_verify() {
        let y = noise(12000 + 2 * MAX_SHIFT, 7);
        let (_, _, m) = verify_track(&y, false, false, &[(50, 0xcafebabe), (50, 0x0badf00d)]);
        assert!(m.is_none());
        let (_, _, m) = verify_track(&y, true, true, &[]);
        assert!(m.is_none());
    }

    #[test]
    fn consensus_shift_picks_the_most_common_nonzero_shift() {
        let t = |status: &str, shift: Option<i32>| TrackReport { track: 1, status: status.into(), confidence: None, version: None, shift_samples: shift, v1: String::new(), v2: String::new() };
        let tracks = [t("verified", Some(0)), t("verified", Some(6)), t("verified", Some(6)), t("verified", Some(-4)), t("not_verified", None)];
        assert_eq!(consensus_shift(&tracks), Some(6));
        assert_eq!(consensus_shift(&[t("verified", Some(0))]), None);
    }

    #[test]
    fn detected_shift_requires_agreement() {
        let t = |shift| TrackReport { track: 1, status: "verified".into(), confidence: Some(1), version: Some(1), shift_samples: Some(shift), v1: String::new(), v2: String::new() };
        assert_eq!(detected_shift(&[t(6), t(6)]), Some(6));
        assert_eq!(detected_shift(&[t(6), t(-2)]), None);
        assert_eq!(detected_shift(&[t(0), t(0)]), None);
    }

    fn write_wav(path: &std::path::Path, words: &[u32]) {
        let data_len = (words.len() * 4) as u32;
        let mut b = Vec::new();
        b.extend(b"RIFF");
        b.extend((36 + data_len).to_le_bytes());
        b.extend(b"WAVEfmt ");
        b.extend(16u32.to_le_bytes());
        b.extend(1u16.to_le_bytes()); // PCM
        b.extend(2u16.to_le_bytes()); // stereo
        b.extend(44100u32.to_le_bytes());
        b.extend((44100u32 * 4).to_le_bytes());
        b.extend(4u16.to_le_bytes());
        b.extend(16u16.to_le_bytes());
        b.extend(b"data");
        b.extend(data_len.to_le_bytes());
        for w in words {
            b.extend(w.to_le_bytes());
        }
        std::fs::write(path, b).unwrap();
    }

    // Whole pipeline on synthetic files: a "drive" whose output is shifted by 6 samples
    // relative to the reference rip, one track whose database entry only holds a v2 checksum,
    // and one damaged track.
    #[test]
    fn verify_disc_handles_drive_offset_v2_only_entries_and_damage() {
        let lens = [9000usize, 12000, 15000, 10000]; // words per track (not sector-aligned; fine for the maths)
        let total_words: usize = lens.iter().sum();
        let disc = noise(total_words, 2024); // what the reference drive read

        let mut starts = vec![0usize];
        for l in &lens { starts.push(starts.last().unwrap() + l); }

        // Database: checksums of the reference tracks.
        let n = lens.len();
        let mut entries: Vec<(u8, u32)> = Vec::new();
        for i in 0..n {
            let t = &disc[starts[i]..starts[i + 1]];
            let (first, last) = (i == 0, i == n - 1);
            entries.push(if i == 1 { (7, checksum_v2(t, first, last)) } else { (30 + i as u8, checksum_v1(t, first, last)) });
        }
        let responses = vec![Response { tracks: entries }];

        // Our drive returns the disc shifted by 6 samples (and track 3 is damaged).
        let shift = 6usize;
        let mut ripped: Vec<u32> = disc[shift..].to_vec();
        ripped.extend(std::iter::repeat(0).take(shift));
        for w in &mut ripped[starts[2] + 100..starts[2] + 200] { *w ^= 0xdead; }

        let dir = std::env::temp_dir().join(format!("rustydisc_ar_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let wavs: Vec<(usize, String)> = (0..n)
            .map(|i| {
                let path = dir.join(format!("track{:02}.cdda.wav", i + 1));
                write_wav(&path, &ripped[starts[i]..starts[i + 1]]);
                (i + 1, path.to_string_lossy().to_string())
            })
            .collect();

        let toc = toc_from_lengths(&[400, 400, 400, 400], 0, 0);
        let report = verify_disc(&toc, &responses, &wavs, false);
        let _ = std::fs::remove_dir_all(&dir);

        let status: Vec<&str> = report.iter().map(|t| t.status.as_str()).collect();
        assert_eq!(status, ["verified", "verified", "not_verified", "verified"]);
        // Our rip has the reference sample i at index i-6, i.e. it is 6 samples early,
        // and shifting it by -6 lines it up again.
        assert!(report.iter().filter(|t| t.status == "verified").all(|t| t.shift_samples == Some(-6)));
        assert_eq!(report[1].version, Some(2)); // matched through the consensus shift
        assert_eq!(report[0].confidence, Some(30));
        assert_eq!(detected_shift(&report), Some(-6));
    }

    // Database entries that only hold v2 checksums, from a drive with a read offset: no
    // track matches at shift 0 and v1 can't find them, so the shift must be discovered.
    #[test]
    fn verify_disc_discovers_the_offset_for_v2_only_databases() {
        let lens = [9000usize, 12000, 15000, 10000];
        let disc = noise(lens.iter().sum(), 555);
        let mut starts = vec![0usize];
        for l in &lens { starts.push(starts.last().unwrap() + l); }
        let n = lens.len();
        let entries: Vec<(u8, u32)> = (0..n)
            .map(|i| (9, checksum_v2(&disc[starts[i]..starts[i + 1]], i == 0, i == n - 1)))
            .collect();

        let shift = 42usize;
        let mut ripped: Vec<u32> = disc[shift..].to_vec();
        ripped.extend(std::iter::repeat(0).take(shift));

        let dir = std::env::temp_dir().join(format!("rustydisc_ar_test_v2_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let wavs: Vec<(usize, String)> = (0..n)
            .map(|i| {
                let path = dir.join(format!("track{:02}.cdda.wav", i + 1));
                write_wav(&path, &ripped[starts[i]..starts[i + 1]]);
                (i + 1, path.to_string_lossy().to_string())
            })
            .collect();
        let toc = toc_from_lengths(&[400, 400, 400, 400], 0, 0);
        let report = verify_disc(&toc, &[Response { tracks: entries }], &wavs, false);
        let _ = std::fs::remove_dir_all(&dir);

        assert!(report.iter().all(|t| t.status == "verified"), "{:?}", report.iter().map(|t| &t.status).collect::<Vec<_>>());
        assert!(report.iter().all(|t| t.version == Some(2) && t.shift_samples == Some(-(shift as i32))));
    }
}
