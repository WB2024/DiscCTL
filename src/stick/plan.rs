//! Working out what goes where on the stick, and whether it fits.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use serde::Serialize;

use super::{
    layout::{self, Layout},
    scan::{natural_cmp, Scan, SourceTrack},
};
use crate::{
    backend::transcode::{OutputFormat, TranscodeSpec},
    error::Error,
    planner::discs,
};

#[derive(Debug, Clone)]
pub struct StickOptions {
    pub layout: Layout,
    pub transcode: Option<String>,
    /// Keep embedded artwork in converted MP3/M4A/FLAC files (it costs space, but car stereos and
    /// phones show it).
    pub keep_art: bool,
    /// Put the cover image (`cover.jpg`) in each album folder.
    pub copy_covers: bool,
    /// Leave files that are already on the stick alone.
    pub skip_existing: bool,
    /// Empty the destination before writing.
    pub clear: bool,
    /// Folder on the stick to write into ("" = the root).
    pub dest_subfolder: String,
}

impl Default for StickOptions {
    fn default() -> Self {
        StickOptions {
            layout: Layout { template: layout::PRESETS[0].template.into(), options: Default::default() },
            transcode: None,
            keep_art: true,
            copy_covers: true,
            skip_existing: true,
            clear: false,
            dest_subfolder: String::new(),
        }
    }
}

/// The stick (or folder) being written to.
#[derive(Debug, Clone)]
pub struct TargetInfo {
    pub mount_point: PathBuf,
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub block_size: u64,
    pub max_file_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Write,
    /// Already on the stick.
    SkipExists,
    /// The same track was chosen twice.
    Duplicate,
    /// Bigger than the filesystem allows.
    TooBig,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlannedTrack {
    pub src: String,
    /// Path on the stick, relative to the destination folder.
    pub dest: String,
    pub original_bytes: u64,
    pub est_bytes: u64,
    pub transcode: bool,
    pub action: Action,
    pub artist: String,
    pub album: String,
    pub title: String,
    pub art_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CoverCopy {
    pub src: String,
    pub dest: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Suggestion {
    pub transcode: String,
    pub label: String,
    pub needed_bytes: u64,
    pub fits: bool,
}

#[derive(Debug, Serialize)]
pub struct StickPlan {
    pub dest_root: String,
    pub layout: String,
    /// Label of the conversion, e.g. "MP3 320k".
    pub transcode: Option<String>,
    /// The conversion as given, e.g. "mp3:320".
    pub transcode_spec: Option<String>,
    pub tracks: Vec<PlannedTrack>,
    pub covers: Vec<CoverCopy>,
    pub original_bytes: u64,
    /// Size of everything that will be written, after converting.
    pub write_bytes: u64,
    /// The same, counting the space files really take (whole clusters, directory entries).
    pub needed_bytes: u64,
    pub to_write: usize,
    pub already_there: usize,
    pub duplicates: usize,
    pub converted: usize,
    pub total_bytes: u64,
    pub free_bytes: u64,
    /// What is on the destination that would be deleted (only when clearing).
    pub clear_bytes: u64,
    pub available_bytes: u64,
    pub fits: bool,
    pub percent_after: f32,
    pub suggestions: Vec<Suggestion>,
    pub skipped: Vec<serde_json::Value>,
    pub warnings: Vec<String>,
}

const PER_FILE_OVERHEAD: u64 = 128;

/// Would this file be converted, and how big will it be? Reuses the disc planner's rules: only
/// convert when it helps.
pub fn estimate(t: &SourceTrack, spec: Option<&TranscodeSpec>, keep_art: bool) -> (bool, u64, bool) {
    let duration = t.duration_secs.or_else(|| if spec.is_some() { discs::probe_duration(&t.path) } else { None });
    let d = discs::decide(&t.path, t.size, duration, spec);
    if !d.transcode {
        return (false, t.size, false);
    }
    let mut est = d.est_bytes;
    if let Some(s) = spec {
        // Artwork survives into these formats; opus and wav files can't carry it.
        if keep_art && matches!(s.format, OutputFormat::Mp3 | OutputFormat::Aac | OutputFormat::Flac) {
            est += t.art_bytes;
        }
    }
    (true, est, d.unknown_duration)
}

fn cost(bytes: u64, block: u64) -> u64 {
    bytes.div_ceil(block.max(512)) * block.max(512) + PER_FILE_OVERHEAD
}

pub fn dir_size(path: &Path) -> u64 {
    let mut total = 0;
    if let Ok(rd) = std::fs::read_dir(path) {
        for e in rd.filter_map(|e| e.ok()) {
            if let Ok(m) = e.metadata() {
                total += if m.is_dir() { dir_size(&e.path()) } else { m.len() };
            }
        }
    }
    total
}

fn ext_of(path: &str) -> String {
    Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase()
}

/// Which converted-size choices to offer when it doesn't fit.
const SUGGESTED: &[&str] = &["mp3:320", "mp3:256", "mp3:192", "mp3:160", "mp3:128", "opus:96"];

pub fn build_plan(scan: &Scan, opts: &StickOptions, target: &TargetInfo) -> Result<StickPlan, Error> {
    layout::validate_template(&opts.layout.template)?;
    let spec = match opts.transcode.as_deref().filter(|t| !t.trim().is_empty()) {
        Some(t) => Some(TranscodeSpec::parse(t)?),
        None => None,
    };

    let dest_root = if opts.dest_subfolder.trim().is_empty() {
        target.mount_point.clone()
    } else {
        target.mount_point.join(sanitize_subfolder(&opts.dest_subfolder)?)
    };

    let metas: Vec<&layout::TrackMeta> = scan.tracks.iter().map(|t| &t.meta).collect();
    let infos = layout::album_infos(&metas);

    let mut tracks: Vec<PlannedTrack> = Vec::with_capacity(scan.tracks.len());
    let mut sources: Vec<&SourceTrack> = Vec::with_capacity(scan.tracks.len());
    let mut unknown_len = 0usize;
    let mut seen_track: HashSet<(String, String, u32, u32, String)> = HashSet::new();
    let mut used_paths: HashMap<String, usize> = HashMap::new();
    let mut duplicates = 0usize;

    for t in &scan.tracks {
        let (transcode, est, unknown) = estimate(t, spec.as_ref(), opts.keep_art);
        if unknown {
            unknown_len += 1;
        }
        let ext = match (transcode, spec.as_ref()) {
            (true, Some(s)) => s.extension().to_string(),
            _ => ext_of(&t.path),
        };
        let info = infos.get(&layout::album_key(&t.meta)).copied().unwrap_or_default();
        let mut dest = layout::render(&t.meta, info, &opts.layout, &ext)?;

        // The same track chosen twice (say, an album folder and a playlist that lists it).
        let ident = (
            layout::album_key(&t.meta).0,
            layout::album_key(&t.meta).1,
            t.meta.disc.unwrap_or(1),
            t.meta.track.unwrap_or(0),
            t.meta.title.to_lowercase(),
        );
        let duplicate = t.meta.track.is_some() && !seen_track.insert(ident);

        let mut action = if duplicate { Action::Duplicate } else { Action::Write };
        if duplicate {
            duplicates += 1;
        } else {
            // Two different tracks that would land on one name: keep both.
            let key = dest.to_lowercase();
            let n = used_paths.entry(key).or_insert(0);
            *n += 1;
            if *n > 1 {
                let (stem, e) = match dest.rsplit_once('.') {
                    Some((s, e)) => (s.to_string(), format!(".{e}")),
                    None => (dest.clone(), String::new()),
                };
                dest = format!("{stem} ({n}){e}");
            }
        }

        if action == Action::Write {
            if let Some(max) = target.max_file_bytes {
                if est > max {
                    action = Action::TooBig;
                }
            }
        }
        if action == Action::Write && opts.skip_existing && !opts.clear {
            if let Ok(m) = std::fs::metadata(dest_root.join(&dest)) {
                if m.is_file() && m.len() > 0 && (transcode || m.len() == t.size) {
                    action = Action::SkipExists;
                }
            }
        }

        tracks.push(PlannedTrack {
            src: t.path.clone(),
            dest,
            original_bytes: t.size,
            est_bytes: est,
            transcode,
            action,
            artist: if t.meta.album_artist.is_empty() { t.meta.artist.clone() } else { t.meta.album_artist.clone() },
            album: t.meta.album.clone(),
            title: t.meta.title.clone(),
            art_bytes: t.art_bytes,
        });
        sources.push(t);
    }

    // Write in a sorted order: many car stereos and players play a folder in the order the files
    // were written, not alphabetically.
    let mut order: Vec<usize> = (0..tracks.len()).collect();
    order.sort_by(|&a, &b| natural_cmp(&tracks[a].dest, &tracks[b].dest));
    let sorted_tracks: Vec<PlannedTrack> = order.iter().map(|&i| tracks[i].clone()).collect();
    let sorted_sources: Vec<&SourceTrack> = order.iter().map(|&i| sources[i]).collect();
    let tracks = sorted_tracks;

    // Cover art: one per folder that gets tracks.
    let mut covers: Vec<CoverCopy> = Vec::new();
    if opts.copy_covers {
        let mut done: HashSet<String> = HashSet::new();
        for (t, s) in tracks.iter().zip(sorted_sources.iter()) {
            if t.action != Action::Write && t.action != Action::SkipExists {
                continue;
            }
            let (Some(cover), Some(dir)) = (&s.cover, Path::new(&t.dest).parent()) else { continue };
            let dir = dir.to_string_lossy().to_string();
            if dir.is_empty() || !done.insert(dir.clone()) {
                continue;
            }
            let ext = cover.extension().and_then(|e| e.to_str()).unwrap_or("jpg").to_lowercase();
            let dest = format!("{dir}/cover.{ext}");
            if opts.skip_existing && !opts.clear && dest_root.join(&dest).is_file() {
                continue;
            }
            let bytes = std::fs::metadata(cover).map(|m| m.len()).unwrap_or(0);
            covers.push(CoverCopy { src: cover.to_string_lossy().to_string(), dest, bytes });
        }
    }

    // Space.
    let block = target.block_size;
    let writes: Vec<&PlannedTrack> = tracks.iter().filter(|t| t.action == Action::Write).collect();
    let write_bytes: u64 = writes.iter().map(|t| t.est_bytes).sum::<u64>() + covers.iter().map(|c| c.bytes).sum::<u64>();
    let mut needed: u64 = writes.iter().map(|t| cost(t.est_bytes, block)).sum::<u64>() + covers.iter().map(|c| cost(c.bytes, block)).sum::<u64>();

    // Each new folder takes a cluster or two.
    let mut new_dirs: HashSet<String> = HashSet::new();
    for t in &writes {
        let mut p = String::new();
        if let Some(parent) = Path::new(&t.dest).parent() {
            for comp in parent.components() {
                if !p.is_empty() {
                    p.push('/');
                }
                p.push_str(&comp.as_os_str().to_string_lossy());
                if !dest_root.join(&p).exists() {
                    new_dirs.insert(p.clone());
                }
            }
        }
    }
    needed += new_dirs.len() as u64 * block.max(4096);

    let clear_bytes = if opts.clear { dir_size(&dest_root) } else { 0 };
    let available = target.free_bytes + clear_bytes;
    let safety = (target.total_bytes / 200).max(32 * 1024 * 1024);
    let fits = needed + safety <= available;
    let used_now = target.total_bytes.saturating_sub(target.free_bytes);
    let used_after = used_now.saturating_sub(clear_bytes) + needed;
    let percent_after = if target.total_bytes > 0 { used_after as f32 / target.total_bytes as f32 * 100.0 } else { 0.0 };

    // What would fit? Only sizes that are smaller than the current plan are worth suggesting.
    let mut suggestions = Vec::new();
    if !scan.tracks.is_empty() {
        for s in SUGGESTED {
            let Ok(sp) = TranscodeSpec::parse(s) else { continue };
            let mut sum = 0u64;
            for t in &sorted_sources {
                let (_, est, _) = estimate(t, Some(&sp), opts.keep_art);
                sum += cost(est, block);
            }
            for c in &covers {
                sum += cost(c.bytes, block);
            }
            let already: u64 = tracks.iter().filter(|t| t.action != Action::Write).map(|_| 0u64).sum();
            let needed_s = sum + already + new_dirs.len() as u64 * block.max(4096);
            suggestions.push(Suggestion {
                transcode: (*s).to_string(),
                label: sp.label(),
                needed_bytes: needed_s,
                fits: needed_s + safety <= available,
            });
        }
    }

    let mut warnings = Vec::new();
    if unknown_len > 0 {
        warnings.push(format!("The length of {unknown_len} file(s) couldn't be read (is ffprobe installed?), so they are counted at their original size."));
    }
    let too_big: Vec<&PlannedTrack> = tracks.iter().filter(|t| t.action == Action::TooBig).collect();
    if !too_big.is_empty() {
        warnings.push(format!("{} file(s) are too big for this filesystem (FAT32 allows files up to 4 GB) and will be skipped.", too_big.len()));
    }
    let untagged = scan.tracks.iter().filter(|t| t.meta.artist.is_empty() && t.meta.album_artist.is_empty()).count();
    if untagged > 0 {
        warnings.push(format!("{untagged} file(s) have no artist tag, so they will be filed as 'Unknown Artist'."));
    }
    if opts.clear && clear_bytes > 0 {
        warnings.push(format!("Everything in {} ({:.1} GB) will be deleted first.", dest_root.display(), clear_bytes as f64 / 1e9));
    }

    Ok(StickPlan {
        dest_root: dest_root.to_string_lossy().to_string(),
        layout: opts.layout.template.clone(),
        transcode: spec.as_ref().map(|s| s.label()),
        transcode_spec: opts.transcode.clone().filter(|t| !t.trim().is_empty()),
        original_bytes: tracks.iter().map(|t| t.original_bytes).sum(),
        write_bytes,
        needed_bytes: needed,
        to_write: writes.len(),
        already_there: tracks.iter().filter(|t| t.action == Action::SkipExists).count(),
        duplicates,
        converted: writes.iter().filter(|t| t.transcode).count(),
        total_bytes: target.total_bytes,
        free_bytes: target.free_bytes,
        clear_bytes,
        available_bytes: available,
        fits,
        percent_after,
        suggestions,
        skipped: scan.skipped.iter().map(|(e, r)| serde_json::json!({"entry": e, "reason": r})).collect(),
        warnings,
        tracks,
        covers,
    })
}

/// A folder name on the stick given by the user: relative, no `..`.
pub fn sanitize_subfolder(s: &str) -> Result<PathBuf, Error> {
    let mut out = PathBuf::new();
    for part in s.split('/').map(str::trim).filter(|p| !p.is_empty()) {
        if part == ".." || part == "." {
            return Err(Error::validation("The folder on the stick can't contain . or .."));
        }
        out.push(layout::sanitize_component(part, true, 100));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stick::layout::{Layout, LayoutOptions, TrackMeta};

    fn track(artist: &str, album: &str, title: &str, n: u32, disc: u32, size: u64, secs: f64, ext: &str) -> SourceTrack {
        SourceTrack {
            path: format!("/music/{artist}/{album}/{disc}-{n:02}.{ext}"),
            size,
            meta: TrackMeta {
                artist: artist.into(), album_artist: artist.into(), album: album.into(), title: title.into(),
                track: Some(n), disc: Some(disc), ..Default::default()
            },
            duration_secs: Some(secs),
            art_bytes: 0,
            cover: None,
        }
    }

    fn target(free_mb: u64) -> TargetInfo {
        TargetInfo {
            mount_point: std::env::temp_dir().join("rd_stick_plan_nonexistent"),
            total_bytes: 64 * 1024 * 1024 * 1024,
            free_bytes: free_mb * 1024 * 1024,
            block_size: 32 * 1024,
            max_file_bytes: Some(4 * 1024 * 1024 * 1024 - 1),
        }
    }

    fn opts(template: &str) -> StickOptions {
        StickOptions { layout: Layout { template: template.into(), options: LayoutOptions::default() }, ..Default::default() }
    }

    #[test]
    fn plans_the_layout_in_sorted_order() {
        let scan = Scan {
            tracks: vec![
                track("Beta", "Two", "B2", 2, 1, 10_000_000, 240.0, "flac"),
                track("Alpha", "One", "A10", 10, 1, 10_000_000, 240.0, "flac"),
                track("Alpha", "One", "A2", 2, 1, 10_000_000, 240.0, "flac"),
            ],
            skipped: vec![],
        };
        let p = build_plan(&scan, &opts("{albumartist}/{album}/{track} - {title}"), &target(10_000)).unwrap();
        let dests: Vec<&str> = p.tracks.iter().map(|t| t.dest.as_str()).collect();
        assert_eq!(dests, ["Alpha/One/02 - A2.flac", "Alpha/One/10 - A10.flac", "Beta/Two/02 - B2.flac"]);
        assert!(p.fits && p.to_write == 3 && p.already_there == 0);
    }

    #[test]
    fn converting_shrinks_the_plan_and_changes_the_extension() {
        // 100 tracks of 5 minutes, 35 MB FLAC each
        let tracks: Vec<SourceTrack> = (1..=100).map(|i| track("A", "Album", &format!("T{i}"), i, 1, 35_000_000, 300.0, "flac")).collect();
        let scan = Scan { tracks, skipped: vec![] };
        let plain = build_plan(&scan, &opts("{albumartist}/{album}/{track} - {title}"), &target(10_000)).unwrap();
        let mut o = opts("{albumartist}/{album}/{track} - {title}");
        o.transcode = Some("mp3:320".into());
        let mp3 = build_plan(&scan, &o, &target(10_000)).unwrap();
        assert!(mp3.write_bytes * 2 < plain.write_bytes, "{} vs {}", mp3.write_bytes, plain.write_bytes);
        assert!(mp3.tracks.iter().all(|t| t.transcode && t.dest.ends_with(".mp3")));
        assert_eq!(mp3.converted, 100);
        assert_eq!(mp3.transcode.as_deref(), Some("MP3 320k"));
    }

    #[test]
    fn says_when_it_does_not_fit_and_what_would() {
        let tracks: Vec<SourceTrack> = (1..=100).map(|i| track("A", "Album", &format!("T{i}"), i, 1, 35_000_000, 300.0, "flac")).collect();
        let scan = Scan { tracks, skipped: vec![] };
        // 3.5 GB of FLAC on a stick with 2.5 GB free
        let p = build_plan(&scan, &opts("{albumartist}/{album}/{track} - {title}"), &target(2500)).unwrap();
        assert!(!p.fits);
        let mp3_256 = p.suggestions.iter().find(|s| s.transcode == "mp3:256").unwrap();
        let mp3_320 = p.suggestions.iter().find(|s| s.transcode == "mp3:320").unwrap();
        assert!(mp3_256.needed_bytes < mp3_320.needed_bytes);
        assert!(p.suggestions.iter().any(|s| s.fits), "some smaller size should fit");
        assert!(p.suggestions.iter().any(|s| !s.fits) || p.suggestions.iter().all(|s| s.fits));
    }

    #[test]
    fn multi_disc_albums_get_disc_folders() {
        let scan = Scan {
            tracks: vec![
                track("A", "Box", "d1", 1, 1, 1_000_000, 200.0, "mp3"),
                track("A", "Box", "d2", 1, 2, 1_000_000, 200.0, "mp3"),
                track("A", "Single", "s", 1, 1, 1_000_000, 200.0, "mp3"),
            ],
            skipped: vec![],
        };
        let p = build_plan(&scan, &opts("{albumartist}/{album}/{discfolder}/{track} - {title}"), &target(10_000)).unwrap();
        let dests: Vec<&str> = p.tracks.iter().map(|t| t.dest.as_str()).collect();
        assert_eq!(dests, ["A/Box/Disc 1/01 - d1.mp3", "A/Box/Disc 2/01 - d2.mp3", "A/Single/01 - s.mp3"]);
    }

    #[test]
    fn duplicates_and_name_clashes() {
        let mut a = track("A", "Album", "Song", 1, 1, 1_000_000, 200.0, "mp3");
        let mut dup = a.clone();
        dup.path = "/elsewhere/Album/01.mp3".into(); // the same track from another folder
        // a different track that would get the same name
        let mut clash = track("A", "Album", "Song", 1, 1, 2_000_000, 200.0, "mp3");
        clash.meta.track = None;
        a.meta.track = Some(1);
        let scan = Scan { tracks: vec![a, dup, clash], skipped: vec![] };
        let p = build_plan(&scan, &opts("{albumartist}/{album}/{title}"), &target(10_000)).unwrap();
        assert_eq!(p.duplicates, 1);
        assert_eq!(p.tracks.iter().filter(|t| t.action == Action::Duplicate).count(), 1);
        let writes: Vec<&str> = p.tracks.iter().filter(|t| t.action == Action::Write).map(|t| t.dest.as_str()).collect();
        assert_eq!(writes, ["A/Album/Song (2).mp3", "A/Album/Song.mp3"]);
    }

    #[test]
    fn files_over_the_fat32_limit_are_flagged() {
        let scan = Scan { tracks: vec![track("A", "Album", "Huge", 1, 1, 5_000_000_000, 3600.0, "wav")], skipped: vec![] };
        let p = build_plan(&scan, &opts("{albumartist}/{album}/{title}"), &target(10_000_000)).unwrap();
        assert_eq!(p.tracks[0].action, Action::TooBig);
        assert!(p.warnings.iter().any(|w| w.contains("FAT32")));
        assert_eq!(p.to_write, 0);
    }

    #[test]
    fn existing_files_are_skipped_and_clearing_frees_space() {
        let root = std::env::temp_dir().join(format!("rd_stick_exist_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("A/Album")).unwrap();
        std::fs::write(root.join("A/Album/01 - Song.mp3"), vec![0u8; 1_000_000]).unwrap();
        std::fs::write(root.join("A/Album/junk.bin"), vec![0u8; 5_000_000]).unwrap();
        let scan = Scan { tracks: vec![track("A", "Album", "Song", 1, 1, 1_000_000, 200.0, "mp3"), track("A", "Album", "New", 2, 1, 1_000_000, 200.0, "mp3")], skipped: vec![] };
        let t = TargetInfo { mount_point: root.clone(), total_bytes: 100_000_000, free_bytes: 10_000_000, block_size: 4096, max_file_bytes: None };
        let p = build_plan(&scan, &opts("{albumartist}/{album}/{track} - {title}"), &t).unwrap();
        assert_eq!((p.already_there, p.to_write), (1, 1));
        // clearing: nothing is skipped, and the destination's contents free their space
        let mut o = opts("{albumartist}/{album}/{track} - {title}");
        o.clear = true;
        let c = build_plan(&scan, &o, &t).unwrap();
        assert_eq!((c.already_there, c.to_write), (0, 2));
        assert!(c.clear_bytes >= 6_000_000 && c.available_bytes >= 16_000_000, "{} {}", c.clear_bytes, c.available_bytes);
        assert!(c.warnings.iter().any(|w| w.contains("deleted")));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn subfolders_are_kept_on_the_stick() {
        assert_eq!(sanitize_subfolder("Music/Rock").unwrap(), PathBuf::from("Music/Rock"));
        assert_eq!(sanitize_subfolder(" / Music: 1 /").unwrap(), PathBuf::from("Music_ 1"));
        assert!(sanitize_subfolder("../etc").is_err());
    }
}
