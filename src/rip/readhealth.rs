//! How cleanly the drive read each track.
//!
//! cdparanoia reports everything it does while reading when run with `--stderr-progress`, one
//! `##: <code> [<name>] @ <position>` line per event, and prints details of drive errors
//! (`scsi_read error: sector=…`, `Sense key: …`) as ordinary lines. This module turns those into a
//! per-track verdict:
//!
//! * **clean**: nothing beyond routine edge jitter, which every drive produces and paranoia fixes.
//! * **repaired**: cdparanoia met real trouble (corrections, scratches, dropped or duplicated
//!   samples, drive errors) and fixed it. AccurateRip can confirm the result.
//! * **suspect**: cdparanoia had to skip sectors, so the audio may have glitches.
//!
//! A "unit attention" error (sense key 6) right after a disc goes in only means the drive reset
//! itself, so it and the fix-ups next to it are not counted against the disc.

use serde::{Deserialize, Serialize};

/// CD audio: 1176 16-bit words per sector.
const WORDS_PER_SECTOR: i64 = 1176;
/// Fix-ups this close to a harmless drive reset belong to it.
const BENIGN_MARGIN: u32 = 32;
/// Beyond this many stored events only the totals are kept (a hopelessly damaged disc).
const MAX_EVENTS: usize = 2_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Jitter,
    Correction,
    Scratch,
    ScratchRepair,
    Skip,
    Drift,
    Backoff,
    Dropped,
    Duped,
    TransportError,
    CacheError,
}

impl Kind {
    fn from_code(code: i32) -> Option<Kind> {
        Some(match code {
            2 => Kind::Jitter,
            3 => Kind::Correction,
            4 => Kind::Scratch,
            5 => Kind::ScratchRepair,
            6 => Kind::Skip,
            7 => Kind::Drift,
            8 => Kind::Backoff,
            10 => Kind::Dropped,
            11 => Kind::Duped,
            12 => Kind::TransportError,
            13 => Kind::CacheError,
            _ => return None, // read, verify, overlap, wrote, finished: progress, not trouble
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct Event {
    sector: u32,
    kind: Kind,
}

#[derive(Debug, Clone, Default)]
struct Scsi {
    sector: Option<u32>,
    length: u32,
    sense_key: Option<u8>,
    asc: Option<u8>,
    text: String,
}

/// Collects cdparanoia's report as it reads.
#[derive(Debug, Default)]
pub struct Collector {
    events: Vec<Event>,
    overflow: u64,
    pending: Scsi,
    /// Sector ranges around harmless drive resets.
    benign: Vec<(u32, u32)>,
    pub benign_resets: u32,
    /// A description of the first real drive error, for the notes.
    pub first_drive_error: Option<String>,
}

fn parse_callback(line: &str) -> Option<(i32, i64)> {
    let rest = line.strip_prefix("##:")?.trim_start();
    let (code, rest) = rest.split_once(' ')?;
    let code: i32 = code.parse().ok()?;
    let pos = rest.rsplit_once('@').and_then(|(_, p)| p.trim().parse::<i64>().ok()).unwrap_or(0);
    Some((code, pos))
}

fn number_after(line: &str, key: &str) -> Option<u32> {
    let rest = line.split(key).nth(1)?;
    rest.trim_start().chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().ok()
}

impl Collector {
    /// Feed one line of cdparanoia's stderr. Returns true for the lines this module owns (the
    /// per-sector callbacks, which are far too many to echo into a log).
    pub fn feed(&mut self, line: &str) -> bool {
        if line.starts_with("##:") {
            if let Some((code, pos)) = parse_callback(line) {
                self.callback(code, pos);
            }
            return true;
        }
        let t = line.trim();
        if t.starts_with("scsi_read error") {
            self.pending = Scsi {
                sector: number_after(t, "sector="),
                length: number_after(t, "length=").unwrap_or(0),
                text: t.to_string(),
                ..Default::default()
            };
        } else if t.starts_with("Sense key:") {
            self.pending.sense_key = number_after(t, "Sense key:").map(|n| n as u8);
            self.pending.asc = number_after(t, "ASC:").map(|n| n as u8);
        }
        false
    }

    fn callback(&mut self, code: i32, pos: i64) {
        let Some(kind) = Kind::from_code(code) else { return };
        let sector = (pos.max(0) / WORDS_PER_SECTOR) as u32;
        if kind == Kind::TransportError {
            let p = std::mem::take(&mut self.pending);
            // Sense key 6 is "unit attention": the drive reset itself (just powered up, or a disc
            // was just loaded). The data is re-read and nothing is wrong with the disc.
            if p.sense_key == Some(6) {
                let at = p.sector.unwrap_or(sector);
                self.benign.push((at.saturating_sub(BENIGN_MARGIN), at + p.length + BENIGN_MARGIN));
                self.benign_resets += 1;
                return;
            }
            if self.first_drive_error.is_none() {
                let key = p.sense_key.map(|k| format!(" (sense key {k}{})", p.asc.map(|a| format!(", ASC {a:#04x}")).unwrap_or_default())).unwrap_or_default();
                self.first_drive_error = Some(format!("{}{}", if p.text.is_empty() { "a drive error".to_string() } else { p.text }, key));
            }
        }
        if self.events.len() >= MAX_EVENTS {
            self.overflow += 1;
            return;
        }
        self.events.push(Event { sector, kind });
    }

    fn is_benign(&self, sector: u32) -> bool {
        self.benign.iter().any(|(a, b)| sector >= *a && sector <= *b)
    }

    /// Give every track a verdict. `tracks` are (number, first sector, end sector).
    pub fn summarize(&self, tracks: &[(usize, u32, u32)]) -> ReadHealth {
        let mut out: Vec<TrackHealth> = tracks.iter().map(|(n, ..)| TrackHealth { number: *n, ..Default::default() }).collect();
        let mut cache_errors = 0u64;
        for e in &self.events {
            if e.kind == Kind::CacheError {
                cache_errors += 1;
                continue;
            }
            // Fix-ups beside a harmless reset are part of it; real errors always count.
            if e.kind != Kind::TransportError && self.is_benign(e.sector) {
                continue;
            }
            let Some(i) = tracks.iter().position(|(_, a, b)| e.sector >= *a && e.sector < *b) else { continue };
            let t = &mut out[i];
            match e.kind {
                Kind::Jitter => t.jitter += 1,
                Kind::Correction => t.corrections += 1,
                Kind::Scratch => t.scratches += 1,
                Kind::ScratchRepair => t.scratches += 0, // the repair of a scratch already counted
                Kind::Skip => t.skips += 1,
                Kind::Drift => t.drift += 1,
                Kind::Backoff => t.backoffs += 1,
                Kind::Dropped | Kind::Duped => t.dropped_or_duped += 1,
                Kind::TransportError => t.drive_errors += 1,
                Kind::CacheError => {}
            }
        }
        for t in &mut out {
            t.status = if t.skips > 0 {
                Status::Suspect
            } else if t.corrections + t.scratches + t.dropped_or_duped + t.drive_errors > 0 {
                Status::Repaired
            } else {
                Status::Clean
            };
        }
        ReadHealth {
            tracks: out,
            cache_errors: cache_errors + self.overflow.min(0),
            harmless_resets: self.benign_resets,
            first_drive_error: self.first_drive_error.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    #[default]
    Clean,
    Repaired,
    Suspect,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrackHealth {
    pub number: usize,
    pub status: Status,
    /// Routine edge jitter fixed (normal for every drive).
    pub jitter: u32,
    pub corrections: u32,
    pub scratches: u32,
    /// Sectors cdparanoia gave up on.
    pub skips: u32,
    pub drift: u32,
    pub backoffs: u32,
    pub dropped_or_duped: u32,
    pub drive_errors: u32,
}

impl TrackHealth {
    /// A short plain-English line.
    pub fn describe(&self) -> String {
        let mut bits: Vec<String> = Vec::new();
        let mut add = |n: u32, one: &str, many: &str| {
            if n > 0 {
                bits.push(format!("{n} {}", if n == 1 { one } else { many }));
            }
        };
        add(self.skips, "skipped sector", "skipped sectors");
        add(self.drive_errors, "drive error", "drive errors");
        add(self.scratches, "scratch", "scratches");
        add(self.corrections, "correction", "corrections");
        add(self.dropped_or_duped, "dropped/duplicated block", "dropped/duplicated blocks");
        let trouble = bits.join(", ");
        let jitter = if self.jitter > 0 { format!("{} edge jitter fix{}", self.jitter, if self.jitter == 1 { "" } else { "es" }) } else { String::new() };
        match self.status {
            Status::Clean if jitter.is_empty() => "clean".to_string(),
            Status::Clean => format!("clean ({jitter}, routine)"),
            Status::Repaired => format!("repaired: {trouble}"),
            Status::Suspect => format!("suspect: {trouble}"),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReadHealth {
    pub tracks: Vec<TrackHealth>,
    /// Times cdparanoia warned the drive appears to cache audio reads.
    pub cache_errors: u64,
    /// Drive resets (unit attention) that were ignored as harmless.
    pub harmless_resets: u32,
    pub first_drive_error: Option<String>,
}

impl ReadHealth {
    pub fn count(&self, s: Status) -> usize {
        self.tracks.iter().filter(|t| t.status == s).count()
    }

    pub fn track(&self, number: usize) -> Option<&TrackHealth> {
        self.tracks.iter().find(|t| t.number == number)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_all(c: &mut Collector, lines: &str) {
        for l in lines.lines() {
            c.feed(l);
        }
    }

    // Track 1 is sectors 0..1000, track 2 is 1000..2000.
    const TRACKS: [(usize, u32, u32); 2] = [(1, 0, 1000), (2, 1000, 2000)];

    fn at(sector: u32) -> i64 {
        sector as i64 * WORDS_PER_SECTOR
    }

    #[test]
    fn parses_the_callback_lines_cdparanoia_prints() {
        assert_eq!(parse_callback("##: 3 [correction] @ 460768"), Some((3, 460768)));
        assert_eq!(parse_callback("##: -1 [finished] @ 353975"), Some((-1, 353975)));
        assert_eq!(parse_callback("##: 12 [transport error] @ 484512"), Some((12, 484512)));
        assert_eq!(parse_callback("##: -2 [wrote] "), Some((-2, 0)));
        assert_eq!(parse_callback("outputting to track01.cdda.wav"), None);
    }

    #[test]
    fn progress_lines_are_owned_but_not_counted() {
        let mut c = Collector::default();
        assert!(c.feed("##: 0 [read] @ 24696"));
        assert!(c.feed("##: -2 [wrote] @ 588"));
        assert!(c.feed("##: 1 [verify] @ 0"));
        assert!(!c.feed("outputting to track01.cdda.wav"));
        let h = c.summarize(&TRACKS);
        assert!(h.tracks.iter().all(|t| t.status == Status::Clean && t.jitter == 0));
    }

    #[test]
    fn jitter_alone_is_clean() {
        let mut c = Collector::default();
        for s in [10, 20, 30] {
            c.feed(&format!("##: 2 [jitter] @ {}", at(s)));
        }
        let h = c.summarize(&TRACKS);
        assert_eq!(h.tracks[0].status, Status::Clean);
        assert_eq!(h.tracks[0].jitter, 3);
        assert_eq!(h.tracks[0].describe(), "clean (3 edge jitter fixes, routine)");
    }

    #[test]
    fn corrections_scratches_and_drive_errors_mean_repaired() {
        let mut c = Collector::default();
        feed_all(&mut c, &format!("##: 3 [correction] @ {0}\n##: 4 [scratch] @ {0}\n##: 5 [scratch repair] @ {0}", at(500)));
        // Track 2 (sector 1000+): a real medium error.
        feed_all(&mut c, &format!("scsi_read error: sector=1500 length=27 retry=0\n  Sense key: 3 ASC: 11 ASCQ: 0\n##: 12 [transport error] @ {}", at(1500)));
        let h = c.summarize(&TRACKS);
        assert_eq!(h.tracks[0].status, Status::Repaired);
        assert_eq!((h.tracks[0].corrections, h.tracks[0].scratches), (1, 1));
        assert_eq!(h.tracks[1].status, Status::Repaired);
        assert_eq!(h.tracks[1].drive_errors, 1);
        assert!(h.first_drive_error.unwrap().contains("sense key 3"));
        assert_eq!(h.tracks[0].describe(), "repaired: 1 scratch, 1 correction");
    }

    #[test]
    fn a_skipped_sector_is_suspect() {
        let mut c = Collector::default();
        c.feed(&format!("##: 6 [skip] @ {}", at(900)));
        let h = c.summarize(&TRACKS);
        assert_eq!(h.tracks[0].status, Status::Suspect);
        assert_eq!(h.count(Status::Suspect), 1);
        assert_eq!(h.tracks[0].describe(), "suspect: 1 skipped sector");
    }

    #[test]
    fn a_harmless_drive_reset_is_not_held_against_the_disc() {
        // What the real drive printed on the first read after a disc was loaded.
        let mut c = Collector::default();
        feed_all(
            &mut c,
            "scsi_read error: sector=399 length=27 retry=0\n\
             \x20                Sense key: 6 ASC: 29 ASCQ: 0\n\
             \x20                Transport error: Unspecified error\n\
             ##: 12 [transport error] @ 484512\n\
             ##: 3 [correction] @ 460768\n\
             ##: 3 [correction] @ 484512",
        );
        let h = c.summarize(&TRACKS);
        assert_eq!(h.tracks[0].status, Status::Clean, "{:?}", h.tracks[0]);
        assert_eq!(h.harmless_resets, 1);
        assert!(h.first_drive_error.is_none());
    }

    #[test]
    fn fix_ups_far_from_the_reset_still_count() {
        let mut c = Collector::default();
        feed_all(&mut c, "scsi_read error: sector=399 length=27 retry=0\n Sense key: 6 ASC: 29 ASCQ: 0\n##: 12 [transport error] @ 484512");
        c.feed(&format!("##: 3 [correction] @ {}", at(700)));
        assert_eq!(c.summarize(&TRACKS).tracks[0].status, Status::Repaired);
    }

    #[test]
    fn cache_warnings_are_counted_for_the_disc() {
        let mut c = Collector::default();
        c.feed("##: 13 [cache error] @ 0");
        c.feed("##: 13 [cache error] @ 0");
        assert_eq!(c.summarize(&TRACKS).cache_errors, 2);
    }
}
