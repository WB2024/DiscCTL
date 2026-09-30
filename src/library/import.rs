//! Moving a finished rip into the music library.
//!
//! The library's layout comes from a Picard naming script (see `script`), run on each file's
//! tags. Nothing is touched until the plan has been made: every destination, every clash with
//! what is already in the library, and every file that will be left behind is known up front.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    str::FromStr,
};

use serde::Serialize;

use super::{script, tags};
use crate::{
    error::Error,
    stick::{
        existing::{self, Conflict},
        scan::is_audio_path,
    },
};

/// How files get from the rip folder into the library.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Move the file (a rename when both are on one disk, otherwise copy then delete).
    #[default]
    Move,
    /// Leave the rip where it is and copy.
    Copy,
    /// Leave the rip and add a hard link (no extra space; needs one filesystem).
    Hardlink,
}

impl FromStr for Mode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s.trim().to_lowercase().as_str() {
            "" | "move" => Ok(Mode::Move),
            "copy" => Ok(Mode::Copy),
            "hardlink" | "hard-link" | "link" => Ok(Mode::Hardlink),
            o => Err(format!("Unknown import mode '{o}': use move, copy or hardlink")),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ImportOptions {
    pub library: PathBuf,
    pub script: String,
    pub mode: Mode,
    /// Bring the cover picture (`cover.jpg`, `folder.png`, ...) along.
    pub include_cover: bool,
    /// Bring every other file along too (logs, cue sheets, booklets...).
    pub include_other: bool,
    /// Afterwards delete whatever is left in the rip folder, and the folder itself.
    pub delete_leftovers: bool,
    /// What to do when a file is already in the library.
    pub conflict: Conflict,
}

impl ImportOptions {
    pub fn new(library: PathBuf, script: String) -> Self {
        ImportOptions { library, script, mode: Mode::Move, include_cover: true, include_other: false, delete_leftovers: false, conflict: Conflict::Skip }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Import,
    /// Written over a copy already in the library.
    Replace,
    /// Not imported: already there, or the rule says to keep what is there.
    Skip,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlannedFile {
    /// Path inside the rip folder.
    pub src: String,
    /// Path inside the library.
    pub dest: String,
    /// "track", "cover" or "other"
    pub kind: &'static str,
    pub action: Action,
    pub bytes: u64,
    pub note: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ImportPlan {
    pub rip: String,
    pub album: Option<String>,
    pub artist: Option<String>,
    /// The album's folder in the library.
    pub album_dir: Option<String>,
    pub files: Vec<PlannedFile>,
    /// Files in the rip folder that are not being imported.
    pub leftovers: Vec<String>,
    pub tracks: usize,
    pub to_import: usize,
    pub replacing: usize,
    pub skipped: usize,
    pub bytes: u64,
    pub untagged: usize,
    pub warnings: Vec<String>,
}

const MARKER: &str = "imported.json";
const COVER_STEMS: &[&str] = &["cover", "folder", "front", "albumart"];

fn is_cover(p: &Path) -> bool {
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
    matches!(ext.as_str(), "jpg" | "jpeg" | "png") && COVER_STEMS.contains(&stem.as_str())
}

fn collect(dir: &Path, root: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        let Ok(m) = std::fs::symlink_metadata(&p) else { continue };
        if m.is_dir() {
            if depth < 8 {
                collect(&p, root, out, depth + 1);
            }
        } else if m.is_file() && p.file_name().is_some_and(|n| n != MARKER) {
            out.push(p.strip_prefix(root).unwrap_or(&p).to_path_buf());
        }
    }
}

/// A rip's own metadata file, for what the tags don't carry (Picard keeps the release comment out of files).
fn release_json(rip: &Path) -> Option<serde_json::Value> {
    ["metadata/musicbrainz.json", "musicbrainz.json"].iter().find_map(|f| std::fs::read(rip.join(f)).ok().and_then(|b| serde_json::from_slice(&b).ok()))
}

fn with_suffix(path: &str, n: usize) -> String {
    match path.rsplit_once('.') {
        Some((stem, e)) => format!("{stem} ({n}).{e}"),
        None => format!("{path} ({n})"),
    }
}

pub fn plan(rip: &Path, opts: &ImportOptions) -> Result<ImportPlan, Error> {
    if !rip.is_dir() {
        return Err(Error::validation(format!("The rip folder isn't there: {}", rip.display())));
    }
    script::check(&opts.script).map_err(|e| Error::validation(format!("The naming script has a problem: {e}")))?;

    let mut all = Vec::new();
    collect(rip, rip, &mut all, 0);
    let audio: Vec<&PathBuf> = all.iter().filter(|p| is_audio_path(p)).collect();
    if audio.is_empty() {
        return Err(Error::validation("There's no music in that folder to import"));
    }
    let json = release_json(rip);

    let mut files: Vec<PlannedFile> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut taken: HashSet<String> = HashSet::new();
    let mut untagged = 0usize;
    let mut album: Option<String> = None;
    let mut artist: Option<String> = None;
    let mut album_dir: Option<String> = None;
    let mut multi_disc = false;

    for rel in &audio {
        let abs = rip.join(rel);
        let mut t = tags::read(&abs);
        if t.untagged || !t.vars.contains_key("title") {
            untagged += 1;
        }
        // Fill album-level gaps from the rip's own metadata.
        if let Some(j) = &json {
            let mut fill = |var: &str, field: &str| {
                if !t.vars.contains_key(var) {
                    if let Some(v) = j.get(field).and_then(|v| v.as_str()).filter(|v| !v.is_empty()) {
                        t.vars.insert(var.to_string(), v.to_string());
                    }
                }
            };
            fill("_releasecomment", "release_comment");
            fill("albumartistsort", "album_artist_sort");
            fill("originaldate", "original_date");
            fill("musicbrainz_albumid", "mb_release_id");
        }
        let out = script::run(&opts.script, &t.vars).map_err(|e| Error::validation(format!("The naming script failed: {e}")))?;
        let comps = script::to_components(&out);
        if comps.is_empty() {
            return Err(Error::validation("The naming script produced an empty path"));
        }
        let ext = t.vars.get("_extension").cloned().unwrap_or_default();
        let mut dest = comps.join("/");
        if !ext.is_empty() {
            dest = format!("{dest}.{ext}");
        }
        // Two tracks the script names identically (say, both untagged): keep both.
        let mut n = 1;
        let base = dest.clone();
        while !taken.insert(dest.to_lowercase()) {
            n += 1;
            dest = with_suffix(&base, n);
        }
        if album.is_none() {
            album = t.vars.get("album").cloned();
            artist = t.vars.get("albumartist").or_else(|| t.vars.get("artist")).cloned();
            multi_disc = t.vars.get("totaldiscs").and_then(|d| d.parse::<u32>().ok()).unwrap_or(1) > 1;
            let dir = dest.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default();
            // A multi-disc album has one more level (CD 1, CD 2) that the album folder sits above.
            album_dir = Some(if multi_disc { dir.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or(dir) } else { dir });
        }
        let bytes = std::fs::metadata(&abs).map(|m| m.len()).unwrap_or(0);
        files.push(PlannedFile { src: rel.to_string_lossy().to_string(), dest, kind: "track", action: Action::Import, bytes, note: None });
    }
    let track_count = files.len();
    let album_dir_s = album_dir.clone().unwrap_or_default();
    let in_album = |name: &str| if album_dir_s.is_empty() { name.to_string() } else { format!("{album_dir_s}/{name}") };
    let _ = multi_disc;

    // Cover and other files.
    let mut cover_done = false;
    for rel in &all {
        if is_audio_path(rel) {
            continue;
        }
        let abs = rip.join(rel);
        let bytes = std::fs::metadata(&abs).map(|m| m.len()).unwrap_or(0);
        let name = rel.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if is_cover(rel) && opts.include_cover && !cover_done && rel.components().count() <= 2 {
            let ext = rel.extension().and_then(|e| e.to_str()).unwrap_or("jpg").to_lowercase();
            cover_done = true;
            files.push(PlannedFile { src: rel.to_string_lossy().to_string(), dest: in_album(&format!("cover.{ext}")), kind: "cover", action: Action::Import, bytes, note: None });
        } else if opts.include_other && !is_cover(rel) {
            let dest = in_album(&name);
            if taken.insert(dest.to_lowercase()) {
                files.push(PlannedFile { src: rel.to_string_lossy().to_string(), dest, kind: "other", action: Action::Import, bytes, note: None });
            } else {
                warnings.push(format!("'{}' has the same name as another file and was left out", rel.display()));
            }
        }
    }

    // What is already in the library.
    let mut album_exists = false;
    for f in files.iter_mut() {
        let target = opts.library.join(&f.dest);
        let Ok(old) = std::fs::metadata(&target) else { continue };
        if !old.is_file() {
            continue;
        }
        album_exists = true;
        let ext = f.dest.rsplit('.').next().unwrap_or("");
        let identical = old.len() == f.bytes;
        let decision: (bool, String) = if identical {
            (false, "Already in the library".into())
        } else if f.kind != "track" {
            match opts.conflict {
                Conflict::Skip | Conflict::KeepBoth => (false, "Already in the library".into()),
                _ => (true, "Replaces the copy in the library".into()),
            }
        } else {
            let new_q = existing::quality_of(ext, f.bytes, tags::read(&rip.join(&f.src)).duration_secs);
            let old_q = existing::quality_of(ext, old.len(), tags::read(&target).duration_secs);
            let ord = existing::compare(&new_q, &old_q);
            match opts.conflict {
                Conflict::Skip => (false, format!("Already in the library ({})", old_q.label)),
                Conflict::Replace => (true, format!("Replaces {} with {}", old_q.label, new_q.label)),
                Conflict::HigherQuality if ord == std::cmp::Ordering::Greater => (true, format!("Better quality: {} replaces {}", new_q.label, old_q.label)),
                Conflict::HigherQuality => (false, format!("Kept {} in the library ({} isn't better)", old_q.label, new_q.label)),
                Conflict::LowerQuality if ord == std::cmp::Ordering::Less => (true, format!("Smaller: {} replaces {}", new_q.label, old_q.label)),
                Conflict::LowerQuality => (false, format!("Kept {} in the library ({} isn't lower)", old_q.label, new_q.label)),
                Conflict::Newer => {
                    if existing::mtime_of(&rip.join(&f.src)) > existing::mtime_of(&target) + 2 {
                        (true, "Newer than the copy in the library".into())
                    } else {
                        (false, "The copy in the library is as new or newer".into())
                    }
                }
                Conflict::KeepBoth => (false, String::new()),
            }
        };
        if opts.conflict == Conflict::KeepBoth && !identical && f.kind == "track" {
            let base = f.dest.clone();
            let mut n = 1;
            while opts.library.join(&f.dest).exists() || taken.contains(&f.dest.to_lowercase()) {
                n += 1;
                f.dest = with_suffix(&base, n);
            }
            taken.insert(f.dest.to_lowercase());
            f.note = Some("Kept both copies".into());
        } else if decision.0 {
            f.action = Action::Replace;
            f.note = Some(decision.1);
        } else {
            f.action = Action::Skip;
            f.note = Some(decision.1);
        }
    }
    if album_exists {
        warnings.push(format!("Part of this album is already in the library ({}).", album_dir_s));
    }
    if untagged > 0 {
        warnings.push(format!("{untagged} file(s) have little or no tags, so the naming script's fallbacks (like [Unknown Artist]) are used for them."));
    }
    if opts.mode == Mode::Hardlink && !same_device(rip, &opts.library) {
        warnings.push("The rip folder and the library are on different disks, so hard links can't be made; files will be copied.".into());
    }

    let imported: HashSet<&str> = files.iter().filter(|f| f.action != Action::Skip).map(|f| f.src.as_str()).collect();
    let leftovers: Vec<String> = all
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .filter(|p| !imported.contains(p.as_str()))
        .collect();

    Ok(ImportPlan {
        rip: rip.to_string_lossy().to_string(),
        album,
        artist,
        album_dir,
        tracks: track_count,
        to_import: files.iter().filter(|f| f.action == Action::Import).count(),
        replacing: files.iter().filter(|f| f.action == Action::Replace).count(),
        skipped: files.iter().filter(|f| f.action == Action::Skip).count(),
        bytes: files.iter().filter(|f| f.action != Action::Skip).map(|f| f.bytes).sum(),
        untagged,
        warnings,
        files,
        leftovers,
    })
}

fn same_device(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let dev = |p: &Path| {
        let mut p = p.to_path_buf();
        while !p.exists() {
            if !p.pop() {
                break;
            }
        }
        std::fs::metadata(&p).map(|m| m.dev()).ok()
    };
    dev(a).is_some() && dev(a) == dev(b)
}

#[derive(Debug, Default, Serialize)]
pub struct Summary {
    pub imported: usize,
    pub replaced: usize,
    pub skipped: usize,
    pub bytes: u64,
    pub deleted: usize,
    /// The rip folder is gone (everything was imported and the leftovers deleted).
    pub rip_removed: bool,
}

fn put(src: &Path, dest: &Path, mode: Mode, replace: bool) -> Result<(), Error> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| Error::device(format!("Can't create {}: {e}", parent.display())))?;
    }
    // Written under a temporary name and renamed, so a half-copied file never looks finished.
    let part = PathBuf::from(format!("{}.rustydisc-part", dest.display()));
    let err = |e: std::io::Error| Error::device(format!("Can't put '{}' into the library: {e}", src.display()));
    match mode {
        Mode::Move => match std::fs::rename(src, &part) {
            Ok(()) => {}
            Err(_) => {
                // Another disk: copy, then remove the original once the copy is complete.
                std::fs::copy(src, &part).map_err(|e| { let _ = std::fs::remove_file(&part); err(e) })?;
                std::fs::remove_file(src).map_err(err)?;
            }
        },
        Mode::Copy => {
            std::fs::copy(src, &part).map_err(|e| { let _ = std::fs::remove_file(&part); err(e) })?;
        }
        Mode::Hardlink => {
            if std::fs::hard_link(src, &part).is_err() {
                std::fs::copy(src, &part).map_err(|e| { let _ = std::fs::remove_file(&part); err(e) })?;
            }
        }
    }
    let _ = replace; // rename over an existing file replaces it
    std::fs::rename(&part, dest).map_err(err)
}

fn prune_empty(dir: &Path, stop: &Path) {
    let mut d = dir.to_path_buf();
    while d.starts_with(stop) && d != stop {
        if std::fs::remove_dir(&d).is_err() {
            return;
        }
        match d.parent() {
            Some(p) => d = p.to_path_buf(),
            None => return,
        }
    }
}

pub struct Progress<'a> {
    pub step: &'a dyn Fn(&str),
    pub pct: &'a dyn Fn(f32),
}

pub fn execute(rip: &Path, plan: &ImportPlan, opts: &ImportOptions, progress: &Progress) -> Result<Summary, Error> {
    let mut summary = Summary::default();
    std::fs::create_dir_all(&opts.library).map_err(|e| Error::device(format!("Can't use the library folder {}: {e}", opts.library.display())))?;
    let probe = opts.library.join(".rustydisc-write-test");
    std::fs::write(&probe, b"x").map_err(|e| Error::device(format!("The library folder isn't writable ({}): {e}", opts.library.display())))?;
    let _ = std::fs::remove_file(&probe);

    let total = plan.files.iter().filter(|f| f.action != Action::Skip).count().max(1);
    let mut done = 0usize;
    let mut imported_dests: Vec<String> = Vec::new();
    let mut failed_srcs: HashSet<String> = HashSet::new();
    for f in &plan.files {
        if f.action == Action::Skip {
            summary.skipped += 1;
            continue;
        }
        (progress.step)(&format!("{} {} of {} — {}", if opts.mode == Mode::Move { "Moving" } else { "Copying" }, done + 1, total, f.dest));
        put(&rip.join(&f.src), &opts.library.join(&f.dest), opts.mode, f.action == Action::Replace)?;
        match f.action {
            Action::Replace => summary.replaced += 1,
            _ => summary.imported += 1,
        }
        summary.bytes += f.bytes;
        imported_dests.push(f.dest.clone());
        done += 1;
        (progress.pct)(done as f32 / total as f32 * 95.0);
        let _ = &mut failed_srcs;
    }

    // Tidy the rip folder.
    let source_gone = opts.mode == Mode::Move;
    if source_gone {
        // Moved files leave empty folders behind.
        for f in plan.files.iter().filter(|f| f.action != Action::Skip) {
            if let Some(parent) = rip.join(&f.src).parent() {
                prune_empty(parent, rip);
            }
        }
    }
    let audio_left = plan.files.iter().any(|f| f.kind == "track" && f.action == Action::Skip);
    if opts.delete_leftovers {
        (progress.step)("Deleting what's left in the rip folder...");
        // Skipped tracks stay: they were not imported, so they are not "left over".
        let keep: HashSet<PathBuf> = plan.files.iter().filter(|f| f.kind == "track" && f.action == Action::Skip).map(|f| rip.join(&f.src)).collect();
        fn remove_all(dir: &Path, keep: &HashSet<PathBuf>, n: &mut usize) {
            let Ok(rd) = std::fs::read_dir(dir) else { return };
            for e in rd.filter_map(|e| e.ok()) {
                let p = e.path();
                if p.is_dir() && !std::fs::symlink_metadata(&p).map(|m| m.file_type().is_symlink()).unwrap_or(false) {
                    remove_all(&p, keep, n);
                    let _ = std::fs::remove_dir(&p);
                } else if !keep.contains(&p) && std::fs::remove_file(&p).is_ok() {
                    *n += 1;
                }
            }
        }
        remove_all(rip, &keep, &mut summary.deleted);
        if !audio_left {
            summary.rip_removed = std::fs::remove_dir(rip).is_ok();
        }
    }

    if !summary.rip_removed && rip.is_dir() && (summary.imported + summary.replaced) > 0 {
        let marker = serde_json::json!({
            "imported_at": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
            "library": opts.library.to_string_lossy(),
            "mode": opts.mode,
            "album_dir": plan.album_dir,
            "files": imported_dests,
        });
        let _ = std::fs::write(rip.join(MARKER), serde_json::to_vec_pretty(&marker).unwrap_or_default());
    }
    (progress.pct)(99.0);
    Ok(summary)
}

/// Delete everything in a rip folder and the folder itself. Returns the number of files deleted
/// and whether the folder is gone.
pub fn remove_leftovers(rip: &Path) -> (usize, bool) {
    fn walk(dir: &Path, n: &mut usize) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.filter_map(|e| e.ok()) {
            let p = e.path();
            let is_link = std::fs::symlink_metadata(&p).map(|m| m.file_type().is_symlink()).unwrap_or(false);
            if p.is_dir() && !is_link {
                walk(&p, n);
                let _ = std::fs::remove_dir(&p);
            } else if std::fs::remove_file(&p).is_ok() {
                *n += 1;
            }
        }
    }
    let mut n = 0;
    walk(rip, &mut n);
    (n, std::fs::remove_dir(rip).is_ok())
}

/// Has this rip been imported already? (`imported.json` is written by [`execute`].)
pub fn imported_marker(rip: &Path) -> Option<serde_json::Value> {
    std::fs::read(rip.join(MARKER)).ok().and_then(|b| serde_json::from_slice(&b).ok())
}

/// A quick look for the import list: how many tracks, and what the first one is called.
pub fn peek(rip: &Path) -> HashMap<&'static str, String> {
    let mut all = Vec::new();
    collect(rip, rip, &mut all, 0);
    let mut out = HashMap::new();
    let audio: Vec<&PathBuf> = all.iter().filter(|p| is_audio_path(p)).collect();
    out.insert("tracks", audio.len().to_string());
    if let Some(first) = audio.first() {
        let t = tags::read(&rip.join(first));
        for (k, v) in [("album", "album"), ("artist", "albumartist")] {
            if let Some(x) = t.vars.get(v) {
                out.insert(k, x.clone());
            }
        }
        if !out.contains_key("artist") {
            if let Some(x) = t.vars.get("artist") {
                out.insert("artist", x.clone());
            }
        }
        out.insert("tagged", (!t.untagged && t.vars.contains_key("title")).to_string());
        out.insert("has_mb", t.vars.contains_key("musicbrainz_albumid").to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rip::{
        encoder::TrackTags,
        musicbrainz::{MbTrackInfo, ReleaseInfo},
        tagging,
    };
    use std::process::Command;

    fn have_ffmpeg() -> bool {
        Command::new("ffmpeg").arg("-version").output().is_ok()
    }

    /// A tagged FLAC of a rip, made the way a real rip is made.
    fn track(dir: &Path, n: usize, title: &str, release: &ReleaseInfo, secs: u32) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let p = dir.join(format!("{n:02} - {title}.flac"));
        assert!(Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", &format!("sine=frequency={}:duration={secs}", 300 + n * 40), "-c:a", "flac"]).arg(&p).status().unwrap().success());
        let t = release.tracks.iter().find(|t| t.number == n).cloned();
        let basic = TrackTags { title: Some(title.into()), artist: Some(release.album_artist.clone()), album: Some(release.album.clone()), album_artist: Some(release.album_artist.clone()), track_number: Some(n), track_total: Some(release.tracks.len()), ..Default::default() };
        tagging::apply(p.to_str().unwrap(), &basic, Some(release), t.as_ref(), false);
        p
    }

    fn release(discs: usize) -> ReleaseInfo {
        ReleaseInfo {
            mb_release_id: "55555555-5555-4555-8555-555555555555".into(), album: "Wild Thing".into(), album_artist: "The Troggs".into(),
            album_artist_sort: Some("Troggs, The".into()), album_artist_ids: vec!["33333333-3333-4333-8333-333333333333".into()],
            date: Some("1966-06-01".into()), disc_number: Some(1), disc_total: Some(discs), release_comment: Some("remastered".into()),
            tracks: (1..=3).map(|n| MbTrackInfo { number: n, title: format!("Song {n}"), ..Default::default() }).collect(),
            ..Default::default()
        }
    }

    fn setup(name: &str, discs: usize) -> (PathBuf, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("rd_import_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let rip = root.join("rips/The Troggs - Wild Thing (1966)");
        let lib = root.join("library");
        std::fs::create_dir_all(&lib).unwrap();
        let r = release(discs);
        for (n, t) in [(1, "Song 1"), (2, "Song 2"), (3, "Song 3")] {
            track(&rip, n, t, &r, 2);
        }
        std::fs::write(rip.join("cover.jpg"), b"jpgdata").unwrap();
        std::fs::write(rip.join("rip.log"), b"log").unwrap();
        (root, rip, lib)
    }

    fn quiet<T>(f: impl FnOnce(&Progress) -> T) -> T {
        f(&Progress { step: &|_| {}, pct: &|_| {} })
    }

    #[test]
    fn a_rip_is_planned_and_moved_into_the_picard_layout() {
        if !have_ffmpeg() {
            eprintln!("skipping: ffmpeg not installed");
            return;
        }
        let (root, rip, lib) = setup("move", 1);
        let opts = ImportOptions::new(lib.clone(), script::DEFAULT_SCRIPT.to_string());
        let plan = plan(&rip, &opts).unwrap();
        let dests: Vec<&str> = plan.files.iter().map(|f| f.dest.as_str()).collect();
        assert_eq!(dests, [
            "T/Troggs, The/[1966] Wild Thing (remastered)/01 - The Troggs. Song 1.flac",
            "T/Troggs, The/[1966] Wild Thing (remastered)/02 - The Troggs. Song 2.flac",
            "T/Troggs, The/[1966] Wild Thing (remastered)/03 - The Troggs. Song 3.flac",
            "T/Troggs, The/[1966] Wild Thing (remastered)/cover.jpg",
        ]);
        assert_eq!((plan.tracks, plan.to_import, plan.skipped), (3, 4, 0));
        assert_eq!(plan.leftovers, ["rip.log"]);
        assert_eq!(plan.album.as_deref(), Some("Wild Thing"));

        let s = quiet(|p| execute(&rip, &plan, &opts, p)).unwrap();
        assert_eq!((s.imported, s.replaced), (4, 0));
        let album = lib.join("T/Troggs, The/[1966] Wild Thing (remastered)");
        assert!(album.join("01 - The Troggs. Song 1.flac").is_file() && album.join("cover.jpg").is_file());
        assert!(!rip.join("01 - Song 1.flac").exists(), "moved, not copied");
        assert!(rip.join("rip.log").exists(), "extras stay unless asked");
        assert!(imported_marker(&rip).is_some());
        // the tags survive the move untouched
        let t = tags::read(&album.join("02 - The Troggs. Song 2.flac"));
        assert_eq!(t.vars.get("musicbrainz_albumid").map(String::as_str), Some("55555555-5555-4555-8555-555555555555"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn multi_disc_rips_get_disc_folders_and_the_cover_goes_above_them() {
        if !have_ffmpeg() {
            return;
        }
        let (root, rip, lib) = setup("multi", 2);
        let opts = ImportOptions::new(lib, script::DEFAULT_SCRIPT.to_string());
        let plan = plan(&rip, &opts).unwrap();
        assert_eq!(plan.files[0].dest, "T/Troggs, The/[1966] Wild Thing (remastered)/CD 1/01 - The Troggs. Song 1.flac");
        assert_eq!(plan.files[3].dest, "T/Troggs, The/[1966] Wild Thing (remastered)/cover.jpg");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn copy_and_hardlink_leave_the_rip_and_extras_and_leftovers_are_options() {
        if !have_ffmpeg() {
            return;
        }
        let (root, rip, lib) = setup("copy", 1);
        let mut opts = ImportOptions::new(lib.clone(), script::DEFAULT_SCRIPT.to_string());
        opts.mode = Mode::Copy;
        opts.include_other = true;
        let p = plan(&rip, &opts).unwrap();
        assert!(p.files.iter().any(|f| f.kind == "other" && f.dest.ends_with("/rip.log")));
        assert!(p.leftovers.is_empty());
        quiet(|pr| execute(&rip, &p, &opts, pr)).unwrap();
        assert!(rip.join("01 - Song 1.flac").exists(), "a copy leaves the rip");
        assert!(lib.join("T/Troggs, The/[1966] Wild Thing (remastered)/rip.log").is_file());

        // hard links share the data
        let lib2 = root.join("library2");
        let mut o2 = ImportOptions::new(lib2.clone(), script::DEFAULT_SCRIPT.to_string());
        o2.mode = Mode::Hardlink;
        let p2 = plan(&rip, &o2).unwrap();
        quiet(|pr| execute(&rip, &p2, &o2, pr)).unwrap();
        use std::os::unix::fs::MetadataExt;
        let linked = lib2.join("T/Troggs, The/[1966] Wild Thing (remastered)/01 - The Troggs. Song 1.flac");
        assert!(std::fs::metadata(&linked).unwrap().nlink() >= 2);

        // move + delete leftovers removes the rip folder entirely
        let lib3 = root.join("library3");
        let mut o3 = ImportOptions::new(lib3, script::DEFAULT_SCRIPT.to_string());
        o3.delete_leftovers = true;
        let p3 = plan(&rip, &o3).unwrap();
        assert_eq!(p3.leftovers, ["rip.log"]);
        let s = quiet(|pr| execute(&rip, &p3, &o3, pr)).unwrap();
        assert!(s.rip_removed && !rip.exists());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn what_is_already_in_the_library_is_handled_by_the_rule() {
        if !have_ffmpeg() {
            return;
        }
        let (root, rip, lib) = setup("conflict", 1);
        let mut opts = ImportOptions::new(lib.clone(), script::DEFAULT_SCRIPT.to_string());
        opts.mode = Mode::Copy;
        let first = plan(&rip, &opts).unwrap();
        quiet(|pr| execute(&rip, &first, &opts, pr)).unwrap();

        // importing again: everything is already there
        let again = plan(&rip, &opts).unwrap();
        assert_eq!((again.to_import, again.skipped), (0, 4));
        assert!(again.warnings.iter().any(|w| w.contains("already in the library")));

        // a different file at one destination: skipped, or replaced when told to
        let dest = lib.join(&first.files[0].dest);
        std::fs::write(&dest, b"old and different").unwrap();
        assert_eq!(plan(&rip, &opts).unwrap().skipped, 4);
        opts.conflict = Conflict::Replace;
        let r = plan(&rip, &opts).unwrap();
        assert_eq!((r.replacing, r.skipped), (1, 3));
        quiet(|pr| execute(&rip, &r, &opts, pr)).unwrap();
        assert!(std::fs::metadata(&dest).unwrap().len() > 1000);

        // keep both
        std::fs::write(&dest, b"different again").unwrap();
        opts.conflict = Conflict::KeepBoth;
        let both = plan(&rip, &opts).unwrap();
        assert!(both.files[0].dest.ends_with("Song 1 (2).flac"), "{}", both.files[0].dest);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn bad_scripts_and_empty_folders_are_refused_clearly() {
        let dir = std::env::temp_dir().join(format!("rd_import_bad_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let opts = ImportOptions::new(dir.join("lib"), "$nope(x)".into());
        assert!(plan(&dir, &opts).unwrap_err().to_string().contains("no music"));
        assert!(plan(&dir.join("missing"), &opts).is_err());
        let broken = ImportOptions::new(dir.join("lib"), "$if(a,b".into());
        assert!(plan(&dir, &broken).unwrap_err().to_string().contains("naming script"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
