//! Hardware-free simulation (`serve --mock`) so the web UI can be developed and
//! tested on a machine with no optical drive.

use std::{path::PathBuf, sync::Arc, time::Duration};

use serde_json::{json, Value};

use super::jobs::{fail, Event, Job, Status};
use crate::rip::{metadata, musicbrainz::{MbTrackInfo, ReleaseInfo}};

const ARTIST: &str = "The Static Lights";
/// A mock disc that MusicBrainz doesn't know, to try the release override.
pub const UNMATCHED_DISCID: &str = "UnmatchedMockDiscId0000000000-";
const ALBUM: &str = "Neon Cathedral";

const TRACKS: &[(&str, f64)] = &[
    ("Cathedral of Neon", 254.2),
    ("Tape Hiss Lullaby", 198.7),
    ("Parallel Lines Redux", 312.0),
    ("Static in the Snow", 221.4),
    ("Hidden Track (Reprise)", 163.9),
];

/// A generated placeholder cover so demos and screenshots have artwork without network access.
pub fn cover_svg() -> String {
    r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 400 400"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#1b1035"/><stop offset="1" stop-color="#e8743b"/></linearGradient></defs><rect width="400" height="400" fill="url(#g)"/><circle cx="200" cy="200" r="120" fill="none" stroke="#fff" stroke-opacity=".25" stroke-width="2"/><circle cx="200" cy="200" r="80" fill="none" stroke="#fff" stroke-opacity=".35" stroke-width="2"/><circle cx="200" cy="200" r="40" fill="#0e0a1c"/><circle cx="200" cy="200" r="8" fill="#e8743b"/><text x="24" y="368" font-family="sans-serif" font-size="26" font-weight="700" fill="#fff">NEON CATHEDRAL</text></svg>"##.to_string()
}

pub fn info(scenario: &str, device: &str) -> Result<Value, (String, String)> {
    let mut lba = 150u32;
    let audio_tracks: Vec<Value> = TRACKS
        .iter()
        .enumerate()
        .map(|(i, (_, secs))| {
            let frames = (*secs * 75.0) as u32;
            let t = json!({
                "number": i + 1, "kind": "audio", "duration_secs": secs,
                "lba_start": lba, "lba_end": lba + frames - 1,
            });
            lba += frames;
            t
        })
        .collect();

    let audio_session = json!({
        "index": 1, "kind": {"type": "audio"}, "tracks": audio_tracks,
        "cd_text": {"title": ALBUM, "artist": ARTIST},
    });
    let data_session = |index: u32, first: usize| json!({
        "index": index,
        "kind": {"type": "data", "volume_label": "BONUS_CONTENT", "size_mb": 182.4, "filesystem": "ISO9660"},
        "tracks": [{"number": first, "kind": "data", "lba_start": lba + 11400, "lba_end": lba + 11400 + 93400}],
    });

    match scenario {
        "none" => Err(("DEVICE_ERROR".into(), "No disc in drive (mock scenario 'none').".into())),
        "data" => Ok(json!({
            "format": "datacd", "is_writable": false, "device": device,
            "sessions": [data_session(1, 1)],
        })),
        "bluebook" => Ok(json!({
            "format": "bluebook", "is_writable": false, "device": device,
            "discid": "Wn8eRBtfLDfM0qjYPdxrz.Zjs_U-",
            "sessions": [audio_session, data_session(2, TRACKS.len() + 1)],
        })),
        "unmatched" => Ok(json!({
            "format": "redbook", "is_writable": false, "device": device,
            "discid": UNMATCHED_DISCID,
            "sessions": [json!({"index": 1, "kind": {"type": "audio"}, "tracks": audio_tracks})],
        })),
        _ => Ok(json!({
            "format": "redbook", "is_writable": false, "device": device,
            "discid": "Wn8eRBtfLDfM0qjYPdxrz.Zjs_U-",
            "sessions": [audio_session],
        })),
    }
}

pub fn release() -> ReleaseInfo {
    ReleaseInfo {
        mb_release_id: "00000000-0000-4000-8000-000000000000".into(),
        album: ALBUM.into(),
        album_artist: ARTIST.into(),
        mb_artist_id: None,
        date: Some("1999-04-12".into()),
        year: Some("1999".into()),
        tracks: TRACKS
            .iter()
            .enumerate()
            .map(|(i, (t, _))| MbTrackInfo {
                number: i + 1,
                title: (*t).into(),
                artist: None,
                mb_recording_id: None,
                mb_artist_id: None,
            })
            .collect(),
        mb_release_group_id: Some("11111111-1111-4111-8111-111111111111".into()),
        total_releases: 1,
    }
}

async fn nap(ms: u64) {
    tokio::time::sleep(Duration::from_millis(ms)).await;
}

/// Sleep in slices, returning early (true) if the job was cancelled.
async fn work(job: &Job, ms: u64) -> bool {
    tokio::select! {
        _ = nap(ms) => false,
        _ = job.cancel_notified().notified() => true,
    }
}

fn cancelled(job: &Job) {
    job.push(Event::Step { msg: "Cancelled".into() });
    job.push(Event::Status { status: Status::Cancelled });
}

pub async fn rip(job: Arc<Job>, rips_dir: PathBuf, folder: Option<String>, archive: bool, format: String, no_mb: bool, no_ar: bool, mb_chosen: bool, cover: crate::rip::cover::CoverOptions) {
    job.push(Event::Step { msg: "Analysing disc...".into() });
    job.push(Event::Progress { pct: 0.0 });
    if work(&job, 700).await { return cancelled(&job); }
    job.push(Event::Step { msg: "Detected: Red Book Audio CD".into() });
    if mb_chosen {
        job.push(Event::Step { msg: "Fetching the chosen MusicBrainz release...".into() });
        if work(&job, 600).await { return cancelled(&job); }
        job.push(Event::Step { msg: format!("Using: {ALBUM} — {ARTIST}") });
    } else if !no_mb {
        job.push(Event::Step { msg: "Looking up metadata on MusicBrainz...".into() });
        if work(&job, 600).await { return cancelled(&job); }
        job.push(Event::Step { msg: format!("Found: {ALBUM} — {ARTIST}") });
    }

    if cover.wanted() {
        job.push(Event::Step { msg: "Fetching cover art...".into() });
        if work(&job, 500).await { return cancelled(&job); }
        let source = cover.sources.first().copied().unwrap_or(crate::rip::cover::CoverSource::CoverArtArchive);
        let what = match (cover.save_file, cover.embed) { (true, true) => "saved as cover file and embedded", (true, false) => "saved as cover file", _ => "embedded only" };
        job.push(Event::Step { msg: format!("Cover art from {} (212 KB) — {}", source.label(), what) });
        job.push(Event::Result { name: "cover".into(), data: json!({"type": "cover", "source": source.id(), "label": source.label(), "file": null, "embedded": cover.embed}) });
    }

    let name = folder.unwrap_or_else(|| format!("{ARTIST} - {ALBUM} (1999)"));
    let out = rips_dir.join(&name);
    let audio_dir = if archive { out.join("audio") } else { out.clone() };
    if let Err(e) = std::fs::create_dir_all(&audio_dir) {
        return fail(&job, "IO_ERROR", &e.to_string(), false);
    }

    job.push(Event::Step { msg: "Ripping audio tracks from disc...".into() });
    for i in 0..TRACKS.len() {
        for s in 0..4 {
            if work(&job, 250).await { return cancelled(&job); }
            let pct = (i as f32 + (s + 1) as f32 / 4.0) / TRACKS.len() as f32 * 85.0;
            job.push(Event::Progress { pct });
        }
        job.push(Event::Log { msg: format!("  Ripping track {} of {}...", i + 1, TRACKS.len()) });
    }
    job.push(Event::Step { msg: "  Rip complete — encoding...".into() });
    for (i, (title, _)) in TRACKS.iter().enumerate() {
        let file = format!("{:02} - {}.{}", i + 1, title, format);
        job.push(Event::Step { msg: format!("Encoding track {} of {} — {}", i + 1, TRACKS.len(), file) });
        if work(&job, 300).await { return cancelled(&job); }
        // Placeholder bytes (not real audio) so the library and verify have something to work with.
        let _ = std::fs::write(audio_dir.join(&file), format!("mock audio data for {title}\n").repeat(2000));
        job.push(Event::Progress { pct: 85.0 + (i + 1) as f32 / TRACKS.len() as f32 * 12.0 });
    }

    let mut ar_report = None;
    if !no_ar {
        job.push(Event::Step { msg: "Checking rip against the AccurateRip database...".into() });
        if work(&job, 900).await { return cancelled(&job); }
        const CONFIDENCE: [u32; 5] = [48, 51, 47, 44, 12];
        let tracks: Vec<Value> = TRACKS.iter().enumerate().map(|(i, _)| json!({
            "track": i + 1, "status": "verified", "confidence": CONFIDENCE[i],
            "version": if i == 3 { 1 } else { 2 }, "shift_samples": -6,
            "v1": "00000000", "v2": "00000000",
        })).collect();
        let report = json!({
            "found": true, "database_url": "http://www.accuraterip.com/accuraterip/…",
            "pressings": 2, "verified": TRACKS.len(), "total": TRACKS.len(),
            "detected_shift_samples": -6, "tracks": tracks,
        });
        job.push(Event::Step { msg: format!("AccurateRip: {0} of {0} tracks verified", TRACKS.len()) });
        job.push(Event::Result { name: "accuraterip".into(), data: report.clone() });
        ar_report = Some(report);
    }

    if archive {
        job.push(Event::Step { msg: "Writing metadata and checksums...".into() });
        let meta = out.join("metadata");
        let _ = std::fs::create_dir_all(&meta);
        let _ = std::fs::write(meta.join("disc.json"), serde_json::to_string_pretty(&info("redbook", "mock").unwrap()).unwrap());
        if !no_mb || mb_chosen {
            let _ = std::fs::write(meta.join("musicbrainz.json"), serde_json::to_string_pretty(&release()).unwrap());
        }
        if let Some(r) = &ar_report {
            let _ = std::fs::write(meta.join("accuraterip.json"), serde_json::to_string_pretty(r).unwrap());
        }
        let dir = out.to_string_lossy().to_string();
        match metadata::generate_checksums(&dir).and_then(|m| metadata::write_checksums(&m, &meta.to_string_lossy())) {
            Ok(()) => {}
            Err(e) => return fail(&job, "IO_ERROR", &e.to_string(), false),
        }
    }
    job.push(Event::Progress { pct: 100.0 });
    job.push(Event::Status { status: Status::Done });
}

pub async fn burn(job: Arc<Job>, dry_run: bool) {
    let steps: &[(&str, f32)] = &[
        ("Building ISO image...", 20.0),
        ("Writing to disc...", 60.0),
        ("Closing disc...", 95.0),
    ];
    for (msg, target) in steps {
        job.push(Event::Step { msg: (*msg).into() });
        let start = job.summary().pct;
        for s in 1..=6 {
            if work(&job, 300).await { return cancelled(&job); }
            job.push(Event::Progress { pct: start + (target - start) * s as f32 / 6.0 });
        }
        if dry_run { break; }
    }
    job.push(Event::Progress { pct: 100.0 });
    job.push(Event::Status { status: Status::Done });
}

pub async fn recover(job: Arc<Job>, blank: Option<String>) {
    job.push(Event::Step {
        msg: match &blank {
            Some(m) => format!("Blanking CD-RW (mode: {m})..."),
            None => "Checking disc state...".into(),
        },
    });
    for s in 1..=4 {
        if work(&job, 400).await { return cancelled(&job); }
        job.push(Event::Progress { pct: s as f32 * 25.0 });
    }
    job.push(Event::Log { msg: "Disc on /dev/sr0 is blank. No recovery needed.".into() });
    job.push(Event::Status { status: Status::Done });
}

/// Canned search results so the search dialog can be tried without network access.
pub fn search(q: &crate::rip::musicbrainz::SearchQuery) -> Value {
    let all = [
        ("00000000-0000-4000-8000-000000000000", ALBUM, ARTIST, "1999-04-12", "GB", "Static Records", "SR001", "CD", vec![5usize], 96),
        ("00000000-0000-4000-8000-000000000001", ALBUM, ARTIST, "1999-05-03", "US", "Nightline", "NL-4471", "CD", vec![5], 91),
        ("00000000-0000-4000-8000-000000000002", "Neon Cathedral (Deluxe Edition)", ARTIST, "2009-10-26", "GB", "Static Records", "SR001X", "2×CD", vec![5, 9], 84),
        ("00000000-0000-4000-8000-000000000003", "Neon Cathedral", ARTIST, "1999", "JP", "Tokyo Wax", "TW-1188", "CD", vec![6], 80),
        ("00000000-0000-4000-8000-000000000004", "Midnight Ferry", "Velvet Harbour", "2007-03-19", "GB", "Harbour Recordings", "HR12", "CD", vec![6], 70),
    ];
    let needle = format!("{} {}", q.text, q.artist).to_lowercase();
    let releases: Vec<Value> = all.iter()
        .filter(|(_, title, artist, ..)| needle.split_whitespace().all(|w| title.to_lowercase().contains(w) || artist.to_lowercase().contains(w)))
        .filter(|(_, _, _, _, _, _, _, _, counts, _)| q.tracks.map_or(true, |n| counts.contains(&n)))
        .map(|(id, title, artist, date, country, label, cat, format, counts, score)| json!({
            "mb_release_id": id, "title": title, "artist": artist, "date": date, "year": &date[..4],
            "country": country, "status": "Official", "label": label, "catalog_number": cat, "barcode": null,
            "disambiguation": if *id == "00000000-0000-4000-8000-000000000003" { json!("Japanese edition with bonus track") } else { Value::Null },
            "format": format, "disc_track_counts": counts, "score": score,
        }))
        .collect();
    json!({"count": releases.len(), "offset": 0, "releases": releases})
}
