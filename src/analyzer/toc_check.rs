//! Sanity checks on a disc's table of contents.
//!
//! Most discs are ordinary, but a few have a table of contents that is odd, damaged or built to
//! confuse rippers. Looking before ripping explains problems that would otherwise show up as a
//! failed AccurateRip check or the wrong tags. Everything here works on the table of contents
//! already read from the disc; nothing touches the drive.

use serde::{Deserialize, Serialize};

use super::{DiscInfo, SessionKind, TrackKind};

/// Red Book: a track is at least four seconds long.
const MIN_TRACK_SECTORS: u32 = 300;
/// 80 minutes of audio.
const EIGHTY_MINUTES: u32 = 80 * 60 * 75;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// Worth knowing, nothing is wrong.
    Note,
    /// Something is unusual or broken and may affect the rip.
    Warning,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Finding {
    pub level: Level,
    /// A stable name for the check, for scripts.
    pub code: String,
    pub message: String,
    /// The tracks it is about, if any.
    #[serde(default)]
    pub tracks: Vec<usize>,
}

fn f(level: Level, code: &str, message: String, tracks: Vec<usize>) -> Finding {
    Finding { level, code: code.into(), message, tracks }
}

fn mmss(sectors: u32) -> String {
    let s = (sectors as f64 / 75.0).round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Check the table of contents. Findings come back in the order they should be read.
pub fn check(info: &DiscInfo) -> Vec<Finding> {
    let mut out = Vec::new();
    let tracks: Vec<_> = info.sessions.iter().flat_map(|s| s.tracks.iter().map(move |t| (s.index, t))).collect();
    if tracks.is_empty() {
        return out;
    }

    // Numbering: 1, 2, 3 ... with nothing missing or repeated.
    let mut numbers: Vec<usize> = tracks.iter().map(|(_, t)| t.number).collect();
    numbers.sort_unstable();
    let consecutive = numbers.iter().enumerate().all(|(i, n)| *n == numbers[0] + i);
    if numbers[0] != 1 || !consecutive {
        out.push(f(Level::Warning, "track_numbers", format!("The track numbers are not 1, 2, 3 ... in order ({}), so the table of contents may be damaged or unusual.", numbers.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(", ")), numbers.clone()));
    }

    // Order: each track should start after the one before it.
    let mut by_number: Vec<_> = tracks.iter().map(|(_, t)| *t).collect();
    by_number.sort_by_key(|t| t.number);
    for w in by_number.windows(2) {
        if w[1].lba_start <= w[0].lba_start {
            out.push(f(Level::Warning, "track_order", format!("Track {} starts at or before track {} ({} vs {}), so the table of contents is out of order.", w[1].number, w[0].number, mmss(w[1].lba_start), mmss(w[0].lba_start)), vec![w[0].number, w[1].number]));
        }
    }

    // Lengths.
    for (_, t) in &tracks {
        if t.kind != TrackKind::Audio {
            continue;
        }
        let len = t.lba_end.saturating_sub(t.lba_start);
        if t.lba_end <= t.lba_start {
            out.push(f(Level::Warning, "zero_length", format!("Track {} has no length in the table of contents.", t.number), vec![t.number]));
        } else if len < MIN_TRACK_SECTORS {
            out.push(f(Level::Note, "short_track", format!("Track {} is only {:.1} s long; the Red Book standard asks for at least 4 s, so this is unusual.", t.number, len as f64 / 75.0), vec![t.number]));
        }
    }

    // The end of the disc: if the lead-out could not be read, the last track's length is a guess.
    if let Some(last) = by_number.last() {
        if last.lba_end < last.lba_start + MIN_TRACK_SECTORS && last.kind == TrackKind::Audio {
            out.push(f(Level::Warning, "leadout_unreadable", "The end of the disc could not be read, so the last track's length is a guess and AccurateRip can't check this rip.".into(), vec![last.number]));
        }
        if last.lba_end > EIGHTY_MINUTES {
            out.push(f(Level::Note, "long_disc", format!("The audio runs for {}, longer than the usual 80 minutes (an extended or overburned disc).", mmss(last.lba_end)), vec![]));
        }
    }

    // Hidden audio in front of track 1.
    if let Some(first) = by_number.iter().find(|t| t.kind == TrackKind::Audio) {
        if first.number == 1 && first.lba_start >= crate::rip::gaps::MIN_HIDDEN_SECTORS {
            out.push(f(Level::Note, "hidden_audio", format!("Track 1 starts {} into the disc, which usually means hidden audio in front of it (a hidden track).", mmss(first.lba_start)), vec![1]));
        }
    }

    // Data and audio mixed.
    for s in &info.sessions {
        let has_audio = s.tracks.iter().any(|t| t.kind == TrackKind::Audio);
        let data_first = s.tracks.first().is_some_and(|t| t.kind == TrackKind::Data);
        if has_audio && data_first {
            out.push(f(Level::Note, "mixed_mode", "A data track comes before the audio tracks (a mixed-mode disc). The data is not part of the music and is skipped when ripping audio.".into(), vec![s.tracks[0].number]));
        }
    }
    let mut seen_data_session = false;
    for s in &info.sessions {
        match s.kind {
            SessionKind::Data { .. } => seen_data_session = true,
            SessionKind::Audio if seen_data_session => {
                out.push(f(Level::Warning, "audio_after_data", format!("Session {} holds audio after a data session, the reverse of the usual Enhanced CD order, so some software won't find it.", s.index), vec![]));
            }
            _ => {}
        }
    }

    // Number of tracks.
    if tracks.len() > 99 {
        out.push(f(Level::Warning, "too_many_tracks", format!("The disc lists {} tracks; a CD can hold at most 99.", tracks.len()), vec![]));
    }
    out
}

/// A disc whose track count doesn't match the MusicBrainz release it was matched to.
pub fn release_mismatch(info: &DiscInfo, release_tracks: usize) -> Option<Finding> {
    let audio = info.sessions.iter().flat_map(|s| s.tracks.iter()).filter(|t| t.kind == TrackKind::Audio).count();
    (release_tracks > 0 && audio > 0 && release_tracks != audio).then(|| {
        f(Level::Warning, "release_mismatch", format!("The disc has {audio} audio tracks but the MusicBrainz release has {release_tracks}, so the titles and tags may belong to a different edition."), vec![])
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::{DiscFormat, SessionInfo, TrackInfo};

    fn t(number: usize, kind: TrackKind, a: u32, b: u32) -> TrackInfo {
        TrackInfo { number, kind, duration_secs: None, lba_start: a, lba_end: b, cd_text: None }
    }

    fn disc(sessions: Vec<(SessionKind, Vec<TrackInfo>)>) -> DiscInfo {
        DiscInfo {
            format: DiscFormat::RedBook,
            sessions: sessions.into_iter().enumerate().map(|(i, (kind, tracks))| SessionInfo { index: i, kind, tracks, cd_text: None }).collect(),
            is_writable: false,
            device: "/dev/null".into(),
            discid: None,
        }
    }

    fn data() -> SessionKind {
        SessionKind::Data { volume_label: None, size_mb: 10.0, filesystem: "iso9660".into() }
    }

    fn codes(info: &DiscInfo) -> Vec<String> {
        check(info).into_iter().map(|f| f.code).collect()
    }

    fn audio3() -> Vec<TrackInfo> {
        vec![t(1, TrackKind::Audio, 0, 20000), t(2, TrackKind::Audio, 20000, 40000), t(3, TrackKind::Audio, 40000, 60000)]
    }

    #[test]
    fn an_ordinary_disc_has_nothing_to_report() {
        assert!(check(&disc(vec![(SessionKind::Audio, audio3())])).is_empty());
    }

    #[test]
    fn the_real_morrissey_disc_is_clean() {
        // Start sectors from the disc in the drive: 18 tracks, ending at 333769.
        let starts = [0u32, 20692, 37846, 61531, 74810, 89864, 110607, 129166, 145439, 160148, 181909, 199353, 219757, 238238, 255875, 270976, 283699, 315973, 333769];
        let tracks = (0..18).map(|i| t(i + 1, TrackKind::Audio, starts[i], starts[i + 1])).collect();
        assert!(check(&disc(vec![(SessionKind::Audio, tracks)])).is_empty());
    }

    #[test]
    fn missing_or_repeated_track_numbers_are_flagged() {
        let tracks = vec![t(1, TrackKind::Audio, 0, 20000), t(3, TrackKind::Audio, 20000, 40000)];
        assert_eq!(codes(&disc(vec![(SessionKind::Audio, tracks)])), ["track_numbers"]);
        let from_two = vec![t(2, TrackKind::Audio, 0, 20000), t(3, TrackKind::Audio, 20000, 40000)];
        assert!(codes(&disc(vec![(SessionKind::Audio, from_two)])).contains(&"track_numbers".to_string()));
    }

    #[test]
    fn out_of_order_and_empty_tracks_are_flagged() {
        let tracks = vec![t(1, TrackKind::Audio, 0, 20000), t(2, TrackKind::Audio, 20000, 20000), t(3, TrackKind::Audio, 15000, 40000)];
        let c = codes(&disc(vec![(SessionKind::Audio, tracks)]));
        assert!(c.contains(&"track_order".to_string()) && c.contains(&"zero_length".to_string()), "{c:?}");
    }

    #[test]
    fn a_very_short_track_is_a_note() {
        let tracks = vec![t(1, TrackKind::Audio, 0, 20000), t(2, TrackKind::Audio, 20000, 20100), t(3, TrackKind::Audio, 20100, 40000)];
        let f = check(&disc(vec![(SessionKind::Audio, tracks)]));
        assert_eq!((f[0].code.as_str(), f[0].level), ("short_track", Level::Note));
        assert!(f[0].message.contains("1.3 s"), "{}", f[0].message);
    }

    #[test]
    fn an_unreadable_end_of_disc_is_a_warning() {
        // The analyzer's placeholder for an unreadable lead-out: the end sits just past the start.
        let tracks = vec![t(1, TrackKind::Audio, 0, 20000), t(2, TrackKind::Audio, 20000, 20001)];
        assert!(codes(&disc(vec![(SessionKind::Audio, tracks)])).contains(&"leadout_unreadable".to_string()));
    }

    #[test]
    fn long_discs_and_hidden_audio_are_notes() {
        let tracks = vec![t(1, TrackKind::Audio, 7125, 200000), t(2, TrackKind::Audio, 200000, 380000)];
        let f = check(&disc(vec![(SessionKind::Audio, tracks)]));
        let c: Vec<&str> = f.iter().map(|x| x.code.as_str()).collect();
        assert!(c.contains(&"long_disc") && c.contains(&"hidden_audio"), "{c:?}");
        assert!(f.iter().all(|x| x.level == Level::Note));
    }

    #[test]
    fn data_and_audio_orderings() {
        // Enhanced CD, the usual order: no complaint.
        assert!(check(&disc(vec![(SessionKind::Audio, audio3()), (data(), vec![t(4, TrackKind::Data, 70000, 80000)])])).is_empty());
        // Mixed mode: data first in a session that also has audio.
        let mixed = vec![t(1, TrackKind::Data, 0, 5000), t(2, TrackKind::Audio, 5000, 40000)];
        assert!(codes(&disc(vec![(SessionKind::Audio, mixed)])).contains(&"mixed_mode".to_string()));
        // Audio after data.
        let reversed = disc(vec![(data(), vec![t(1, TrackKind::Data, 0, 5000)]), (SessionKind::Audio, vec![t(2, TrackKind::Audio, 20000, 40000)])]);
        assert!(codes(&reversed).contains(&"audio_after_data".to_string()));
    }

    #[test]
    fn a_release_with_a_different_track_count_is_flagged() {
        let d = disc(vec![(SessionKind::Audio, audio3())]);
        assert!(release_mismatch(&d, 3).is_none());
        assert!(release_mismatch(&d, 0).is_none(), "an unknown count is not a mismatch");
        assert!(release_mismatch(&d, 12).unwrap().message.contains("3 audio tracks but the MusicBrainz release has 12"));
    }
}
