use std::path::{Path, PathBuf};
use crate::error::Error;

pub struct PlaylistEntry {
    pub path: String,
    /// Track duration in whole seconds from #EXTINF, if present.
    pub duration_secs: Option<u64>,
    /// From #EXTINF: "Artist - Title" display string, if present.
    pub display: Option<String>,
}

/// An entry that could not be used, and why.
#[derive(Debug, Clone)]
pub struct Skipped {
    pub entry: String,
    pub reason: String,
}

pub struct Parsed {
    pub entries: Vec<PlaylistEntry>,
    pub skipped: Vec<Skipped>,
}

/// Parse an M3U or M3U8 playlist and return resolved absolute track paths.
/// Entries that can't be used are reported on stderr and skipped.
pub fn parse(playlist_path: &str) -> Result<Vec<PlaylistEntry>, Error> {
    parse_within(playlist_path, None)
}

/// Like [`parse`], and when `root` is given, entries that resolve outside it are skipped
/// (the web UI uses this so a playlist can only pull in files from the media folder).
pub fn parse_within(playlist_path: &str, root: Option<&Path>) -> Result<Vec<PlaylistEntry>, Error> {
    let parsed = parse_detailed(playlist_path, root)?;
    for s in &parsed.skipped {
        eprintln!("Warning: playlist entry skipped ({}): {}", s.reason, s.entry);
    }
    if parsed.entries.is_empty() {
        return Err(Error::validation(format!(
            "Playlist '{}' contains no resolvable track paths",
            playlist_path
        )));
    }
    Ok(parsed.entries)
}

/// Decode playlist bytes. `.m3u8` is UTF-8 by definition, but plain `.m3u` files are
/// often Windows-1252, so anything that isn't valid UTF-8 is read as Latin-1.
fn decode(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    }
}

/// Parse a playlist, keeping track of everything that was skipped.
pub fn parse_detailed(playlist_path: &str, root: Option<&Path>) -> Result<Parsed, Error> {
    let content = decode(&std::fs::read(playlist_path)?);
    let playlist_dir = Path::new(playlist_path).parent().unwrap_or(Path::new("."));
    let root = root.and_then(|r| r.canonicalize().ok());

    let mut entries = Vec::new();
    let mut skipped = Vec::new();
    let mut pending_display: Option<String> = None;
    let mut pending_duration: Option<u64> = None;

    for line in content.lines() {
        let line = line.trim().trim_start_matches('\u{feff}');

        if line.is_empty() || line.starts_with("#EXTM3U") {
            continue;
        }

        if let Some(rest) = line.strip_prefix("#EXTINF:") {
            // Format: #EXTINF:<duration>,<display name>
            let mut parts = rest.splitn(2, ',');
            pending_duration = parts
                .next()
                .and_then(|d| d.trim().split_whitespace().next().and_then(|d| d.parse::<f64>().ok()))
                .filter(|d| *d >= 0.0)
                .map(|f| f as u64);
            pending_display = parts.next().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            continue;
        }

        if line.starts_with('#') {
            continue;
        }

        // Skip stream URLs
        let lower = line.to_ascii_lowercase();
        if ["http://", "https://", "ftp://", "rtsp://", "mms://"].iter().any(|p| lower.starts_with(p)) {
            skipped.push(Skipped { entry: line.to_string(), reason: "streams are not supported".into() });
            pending_display = None;
            pending_duration = None;
            continue;
        }

        match resolve_entry(playlist_dir, line) {
            Some(canonical) => {
                if let Some(root) = &root {
                    if !canonical.starts_with(root) {
                        skipped.push(Skipped { entry: line.to_string(), reason: "outside the allowed folder".into() });
                        pending_display = None;
                        pending_duration = None;
                        continue;
                    }
                }
                if canonical.is_dir() {
                    skipped.push(Skipped { entry: line.to_string(), reason: "is a folder, not a file".into() });
                    pending_display = None;
                    pending_duration = None;
                    continue;
                }
                entries.push(PlaylistEntry {
                    path: canonical.to_string_lossy().to_string(),
                    duration_secs: pending_duration.take(),
                    display: pending_display.take(),
                });
            }
            None => {
                skipped.push(Skipped { entry: line.to_string(), reason: "file not found".into() });
                pending_display = None;
                pending_duration = None;
            }
        }
    }

    Ok(Parsed { entries, skipped })
}

/// Undo `%20`-style escapes; leaves anything malformed untouched.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = ((b[i + 1] as char).to_digit(16), (b[i + 2] as char).to_digit(16)) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// Ways an entry might have been written, most literal first: as-is, with `file://` and
/// `%xx` escapes undone, and with Windows separators and drive letters fixed.
fn variants(entry: &str) -> Vec<String> {
    let mut base = entry.trim().trim_matches('"').to_string();
    if let Some(rest) = base.strip_prefix("file://") {
        base = rest.strip_prefix("localhost").unwrap_or(rest).to_string();
    }
    let mut v = vec![base.clone()];
    let decoded = percent_decode(&base);
    if decoded != base {
        v.push(decoded);
    }
    for cand in v.clone() {
        let mut c = cand.replace('\\', "/");
        // "C:/Music/x.mp3" → "/Music/x.mp3"
        let b = c.as_bytes();
        if b.len() > 2 && b[1] == b':' && b[0].is_ascii_alphabetic() && b[2] == b'/' {
            c = c[2..].to_string();
        }
        if !v.contains(&c) {
            v.push(c);
        }
    }
    v
}

/// Find the file a playlist line refers to.
///
/// Relative paths are resolved against the playlist's own folder. If the file isn't where
/// the line says (a playlist written on another machine, say `C:\Music\Album\01.flac`),
/// progressively shorter tails of the path are tried against the playlist's folder, so
/// `Album/01.flac` next to the playlist is still found.
fn resolve_entry(playlist_dir: &Path, entry: &str) -> Option<PathBuf> {
    let variants = variants(entry);
    for v in &variants {
        let p = Path::new(v);
        let candidate = if p.is_absolute() { p.to_path_buf() } else { playlist_dir.join(p) };
        if let Ok(c) = candidate.canonicalize() {
            return Some(c);
        }
    }
    for v in &variants {
        let comps: Vec<&str> = v.split('/').filter(|c| !c.is_empty() && *c != "." && *c != "..").collect();
        for skip in 1..comps.len() {
            let candidate = playlist_dir.join(comps[skip..].join("/"));
            if let Ok(c) = candidate.canonicalize() {
                return Some(c);
            }
        }
    }
    None
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

    #[test]
    fn m3u_and_m3u8_are_read_the_same_way() {
        let dir = std::env::temp_dir().join("discctl_pl_test_ext");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.flac"), b"x").unwrap();
        for name in ["list.m3u", "list.m3u8", "LIST.M3U8"] {
            let pl = write_playlist(&dir, name, "a.flac\n");
            assert_eq!(parse(&pl).unwrap().len(), 1, "{name}");
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn handles_bom_crlf_and_latin1() {
        let dir = std::env::temp_dir().join("discctl_pl_test_enc");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("caf\u{e9}.flac"), b"x").unwrap();
        // UTF-8 with BOM and CRLF line endings
        let mut utf8 = vec![0xEF, 0xBB, 0xBF];
        utf8.extend("#EXTM3U\r\n#EXTINF:10,Caf\u{e9}\r\ncaf\u{e9}.flac\r\n".as_bytes());
        fs::write(dir.join("utf8.m3u8"), &utf8).unwrap();
        let e = parse(dir.join("utf8.m3u8").to_str().unwrap()).unwrap();
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].display.as_deref(), Some("Caf\u{e9}"));
        // Latin-1 / Windows-1252 .m3u (é is the single byte 0xE9)
        fs::write(dir.join("latin.m3u"), b"caf\xe9.flac\n").unwrap();
        assert_eq!(parse(dir.join("latin.m3u").to_str().unwrap()).unwrap().len(), 1);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn understands_windows_paths_file_uris_and_percent_escapes() {
        let dir = std::env::temp_dir().join("discctl_pl_test_forms");
        fs::create_dir_all(dir.join("Album One")).unwrap();
        fs::write(dir.join("Album One").join("01 Track.flac"), b"x").unwrap();
        let abs = dir.join("Album One").join("01 Track.flac");
        let pl = write_playlist(&dir, "forms.m3u8", &format!(
            "Album One\\01 Track.flac\nAlbum%20One/01%20Track.flac\nfile://{}\nfile://{}\n\"Album One/01 Track.flac\"\n",
            abs.display(),
            abs.display().to_string().replace(' ', "%20"),
        ));
        let parsed = parse_detailed(&pl, None).unwrap();
        assert_eq!(parsed.entries.len(), 5, "skipped: {:?}", parsed.skipped);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn finds_files_from_a_playlist_written_elsewhere() {
        let dir = std::env::temp_dir().join("discctl_pl_test_moved");
        fs::create_dir_all(dir.join("Album")).unwrap();
        fs::write(dir.join("Album").join("01.flac"), b"x").unwrap();
        let pl = write_playlist(&dir, "moved.m3u8", "C:\\Users\\me\\Music\\Album\\01.flac\n/home/someone/Music/Album/01.flac\n");
        let parsed = parse_detailed(&pl, None).unwrap();
        assert_eq!(parsed.entries.len(), 2, "{:?}", parsed.skipped);
        assert!(parsed.entries.iter().all(|e| e.path.ends_with("Album/01.flac")));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn reports_skipped_entries_and_honours_the_root() {
        let base = std::env::temp_dir().join("discctl_pl_test_root");
        fs::create_dir_all(base.join("inside")).unwrap();
        fs::create_dir_all(base.join("outside")).unwrap();
        fs::write(base.join("inside").join("a.flac"), b"x").unwrap();
        fs::write(base.join("outside").join("b.flac"), b"x").unwrap();
        let pl = write_playlist(&base.join("inside"), "p.m3u8", "a.flac\n../outside/b.flac\nmissing.flac\nhttp://x/y\n");
        let all = parse_detailed(&pl, None).unwrap();
        assert_eq!(all.entries.len(), 2);
        assert_eq!(all.skipped.len(), 2);
        let restricted = parse_detailed(&pl, Some(&base.join("inside"))).unwrap();
        assert_eq!(restricted.entries.len(), 1);
        assert!(restricted.skipped.iter().any(|s| s.reason.contains("outside")));
        assert!(restricted.skipped.iter().any(|s| s.reason.contains("not found")));
        assert!(restricted.skipped.iter().any(|s| s.reason.contains("streams")));
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn percent_decoding_is_forgiving() {
        assert_eq!(percent_decode("a%20b%2Fc"), "a b/c");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz%4"), "%zz%4");
    }
}
