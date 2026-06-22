use std::process::Command;
use crate::error::Error;

/// Usable data capacity per disc (700MB minus ISO9660 overhead allowance)
pub const DATA_DISC_CAPACITY_BYTES: u64 = 690 * 1024 * 1024;

/// Conservative audio capacity: 74-minute CD minus 30-second safety margin
pub const AUDIO_DISC_CAPACITY_SECS: u64 = 74 * 60 - 30;

/// Red Book maximum tracks per disc
pub const AUDIO_MAX_TRACKS: usize = 99;

// ── Data disc items ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DataItem {
    pub path: String,
    pub size_bytes: u64,
}

#[derive(Debug)]
pub struct DataSlice {
    pub items: Vec<DataItem>,
    pub total_bytes: u64,
}

/// Greedy bin-pack: fill each disc to `capacity_bytes`, then start a new one.
pub fn split_data(items: Vec<DataItem>, capacity_bytes: u64) -> Vec<DataSlice> {
    let mut slices: Vec<DataSlice> = Vec::new();
    let mut current: Vec<DataItem> = Vec::new();
    let mut current_bytes: u64 = 0;

    for item in items {
        if item.size_bytes > capacity_bytes {
            eprintln!(
                "Warning: '{}' ({:.1}MB) exceeds single-disc capacity ({:.0}MB) — skipping.",
                item.path,
                item.size_bytes as f64 / 1_048_576.0,
                capacity_bytes as f64 / 1_048_576.0,
            );
            continue;
        }
        if current_bytes + item.size_bytes > capacity_bytes && !current.is_empty() {
            slices.push(DataSlice {
                total_bytes: current_bytes,
                items: std::mem::take(&mut current),
            });
            current_bytes = 0;
        }
        current_bytes += item.size_bytes;
        current.push(item);
    }
    if !current.is_empty() {
        slices.push(DataSlice { total_bytes: current_bytes, items: current });
    }
    slices
}

// ── Audio disc items ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct AudioItem {
    pub path: String,
    pub duration_secs: u64,
}

#[derive(Debug)]
pub struct AudioSlice {
    pub items: Vec<AudioItem>,
    pub total_secs: u64,
}

/// Greedy split by duration and track count.
pub fn split_audio(
    items: Vec<AudioItem>,
    max_secs: u64,
    max_tracks: usize,
) -> Vec<AudioSlice> {
    let mut slices: Vec<AudioSlice> = Vec::new();
    let mut current: Vec<AudioItem> = Vec::new();
    let mut current_secs: u64 = 0;

    for item in items {
        let time_full = current_secs + item.duration_secs > max_secs;
        let count_full = current.len() >= max_tracks;
        if (time_full || count_full) && !current.is_empty() {
            slices.push(AudioSlice {
                total_secs: current_secs,
                items: std::mem::take(&mut current),
            });
            current_secs = 0;
        }
        current_secs += item.duration_secs;
        current.push(item);
    }
    if !current.is_empty() {
        slices.push(AudioSlice { total_secs: current_secs, items: current });
    }
    slices
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Get file sizes for a list of paths.
pub fn measure_paths(paths: &[String]) -> Vec<DataItem> {
    paths
        .iter()
        .map(|p| DataItem {
            size_bytes: std::fs::metadata(p).map(|m| m.len()).unwrap_or(0),
            path: p.clone(),
        })
        .collect()
}

/// Recursively enumerate all files under `dir` with their sizes.
pub fn enumerate_dir(dir: &str) -> Result<Vec<DataItem>, Error> {
    let mut items = Vec::new();
    walk(std::path::Path::new(dir), &mut items)?;
    items.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(items)
}

fn walk(dir: &std::path::Path, out: &mut Vec<DataItem>) -> Result<(), Error> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out)?;
        } else {
            out.push(DataItem {
                size_bytes: entry.metadata()?.len(),
                path: path.to_string_lossy().to_string(),
            });
        }
    }
    Ok(())
}

/// Estimate duration of an audio file in seconds.
/// Uses WAV header arithmetic for WAV files; falls back to ffprobe for others.
/// Returns 0 if duration cannot be determined.
pub fn duration_secs(path: &str) -> u64 {
    if path.to_lowercase().ends_with(".wav") {
        // CDDA WAV: 44100 Hz × 2 ch × 2 bytes = 176400 bytes/sec
        if let Ok(meta) = std::fs::metadata(path) {
            return meta.len().saturating_sub(44) / 176_400;
        }
    }

    // ffprobe for all other formats
    if let Ok(out) = Command::new("ffprobe")
        .args([
            "-v", "quiet",
            "-show_entries", "format=duration",
            "-of", "default=noprint_wrappers=1:nokey=1",
            path,
        ])
        .output()
    {
        if out.status.success() {
            if let Ok(s) = std::str::from_utf8(&out.stdout) {
                if let Ok(f) = s.trim().parse::<f64>() {
                    return f.ceil() as u64;
                }
            }
        }
    }

    // Unknown — assume 5 minutes as a safe fallback
    300
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn items(sizes: &[u64]) -> Vec<DataItem> {
        sizes
            .iter()
            .enumerate()
            .map(|(i, &s)| DataItem { path: format!("track{:02}.flac", i), size_bytes: s })
            .collect()
    }

    fn tracks(durations: &[u64]) -> Vec<AudioItem> {
        durations
            .iter()
            .enumerate()
            .map(|(i, &d)| AudioItem { path: format!("t{:02}.flac", i), duration_secs: d })
            .collect()
    }

    const MB: u64 = 1_048_576;
    const CAP: u64 = 700 * MB;

    #[test]
    fn single_disc_when_fits() {
        let slices = split_data(items(&[100 * MB, 200 * MB, 300 * MB]), CAP);
        assert_eq!(slices.len(), 1);
        assert_eq!(slices[0].items.len(), 3);
    }

    #[test]
    fn splits_into_two_discs() {
        // 400MB + 400MB = needs 2 discs at 700MB capacity
        let slices = split_data(items(&[400 * MB, 400 * MB]), CAP);
        assert_eq!(slices.len(), 2);
    }

    #[test]
    fn three_disc_split() {
        let slices = split_data(items(&[600 * MB, 600 * MB, 600 * MB]), CAP);
        assert_eq!(slices.len(), 3);
    }

    #[test]
    fn packs_tightly() {
        // 3 × 300MB fits in 2 discs of 700MB: [300+300, 300]
        let slices = split_data(items(&[300 * MB, 300 * MB, 300 * MB]), CAP);
        assert_eq!(slices.len(), 2);
        assert_eq!(slices[0].items.len(), 2);
        assert_eq!(slices[1].items.len(), 1);
    }

    #[test]
    fn skips_oversized_file() {
        let slices = split_data(items(&[800 * MB, 300 * MB]), CAP);
        assert_eq!(slices.len(), 1);
        assert_eq!(slices[0].items.len(), 1);
        assert_eq!(slices[0].items[0].path, "track01.flac"); // only the 300MB one
    }

    #[test]
    fn audio_splits_by_duration() {
        // 2 × 45 min = needs 2 discs at 74-min capacity
        let slices = split_audio(tracks(&[45 * 60, 45 * 60]), 74 * 60, 99);
        assert_eq!(slices.len(), 2);
    }

    #[test]
    fn audio_splits_by_track_count() {
        // 100 tracks of 1 second each — exceeds 99-track limit
        let d: Vec<u64> = vec![1; 100];
        let slices = split_audio(tracks(&d), 74 * 60, 99);
        assert_eq!(slices.len(), 2);
        assert_eq!(slices[0].items.len(), 99);
        assert_eq!(slices[1].items.len(), 1);
    }

    #[test]
    fn audio_single_disc_when_fits() {
        let slices = split_audio(tracks(&[240, 300, 200]), 74 * 60, 99);
        assert_eq!(slices.len(), 1);
    }
}
