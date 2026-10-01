//! What is really in an audio file: format, bit depth, sample rate and bitrate; whether it decodes
//! cleanly; how loud it is and how close it comes to clipping; ReplayGain; and a spectrogram to
//! spot a "lossless" file that started life as an MP3.

use std::{path::Path, process::Command};

use lofty::{
    config::WriteOptions,
    file::{AudioFile, TaggedFileExt},
    probe::Probe,
    tag::{ItemKey, Tag},
};
use serde::Serialize;
use serde_json::Value;

use crate::rip::tagging::set;

/// A track's path and its measured DR (used while ripping).
pub struct TrackDrRow {
    pub path: String,
    pub dr: u32,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct AudioFacts {
    pub codec: String,
    /// "FLAC", "MP3", ...
    pub label: String,
    pub sample_rate: Option<u32>,
    /// Bits per sample. Only meaningful for lossless and PCM files.
    pub bit_depth: Option<u32>,
    pub channels: Option<u32>,
    pub bitrate_kbps: Option<f64>,
    pub duration_secs: Option<f64>,
    pub size: u64,
    pub lossless: bool,
}

fn label_of(codec: &str) -> &'static str {
    match codec {
        "flac" => "FLAC",
        "mp3" => "MP3",
        "alac" => "ALAC",
        "aac" => "AAC",
        "opus" => "Opus",
        "vorbis" => "Vorbis",
        c if c.starts_with("pcm_") => "PCM",
        "wavpack" => "WavPack",
        "ape" => "APE",
        _ => "audio",
    }
}

fn is_lossless(codec: &str) -> bool {
    matches!(codec, "flac" | "alac" | "wavpack" | "ape" | "tta") || codec.starts_with("pcm_")
}

/// Ask ffprobe about the first audio stream.
pub fn facts(path: &Path) -> Result<AudioFacts, String> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "a:0", "-show_entries", "stream=codec_name,sample_rate,channels,bits_per_raw_sample,bits_per_sample,sample_fmt,bit_rate,duration:format=duration,bit_rate,size", "-of", "json"])
        .arg(path)
        .output()
        .map_err(|e| format!("ffprobe isn't available: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).lines().next().unwrap_or("ffprobe failed").to_string());
    }
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    let s = v["streams"].get(0).ok_or("no audio stream")?;
    let num = |x: &Value| x.as_str().and_then(|t| t.parse::<f64>().ok()).or_else(|| x.as_f64());
    let codec = s["codec_name"].as_str().unwrap_or("").to_string();
    let lossless = is_lossless(&codec);
    let raw: Option<u32> = num(&s["bits_per_raw_sample"]).map(|b| b as u32).filter(|b| *b > 0);
    let pcm: Option<u32> = num(&s["bits_per_sample"]).map(|b| b as u32).filter(|b| *b > 0);
    let fmt_bits = match s["sample_fmt"].as_str().unwrap_or("") {
        "s16" | "s16p" => Some(16),
        "s32" | "s32p" => Some(32),
        "u8" | "u8p" => Some(8),
        _ => None,
    };
    let bit_depth = if lossless { raw.or(pcm).or(fmt_bits) } else { None };
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let duration = num(&s["duration"]).or_else(|| num(&v["format"]["duration"]));
    // The file's overall average, which also counts tags and artwork; better than the stream's nominal rate.
    let bitrate = duration.filter(|d| *d > 1.0).map(|d| size as f64 * 8.0 / d / 1000.0).or_else(|| num(&s["bit_rate"]).map(|b| b / 1000.0));
    Ok(AudioFacts {
        label: label_of(&codec).to_string(),
        codec,
        sample_rate: num(&s["sample_rate"]).map(|r| r as u32),
        bit_depth,
        channels: num(&s["channels"]).map(|c| c as u32),
        bitrate_kbps: bitrate,
        duration_secs: duration,
        size,
        lossless,
    })
}

fn khz(hz: u32) -> String {
    let k = hz as f64 / 1000.0;
    if (k - k.round()).abs() < 1e-6 { format!("{k:.0} kHz") } else { format!("{k:.1} kHz") }
}

impl AudioFacts {
    /// "FLAC · 16-bit · 44.1 kHz · stereo"
    pub fn describe(&self) -> String {
        let mut parts = vec![self.label.clone()];
        if let Some(b) = self.bit_depth {
            parts.push(format!("{b}-bit"));
        }
        if let Some(r) = self.sample_rate {
            parts.push(khz(r));
        }
        parts.push(match self.channels {
            Some(1) => "mono".into(),
            Some(2) => "stereo".into(),
            Some(n) => format!("{n} channels"),
            None => "?".into(),
        });
        parts.join(" · ")
    }

    /// The disc's own format: 16-bit, 44.1 kHz, stereo, lossless.
    pub fn is_cd_quality(&self) -> bool {
        self.lossless && self.bit_depth == Some(16) && self.sample_rate == Some(44_100) && self.channels == Some(2)
    }
}

#[derive(Debug, Serialize)]
pub struct Summary {
    pub files: usize,
    /// One line describing the whole set, e.g. "FLAC · 16-bit · 44.1 kHz · stereo".
    pub description: String,
    /// All files share one format description.
    pub uniform: bool,
    pub lossless: bool,
    pub cd_quality: bool,
    pub avg_bitrate_kbps: Option<f64>,
    pub min_bitrate_kbps: Option<f64>,
    pub max_bitrate_kbps: Option<f64>,
    pub total_bytes: u64,
    pub notes: Vec<String>,
}

pub fn summarize(all: &[AudioFacts]) -> Summary {
    let descs: Vec<String> = all.iter().map(AudioFacts::describe).collect();
    let uniform = descs.windows(2).all(|w| w[0] == w[1]);
    let rates: Vec<f64> = all.iter().filter_map(|f| f.bitrate_kbps).collect();
    let mut notes = Vec::new();
    let lossless = !all.is_empty() && all.iter().all(|f| f.lossless);
    let cd = !all.is_empty() && all.iter().all(AudioFacts::is_cd_quality);
    if !uniform {
        notes.push("The files are not all in the same format.".to_string());
    }
    if !all.is_empty() && !lossless && all.iter().any(|f| f.lossless) {
        notes.push("A mix of lossless and lossy files.".to_string());
    }
    if lossless && !cd {
        notes.push("Lossless, but not CD quality (16-bit / 44.1 kHz stereo).".to_string());
    }
    if all.iter().any(|f| !f.lossless) && !all.iter().all(|f| !f.lossless) {
        // covered above
    } else if !all.is_empty() && !lossless {
        notes.push("Lossy: some of the original detail was discarded when it was encoded.".to_string());
    }
    Summary {
        files: all.len(),
        description: descs.first().cloned().unwrap_or_default(),
        uniform,
        lossless,
        cd_quality: cd,
        avg_bitrate_kbps: (!rates.is_empty()).then(|| rates.iter().sum::<f64>() / rates.len() as f64),
        min_bitrate_kbps: rates.iter().cloned().fold(None, |a: Option<f64>, x| Some(a.map_or(x, |m| m.min(x)))),
        max_bitrate_kbps: rates.iter().cloned().fold(None, |a: Option<f64>, x| Some(a.map_or(x, |m| m.max(x)))),
        total_bytes: all.iter().map(|f| f.size).sum(),
        notes,
    }
}

// ── Integrity ─────────────────────────────────────────────────────────────────

/// Decode the whole file and report anything the decoder complained about.
pub fn integrity(path: &Path) -> Result<(), String> {
    let out = Command::new("ffmpeg").args(["-v", "error", "-nostdin", "-err_detect", "crccheck+bitstream+buffer+explode", "-i"]).arg(path).args(["-f", "null", "-"]).output().map_err(|e| e.to_string())?;
    let err = String::from_utf8_lossy(&out.stderr);
    let first = err.lines().find(|l| !l.trim().is_empty());
    if !out.status.success() || first.is_some() {
        return Err(first.unwrap_or("decoding failed").trim().to_string());
    }
    Ok(())
}

// ── Loudness ──────────────────────────────────────────────────────────────────

// The EBU R128 meter lives in `backend::loudness` (burning uses it too).
pub use crate::backend::loudness::{album_loudness, loudness, Loudness};

fn fmt_gain(db: f64) -> String {
    format!("{db:+.2} dB")
}

/// Write ReplayGain 2.0 tags (track and album gain and peak) into a file.
pub fn write_replaygain(path: &Path, track: &Loudness, album: &Loudness) -> Result<(), String> {
    let mut file = Probe::open(path).map_err(|e| e.to_string())?.read().map_err(|e| e.to_string())?;
    if file.primary_tag().is_none() {
        let tt = file.primary_tag_type();
        file.insert_tag(Tag::new(tt));
    }
    let tag = file.primary_tag_mut().ok_or("this format can't hold tags")?;
    if let (Some(g), Some(p)) = (track.gain_db(), track.peak_linear()) {
        set(tag, ItemKey::ReplayGainTrackGain, &fmt_gain(g));
        set(tag, ItemKey::ReplayGainTrackPeak, &format!("{p:.6}"));
    }
    if let (Some(g), Some(p)) = (album.gain_db(), album.peak_linear()) {
        set(tag, ItemKey::ReplayGainAlbumGain, &fmt_gain(g));
        set(tag, ItemKey::ReplayGainAlbumPeak, &format!("{p:.6}"));
    }
    file.save_to_path(path, WriteOptions::default()).map_err(|e| e.to_string())
}

// ── Spectrogram ───────────────────────────────────────────────────────────────

/// Draw the file's spectrogram. A real CD rip fills the picture up to 20 kHz or more; audio that
/// was once an MP3 stops at a hard ceiling (around 16 kHz for 128 kbps).
pub fn spectrogram(path: &Path, out_png: &Path) -> Result<(), String> {
    if let Some(dir) = out_png.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let out = Command::new("ffmpeg")
        .args(["-v", "error", "-nostdin", "-y", "-i"])
        .arg(path)
        .args(["-lavfi", "showspectrumpic=s=1200x480:legend=1:scale=log:color=intensity", "-frames:v", "1"])
        .arg(out_png)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() || !out_png.is_file() {
        return Err(String::from_utf8_lossy(&out.stderr).lines().next().unwrap_or("couldn't draw the spectrogram").to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn have_ffmpeg() -> bool {
        Command::new("ffmpeg").arg("-version").output().is_ok() && Command::new("ffprobe").arg("-version").output().is_ok()
    }

    fn make(dir: &Path, name: &str, args: &[&str]) -> std::path::PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let p = dir.join(name);
        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=3"]).args(args).arg(&p);
        assert!(cmd.status().unwrap().success());
        p
    }

    #[test]
    fn reports_format_bit_depth_sample_rate_and_bitrate() {
        if !have_ffmpeg() {
            return;
        }
        let d = std::env::temp_dir().join(format!("rd_audioinfo_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let flac16 = make(&d, "a.flac", &["-ar", "44100", "-ac", "2", "-sample_fmt", "s16", "-c:a", "flac"]);
        let flac24 = make(&d, "b.flac", &["-ar", "96000", "-ac", "2", "-sample_fmt", "s32", "-c:a", "flac"]);
        let mp3 = make(&d, "c.mp3", &["-ar", "44100", "-ac", "2", "-c:a", "libmp3lame", "-b:a", "128k"]);

        let f = facts(&flac16).unwrap();
        assert_eq!((f.label.as_str(), f.bit_depth, f.sample_rate, f.channels), ("FLAC", Some(16), Some(44_100), Some(2)));
        assert!(f.is_cd_quality() && f.lossless);
        assert_eq!(f.describe(), "FLAC · 16-bit · 44.1 kHz · stereo");

        let f24 = facts(&flac24).unwrap();
        assert!(f24.bit_depth.unwrap() > 16 && f24.sample_rate == Some(96_000) && !f24.is_cd_quality());
        assert!(f24.describe().contains("96 kHz"));

        let m = facts(&mp3).unwrap();
        assert_eq!((m.label.as_str(), m.bit_depth, m.lossless), ("MP3", None, false));
        let kbps = m.bitrate_kbps.unwrap();
        assert!((110.0..150.0).contains(&kbps), "{kbps}");

        let s = summarize(&[facts(&flac16).unwrap(), facts(&flac16).unwrap()]);
        assert!(s.uniform && s.lossless && s.cd_quality && s.notes.is_empty());
        let mixed = summarize(&[facts(&flac16).unwrap(), m]);
        assert!(!mixed.uniform && !mixed.cd_quality && mixed.notes.iter().any(|n| n.contains("mix")));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_damaged_file_fails_the_integrity_test_and_a_good_one_passes() {
        if !have_ffmpeg() {
            return;
        }
        let d = std::env::temp_dir().join(format!("rd_integrity_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let good = make(&d, "a.flac", &["-c:a", "flac"]);
        assert!(integrity(&good).is_ok());
        let mut bytes = std::fs::read(&good).unwrap();
        let mid = bytes.len() / 2;
        for b in &mut bytes[mid..mid + 200] {
            *b ^= 0xA5;
        }
        let bad = d.join("bad.flac");
        std::fs::write(&bad, bytes).unwrap();
        assert!(integrity(&bad).is_err());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn loudness_replaygain_and_the_spectrogram_work() {
        if !have_ffmpeg() {
            return;
        }
        let d = std::env::temp_dir().join(format!("rd_loudness_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let quiet = make(&d, "quiet.flac", &["-af", "volume=-24dB", "-c:a", "flac"]);
        let loud = make(&d, "loud.flac", &["-af", "volume=-6dB", "-c:a", "flac"]);
        let (lq, ll) = (loudness(&quiet).unwrap(), loudness(&loud).unwrap());
        assert!(ll.lufs.unwrap() > lq.lufs.unwrap() + 10.0, "{ll:?} vs {lq:?}");
        assert!(lq.gain_db().unwrap() > ll.gain_db().unwrap(), "the quiet track gets more gain");
        assert!(ll.true_peak_db.unwrap() < 0.0 && lq.peak_linear().unwrap() < ll.peak_linear().unwrap());
        let album = album_loudness(&[&quiet, &loud]).unwrap();
        assert!(album.lufs.unwrap() > lq.lufs.unwrap() && album.lufs.unwrap() < ll.lufs.unwrap());

        write_replaygain(&loud, &ll, &album).unwrap();
        let t = crate::library::tags::read(&loud);
        let _ = t;
        let file = Probe::open(&loud).unwrap().read().unwrap();
        let tag = file.primary_tag().unwrap();
        let gain = tag.get_string(&ItemKey::ReplayGainTrackGain).unwrap();
        assert!(gain.ends_with(" dB") && gain.starts_with(['+', '-']), "{gain}");
        assert!(tag.get_string(&ItemKey::ReplayGainAlbumPeak).is_some());

        let png = d.join("s.png");
        spectrogram(&loud, &png).unwrap();
        assert!(std::fs::read(&png).unwrap().starts_with(&[0x89, b'P', b'N', b'G']));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn ebur128_output_is_parsed() {
        let text = "junk\n[Parsed_ebur128_0 @ 0x1] Summary:\n\n  Integrated loudness:\n    I:         -14.4 LUFS\n    Threshold: -24.4 LUFS\n\n  Loudness range:\n    LRA:         5.7 LU\n\n  True peak:\n    Peak:       -0.2 dBFS\n";
        let l = crate::backend::loudness::parse_ebur128(text);
        assert_eq!((l.lufs, l.lra, l.true_peak_db), (Some(-14.4), Some(5.7), Some(-0.2)));
        assert!((l.gain_db().unwrap() - (-3.6)).abs() < 1e-9);
    }
}
