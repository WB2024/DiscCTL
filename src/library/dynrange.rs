//! Dynamic range (DR), the number the Dynamic Range Database (dr.loudness-war.info) lists for
//! albums: how far the peaks rise above the loud parts of a track. High is dynamic and
//! natural; low is squashed by the loudness war.
//!
//! The method is the published "TT DR" one: per channel, split the audio into 3-second blocks,
//! take each block's RMS (scaled so a full-scale sine reads 0 dB) and peak; DR is the
//! second-highest block peak over the RMS of the loudest 20% of blocks, averaged over the
//! channels. A track's DR is that value rounded, and the album's is the average of its tracks.
//! This is a faithful reimplementation rather than the original meter, so a value can differ
//! from a published one by a point.

use std::{io::Read, path::Path, process::{Command, Stdio}};

use lofty::{config::WriteOptions, file::{AudioFile, TaggedFileExt}, probe::Probe, tag::Tag};

use serde::Serialize;

const BLOCK_SECS: usize = 3;

#[derive(Debug, Clone, Serialize)]
pub struct TrackDr {
    /// The rounded value, as shown in DR logs.
    pub dr: u32,
    /// Before rounding.
    pub dr_exact: f64,
    /// Second-highest peak, dBFS (averaged over channels).
    pub peak_db: f64,
    /// RMS of the loudest 20% of blocks, dBFS (averaged over channels).
    pub rms_db: f64,
    pub seconds: f64,
}

/// Accumulates interleaved samples and reports the DR at the end.
pub struct Meter {
    channels: usize,
    block_len: usize,
    filled: usize,
    sum_sq: Vec<f64>,
    peak: Vec<f64>,
    blocks: Vec<Vec<(f64, f64)>>, // per channel: (mean square * 2, peak) of each finished block
    samples: usize,
    rate: usize,
}

impl Meter {
    pub fn new(channels: usize, rate: usize) -> Meter {
        Meter {
            channels,
            block_len: BLOCK_SECS * rate,
            filled: 0,
            sum_sq: vec![0.0; channels],
            peak: vec![0.0; channels],
            blocks: vec![Vec::new(); channels],
            samples: 0,
            rate,
        }
    }

    fn close_block(&mut self) {
        if self.filled == 0 {
            return;
        }
        for c in 0..self.channels {
            self.blocks[c].push((2.0 * self.sum_sq[c] / self.filled as f64, self.peak[c]));
            self.sum_sq[c] = 0.0;
            self.peak[c] = 0.0;
        }
        self.filled = 0;
    }

    /// Feed interleaved samples (-1.0 to 1.0).
    pub fn push(&mut self, interleaved: &[f32]) {
        for frame in interleaved.chunks_exact(self.channels) {
            for (c, &s) in frame.iter().enumerate() {
                let s = s as f64;
                self.sum_sq[c] += s * s;
                self.peak[c] = self.peak[c].max(s.abs());
            }
            self.filled += 1;
            self.samples += 1;
            if self.filled == self.block_len {
                self.close_block();
            }
        }
    }

    pub fn finish(mut self) -> Option<TrackDr> {
        // A whole number of blocks is what the meter measures; only keep a short last block if that is all there is.
        if self.blocks.first().is_none_or(|b| b.is_empty()) {
            self.close_block();
        }
        let n = self.blocks.first()?.len();
        if n == 0 {
            return None;
        }
        let top = ((n as f64 * 0.2).floor() as usize).max(1);
        let (mut dr_sum, mut peak_sum, mut rms_sum, mut used) = (0.0, 0.0, 0.0, 0usize);
        for ch in &self.blocks {
            let mut rms: Vec<f64> = ch.iter().map(|b| b.0).collect();
            rms.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
            let loud = (rms[..top].iter().sum::<f64>() / top as f64).sqrt();
            let mut peaks: Vec<f64> = ch.iter().map(|b| b.1).collect();
            peaks.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
            let pk = if peaks.len() > 1 { peaks[1] } else { peaks[0] };
            if loud <= 0.0 || pk <= 0.0 {
                continue; // a silent channel says nothing
            }
            dr_sum += 20.0 * (pk / loud).log10();
            peak_sum += 20.0 * pk.log10();
            rms_sum += 20.0 * loud.log10();
            used += 1;
        }
        if used == 0 {
            return None;
        }
        let k = used as f64;
        let exact = (dr_sum / k).max(0.0);
        Some(TrackDr { dr: exact.round() as u32, dr_exact: exact, peak_db: peak_sum / k, rms_db: rms_sum / k, seconds: self.samples as f64 / self.rate as f64 })
    }
}

/// Decode a file with ffmpeg and measure it.
pub fn measure(path: &Path, channels: usize, rate: usize) -> Result<TrackDr, String> {
    if channels == 0 || channels > 8 || rate == 0 {
        return Err("This file's channel layout can't be measured".into());
    }
    let mut child = Command::new("ffmpeg")
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(path)
        .args(["-vn", "-f", "f32le", "-acodec", "pcm_f32le", "-ac", &channels.to_string(), "-ar", &rate.to_string(), "-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("ffmpeg isn't available: {e}"))?;
    let mut out = child.stdout.take().ok_or("no output from ffmpeg")?;
    let mut meter = Meter::new(channels, rate);
    let mut buf = vec![0u8; 1 << 16];
    let mut carry: Vec<u8> = Vec::new();
    loop {
        let n = out.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        carry.extend_from_slice(&buf[..n]);
        let usable = carry.len() / (4 * channels) * (4 * channels);
        let floats: Vec<f32> = carry[..usable].chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
        meter.push(&floats);
        carry.drain(..usable);
    }
    let _ = child.wait();
    meter.finish().ok_or_else(|| "The track is silent or too short to measure".to_string())
}

/// What a DR value means, on the scale people use.
pub fn verdict(dr: u32) -> &'static str {
    match dr {
        14.. => "Excellent: very dynamic, barely compressed",
        11..=13 => "Good: natural dynamics",
        8..=10 => "Average: typical for modern pop and rock",
        6..=7 => "Compressed: loud and squashed",
        _ => "Heavily compressed: a loudness-war master",
    }
}

/// The album's DR: the average of its tracks' (rounded) values.
pub fn album_dr(tracks: &[TrackDr]) -> Option<u32> {
    if tracks.is_empty() {
        return None;
    }
    Some((tracks.iter().map(|t| t.dr as f64).sum::<f64>() / tracks.len() as f64).round() as u32)
}

fn mmss(secs: f64) -> String {
    let s = secs.round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// A DR log in the layout the desktop DR meters write.
pub fn log_text(artist: &str, album: &str, rows: &[(String, TrackDr)], facts: Option<&super::audioinfo::AudioFacts>) -> String {
    let line = "-".repeat(80);
    let mut s = format!("{line}\nAnalyzed: {artist} / {album}\n{line}\n\nDR         Peak         RMS     Duration Track\n{line}\n");
    for (i, (title, t)) in rows.iter().enumerate() {
        s.push_str(&format!("DR{:<4}{:>9.2} dB{:>9.2} dB{:>9}  {:02}-{}\n", t.dr, t.peak_db, t.rms_db, mmss(t.seconds), i + 1, title));
    }
    let tracks: Vec<TrackDr> = rows.iter().map(|r| r.1.clone()).collect();
    s.push_str(&format!("{line}\n\nNumber of tracks:  {}\nOfficial DR value: DR{}\n", rows.len(), album_dr(&tracks).unwrap_or(0)));
    if let Some(f) = facts {
        s.push_str(&format!(
            "\nSamplerate:        {} Hz\nChannels:          {}\nBits per sample:   {}\nBitrate:           {} kbps\nCodec:             {}\n",
            f.sample_rate.unwrap_or(0), f.channels.unwrap_or(0), f.bit_depth.map(|b| b.to_string()).unwrap_or_else(|| "-".into()),
            f.bitrate_kbps.map(|b| format!("{b:.0}")).unwrap_or_else(|| "-".into()), f.label
        ));
    }
    s.push_str(&format!("{}\n\nMeasured by RustyDisc. Values can differ from the original DR meter by a point.\n", "=".repeat(80)));
    s
}

/// Write "DYNAMIC RANGE" and "ALBUM DYNAMIC RANGE" tags, the names the desktop DR meters use.
pub fn write_tags(path: &Path, track: u32, album: u32) -> Result<(), String> {
    let mut file = Probe::open(path).map_err(|e| e.to_string())?.read().map_err(|e| e.to_string())?;
    if file.primary_tag().is_none() {
        let tt = file.primary_tag_type();
        file.insert_tag(Tag::new(tt));
    }
    let tag = file.primary_tag_mut().ok_or("this format can't hold tags")?;
    crate::rip::tagging::custom(tag, ("DYNAMIC RANGE", "DYNAMIC RANGE"), &Some(track.to_string()));
    crate::rip::tagging::custom(tag, ("ALBUM DYNAMIC RANGE", "ALBUM DYNAMIC RANGE"), &Some(album.to_string()));
    file.save_to_path(path, WriteOptions::default()).map_err(|e| e.to_string())
}

/// The Dynamic Range Database's list of this artist's albums (they offer no API, so this is a link to open).
pub fn database_url(artist: &str) -> String {
    let enc: String = artist.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect();
    format!("https://dr.loudness-war.info/album/list/1/dr/asc?artist={enc}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(amp: f32, secs: usize, rate: usize, channels: usize) -> Vec<f32> {
        (0..secs * rate).flat_map(|i| { let v = amp * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / rate as f32).sin(); std::iter::repeat_n(v, channels) }).collect()
    }

    fn dr_of(samples: &[f32], channels: usize) -> TrackDr {
        let mut m = Meter::new(channels, 44_100);
        m.push(samples);
        m.finish().unwrap()
    }

    #[test]
    fn a_steady_sine_has_no_dynamic_range_whatever_its_level() {
        for amp in [1.0f32, 0.5, 0.1] {
            let t = dr_of(&sine(amp, 30, 44_100, 2), 2);
            assert!(t.dr_exact < 0.1, "amp {amp}: {}", t.dr_exact);
            assert_eq!(t.dr, 0);
        }
        // and a full-scale sine reads 0 dBFS for both its peak and its (scaled) RMS
        let t = dr_of(&sine(1.0, 30, 44_100, 1), 1);
        assert!(t.peak_db.abs() < 0.01 && t.rms_db.abs() < 0.05, "{t:?}");
    }

    #[test]
    fn peaks_above_the_body_of_the_music_make_dynamic_range() {
        // a quiet sine (0.1) with two short full-scale spikes: 20 dB of range
        let mut s = sine(0.1, 30, 44_100, 1);
        s[1000] = 1.0;
        s[1_200_000] = -1.0;
        let t = dr_of(&s, 1);
        assert!((t.dr_exact - 20.0).abs() < 0.5, "{t:?}");
        assert_eq!(t.dr, 20);
    }

    #[test]
    fn only_the_loudest_fifth_of_the_blocks_sets_the_loudness() {
        // 10 blocks of 3 s: two loud ones (0.5), eight quiet (0.05); peak 0.5 → measured against the loud blocks
        let mut s = Vec::new();
        for b in 0..10 {
            s.extend(sine(if b < 2 { 0.5 } else { 0.05 }, 3, 44_100, 1));
        }
        let t = dr_of(&s, 1);
        assert!(t.dr_exact < 0.2, "{t:?}");
    }

    #[test]
    fn channels_are_averaged_and_silence_is_refused() {
        assert!(Meter::new(2, 44_100).finish().is_none());
        let mut m = Meter::new(2, 44_100);
        m.push(&vec![0.0f32; 44_100 * 2 * 4]);
        assert!(m.finish().is_none(), "digital silence can't be measured");
    }

    #[test]
    fn verdicts_logs_and_links() {
        assert!(verdict(15).starts_with("Excellent") && verdict(9).starts_with("Average") && verdict(4).starts_with("Heavily"));
        let t = TrackDr { dr: 9, dr_exact: 9.2, peak_db: -0.3, rms_db: -9.5, seconds: 254.0 };
        assert_eq!(album_dr(&[t.clone(), TrackDr { dr: 10, ..t.clone() }]), Some(10));
        let log = log_text("Radiohead", "OK Computer", &[("Airbag".into(), t)], None);
        assert!(log.contains("Official DR value: DR9") && log.contains("01-Airbag") && log.contains("4:14"));
        assert_eq!(database_url("Guns N' Roses"), "https://dr.loudness-war.info/album/list/1/dr/asc?artist=Guns%20N%27%20Roses");
    }

    #[test]
    fn a_real_file_is_measured_through_ffmpeg() {
        if Command::new("ffmpeg").arg("-version").output().is_err() {
            return;
        }
        let d = std::env::temp_dir().join(format!("rd_dr_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("a.flac");
        assert!(Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=10", "-ac", "2", "-c:a", "flac"]).arg(&p).status().unwrap().success());
        let t = measure(&p, 2, 44_100).unwrap();
        assert!(t.dr_exact < 0.3 && (t.seconds - 10.0).abs() < 0.1, "{t:?}");
        std::fs::remove_dir_all(&d).ok();
    }
}
