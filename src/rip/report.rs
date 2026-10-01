//! The rip report: one structured record of how a rip went, saved as `rip-report.json`
//! and as a readable `rip.log`. Later checks (read errors, pregaps, TOC warnings…) add their
//! findings here, so everything about a rip lives in one place.

use std::{
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    analyzer::{DiscInfo, TrackKind},
    rip::{accuraterip, musicbrainz::ReleaseInfo},
};

pub const JSON_NAME: &str = "rip-report.json";
pub const LOG_NAME: &str = "rip.log";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DriveInfo {
    pub device: String,
    pub vendor: Option<String>,
    pub model: Option<String>,
    pub revision: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TocTrack {
    pub number: usize,
    pub kind: String,
    pub start_sector: u32,
    pub end_sector: u32,
    pub seconds: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseSummary {
    pub artist: String,
    pub album: String,
    pub year: Option<String>,
    pub musicbrainz_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Settings {
    pub reader: String,
    pub read_mode: String,
    /// Samples the audio was shifted by to correct the drive's read offset (0 = none).
    pub offset_applied_samples: i32,
    pub format: String,
    pub quality: Option<String>,
    pub archive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccurateRipTrack {
    pub status: String,
    pub confidence: Option<u8>,
    pub version: Option<u8>,
    pub shift_samples: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackEntry {
    pub number: usize,
    pub title: Option<String>,
    pub file: String,
    /// SHA-256 of the raw track exactly as the drive delivered it, before encoding.
    pub read_sha256: String,
    pub accuraterip: Option<AccurateRipTrack>,
    /// How cleanly the drive read this track.
    #[serde(default)]
    pub read: Option<super::readhealth::TrackHealth>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccurateRipSummary {
    pub found: bool,
    pub pressings: usize,
    pub verified: usize,
    pub total: usize,
    pub detected_shift_samples: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RipReport {
    pub rustydisc_version: String,
    pub started: String,
    pub duration_secs: u64,
    pub drive: DriveInfo,
    pub disc_format: String,
    pub discid: Option<String>,
    pub toc: Vec<TocTrack>,
    pub release: Option<ReleaseSummary>,
    pub settings: Settings,
    pub tracks: Vec<TrackEntry>,
    pub accuraterip: Option<AccurateRipSummary>,
    /// How cleanly the drive read the disc as a whole.
    #[serde(default)]
    pub read_quality: Option<ReadSummary>,
    /// What became of any audio before track 1 (a hidden track).
    #[serde(default = "no_hidden")]
    pub hidden_audio: super::gaps::HiddenOutcome,
    /// Gaps before tracks that a gap scan found (empty when no scan was done, or none exist).
    #[serde(default)]
    pub gaps: Vec<super::gaps::TrackGap>,
    pub warnings: Vec<String>,
}

fn no_hidden() -> super::gaps::HiddenOutcome {
    super::gaps::HiddenOutcome::None
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadSummary {
    pub clean: usize,
    pub repaired: usize,
    pub suspect: usize,
    /// Times cdparanoia warned the drive caches audio reads.
    pub cache_errors: u64,
    /// Harmless drive resets (unit attention) that were ignored.
    pub harmless_resets: u32,
    pub reduced_checking: bool,
}

// ── Building ──────────────────────────────────────────────────────────────────

/// A track as ripped: its number, title, the file written, and the raw track file it came from.
pub struct RippedTrack<'a> {
    pub number: usize,
    pub title: Option<&'a str>,
    pub file: &'a str,
    pub raw_path: &'a str,
}

pub struct Inputs<'a> {
    pub info: &'a DiscInfo,
    pub mb: Option<&'a ReleaseInfo>,
    pub settings: Settings,
    pub started: SystemTime,
    pub tracks: Vec<RippedTrack<'a>>,
    pub accuraterip: Option<&'a accuraterip::Report>,
    pub no_accuraterip: bool,
    /// Things worth recording about how the rip was handled (e.g. a read offset correction).
    pub notes: Vec<String>,
    /// How cleanly each track was read.
    pub health: Option<&'a super::readhealth::ReadHealth>,
    /// cdparanoia was run with less than full checking.
    pub paranoia_reduced: bool,
    /// What became of any audio before track 1.
    pub hidden: super::gaps::HiddenOutcome,
    /// Length of that gap in sectors, when the disc has one.
    pub hidden_sectors: Option<u32>,
    /// Gaps between tracks that a scan found.
    pub gaps: Vec<super::gaps::TrackGap>,
}

pub fn build(i: Inputs) -> RipReport {
    let toc: Vec<TocTrack> = i
        .info
        .sessions
        .iter()
        .flat_map(|s| s.tracks.iter())
        .map(|t| TocTrack {
            number: t.number,
            kind: if t.kind == TrackKind::Audio { "audio" } else { "data" }.into(),
            start_sector: t.lba_start,
            end_sector: t.lba_end.saturating_sub(1),
            seconds: t.duration_secs,
        })
        .collect();

    let mut toc = toc;
    if let Some(l) = i.hidden_sectors {
        toc.insert(0, TocTrack { number: 0, kind: "hidden".into(), start_sector: 0, end_sector: l.saturating_sub(1), seconds: Some(super::gaps::seconds(l)) });
    }
    let tracks: Vec<TrackEntry> = i
        .tracks
        .iter()
        .map(|t| TrackEntry {
            number: t.number,
            title: t.title.map(str::to_string),
            file: t.file.to_string(),
            read_sha256: sha256_file(t.raw_path).unwrap_or_default(),
            read: i.health.and_then(|h| h.track(t.number).cloned()),
            accuraterip: i.accuraterip.and_then(|r| r.tracks.iter().find(|x| x.track == t.number)).map(|x| AccurateRipTrack {
                status: x.status.clone(),
                confidence: x.confidence,
                version: x.version,
                shift_samples: x.shift_samples,
            }),
        })
        .collect();

    let mut warnings = i.notes.clone();
    warnings.extend(i.hidden.note());
    match i.accuraterip {
        Some(r) if !r.found => warnings.push("This disc is not in the AccurateRip database, so the rip could not be checked against other people's.".into()),
        Some(r) if r.verified < r.total => warnings.push(format!("Only {} of {} tracks matched AccurateRip.", r.verified, r.total)),
        None if i.no_accuraterip => warnings.push("The AccurateRip check was turned off.".into()),
        None => warnings.push("The AccurateRip check could not be completed.".into()),
        _ => {}
    }
    if let Some(h) = i.health {
        use super::readhealth::Status;
        let verified = |n: usize| i.accuraterip.and_then(|r| r.tracks.iter().find(|t| t.track == n)).is_some_and(|t| t.status == "verified");
        for t in &h.tracks {
            match (t.status, verified(t.number)) {
                (Status::Suspect, false) => warnings.push(format!("Track {:02}: cdparanoia could not read some sectors cleanly ({}), and AccurateRip can't confirm the result, so it may have glitches. Cleaning the disc and ripping again can help.", t.number, t.describe())),
                (Status::Suspect, true) => warnings.push(format!("Track {:02}: cdparanoia reported trouble ({}), but AccurateRip confirms the audio is correct.", t.number, t.describe())),
                (Status::Repaired, false) => warnings.push(format!("Track {:02} needed repairs ({}). cdparanoia fixed them, but AccurateRip can't confirm it.", t.number, t.describe())),
                _ => {}
            }
        }
        if h.cache_errors > 0 {
            warnings.push(format!("cdparanoia warned {} times that this drive appears to cache audio reads, which can hide read errors. Its analysis mode (`cdparanoia -A`) can check.", h.cache_errors));
        }
        if let Some(e) = &h.first_drive_error {
            warnings.push(format!("The drive reported an error while reading: {e}"));
        }
    }
    if i.paranoia_reduced {
        warnings.push("Read checking was reduced (a lower paranoia level), so errors may not have been caught or corrected.".into());
    }
    if let Some(shift) = i.accuraterip.and_then(|r| r.detected_shift_samples).filter(|s| *s != 0) {
        if i.settings.offset_applied_samples == 0 {
            warnings.push(format!("Your drive appears to read {shift:+} samples off. The audio was saved as the drive returned it, so it matches AccurateRip only at that shift. Turn on read offset correction (Settings → Rip defaults) to fix this."));
        } else {
            warnings.push(format!("After correcting {:+} samples, AccurateRip still finds the rip {shift:+} samples off, so the offset looks wrong.", i.settings.offset_applied_samples));
        }
    }
    if i.mb.is_none() {
        warnings.push("No MusicBrainz release was used, so tags come from CD-TEXT or are missing.".into());
    }

    let started_secs = i.started.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let duration = SystemTime::now().duration_since(i.started).map(|d| d.as_secs()).unwrap_or(0);

    RipReport {
        rustydisc_version: env!("CARGO_PKG_VERSION").to_string(),
        started: iso_utc(started_secs),
        duration_secs: duration,
        drive: drive_info(&i.info.device),
        disc_format: i.info.format.to_string(),
        discid: i.info.discid.clone(),
        toc,
        release: i.mb.map(|r| ReleaseSummary {
            artist: r.album_artist.clone(),
            album: r.album.clone(),
            year: r.year.clone(),
            musicbrainz_id: r.mb_release_id.clone(),
        }),
        settings: i.settings,
        tracks,
        hidden_audio: i.hidden.clone(),
        gaps: i.gaps.clone(),
        read_quality: i.health.map(|h| {
            use super::readhealth::Status;
            ReadSummary {
                clean: h.count(Status::Clean),
                repaired: h.count(Status::Repaired),
                suspect: h.count(Status::Suspect),
                cache_errors: h.cache_errors,
                harmless_resets: h.harmless_resets,
                reduced_checking: i.paranoia_reduced,
            }
        }),
        accuraterip: i.accuraterip.map(|r| AccurateRipSummary {
            found: r.found,
            pressings: r.pressings,
            verified: r.verified,
            total: r.total,
            detected_shift_samples: r.detected_shift_samples,
        }),
        warnings,
    }
}

/// Write `rip-report.json` and `rip.log` into `dir`.
pub fn write(report: &RipReport, dir: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(Path::new(dir).join(JSON_NAME), serde_json::to_string_pretty(report).unwrap_or_default())?;
    std::fs::write(Path::new(dir).join(LOG_NAME), render(report))
}

/// Find a rip's saved report: next to the audio, or in its `metadata` folder.
pub fn find(rip_dir: &Path) -> Option<std::path::PathBuf> {
    [rip_dir.join(JSON_NAME), rip_dir.join("metadata").join(JSON_NAME)].into_iter().find(|p| p.is_file())
}

// ── Reading the drive ─────────────────────────────────────────────────────────

fn sysfs(dev: &str, file: &str) -> Option<String> {
    let name = Path::new(dev).file_name()?.to_str()?;
    let s = std::fs::read_to_string(format!("/sys/block/{name}/device/{file}")).ok()?;
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

pub fn drive_info(device: &str) -> DriveInfo {
    DriveInfo { device: device.to_string(), vendor: sysfs(device, "vendor"), model: sysfs(device, "model"), revision: sysfs(device, "rev") }
}

fn sha256_file(path: &str) -> Option<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Some(hex::encode(h.finalize()))
}

/// Seconds since the Unix epoch as `YYYY-MM-DD HH:MM:SS UTC`.
pub fn iso_utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC", rem / 3600, (rem % 3600) / 60, rem % 60)
}

// ── The readable log ──────────────────────────────────────────────────────────

fn mmss(secs: f64) -> String {
    let s = secs.round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

pub fn render(r: &RipReport) -> String {
    let rule = "-".repeat(78);
    let mut s = String::new();
    s.push_str(&format!("RustyDisc {} rip log\n{}\n", r.rustydisc_version, rule));
    s.push_str(&format!("Ripped:      {} ({} min {} s)\n", r.started, r.duration_secs / 60, r.duration_secs % 60));
    if let Some(rel) = &r.release {
        s.push_str(&format!("Album:       {} / {}{}\n", rel.artist, rel.album, rel.year.as_deref().map(|y| format!(" ({y})")).unwrap_or_default()));
        s.push_str(&format!("MusicBrainz: https://musicbrainz.org/release/{}\n", rel.musicbrainz_id));
    }
    s.push_str(&format!("Disc:        {}\n", r.disc_format));
    if let Some(id) = &r.discid {
        s.push_str(&format!("Disc ID:     {id}\n"));
    }
    let drive = [r.drive.vendor.as_deref(), r.drive.model.as_deref()].into_iter().flatten().collect::<Vec<_>>().join(" ");
    s.push_str(&format!(
        "Drive:       {}{}{}\n",
        if drive.is_empty() { "unknown".to_string() } else { drive },
        r.drive.revision.as_deref().map(|v| format!(" (firmware {v})")).unwrap_or_default(),
        format!(" on {}", r.drive.device)
    ));
    s.push_str(&format!("Reader:      {}, {}\n", r.settings.reader, r.settings.read_mode));
    s.push_str(&format!(
        "Read offset: {}\n",
        if r.settings.offset_applied_samples == 0 { "not corrected (audio is exactly what the drive returned)".to_string() } else { format!("{:+} samples corrected", r.settings.offset_applied_samples) }
    ));
    s.push_str(&format!(
        "Saved as:    {}{}{}\n",
        r.settings.format,
        r.settings.quality.as_deref().map(|q| format!(", {q}")).unwrap_or_default(),
        if r.settings.archive { " (archive mode)" } else { "" }
    ));

    s.push_str(&format!("\nTable of contents\n{rule}\n  Track  Type      Start sector   End sector   Length\n"));
    for t in &r.toc {
        s.push_str(&format!("  {:>5}  {:<8}  {:>12}  {:>11}  {:>7}\n", t.number, t.kind, t.start_sector, t.end_sector, t.seconds.map(mmss).unwrap_or_else(|| "-".into())));
    }

    s.push_str(&format!("\nTracks\n{rule}\n"));
    for t in &r.tracks {
        let ar = match &t.accuraterip {
            Some(a) if a.status == "verified" => format!(
                "accurately ripped (v{}, confidence {}{})",
                a.version.unwrap_or(0),
                a.confidence.unwrap_or(0),
                a.shift_samples.filter(|s| *s != 0).map(|s| format!(", {s:+} sample shift")).unwrap_or_default()
            ),
            Some(a) if a.status == "not_verified" => "not in AccurateRip / no match".to_string(),
            Some(_) => "not checked".to_string(),
            None => "not checked".to_string(),
        };
        s.push_str(&format!("Track {:02}  {}\n", t.number, t.title.as_deref().unwrap_or("(untitled)")));
        s.push_str(&format!("    File:        {}\n", t.file));
        if let Some(h) = &t.read {
            s.push_str(&format!("    Read:        {}\n", h.describe()));
        }
        s.push_str(&format!("    AccurateRip: {}\n    Read SHA-256: {}\n", ar, t.read_sha256));
    }

    if let Some(q) = &r.read_quality {
        s.push_str(&format!("\nRead quality\n{rule}\n{} clean, {} repaired, {} suspect", q.clean, q.repaired, q.suspect));
        if q.reduced_checking {
            s.push_str(" (reduced checking)");
        }
        s.push_str(".\n");
        if q.harmless_resets > 0 {
            s.push_str(&format!("The drive reset itself {} time{} (normal after a disc is loaded); that is not held against the disc.\n", q.harmless_resets, if q.harmless_resets == 1 { "" } else { "s" }));
        }
        s.push_str("Edge jitter is routine and fixed by cdparanoia; it is listed only for information.\n");
    }

    if let Some(a) = &r.accuraterip {
        s.push_str(&format!("\nAccurateRip\n{rule}\n"));
        if a.found {
            s.push_str(&format!("{} of {} tracks verified against {} pressing{} in the database.\n", a.verified, a.total, a.pressings, if a.pressings == 1 { "" } else { "s" }));
            if let Some(sh) = a.detected_shift_samples {
                s.push_str(&format!("All verified tracks matched at a {sh:+} sample shift, which suggests your drive's read offset.\n"));
            }
        } else {
            s.push_str("This disc is not in the database.\n");
        }
    }

    s.push_str(&format!("\nNotes\n{rule}\n"));
    if r.warnings.is_empty() {
        s.push_str("No problems found.\n");
    } else {
        for w in &r.warnings {
            s.push_str(&format!("- {w}\n"));
        }
    }
    s.push_str(&format!("{}\n", rule));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_formatted() {
        assert_eq!(iso_utc(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(iso_utc(1_790_797_891), "2026-09-30 19:51:31 UTC");
        assert_eq!(iso_utc(951_782_400), "2000-02-29 00:00:00 UTC");
    }

    fn sample() -> RipReport {
        RipReport {
            rustydisc_version: "1.2.2".into(),
            started: iso_utc(0),
            duration_secs: 125,
            drive: DriveInfo { device: "/dev/sr0".into(), vendor: Some("ASUS".into()), model: Some("SDRW-08D2S-U".into()), revision: Some("B901".into()) },
            disc_format: "Red Book Audio CD".into(),
            discid: Some("abc".into()),
            toc: vec![TocTrack { number: 1, kind: "audio".into(), start_sector: 0, end_sector: 14999, seconds: Some(200.0) }],
            release: Some(ReleaseSummary { artist: "A".into(), album: "B".into(), year: Some("1988".into()), musicbrainz_id: "id".into() }),
            settings: Settings { reader: "cdparanoia".into(), read_mode: "full paranoia".into(), offset_applied_samples: 0, format: "FLAC".into(), quality: None, archive: false },
            tracks: vec![TrackEntry {
                number: 1,
                title: Some("T".into()),
                file: "01.flac".into(),
                read_sha256: "ff".into(),
                accuraterip: Some(AccurateRipTrack { status: "verified".into(), confidence: Some(12), version: Some(2), shift_samples: Some(6) }),
                read: None,
            }],
            read_quality: None,
            accuraterip: Some(AccurateRipSummary { found: true, pressings: 3, verified: 1, total: 1, detected_shift_samples: Some(6) }),
            hidden_audio: crate::rip::gaps::HiddenOutcome::None,
            gaps: vec![],
            warnings: vec![],
        }
    }

    #[test]
    fn log_mentions_the_essentials() {
        let log = render(&sample());
        for needle in ["ASUS SDRW-08D2S-U", "firmware B901", "not corrected", "3:20", "accurately ripped (v2, confidence 12, +6 sample shift)", "1 of 1 tracks verified against 3 pressings", "No problems found."] {
            assert!(log.contains(needle), "missing {needle:?} in:\n{log}");
        }
    }

    #[test]
    fn read_quality_appears_in_the_log_with_warnings() {
        use crate::rip::readhealth::{ReadHealth, Status, TrackHealth};
        let mut r = sample();
        r.tracks[0].read = Some(TrackHealth { number: 1, status: Status::Repaired, corrections: 2, scratches: 1, jitter: 4, ..Default::default() });
        r.read_quality = Some(ReadSummary { clean: 0, repaired: 1, suspect: 0, cache_errors: 0, harmless_resets: 1, reduced_checking: false });
        let log = render(&r);
        for needle in ["Read:        repaired: 1 scratch, 2 corrections", "Read quality", "0 clean, 1 repaired, 0 suspect", "reset itself 1 time"] {
            assert!(log.contains(needle), "missing {needle:?} in:\n{log}");
        }
        let _ = ReadHealth::default();
    }

    #[test]
    fn report_round_trips_and_is_found() {
        let dir = std::env::temp_dir().join(format!("rustydisc_report_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write(&sample(), dir.join("metadata").to_str().unwrap()).unwrap();
        let p = find(&dir).expect("found in metadata");
        let back: RipReport = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
        assert_eq!(back.tracks[0].file, "01.flac");
        assert!(dir.join("metadata").join(LOG_NAME).is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
