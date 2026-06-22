use std::path::{Path, PathBuf};
use crate::error::Error;

pub struct PlaylistEntry {
    pub path: String,
    /// Track duration in whole seconds from #EXTINF, if present.
    pub duration_secs: Option<u64>,
    /// From #EXTINF: "Artist - Title" display string, if present.
    pub display: Option<String>,
}

/// Parse an M3U or M3U8 playlist and return resolved absolute track paths.
/// Relative paths are resolved against the playlist file's parent directory.
pub fn parse(playlist_path: &str) -> Result<Vec<PlaylistEntry>, Error> {
    let content = std::fs::read_to_string(playlist_path)?;
    let playlist_dir = Path::new(playlist_path)
        .parent()
        .unwrap_or(Path::new("."));

    let mut entries = Vec::new();
    let mut pending_display: Option<String> = None;
    let mut pending_duration: Option<u64> = None;

    for line in content.lines() {
        let line = line.trim();

        if line.is_empty() {
            continue;
        }

        if line.starts_with("#EXTM3U") {
            continue;
        }

        if let Some(rest) = line.strip_prefix("#EXTINF:") {
            // Format: #EXTINF:<duration>,<display name>
            let mut parts = rest.splitn(2, ',');
            pending_duration = parts
                .next()
                .and_then(|d| d.trim().parse::<f64>().ok())
                .map(|f| f.abs() as u64);
            pending_display = parts.next().map(|s| s.trim().to_string());
            continue;
        }

        if line.starts_with('#') {
            continue;
        }

        // Skip stream URLs
        if line.starts_with("http://")
            || line.starts_with("https://")
            || line.starts_with("ftp://")
            || line.starts_with("rtsp://")
        {
            eprintln!("Warning: skipping URL (streaming not supported): {}", line);
            pending_display = None;
            pending_duration = None;
            continue;
        }

        let resolved = resolve_path(playlist_dir, line);

        match resolved.canonicalize() {
            Ok(canonical) => {
                entries.push(PlaylistEntry {
                    path: canonical.to_string_lossy().to_string(),
                    duration_secs: pending_duration.take(),
                    display: pending_display.take(),
                });
            }
            Err(_) => {
                eprintln!(
                    "Warning: playlist entry not found, skipping: {}",
                    resolved.display()
                );
                pending_display = None;
                pending_duration = None;
            }
        }
    }

    if entries.is_empty() {
        return Err(Error::validation(format!(
            "Playlist '{}' contains no resolvable track paths",
            playlist_path
        )));
    }

    Ok(entries)
}

fn resolve_path(base: &Path, entry: &str) -> PathBuf {
    let p = Path::new(entry);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_playlist(dir: &Path, name: &str, content: &str) -> String {
        let path = dir.join(name);
        fs::write(&path, content).unwrap();
        path.to_string_lossy().to_string()
    }

    #[test]
    fn parses_absolute_paths() {
        let dir = std::env::temp_dir().join("discctl_pl_test_abs");
        fs::create_dir_all(&dir).unwrap();
        let track = dir.join("track.flac");
        fs::write(&track, b"fake").unwrap();

        let pl = write_playlist(
            &dir,
            "test.m3u8",
            &format!("#EXTM3U\n#EXTINF:240,Artist - Song\n{}\n", track.display()),
        );

        let entries = parse(&pl).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].path.ends_with("track.flac"));
        assert_eq!(entries[0].display.as_deref(), Some("Artist - Song"));
        assert_eq!(entries[0].duration_secs, Some(240));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resolves_relative_paths() {
        let dir = std::env::temp_dir().join("discctl_pl_test_rel");
        fs::create_dir_all(&dir).unwrap();
        let sub = dir.join("sub");
        fs::create_dir_all(&sub).unwrap();
        let track = sub.join("track.flac");
        fs::write(&track, b"fake").unwrap();

        let pl = write_playlist(&dir, "test.m3u8", "#EXTM3U\nsub/track.flac\n");

        let entries = parse(&pl).unwrap();
        assert_eq!(entries.len(), 1);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn skips_missing_files_with_warning() {
        let dir = std::env::temp_dir().join("discctl_pl_test_skip");
        fs::create_dir_all(&dir).unwrap();
        let track = dir.join("real.flac");
        fs::write(&track, b"fake").unwrap();

        let pl = write_playlist(
            &dir,
            "test.m3u8",
            "#EXTM3U\nreal.flac\n/does/not/exist.flac\n",
        );

        let entries = parse(&pl).unwrap();
        assert_eq!(entries.len(), 1);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn errors_on_empty_playlist() {
        let dir = std::env::temp_dir().join("discctl_pl_test_empty");
        fs::create_dir_all(&dir).unwrap();
        let pl = write_playlist(&dir, "test.m3u8", "#EXTM3U\n");
        assert!(parse(&pl).is_err());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn skips_url_entries() {
        let dir = std::env::temp_dir().join("discctl_pl_test_url");
        fs::create_dir_all(&dir).unwrap();
        let track = dir.join("local.flac");
        fs::write(&track, b"fake").unwrap();

        let pl = write_playlist(
            &dir,
            "test.m3u8",
            "#EXTM3U\nhttps://stream.example.com/radio\nlocal.flac\n",
        );

        let entries = parse(&pl).unwrap();
        assert_eq!(entries.len(), 1);
        fs::remove_dir_all(&dir).ok();
    }
}
