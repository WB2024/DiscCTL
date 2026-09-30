use std::process::Command;
use crate::error::Error;

/// Red Book maximum tracks per disc
pub const AUDIO_MAX_TRACKS: usize = 99;

// ── Data disc items ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DataItem {
    /// Where the file is now.
    pub path: String,
    /// Where it goes on the disc, relative to the disc root (e.g. `Music/track.flac`).
    pub rel_path: String,
    pub size_bytes: u64,
}

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

/// Items for individually chosen files: they all go in the disc root, so two files with the
/// same name are told apart (`song.flac`, `song (2).flac`) rather than one overwriting the other.
pub fn flat_items(paths: &[String]) -> Vec<DataItem> {
    let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
    paths
        .iter()
        .map(|p| {
            let name = std::path::Path::new(p)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "file".to_string());
            let (stem, ext) = match name.rsplit_once('.') {
                Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
                _ => (name.clone(), String::new()),
            };
            let mut candidate = name.clone();
            let mut n = 2;
            while !used.insert(candidate.to_lowercase()) {
                candidate = format!("{stem} ({n}){ext}");
                n += 1;
            }
            DataItem {
                size_bytes: std::fs::metadata(p).map(|m| m.len()).unwrap_or(0),
                path: p.clone(),
                rel_path: candidate,
            }
        })
        .collect()
}

/// Recursively enumerate all files under `dir` with their sizes.
pub fn enumerate_dir(dir: &str) -> Result<Vec<DataItem>, Error> {
    let root = std::path::Path::new(dir);
    let mut items = Vec::new();
    walk(root, root, &mut items)?;
    items.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(items)
}

fn walk(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<DataItem>) -> Result<(), Error> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk(root, &path, out)?;
        } else {
            out.push(DataItem {
                // metadata() follows symlinks, so a link to a file reports the file's size
                size_bytes: std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
                rel_path: path.strip_prefix(root).unwrap_or(&path).to_string_lossy().to_string(),
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
            .map(|(i, &s)| DataItem { path: format!("track{:02}.flac", i), rel_path: format!("track{:02}.flac", i), size_bytes: s })
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

    #[test]
    fn flat_items_never_collide() {
        let paths: Vec<String> = ["/a/song.flac", "/b/song.flac", "/c/SONG.FLAC", "/d/readme", "/e/readme", "/f/.hidden"]
            .iter().map(|s| s.to_string()).collect();
        let names: Vec<String> = flat_items(&paths).into_iter().map(|i| i.rel_path).collect();
        assert_eq!(names, ["song.flac", "song (2).flac", "SONG (3).FLAC", "readme", "readme (2)", ".hidden"]);
    }

    #[test]
    fn enumerate_dir_keeps_the_folder_structure() {
        let root = std::env::temp_dir().join(format!("rd_enum_{}", std::process::id()));
        std::fs::create_dir_all(root.join("sub/deeper")).unwrap();
        std::fs::write(root.join("top.txt"), b"1").unwrap();
        std::fs::write(root.join("sub/a.txt"), b"22").unwrap();
        std::fs::write(root.join("sub/deeper/a.txt"), b"333").unwrap(); // same name, different folder
        let items = enumerate_dir(root.to_str().unwrap()).unwrap();
        let mut rels: Vec<&str> = items.iter().map(|i| i.rel_path.as_str()).collect();
        rels.sort();
        assert_eq!(rels, ["sub/a.txt", "sub/deeper/a.txt", "top.txt"]);
        std::fs::remove_dir_all(&root).ok();
    }
}
