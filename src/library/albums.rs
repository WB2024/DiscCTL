//! Finding the albums inside a rips folder.
//!
//! RustyDisc's own rips are one folder per disc, straight inside the rips folder. People also keep
//! older archives there, laid out as `Artist/Album/tracks`, sometimes inside one container folder
//! (for example `Archive/Avril Lavigne/Let Go/*.flac`). Treating each top-level folder as one
//! album would turn such a container into a single "album" holding every track of every album in
//! it, so albums are found by structure instead:
//!
//! * a folder that holds audio itself (or in `audio/`, as archive-mode rips do) is one album,
//!   and everything beneath it (artwork folders, `data/`, `metadata/`) belongs to that album;
//! * a folder with no audio of its own that contains albums is only a container, and its albums
//!   are listed one by one;
//! * a folder that is neither (a data disc, a pile of video files) stays a single entry, as before.
//!
//! `CD1`/`Disc 2` style folders under an album do not split it: a multi-disc album is one album.

use std::path::{Path, PathBuf};

const AUDIO: &[&str] = &["flac", "mp3", "m4a", "aac", "ogg", "opus", "wav", "aiff", "aif", "alac", "wma", "ape", "wv"];
const MAX_DEPTH: usize = 5;

fn is_audio(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| AUDIO.contains(&e.to_lowercase().as_str()))
}

fn hidden(name: &str) -> bool {
    name.starts_with('.') || name.starts_with('@') // dot-folders and NAS recycle/metadata folders
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut v: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir() && !hidden(&p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()))
        .collect();
    v.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    v
}

fn has_direct_audio(dir: &Path) -> bool {
    std::fs::read_dir(dir).map(|rd| rd.filter_map(|e| e.ok()).any(|e| e.path().is_file() && is_audio(&e.path()))).unwrap_or(false)
}

/// "CD1", "Disc 2", "Disk-3", "cd 01".
fn is_disc_folder(name: &str) -> bool {
    let n = name.trim().to_lowercase();
    ["cd", "disc", "disk"].iter().any(|p| {
        n.strip_prefix(p).is_some_and(|rest| {
            let rest = rest.trim_start_matches([' ', '-', '_', '.']);
            !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit())
        })
    })
}

/// Does this folder hold one album's music, in itself or in an `audio/` folder or per-disc folders?
fn is_album(dir: &Path) -> bool {
    if has_direct_audio(dir) || has_direct_audio(&dir.join("audio")) {
        return true;
    }
    let with_audio: Vec<PathBuf> = subdirs(dir).into_iter().filter(|d| has_direct_audio(d) || has_direct_audio(&d.join("audio"))).collect();
    !with_audio.is_empty() && with_audio.iter().all(|d| is_disc_folder(&d.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()))
}

/// The albums found below `dir` (not including `dir`), as full paths.
fn albums_below(dir: &Path, depth: usize) -> Vec<PathBuf> {
    if depth >= MAX_DEPTH {
        return Vec::new();
    }
    let mut out = Vec::new();
    for d in subdirs(dir) {
        if is_album(&d) {
            out.push(d);
        } else {
            out.extend(albums_below(&d, depth + 1));
        }
    }
    out
}

/// Every entry of the rips folder: albums (at any depth inside container folders), and any other
/// top-level folder as one entry. Paths are full; use `relative` for the name.
pub fn discover(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for d in subdirs(root) {
        if is_album(&d) {
            out.push(d);
            continue;
        }
        let inner = albums_below(&d, 1);
        if inner.is_empty() {
            out.push(d); // not music (a data rip, video files...): one entry, as it always was
        } else {
            out.extend(inner);
        }
    }
    out
}

/// The name of an entry: its path below the rips folder, with `/` between the parts.
pub fn relative(root: &Path, dir: &Path) -> String {
    dir.strip_prefix(root).unwrap_or(dir).components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect::<Vec<_>>().join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(root: &Path, rel: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"x").unwrap();
    }

    fn tree(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rustydisc_albums_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn names(root: &Path) -> Vec<String> {
        discover(root).iter().map(|p| relative(root, p)).collect()
    }

    #[test]
    fn a_container_of_artist_album_folders_becomes_its_albums() {
        let root = tree("archive");
        for rel in [
            "Archive/Avril Lavigne/Let Go/01 a.flac",
            "Archive/Avril Lavigne/Let Go/Let Go.png",
            "Archive/Avril Lavigne/Under My Skin/01 b.flac",
            "Archive/Morrissey/Your Arsenal/01 c.flac",
            "Archive/Morrissey/Your Arsenal/Art/back.jpg",
            "Archive/Kate Nash/Made of Bricks/cover.jpg",
        ] {
            touch(&root, rel);
        }
        assert_eq!(
            names(&root),
            ["Archive/Avril Lavigne/Let Go", "Archive/Avril Lavigne/Under My Skin", "Archive/Morrissey/Your Arsenal"],
            "albums only: no 'Archive' entry, and the artwork folder is not an album"
        );
    }

    #[test]
    fn ordinary_rips_stay_one_entry_each_in_either_layout() {
        let root = tree("plain");
        touch(&root, "Artist - Flat (2000)/01.flac");
        touch(&root, "Artist - Archive (2001)/audio/01.flac");
        touch(&root, "Artist - Archive (2001)/metadata/disc.json");
        touch(&root, "Artist - Archive (2001)/data/readme.txt");
        assert_eq!(names(&root), ["Artist - Archive (2001)", "Artist - Flat (2000)"]);
    }

    #[test]
    fn non_music_folders_remain_a_single_entry() {
        let root = tree("data");
        touch(&root, "Other Content/Video/clip.mp4");
        touch(&root, "Data Disc/files/readme.txt");
        assert_eq!(names(&root), ["Data Disc", "Other Content"]);
    }

    #[test]
    fn multi_disc_albums_are_not_split() {
        let root = tree("multi");
        touch(&root, "Big Box/CD1/01.flac");
        touch(&root, "Big Box/CD2/01.flac");
        touch(&root, "Other Box/Disc 1/01.flac");
        assert_eq!(names(&root), ["Big Box", "Other Box"]);
    }

    #[test]
    fn hidden_and_nas_folders_are_ignored() {
        let root = tree("hidden");
        touch(&root, ".trash/01.flac");
        touch(&root, "@eaDir/x/01.flac");
        touch(&root, "Real/01.flac");
        assert_eq!(names(&root), ["Real"]);
    }

    #[test]
    fn disc_folder_names() {
        for n in ["CD1", "cd 2", "Disc 03", "Disk-4", "disc_1"] {
            assert!(is_disc_folder(n), "{n}");
        }
        for n in ["Art", "Discography", "CD", "Disc One", "Bonus"] {
            assert!(!is_disc_folder(n), "{n}");
        }
    }
}
