//! Finding the music to put on a stick and reading its tags.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
};

use lofty::prelude::*;
use lofty::probe::Probe;
use lofty::tag::ItemKey;

use super::layout::TrackMeta;
use crate::{error::Error, parser};

pub const AUDIO_EXTS: &[&str] = &["flac", "mp3", "m4a", "aac", "ogg", "opus", "wma", "wav", "aiff", "aif", "ape", "wv", "alac"];

pub fn is_audio_path(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTS.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

/// Where the music comes from. Any mix of folders, files and playlists.
#[derive(Debug, Default, Clone)]
pub struct SourceSpec {
    pub folders: Vec<String>,
    pub files: Vec<String>,
    pub playlists: Vec<String>,
    /// Playlist entries must live inside this folder (the web UI sets it).
    pub playlist_root: Option<PathBuf>,
}

impl SourceSpec {
    pub fn is_empty(&self) -> bool {
        self.folders.is_empty() && self.files.is_empty() && self.playlists.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct SourceTrack {
    pub path: String,
    pub size: u64,
    pub meta: TrackMeta,
    pub duration_secs: Option<f64>,
    /// Size of the artwork embedded in the file, if any.
    pub art_bytes: u64,
    /// Cover art file next to the track (`cover.jpg`, `folder.png`, ...).
    pub cover: Option<PathBuf>,
}

#[derive(Debug, Default)]
pub struct Scan {
    pub tracks: Vec<SourceTrack>,
    /// Things that were left out, with the reason.
    pub skipped: Vec<(String, String)>,
}

fn collect_folder(dir: &Path, out: &mut Vec<PathBuf>, skipped: &mut Vec<(String, String)>, depth: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        skipped.push((dir.to_string_lossy().to_string(), "can't read this folder".into()));
        return;
    };
    for entry in rd.filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let Ok(meta) = std::fs::metadata(&path) else { continue };
        if meta.is_dir() {
            if depth < 12 {
                collect_folder(&path, out, skipped, depth + 1);
            }
        } else if is_audio_path(&path) {
            out.push(path);
        }
    }
}

/// Find every audio file the sources name, in a stable order, without duplicates.
pub fn gather_paths(spec: &SourceSpec) -> Result<(Vec<PathBuf>, Vec<(String, String)>), Error> {
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut skipped: Vec<(String, String)> = Vec::new();

    for f in &spec.folders {
        let dir = Path::new(f);
        if !dir.is_dir() {
            return Err(Error::validation(format!("Folder not found: {f}")));
        }
        let mut found = Vec::new();
        collect_folder(dir, &mut found, &mut skipped, 0);
        found.sort_by(|a, b| natural_cmp(&a.to_string_lossy(), &b.to_string_lossy()));
        paths.extend(found);
    }
    for f in &spec.files {
        let p = Path::new(f);
        if !p.is_file() {
            return Err(Error::validation(format!("File not found: {f}")));
        }
        if !is_audio_path(p) {
            skipped.push((f.clone(), "not an audio file".into()));
            continue;
        }
        paths.push(p.to_path_buf());
    }
    for pl in &spec.playlists {
        let parsed = parser::playlist::parse_detailed(pl, spec.playlist_root.as_deref())?;
        for s in parsed.skipped {
            skipped.push((s.entry, s.reason));
        }
        for e in parsed.entries {
            let p = PathBuf::from(&e.path);
            if is_audio_path(&p) {
                paths.push(p);
            } else {
                skipped.push((e.path, "not an audio file".into()));
            }
        }
    }

    // De-duplicate by real path, keeping the first.
    let mut seen: HashSet<PathBuf> = HashSet::new();
    paths.retain(|p| seen.insert(std::fs::canonicalize(p).unwrap_or_else(|_| p.clone())));
    Ok((paths, skipped))
}

/// Read the tags of every file (several at a time: the files may be on a slow network share).
pub fn scan(spec: &SourceSpec) -> Result<Scan, Error> {
    let (paths, skipped) = gather_paths(spec)?;
    if paths.is_empty() {
        return Err(Error::validation("No audio files found in the chosen sources"));
    }

    let results: Mutex<Vec<Option<SourceTrack>>> = Mutex::new(vec![None; paths.len()]);
    let next = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).clamp(2, 8);
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(p) = paths.get(i) else { break };
                let track = read_track(p);
                results.lock().unwrap()[i] = Some(track);
            });
        }
    });

    Ok(Scan { tracks: results.into_inner().unwrap().into_iter().flatten().collect(), skipped })
}

fn non_empty(s: Option<impl AsRef<str>>) -> String {
    s.map(|s| s.as_ref().trim().to_string()).unwrap_or_default()
}

pub fn read_track(path: &Path) -> SourceTrack {
    let path_str = path.to_string_lossy().to_string();
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let mut meta = TrackMeta::default();
    let mut duration = None;
    let mut art_bytes = 0u64;

    if let Ok(tagged) = Probe::open(path).and_then(|p| p.read()) {
        let d = tagged.properties().duration().as_secs_f64();
        if d > 0.0 {
            duration = Some(d);
        }
        if let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) {
            meta.title = non_empty(tag.title());
            meta.artist = non_empty(tag.artist());
            meta.album = non_empty(tag.album());
            meta.album_artist = non_empty(tag.get_string(&ItemKey::AlbumArtist));
            meta.genre = non_empty(tag.genre());
            meta.track = tag.track();
            meta.track_total = tag.track_total();
            meta.disc = tag.disk();
            meta.disc_total = tag.disk_total();
            meta.year = tag.year();
            art_bytes = tag.pictures().iter().map(|p| p.data().len() as u64).max().unwrap_or(0);
        }
    }
    fill_from_path(path, &mut meta);

    SourceTrack {
        path: path_str.clone(),
        size,
        meta,
        duration_secs: duration,
        art_bytes,
        cover: crate::backend::dvd::find_cover(&path_str),
    }
}

/// Fill in what the tags didn't say, guessing from the folders and the file name:
/// `Artist/Album/CD 2/03 - Song.flac`.
pub fn fill_from_path(path: &Path, meta: &mut TrackMeta) {
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let (num, name) = split_track_prefix(&stem);
    if meta.title.is_empty() {
        meta.title = name;
    }
    if meta.track.is_none() {
        meta.track = num;
    }

    let mut dirs = path.ancestors().skip(1).filter_map(|p| p.file_name()).map(|n| n.to_string_lossy().to_string());
    let parent = dirs.next().unwrap_or_default();
    let (album_dir, disc_from_dir) = match parse_disc_folder(&parent) {
        Some(d) => (dirs.next().unwrap_or_default(), Some(d)),
        None => (parent, None),
    };
    if meta.disc.is_none() {
        meta.disc = disc_from_dir;
    }
    if meta.album.is_empty() {
        meta.album = album_dir;
    }
    if meta.artist.is_empty() && meta.album_artist.is_empty() {
        let artist_dir = dirs.next().unwrap_or_default();
        // A top-level folder that's clearly not an artist (e.g. "Music", "Downloads") isn't used.
        if !artist_dir.is_empty() {
            meta.artist = artist_dir;
        }
    }
}

/// `"03 - Song"` → (Some(3), "Song"); `"Song"` → (None, "Song").
pub fn split_track_prefix(stem: &str) -> (Option<u32>, String) {
    const SEPS: &str = " .-_)";
    let digits: String = stem.chars().take_while(|c| c.is_ascii_digit()).collect();
    let rest = &stem[digits.len()..];
    if digits.is_empty() || digits.len() > 3 || !rest.starts_with(|c: char| SEPS.contains(c)) {
        return (None, stem.trim().to_string());
    }
    let name = rest.trim_start_matches(|c: char| SEPS.contains(c)).trim();
    if name.is_empty() {
        return (None, stem.trim().to_string());
    }
    (digits.parse().ok(), name.to_string())
}

/// `"CD 2"`, `"Disc2"`, `"disk 3"` → Some(2/2/3).
pub fn parse_disc_folder(name: &str) -> Option<u32> {
    let lower = name.trim().to_lowercase();
    for prefix in ["disc", "disk", "cd"] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            let n = rest.trim().trim_start_matches(['_', '-', '.']).trim();
            if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) {
                return n.parse().ok();
            }
        }
    }
    None
}

/// Compare names the way people order tracks: "2 - x" before "10 - x", ignoring case.
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    fn chunks(s: &str) -> Vec<(bool, String)> {
        let mut out: Vec<(bool, String)> = Vec::new();
        for c in s.to_lowercase().chars() {
            let digit = c.is_ascii_digit();
            match out.last_mut() {
                Some((d, buf)) if *d == digit => buf.push(c),
                _ => out.push((digit, c.to_string())),
            }
        }
        out
    }
    let (ca, cb) = (chunks(a), chunks(b));
    for (x, y) in ca.iter().zip(cb.iter()) {
        let ord = if x.0 && y.0 {
            let (nx, ny) = (x.1.trim_start_matches('0'), y.1.trim_start_matches('0'));
            nx.len().cmp(&ny.len()).then_with(|| nx.cmp(ny))
        } else {
            x.1.cmp(&y.1)
        };
        if ord != std::cmp::Ordering::Equal {
            return ord;
        }
    }
    ca.len().cmp(&cb.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_track_numbers_from_file_names() {
        assert_eq!(split_track_prefix("03 - Song"), (Some(3), "Song".into()));
        assert_eq!(split_track_prefix("03. Song"), (Some(3), "Song".into()));
        assert_eq!(split_track_prefix("12_Song"), (Some(12), "Song".into()));
        assert_eq!(split_track_prefix("Song"), (None, "Song".into()));
        assert_eq!(split_track_prefix("1979"), (None, "1979".into())); // a title, not a track number
        assert_eq!(split_track_prefix("99 Problems"), (Some(99), "Problems".into()));
        assert_eq!(split_track_prefix("2Pac - Changes"), (None, "2Pac - Changes".into()));
    }

    #[test]
    fn recognises_disc_folders() {
        assert_eq!(parse_disc_folder("CD 2"), Some(2));
        assert_eq!(parse_disc_folder("Disc2"), Some(2));
        assert_eq!(parse_disc_folder("disk_3"), Some(3));
        assert_eq!(parse_disc_folder("Discovery"), None);
        assert_eq!(parse_disc_folder("CD Singles"), None);
    }

    #[test]
    fn fills_gaps_from_the_folder_structure() {
        let mut m = TrackMeta::default();
        fill_from_path(Path::new("/music/Pink Floyd/The Wall/CD 2/04 - Comfortably Numb.flac"), &mut m);
        assert_eq!((m.artist.as_str(), m.album.as_str(), m.title.as_str()), ("Pink Floyd", "The Wall", "Comfortably Numb"));
        assert_eq!((m.disc, m.track), (Some(2), Some(4)));
        // tags win over the folders
        let mut t = TrackMeta { artist: "Tagged".into(), album: "Album".into(), title: "T".into(), track: Some(9), ..Default::default() };
        fill_from_path(Path::new("/x/Folder/Other/01 - File.flac"), &mut t);
        assert_eq!((t.artist.as_str(), t.album.as_str(), t.title.as_str(), t.track), ("Tagged", "Album", "T", Some(9)));
    }

    #[test]
    fn natural_order() {
        let mut v = vec!["10 - b", "2 - a", "01 - z", "Album/1", "album/02"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["01 - z", "2 - a", "10 - b", "Album/1", "album/02"]);
    }

    #[test]
    fn gathers_folders_files_and_playlists_without_duplicates() {
        let root = std::env::temp_dir().join(format!("rd_stick_scan_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("A/Album/CD 1")).unwrap();
        std::fs::create_dir_all(root.join("A/.hidden")).unwrap();
        for f in ["A/Album/CD 1/01.flac", "A/Album/CD 1/02.flac", "A/Album/cover.jpg", "A/.hidden/x.flac", "loose.mp3"] {
            std::fs::write(root.join(f), b"x").unwrap();
        }
        std::fs::write(root.join("list.m3u8"), "A/Album/CD 1/02.flac\nloose.mp3\nmissing.mp3\n").unwrap();
        let spec = SourceSpec {
            folders: vec![root.join("A").to_string_lossy().to_string()],
            files: vec![root.join("loose.mp3").to_string_lossy().to_string(), root.join("A/Album/cover.jpg").to_string_lossy().to_string()],
            playlists: vec![root.join("list.m3u8").to_string_lossy().to_string()],
            playlist_root: None,
        };
        let (paths, skipped) = gather_paths(&spec).unwrap();
        let names: Vec<String> = paths.iter().map(|p| p.file_name().unwrap().to_string_lossy().to_string()).collect();
        assert_eq!(names, ["01.flac", "02.flac", "loose.mp3"]); // hidden folders skipped, duplicates dropped
        assert!(skipped.iter().any(|(e, r)| e.ends_with("cover.jpg") && r.contains("not an audio")));
        assert!(skipped.iter().any(|(e, r)| e.contains("missing.mp3") && r.contains("not found")));
        assert!(gather_paths(&SourceSpec { folders: vec!["/no/such/folder".into()], ..Default::default() }).is_err());
        std::fs::remove_dir_all(&root).ok();
    }
}
