//! Read offset correction.
//!
//! Every drive reads the disc a fixed number of samples early or late. AccurateRip reveals it
//! as the shift at which a rip matches (`+6` means the real track starts 6 samples into what the
//! drive returned). Correcting it shifts the whole disc's audio by that amount and re-cuts it at
//! the track boundaries: each track loses its first `N` samples and gains the next track's first
//! `N` (or the reverse for a negative offset). Only the very start or end of the disc, where
//! there is nothing to borrow from, is padded with silence.

use std::{
    fs::File,
    io::{BufWriter, Write},
    path::Path,
};

use crate::{error::Error, rip::accuraterip::open_wav};

/// How the drive's read offset is handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OffsetMode {
    /// Keep the audio exactly as the drive returned it.
    #[default]
    Off,
    /// Correct by the shift AccurateRip proves, then check the result.
    Auto,
    /// Always correct by this many samples.
    Fixed(i32),
}

impl OffsetMode {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_lowercase().as_str() {
            "" | "off" | "none" | "0" => Ok(OffsetMode::Off),
            "auto" => Ok(OffsetMode::Auto),
            n => n
                .trim_start_matches('+')
                .parse::<i32>()
                .ok()
                .filter(|v| v.abs() <= MAX_OFFSET)
                .map(OffsetMode::Fixed)
                .ok_or_else(|| format!("'{s}' is not off, auto, or a number of samples (up to ±{MAX_OFFSET})")),
        }
    }
}

/// The largest offset accepted (the same range AccurateRip searches).
pub const MAX_OFFSET: i32 = 2939;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Outcome {
    pub offset: i32,
    /// Samples at the very start or end of the disc that had to be silence.
    pub padded_samples: usize,
}

const CHUNK: usize = 1 << 18;

fn write_header(w: &mut impl Write, words: usize) -> std::io::Result<()> {
    let data = (words * 4) as u32;
    w.write_all(b"RIFF")?;
    w.write_all(&(36 + data).to_le_bytes())?;
    w.write_all(b"WAVEfmt ")?;
    w.write_all(&16u32.to_le_bytes())?;
    w.write_all(&1u16.to_le_bytes())?; // PCM
    w.write_all(&2u16.to_le_bytes())?; // stereo
    w.write_all(&44_100u32.to_le_bytes())?;
    w.write_all(&(44_100u32 * 4).to_le_bytes())?;
    w.write_all(&4u16.to_le_bytes())?;
    w.write_all(&16u16.to_le_bytes())?;
    w.write_all(b"data")?;
    w.write_all(&data.to_le_bytes())
}

fn put(w: &mut impl Write, words: &[u32]) -> std::io::Result<()> {
    let mut buf = Vec::with_capacity(words.len() * 4);
    for x in words {
        buf.extend_from_slice(&x.to_le_bytes());
    }
    w.write_all(&buf)
}

fn zeros(w: &mut impl Write, n: usize) -> std::io::Result<()> {
    put(w, &vec![0u32; n])
}

fn orig_path(p: &str) -> String {
    format!("{p}.orig")
}

/// Shift the audio by `offset` samples. `contiguous[i]` says track `i + 1` of `wavs` follows track
/// `i` directly on the disc (so audio can be borrowed across the boundary). The original files
/// are kept as `<name>.orig` until `discard_originals` or `restore` is called.
pub fn apply(wavs: &[(usize, String)], contiguous: &[bool], offset: i32) -> Result<Outcome, Error> {
    if offset == 0 || wavs.is_empty() {
        return Ok(Outcome { offset: 0, padded_samples: 0 });
    }
    let n = offset.unsigned_abs() as usize;
    let sources: Vec<_> = wavs.iter().map(|(_, p)| open_wav(p)).collect::<Result<_, _>>()?;
    let joined = |i: usize| contiguous.get(i).copied().unwrap_or(false);
    let mut padded = 0usize;

    // Write every corrected track under a temporary name first; neighbours are still needed.
    for (k, src) in sources.iter().enumerate() {
        let tmp = format!("{}.new", wavs[k].1);
        let mut w = BufWriter::new(File::create(&tmp)?);
        write_header(&mut w, src.words)?;
        let own = src.words;
        if offset > 0 {
            // Drop the first n samples, then take the next track's first n.
            let keep = own.saturating_sub(n);
            let mut at = n.min(own);
            while at < own {
                let c = src.read(at, CHUNK.min(own - at))?;
                put(&mut w, &c)?;
                at += c.len();
            }
            let need = own - keep;
            let borrowed = if k + 1 < sources.len() && joined(k) { sources[k + 1].read(0, need)? } else { Vec::new() };
            put(&mut w, &borrowed)?;
            zeros(&mut w, need - borrowed.len())?;
            padded += need - borrowed.len();
        } else {
            // Take the previous track's last n samples, then this track without its last n.
            let head = n.min(own);
            let borrowed = if k > 0 && joined(k - 1) {
                let prev = &sources[k - 1];
                prev.read(prev.words.saturating_sub(head), head)?
            } else {
                Vec::new()
            };
            zeros(&mut w, head - borrowed.len())?;
            padded += head - borrowed.len();
            put(&mut w, &borrowed)?;
            let mut at = 0;
            let end = own - head;
            while at < end {
                let c = src.read(at, CHUNK.min(end - at))?;
                put(&mut w, &c)?;
                at += c.len();
            }
        }
        w.flush()?;
    }

    // Everything was written; now swap the corrected files in, keeping the originals.
    for (_, p) in wavs {
        std::fs::rename(p, orig_path(p))?;
        std::fs::rename(format!("{p}.new"), p)?;
    }
    Ok(Outcome { offset, padded_samples: padded })
}

/// Put the original files back (after a correction that didn't help).
pub fn restore(wavs: &[(usize, String)]) {
    for (_, p) in wavs {
        if Path::new(&orig_path(p)).exists() {
            let _ = std::fs::rename(orig_path(p), p);
        }
    }
}

/// Delete the kept originals once the correction is accepted.
pub fn discard_originals(wavs: &[(usize, String)]) {
    for (_, p) in wavs {
        let _ = std::fs::remove_file(orig_path(p));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_wav(path: &Path, words: &[u32]) {
        let mut w = BufWriter::new(File::create(path).unwrap());
        write_header(&mut w, words.len()).unwrap();
        put(&mut w, words).unwrap();
        w.flush().unwrap();
    }

    fn read_all(path: &str) -> Vec<u32> {
        let w = open_wav(path).unwrap();
        w.read(0, w.words).unwrap()
    }

    fn tmpdir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("rustydisc_offset_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A disc of three tracks cut from one continuous stream.
    fn disc(dir: &Path) -> (Vec<u32>, Vec<(usize, String)>) {
        let stream: Vec<u32> = (1..=300u32).collect();
        let cuts = [(0usize, 100usize), (100, 200), (200, 300)];
        let mut wavs = Vec::new();
        for (i, (a, b)) in cuts.iter().enumerate() {
            let p = dir.join(format!("track{:02}.cdda.wav", i + 1));
            write_wav(&p, &stream[*a..*b]);
            wavs.push((i + 1, p.to_string_lossy().to_string()));
        }
        (stream, wavs)
    }

    #[test]
    fn parses_modes() {
        assert_eq!(OffsetMode::parse("off"), Ok(OffsetMode::Off));
        assert_eq!(OffsetMode::parse("auto"), Ok(OffsetMode::Auto));
        assert_eq!(OffsetMode::parse("+6"), Ok(OffsetMode::Fixed(6)));
        assert_eq!(OffsetMode::parse("-30"), Ok(OffsetMode::Fixed(-30)));
        assert_eq!(OffsetMode::parse("0"), Ok(OffsetMode::Off));
        assert!(OffsetMode::parse("5000").is_err());
        assert!(OffsetMode::parse("abc").is_err());
    }

    #[test]
    fn positive_offset_borrows_from_the_next_track() {
        let dir = tmpdir("pos");
        let (stream, wavs) = disc(&dir);
        let out = apply(&wavs, &[true, true], 6).unwrap();
        assert_eq!(out.padded_samples, 6, "only the end of the disc is silence");
        // Track 1 is stream[6..106], track 2 is stream[106..206], track 3 ends in silence.
        assert_eq!(read_all(&wavs[0].1), stream[6..106]);
        assert_eq!(read_all(&wavs[1].1), stream[106..206]);
        let t3 = read_all(&wavs[2].1);
        assert_eq!(&t3[..94], &stream[206..300]);
        assert_eq!(&t3[94..], &[0u32; 6]);
        discard_originals(&wavs);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn negative_offset_borrows_from_the_previous_track() {
        let dir = tmpdir("neg");
        let (stream, wavs) = disc(&dir);
        let out = apply(&wavs, &[true, true], -4).unwrap();
        assert_eq!(out.padded_samples, 4, "only the start of the disc is silence");
        let t1 = read_all(&wavs[0].1);
        assert_eq!(&t1[..4], &[0u32; 4]);
        assert_eq!(&t1[4..], &stream[0..96]);
        assert_eq!(read_all(&wavs[1].1), stream[96..196]);
        assert_eq!(read_all(&wavs[2].1), stream[196..296]);
        discard_originals(&wavs);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_gap_between_tracks_is_padded_not_borrowed() {
        let dir = tmpdir("gap");
        let (stream, wavs) = disc(&dir);
        let out = apply(&wavs, &[true, false], 5).unwrap();
        assert_eq!(out.padded_samples, 10, "track 2 and track 3 both end in silence");
        assert_eq!(read_all(&wavs[0].1), stream[5..105]);
        assert_eq!(&read_all(&wavs[1].1)[95..], &[0u32; 5]);
        discard_originals(&wavs);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_brings_the_originals_back_and_zero_is_a_no_op() {
        let dir = tmpdir("restore");
        let (stream, wavs) = disc(&dir);
        assert_eq!(apply(&wavs, &[true, true], 0).unwrap().padded_samples, 0);
        apply(&wavs, &[true, true], 9).unwrap();
        restore(&wavs);
        assert_eq!(read_all(&wavs[1].1), stream[100..200]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
