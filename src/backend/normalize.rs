//! Levelling audio before it is burned.
//!
//! Burning a mix of tracks from different sources onto a CD can leave some far louder than
//! others. This works out a gain for each track from its EBU R128 loudness and applies it while the
//! tracks are converted for the disc. It is opt-in, because it changes the audio.
//!
//! * **album**: one gain for every track, chosen so the whole disc averages the target loudness.
//!   Differences between tracks are kept (the way an album's ReplayGain works).
//! * **track**: each track gets its own gain so every track reaches the target. Differences
//!   between tracks are evened out.
//!
//! A gain is never allowed to push a track's true peak above -1 dBFS, so nothing clips; a track
//! held back this way ends up quieter than the target, and the log says so.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{backend::loudness as meter, error::Error};

/// Loudest a true peak may be after the gain, in dBFS.
pub const PEAK_CEILING_DB: f64 = -1.0;
/// Gains smaller than this are treated as none (no conversion needed).
const NEGLIGIBLE_DB: f64 = 0.05;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Album,
    Track,
}

/// What the user asked for.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Spec {
    pub mode: Mode,
    /// The loudness to aim for, in LUFS.
    #[serde(default = "default_target")]
    pub target_lufs: f64,
}

pub fn default_target() -> f64 {
    -14.0
}

/// `off`, `album` or `track` (and the target, which defaults to -14 LUFS).
pub fn parse(mode: &str, target: Option<f64>) -> Result<Option<Spec>, String> {
    let mode = match mode.trim().to_lowercase().as_str() {
        "" | "off" | "none" => return Ok(None),
        "album" => Mode::Album,
        "track" => Mode::Track,
        other => return Err(format!("'{other}' is not a normalization choice (use off, album or track)")),
    };
    let target = target.unwrap_or_else(default_target);
    if !(-30.0..=-6.0).contains(&target) {
        return Err(format!("The target loudness must be between -30 and -6 LUFS, got {target}"));
    }
    Ok(Some(Spec { mode, target_lufs: target }))
}

/// What measuring found for one track.
#[derive(Debug, Clone, Copy)]
pub struct Measured {
    pub lufs: Option<f64>,
    pub true_peak_db: Option<f64>,
}

/// The gain chosen for one track.
#[derive(Debug, Clone, Serialize)]
pub struct TrackGain {
    pub path: String,
    pub lufs: Option<f64>,
    pub true_peak_db: Option<f64>,
    pub gain_db: f64,
    /// The gain was cut back so the track doesn't clip.
    pub limited: bool,
}

/// Hold a boost back to the headroom. A cut is never limited (it cannot raise the peak), and a
/// track already over the ceiling is simply not boosted rather than cut.
fn limit(want: f64, headroom: f64) -> f64 {
    if want <= 0.0 { want } else { want.min(headroom.max(0.0)) }
}

/// Work out the gains. `album` is the loudness of all the tracks played back to back.
pub fn compute(spec: Spec, tracks: &[(String, Measured)], album: Option<f64>) -> Vec<TrackGain> {
    // The most gain a track can take before its peak passes the ceiling.
    let headroom = |m: &Measured| m.true_peak_db.map(|p| PEAK_CEILING_DB - p).unwrap_or(f64::INFINITY);
    let finish = |g: f64| if g.abs() < NEGLIGIBLE_DB { 0.0 } else { (g * 100.0).round() / 100.0 };

    match spec.mode {
        Mode::Album => {
            let want = album.map(|a| spec.target_lufs - a).unwrap_or(0.0);
            let cap = tracks.iter().map(|(_, m)| headroom(m)).fold(f64::INFINITY, f64::min);
            let gain = limit(want, cap);
            let limited = gain < want - 1e-9;
            tracks
                .iter()
                .map(|(p, m)| TrackGain { path: p.clone(), lufs: m.lufs, true_peak_db: m.true_peak_db, gain_db: finish(gain), limited })
                .collect()
        }
        Mode::Track => tracks
            .iter()
            .map(|(p, m)| {
                // A track with no measurable loudness (silence) is left alone.
                let want = m.lufs.map(|l| spec.target_lufs - l).unwrap_or(0.0);
                let gain = limit(want, headroom(m));
                TrackGain { path: p.clone(), lufs: m.lufs, true_peak_db: m.true_peak_db, gain_db: finish(gain), limited: gain < want - 1e-9 }
            })
            .collect(),
    }
}

/// Measure every track, and all of them together, with ffmpeg's EBU R128 meter.
pub fn measure(paths: &[String]) -> Result<(Vec<(String, Measured)>, Option<f64>), Error> {
    let mut out = Vec::new();
    for p in paths {
        let l = meter::loudness(Path::new(p)).map_err(|e| Error::backend(format!("Could not measure the loudness of '{p}': {e}")))?;
        out.push((p.clone(), Measured { lufs: l.lufs, true_peak_db: l.true_peak_db }));
    }
    let refs: Vec<&Path> = paths.iter().map(|p| Path::new(p.as_str())).collect();
    let album = meter::album_loudness(&refs).map_err(|e| Error::backend(format!("Could not measure the disc's loudness: {e}")))?.lufs;
    Ok((out, album))
}

/// A line for the burn log.
pub fn describe(gains: &[TrackGain], spec: Spec) -> String {
    let changed = gains.iter().filter(|g| g.gain_db != 0.0).count();
    let limited = gains.iter().filter(|g| g.limited).count();
    let how = match spec.mode {
        Mode::Album => "album",
        Mode::Track => "track",
    };
    format!(
        "Normalizing ({how}, target {:.0} LUFS): {changed} of {} track{} change{}{}",
        spec.target_lufs,
        gains.len(),
        if gains.len() == 1 { "" } else { "s" },
        if changed == 1 { "s" } else { "" },
        if limited > 0 { format!("; {limited} held back so they don't clip") } else { String::new() }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(lufs: f64, peak: f64) -> Measured {
        Measured { lufs: Some(lufs), true_peak_db: Some(peak) }
    }
    fn tracks() -> Vec<(String, Measured)> {
        vec![("a.flac".into(), m(-20.0, -6.0)), ("b.flac".into(), m(-10.0, -0.5)), ("c.flac".into(), m(-16.0, -3.0))]
    }

    #[test]
    fn parses_choices_and_targets() {
        assert_eq!(parse("off", None), Ok(None));
        assert_eq!(parse("", None), Ok(None));
        assert_eq!(parse("album", None), Ok(Some(Spec { mode: Mode::Album, target_lufs: -14.0 })));
        assert_eq!(parse("Track", Some(-16.0)), Ok(Some(Spec { mode: Mode::Track, target_lufs: -16.0 })));
        assert!(parse("loud", None).is_err());
        assert!(parse("album", Some(-3.0)).is_err());
        assert!(parse("album", Some(-40.0)).is_err());
    }

    #[test]
    fn track_mode_brings_each_track_to_the_target() {
        let g = compute(Spec { mode: Mode::Track, target_lufs: -14.0 }, &tracks(), None);
        // a: -20 -> -14 needs +6, headroom is -1 - -6 = 5, so it is held back to +5.
        assert_eq!((g[0].gain_db, g[0].limited), (5.0, true));
        // b: -10 -> -14 is -4, nothing to limit.
        assert_eq!((g[1].gain_db, g[1].limited), (-4.0, false));
        // c: -16 -> -14 is +2, headroom 2.0, exactly fits.
        assert_eq!((g[2].gain_db, g[2].limited), (2.0, false));
    }

    #[test]
    fn album_mode_keeps_the_differences_between_tracks() {
        // The disc averages -14 already: no change at all.
        let g = compute(Spec { mode: Mode::Album, target_lufs: -14.0 }, &tracks(), Some(-14.0));
        assert!(g.iter().all(|x| x.gain_db == 0.0 && !x.limited));
        // Quieter than the target by 3 dB would want +3 for every track, but track b's peak is
        // already above the ceiling, so nothing may be boosted (and nothing is cut either).
        let g = compute(Spec { mode: Mode::Album, target_lufs: -14.0 }, &tracks(), Some(-17.0));
        assert!(g.iter().all(|x| x.gain_db == 0.0 && x.limited), "{g:?}");
        // Louder than the target: a cut applies to every track, and is never limited.
        let g = compute(Spec { mode: Mode::Album, target_lufs: -14.0 }, &tracks(), Some(-11.0));
        assert!(g.iter().all(|x| x.gain_db == -3.0 && !x.limited), "{g:?}");
        // With headroom all round, the same gain is given to every track.
        let roomy = vec![("a".to_string(), m(-20.0, -10.0)), ("b".to_string(), m(-12.0, -8.0))];
        let g = compute(Spec { mode: Mode::Album, target_lufs: -14.0 }, &roomy, Some(-17.0));
        assert!(g.iter().all(|x| x.gain_db == 3.0 && !x.limited), "{g:?}");
    }

    #[test]
    fn a_boost_never_pushes_a_peak_past_the_ceiling() {
        for spec in [Spec { mode: Mode::Track, target_lufs: -9.0 }, Spec { mode: Mode::Album, target_lufs: -9.0 }] {
            for g in compute(spec, &tracks(), Some(-15.0)) {
                if let Some(p) = g.true_peak_db {
                    assert!(g.gain_db <= 0.0 || p + g.gain_db <= PEAK_CEILING_DB + 1e-6, "a boost never pushes a peak past the ceiling: {g:?}");
                }
            }
        }
    }

    #[test]
    fn silence_and_tiny_gains_are_left_alone() {
        let t = vec![("s.wav".into(), Measured { lufs: None, true_peak_db: None }), ("x.wav".into(), m(-14.02, -8.0))];
        let g = compute(Spec { mode: Mode::Track, target_lufs: -14.0 }, &t, None);
        assert_eq!((g[0].gain_db, g[1].gain_db), (0.0, 0.0));
    }

    #[test]
    fn the_log_line_says_what_happened() {
        let spec = Spec { mode: Mode::Track, target_lufs: -14.0 };
        let line = describe(&compute(spec, &tracks(), None), spec);
        assert!(line.contains("track, target -14 LUFS") && line.contains("3 of 3 tracks change") && line.contains("1 held back"), "{line}");
    }

    /// The point of it all: tracks of very different loudness, levelled and converted, measure at
    /// the target afterwards. Needs ffmpeg, so it is skipped where there is none.
    #[test]
    fn levelled_tracks_really_measure_at_the_target() {
        if std::process::Command::new("ffmpeg").arg("-version").output().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("rustydisc_norm_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Two stereo CD-format tones, one quiet and one loud (a noisy tone keeps the meter happy).
        let mut paths = Vec::new();
        for (name, vol) in [("quiet.wav", "0.5"), ("loud.wav", "6")] {
            let p = dir.join(name);
            let ok = std::process::Command::new("ffmpeg")
                .args(["-loglevel", "error", "-y", "-f", "lavfi", "-i", "sine=f=330:d=8", "-af", &format!("volume={vol}"), "-ac", "2", "-ar", "44100", "-sample_fmt", "s16"])
                .arg(&p)
                .status()
                .unwrap()
                .success();
            assert!(ok);
            paths.push(p.to_string_lossy().to_string());
        }
        let spec = Spec { mode: Mode::Track, target_lufs: -20.0 };
        let (measured, album) = measure(&paths).unwrap();
        let gains = compute(spec, &measured, album);
        assert!(gains[0].gain_db > 0.0 && gains[1].gain_db < 0.0, "{gains:?}");

        let mut after = Vec::new();
        for (p, g) in paths.iter().zip(&gains) {
            let out = super::super::convert::to_cdda_wav_with_gain(p, g.gain_db, false).unwrap();
            assert_ne!(&out, p, "a gain always makes a new file, even from a WAV");
            after.push(out);
        }
        let (re, _) = measure(&after).unwrap();
        for (_, m) in &re {
            assert!((m.lufs.unwrap() + 20.0).abs() < 0.3, "now at the target: {re:?}");
        }
        for p in after {
            let _ = std::fs::remove_file(p);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
