//! Audio that sits outside the tracks: the "hidden track one audio" before track 1.
//!
//! A normal disc's first track starts right at the beginning of the audio area (sector 0 in the
//! numbering used here). Some discs put music in front of it, a hidden track that a player only
//! reaches by rewinding from the start of track 1. The table of contents gives it away: track 1
//! starts later than sector 0. cdparanoia calls that span "track 0".

use serde::{Deserialize, Serialize};

use crate::{
    analyzer::{DiscInfo, TrackKind},
    error::Error,
    rip::accuraterip::open_wav,
};

/// Less than this is just a slightly long lead-in, not worth a track.
pub const MIN_HIDDEN_SECTORS: u32 = 75;

/// The loudest sample (of 32768) that still counts as silence.
const SILENCE_PEAK: i32 = 16;

/// How the hidden audio before track 1 is handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HiddenTrack {
    /// Rip it when there is one and it isn't silence.
    #[default]
    Auto,
    /// Leave it out.
    Skip,
}

impl HiddenTrack {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_lowercase().as_str() {
            "" | "auto" | "rip" | "keep" => Ok(HiddenTrack::Auto),
            "skip" | "off" | "no" => Ok(HiddenTrack::Skip),
            other => Err(format!("'{other}' is not a hidden-track choice (use auto or skip)")),
        }
    }
}

/// Sectors of audio in front of track 1, if the disc has any worth looking at.
pub fn hidden_sectors(info: &DiscInfo) -> Option<u32> {
    let first = info.sessions.iter().flat_map(|s| s.tracks.iter()).filter(|t| t.kind == TrackKind::Audio).min_by_key(|t| t.number)?;
    (first.number == 1 && first.lba_start >= MIN_HIDDEN_SECTORS).then_some(first.lba_start)
}

pub fn seconds(sectors: u32) -> f64 {
    sectors as f64 / 75.0
}

pub fn mmss(sectors: u32) -> String {
    let s = seconds(sectors).round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Is every sample (near enough) zero? Digital silence in front of track 1 isn't music.
pub fn is_silent(path: &str) -> Result<bool, Error> {
    let w = open_wav(path)?;
    let mut at = 0;
    while at < w.words {
        let chunk = w.read(at, 1 << 18)?;
        if chunk.is_empty() {
            break;
        }
        for word in &chunk {
            let l = (*word & 0xFFFF) as u16 as i16 as i32;
            let r = (*word >> 16) as u16 as i16 as i32;
            if l.abs() > SILENCE_PEAK || r.abs() > SILENCE_PEAK {
                return Ok(false);
            }
        }
        at += chunk.len();
    }
    Ok(true)
}

// ── Gaps between tracks ──────────────────────────────────────────────────────
//
// Track N+1 can begin with a gap (a "pregap", index 00): audio that plays before the track's
// official start, such as applause that leads into the next song on a live album. Ripping by the
// table of contents cuts at the official starts, so that audio ends up at the end of the previous
// track. The table of contents doesn't say whether a gap exists; only a scan of the disc's
// subchannel data does, and `cdrdao read-toc` does that scan (it takes about five minutes).

/// What to do about gaps between tracks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GapMode {
    /// Don't look (the default): gap audio stays at the end of the previous track.
    #[default]
    Off,
    /// Scan, and say in the log which tracks have gaps. The audio is left as is.
    Report,
    /// Scan, and move each gap to the start of the track it leads into.
    OwnTrack,
}

impl GapMode {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_lowercase().as_str() {
            "" | "off" | "no" | "leave" => Ok(GapMode::Off),
            "report" | "scan" => Ok(GapMode::Report),
            "own" | "own-track" | "start" | "prepend" => Ok(GapMode::OwnTrack),
            other => Err(format!("'{other}' is not a gap choice (use off, report or own-track)")),
        }
    }
}

/// A gap in front of a track, in sectors (75 to a second).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrackGap {
    pub track: usize,
    pub sectors: u32,
}

/// `mm:ss:ff` as sectors.
fn msf(s: &str) -> Option<u32> {
    let mut p = s.trim().split(':');
    let (m, sec, f) = (p.next()?.parse::<u32>().ok()?, p.next()?.parse::<u32>().ok()?, p.next()?.parse::<u32>().ok()?);
    Some(m * 60 * 75 + sec * 75 + f)
}

/// The gaps in a TOC file written by `cdrdao read-toc`: each `TRACK` block's `START` line is
/// where the track officially begins inside the audio cdrdao saved for it, i.e. the length of its gap.
pub fn parse_read_toc(text: &str) -> Vec<TrackGap> {
    let mut out = Vec::new();
    let mut track = 0usize;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("TRACK ") {
            track += 1;
        } else if let Some(rest) = t.strip_prefix("START") {
            if let Some(n) = msf(rest).filter(|n| *n > 0) {
                if track > 0 {
                    out.push(TrackGap { track, sectors: n });
                }
            }
        }
    }
    out
}

/// Scan the disc for gaps. Slow (about five minutes for a full disc).
pub fn scan(device: &str, debug: bool) -> Result<Vec<TrackGap>, String> {
    let program = std::env::var("RUSTYDISC_CDRDAO").ok().filter(|p| !p.is_empty()).unwrap_or_else(|| "cdrdao".into());
    let toc = format!("/tmp/rustydisc_gaps_{}.toc", std::process::id());
    let _ = std::fs::remove_file(&toc);
    let mut cmd = std::process::Command::new(program);
    cmd.args(["read-toc", "--device", device, "--driver", "generic-mmc:0x10", &toc]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    if debug {
        eprintln!("Running: {:?}", cmd);
    }
    let status = cmd.status().map_err(|e| format!("could not run cdrdao ({e})"))?;
    let text = std::fs::read_to_string(&toc).map_err(|_| format!("cdrdao did not produce a table of contents (exit {:?})", status.code()));
    let _ = std::fs::remove_file(&toc);
    Ok(parse_read_toc(&text?))
}

/// Stereo samples in one sector.
const SAMPLES_PER_SECTOR: usize = 588;

fn write_wav_header(w: &mut impl std::io::Write, words: usize) -> std::io::Result<()> {
    let data = (words * 4) as u32;
    w.write_all(b"RIFF")?;
    w.write_all(&(36 + data).to_le_bytes())?;
    w.write_all(b"WAVEfmt ")?;
    w.write_all(&16u32.to_le_bytes())?;
    w.write_all(&[1, 0, 2, 0])?;
    w.write_all(&44_100u32.to_le_bytes())?;
    w.write_all(&(44_100u32 * 4).to_le_bytes())?;
    w.write_all(&[4, 0, 16, 0])?;
    w.write_all(b"data")?;
    w.write_all(&data.to_le_bytes())
}

fn copy_range(src: &crate::rip::accuraterip::Wav, from: usize, count: usize, w: &mut impl std::io::Write) -> Result<(), Error> {
    let mut at = from;
    let end = from + count;
    while at < end {
        let chunk = src.read(at, (1 << 18).min(end - at))?;
        if chunk.is_empty() {
            break;
        }
        let mut buf = Vec::with_capacity(chunk.len() * 4);
        for x in &chunk {
            buf.extend_from_slice(&x.to_le_bytes());
        }
        w.write_all(&buf)?;
        at += chunk.len();
    }
    Ok(())
}

/// Move each gap from the end of the previous track to the start of its own track. The tracks
/// keep their total length together; only the cut points move. Returns how many gaps were moved.
/// `wavs` are in disc order; a track is only given a gap when the one before it is its direct
/// neighbour (the `contiguous` flags, as for read offset correction).
pub fn move_gaps_to_own_track(wavs: &[(usize, String)], contiguous: &[bool], gaps: &[TrackGap]) -> Result<usize, Error> {
    let gap_of = |track: usize| gaps.iter().find(|g| g.track == track).map(|g| g.sectors as usize * SAMPLES_PER_SECTOR).unwrap_or(0);
    let sources: Vec<_> = wavs.iter().map(|(_, p)| open_wav(p)).collect::<Result<_, _>>()?;
    // gap_in[k]: samples moved from the end of file k-1 to the start of file k.
    let gap_in: Vec<usize> = (0..wavs.len())
        .map(|k| {
            // Track 1's "gap" is the hidden track or the lead-in, which is not moved into it.
            if k == 0 || !contiguous.get(k - 1).copied().unwrap_or(false) || wavs[k].0 <= 1 {
                return 0;
            }
            gap_of(wavs[k].0).min(sources[k - 1].words / 2)
        })
        .collect();
    let mut moved = 0;
    for (k, src) in sources.iter().enumerate() {
        let tmp = format!("{}.new", wavs[k].1);
        let mut w = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
        let lost_at_end = gap_in.get(k + 1).copied().unwrap_or(0).min(src.words);
        let kept = src.words - lost_at_end;
        write_wav_header(&mut w, gap_in[k] + kept)?;
        if gap_in[k] > 0 {
            let prev = &sources[k - 1];
            copy_range(prev, prev.words - gap_in[k], gap_in[k], &mut w)?;
            moved += 1;
        }
        copy_range(src, 0, kept, &mut w)?;
        std::io::Write::flush(&mut w)?;
    }
    for (_, p) in wavs {
        std::fs::rename(format!("{p}.new"), p)?;
    }
    Ok(moved)
}

/// What became of the hidden audio, for the log.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum HiddenOutcome {
    /// There is none.
    None,
    /// The user chose not to rip it.
    Skipped { seconds: f64 },
    /// Ripped and kept as track 00.
    Kept { seconds: f64 },
    /// There is a gap before track 1 but it holds only silence, so it wasn't kept.
    Silent { seconds: f64 },
    /// The drive wouldn't give it up.
    Failed { seconds: f64, message: String },
}

impl HiddenOutcome {
    /// A line for the rip log, when there is anything to say.
    pub fn note(&self) -> Option<String> {
        let len = |s: &f64| format!("{}:{:02}", (*s as u64) / 60, (*s as u64) % 60);
        match self {
            HiddenOutcome::None => None,
            HiddenOutcome::Skipped { seconds } => Some(format!("This disc has {} of audio before track 1 (a hidden track), which you chose not to rip.", len(seconds))),
            HiddenOutcome::Kept { seconds } => Some(format!("Hidden audio before track 1 ({}) was ripped and saved as track 00.", len(seconds))),
            HiddenOutcome::Silent { seconds } => Some(format!("The {} before track 1 holds only silence, so nothing was saved for it.", len(seconds))),
            HiddenOutcome::Failed { seconds, message } => Some(format!("This disc has {} of audio before track 1, but the drive could not read it ({message}), so it was not saved.", len(seconds))),
        }
    }
}

#[cfg(test)]
mod gap_tests {
    use super::*;

    #[test]
    fn gap_modes_parse() {
        assert_eq!(GapMode::parse("off"), Ok(GapMode::Off));
        assert_eq!(GapMode::parse("report"), Ok(GapMode::Report));
        assert_eq!(GapMode::parse("own-track"), Ok(GapMode::OwnTrack));
        assert!(GapMode::parse("both").is_err());
    }

    #[test]
    fn reads_gaps_from_a_cdrdao_toc() {
        // Shaped like cdrdao's output; tracks 2 and 4 have gaps (2.4 s and 1 s).
        let toc = "CD_DA\n\n// Track 1\nTRACK AUDIO\nFILE \"data.wav\" 0 04:35:67\n\n// Track 2\nTRACK AUDIO\nFILE \"data.wav\" 04:33:30 03:50:00\nSTART 00:02:30\n\n// Track 3\nTRACK AUDIO\nFILE \"data.wav\" 08:24:46 05:15:60\n\n// Track 4\nTRACK AUDIO\nFILE \"data.wav\" 13:39:31 02:58:04\nSTART 00:01:00\n";
        assert_eq!(parse_read_toc(toc), vec![TrackGap { track: 2, sectors: 180 }, TrackGap { track: 4, sectors: 75 }]);
        assert!(parse_read_toc("CD_DA\nTRACK AUDIO\nSTART 00:00:00\n").is_empty(), "a zero start is no gap");
    }

    #[test]
    fn the_scan_runs_cdrdao_and_reads_its_toc() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("rustydisc_scan_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("cdrdao");
        // Writes a TOC with a gap before track 2 to the file named by the last argument.
        std::fs::write(&script, "#!/bin/sh\nfor a in \"$@\"; do last=\"$a\"; done\nprintf 'CD_DA\\nTRACK AUDIO\\nTRACK AUDIO\\nSTART 00:03:00\\n' > \"$last\"\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        // SAFETY: only this test uses this variable.
        unsafe { std::env::set_var("RUSTYDISC_CDRDAO", &script) };
        let found = scan("/dev/null", false);
        unsafe { std::env::remove_var("RUSTYDISC_CDRDAO") };
        assert_eq!(found.unwrap(), vec![TrackGap { track: 2, sectors: 225 }]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn write_wav(path: &std::path::Path, words: &[u32]) {
        let mut b = Vec::new();
        write_wav_header(&mut b, words.len()).unwrap();
        for w in words {
            b.extend_from_slice(&w.to_le_bytes());
        }
        std::fs::write(path, b).unwrap();
    }

    fn read_all(p: &str) -> Vec<u32> {
        let w = open_wav(p).unwrap();
        w.read(0, w.words).unwrap()
    }

    #[test]
    fn a_gap_moves_from_the_end_of_one_track_to_the_start_of_the_next() {
        let dir = std::env::temp_dir().join(format!("rustydisc_gapmove_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // One continuous stream of 30 sectors cut into three tracks of 10 sectors.
        let n = 10 * SAMPLES_PER_SECTOR;
        let stream: Vec<u32> = (1..=(3 * n) as u32).collect();
        let wavs: Vec<(usize, String)> = (0..3)
            .map(|i| {
                let p = dir.join(format!("track{:02}.cdda.wav", i + 1));
                write_wav(&p, &stream[i * n..(i + 1) * n]);
                (i + 1, p.to_string_lossy().to_string())
            })
            .collect();
        // Track 2 has a 3 sector gap; track 3 none.
        let gap = 3 * SAMPLES_PER_SECTOR;
        let moved = move_gaps_to_own_track(&wavs, &[true, true], &[TrackGap { track: 2, sectors: 3 }]).unwrap();
        assert_eq!(moved, 1);
        assert_eq!(read_all(&wavs[0].1), stream[0..n - gap], "track 1 loses the gap");
        assert_eq!(read_all(&wavs[1].1), stream[n - gap..2 * n], "track 2 starts with its gap");
        assert_eq!(read_all(&wavs[2].1), stream[2 * n..3 * n], "track 3 is unchanged");
        // Together they still make the same continuous audio.
        let joined: Vec<u32> = wavs.iter().flat_map(|(_, p)| read_all(p)).collect();
        assert_eq!(joined, stream);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_gap_is_not_moved_across_a_break_in_the_disc() {
        let dir = std::env::temp_dir().join(format!("rustydisc_gapbreak_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let n = 10 * SAMPLES_PER_SECTOR;
        let wavs: Vec<(usize, String)> = (0..2)
            .map(|i| {
                let p = dir.join(format!("track{:02}.cdda.wav", i + 1));
                write_wav(&p, &vec![(i + 1) as u32; n]);
                (i + 1, p.to_string_lossy().to_string())
            })
            .collect();
        assert_eq!(move_gaps_to_own_track(&wavs, &[false], &[TrackGap { track: 2, sectors: 3 }]).unwrap(), 0);
        assert_eq!(read_all(&wavs[0].1).len(), n);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::{DiscFormat, SessionInfo, SessionKind, TrackInfo};

    fn info(first_start: u32) -> DiscInfo {
        let track = |number: usize, a: u32, b: u32| TrackInfo { number, kind: TrackKind::Audio, duration_secs: None, lba_start: a, lba_end: b, cd_text: None };
        DiscInfo {
            format: DiscFormat::RedBook,
            sessions: vec![SessionInfo { index: 0, kind: SessionKind::Audio, tracks: vec![track(1, first_start, first_start + 1000), track(2, first_start + 1000, first_start + 2000)], cd_text: None }],
            is_writable: false,
            device: "/dev/null".into(),
            discid: None,
        }
    }

    #[test]
    fn a_hidden_track_shows_as_track_one_starting_late() {
        assert_eq!(hidden_sectors(&info(0)), None, "a normal disc has none");
        assert_eq!(hidden_sectors(&info(40)), None, "under a second is only a long lead-in");
        assert_eq!(hidden_sectors(&info(75 * 95)), Some(7125));
        assert_eq!(mmss(7125), "1:35");
    }

    #[test]
    fn choices_parse() {
        assert_eq!(HiddenTrack::parse("auto"), Ok(HiddenTrack::Auto));
        assert_eq!(HiddenTrack::parse("skip"), Ok(HiddenTrack::Skip));
        assert!(HiddenTrack::parse("maybe").is_err());
    }

    fn wav(path: &std::path::Path, words: &[u32]) {
        let data = (words.len() * 4) as u32;
        let mut b = Vec::new();
        b.extend(b"RIFF");
        b.extend((36 + data).to_le_bytes());
        b.extend(b"WAVEfmt ");
        b.extend(16u32.to_le_bytes());
        b.extend([1, 0, 2, 0]);
        b.extend(44_100u32.to_le_bytes());
        b.extend((44_100u32 * 4).to_le_bytes());
        b.extend([4, 0, 16, 0]);
        b.extend(b"data");
        b.extend(data.to_le_bytes());
        for w in words {
            b.extend(w.to_le_bytes());
        }
        std::fs::write(path, b).unwrap();
    }

    #[test]
    fn silence_is_told_from_music() {
        let dir = std::env::temp_dir().join(format!("rustydisc_gaps_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let silent = dir.join("s.wav");
        let music = dir.join("m.wav");
        let faint = dir.join("f.wav");
        wav(&silent, &vec![0u32; 5000]);
        let mut loud = vec![0u32; 5000];
        loud[4000] = 0x0000_2000; // one audible sample
        wav(&music, &loud);
        let mut quiet = vec![0u32; 5000];
        quiet[10] = 0x0005_0003; // dither-level noise
        wav(&faint, &quiet);
        assert!(is_silent(silent.to_str().unwrap()).unwrap());
        assert!(!is_silent(music.to_str().unwrap()).unwrap());
        assert!(is_silent(faint.to_str().unwrap()).unwrap(), "a few LSBs of noise is not music");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn notes_say_what_happened() {
        assert!(HiddenOutcome::Kept { seconds: 95.0 }.note().unwrap().contains("1:35") && HiddenOutcome::Kept { seconds: 95.0 }.note().unwrap().contains("track 00"));
        assert!(HiddenOutcome::Silent { seconds: 3.0 }.note().unwrap().contains("silence"));
        assert!(HiddenOutcome::Failed { seconds: 10.0, message: "boom".into() }.note().unwrap().contains("boom"));
        assert!(HiddenOutcome::None.note().is_none());
    }
}
