//! Writing the plan to the stick.

use std::{
    collections::HashMap,
    io::ErrorKind,
    path::{Path, PathBuf},
    process::Command,
    sync::{Condvar, Mutex},
    time::Instant,
};

use super::plan::{Action, StickPlan};
use crate::{
    backend::transcode::{transcode_file_art, TranscodeSpec},
    error::Error,
};

const PART: &str = ".rustydisc-part";
/// How many converted files may wait in the staging folder for the writer.
const LOOKAHEAD: usize = 6;
/// Makes each write's staging folder unique, even for two writes in one process.
static STAGE_SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub struct WriteOptions {
    pub keep_art: bool,
    pub clear: bool,
    /// Must equal the destination folder's name to allow `clear`.
    pub confirm_clear: Option<String>,
    pub dry_run: bool,
    pub debug: bool,
    pub progress_json: bool,
    pub stage_dir: Option<String>,
    /// Keep converted files here so the next job can reuse them.
    pub cache: Option<crate::backend::cache::Cache>,
}

#[derive(Debug, Default)]
pub struct Summary {
    pub written: usize,
    pub converted: usize,
    pub covers: usize,
    pub bytes: u64,
    pub skipped_existing: usize,
    pub moved: usize,
    pub replaced: usize,
    pub deleted: usize,
    pub seconds: f64,
}

fn say(opts: &WriteOptions, msg: &str) {
    if opts.progress_json {
        println!("{}", serde_json::json!({"type": "step", "msg": msg}));
    } else {
        eprintln!("{msg}");
    }
}

fn progress(opts: &WriteOptions, pct: f32) {
    if opts.progress_json {
        println!("{{\"type\":\"progress\",\"pct\":{:.1}}}", pct.min(99.0));
    }
}

fn io_error(what: &str, e: std::io::Error) -> Error {
    if e.kind() == ErrorKind::StorageFull || e.raw_os_error() == Some(28) {
        Error::device("The stick is full. Free some space, use a lower bitrate, or write fewer files.")
    } else if e.raw_os_error() == Some(30) {
        Error::device("The stick is read-only. Check its write-protect switch, or remount it read-write.")
    } else {
        Error::device(format!("{what}: {e}"))
    }
}

/// Remove files a cancelled run left half-written.
fn remove_partials(dir: &Path) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.filter_map(|e| e.ok()) {
            let p = e.path();
            if p.is_dir() {
                remove_partials(&p);
            } else if p.to_string_lossy().ends_with(PART) {
                let _ = std::fs::remove_file(p);
            }
        }
    }
}

/// Delete everything inside `dest` (not the folder itself).
fn clear_dir(dest: &Path) -> Result<usize, Error> {
    let mut failed = 0;
    for e in std::fs::read_dir(dest).map_err(|e| io_error("Can't read the destination", e))?.filter_map(|e| e.ok()) {
        let p = e.path();
        let r = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
        if r.is_err() {
            failed += 1; // e.g. a protected system folder
        }
    }
    Ok(failed)
}

enum Item<'a> {
    Track { idx: usize, plan: &'a super::plan::PlannedTrack },
    Cover(&'a super::plan::CoverCopy),
}

impl Item<'_> {
    fn dest(&self) -> &str {
        match self {
            Item::Track { plan, .. } => &plan.dest,
            Item::Cover(c) => &c.dest,
        }
    }
    fn bytes(&self) -> u64 {
        match self {
            Item::Track { plan, .. } => plan.est_bytes,
            Item::Cover(c) => c.bytes,
        }
    }
}

struct Pipeline {
    /// A converted file and whether it lives in the cache (and so must not be deleted).
    ready: Mutex<HashMap<usize, Result<(PathBuf, bool), String>>>,
    consumed: Mutex<usize>,
    cv: Condvar,
}

pub fn write(plan: &StickPlan, opts: &WriteOptions) -> Result<Summary, Error> {
    let started = Instant::now();
    let dest_root = PathBuf::from(&plan.dest_root);
    let mut summary = Summary::default();

    // Merge tracks and covers into one ordered list so the stick is written in sorted order.
    let mut items: Vec<Item> = plan
        .tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| matches!(t.action, Action::Write | Action::Replace))
        .map(|(idx, plan)| Item::Track { idx, plan })
        .chain(plan.covers.iter().map(Item::Cover))
        .collect();
    items.sort_by(|a, b| super::scan::natural_cmp(a.dest(), b.dest()));
    summary.skipped_existing = plan.already_there;

    if opts.dry_run {
        say(opts, &format!("Dry run: would write {} file(s) ({:.0} MB) to {}", items.len(), plan.write_bytes as f64 / 1e6, plan.dest_root));
        if !plan.moves.is_empty() {
            say(opts, &format!("Dry run: would move {} file(s) already on the stick", plan.moves.len()));
        }
        if !plan.deletes.is_empty() {
            say(opts, &format!("Dry run: would remove {} older copies", plan.deletes.len()));
        }
        return Ok(summary);
    }

    // Preflight: can we write here at all?
    std::fs::create_dir_all(&dest_root).map_err(|e| io_error(&format!("Can't create {}", dest_root.display()), e))?;
    let probe = dest_root.join(".rustydisc-write-test");
    std::fs::write(&probe, b"x").map_err(|e| io_error("Can't write to the stick", e))?;
    let _ = std::fs::remove_file(&probe);

    if opts.clear {
        let name = dest_root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if opts.confirm_clear.as_deref() != Some(name.as_str()) || name.is_empty() {
            return Err(Error::validation(format!(
                "To empty {} first, confirm with --confirm-clear {}",
                dest_root.display(),
                if name.is_empty() { "<name>" } else { &name }
            )));
        }
        say(opts, &format!("Emptying {}...", dest_root.display()));
        let failed = clear_dir(&dest_root)?;
        if failed > 0 {
            say(opts, &format!("{failed} item(s) couldn't be deleted (protected system folders are left alone)."));
        }
    }
    remove_partials(&dest_root);

    // Re-file what is already on the stick before anything new is written.
    if !plan.moves.is_empty() {
        say(opts, &format!("Moving {} file(s) already on the stick into the new layout...", plan.moves.len()));
        let mut old_dirs: Vec<PathBuf> = Vec::new();
        for (n, m) in plan.moves.iter().enumerate() {
            let (from, to) = (dest_root.join(&m.from), dest_root.join(&m.to));
            if !from.is_file() {
                continue;
            }
            if to.exists() {
                return Err(Error::device(format!("Can't move '{}': '{}' already exists", m.from, m.to)));
            }
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent).map_err(|e| io_error("Can't create a folder on the stick", e))?;
            }
            std::fs::rename(&from, &to).map_err(|e| io_error(&format!("Can't move '{}'", m.from), e))?;
            if m.kind == "track" {
                summary.moved += 1;
            }
            if let Some(d) = from.parent() {
                if !old_dirs.contains(&d.to_path_buf()) {
                    old_dirs.push(d.to_path_buf());
                }
            }
            if n % 25 == 0 {
                say(opts, &format!("Moved {} of {}", n + 1, plan.moves.len()));
            }
        }
        for d in old_dirs {
            prune_empty(&d, &dest_root);
        }
    }

    if items.is_empty() {
        say(opts, if summary.moved > 0 { "Nothing new to write." } else { "Nothing new to write: everything is already on the stick." });
        finish_deletes(plan, &dest_root, opts, &mut summary);
        let _ = Command::new("sync").arg("-f").arg(&dest_root).status();
        progress(opts, 99.0);
        return Ok(summary);
    }

    let total_bytes: u64 = items.iter().map(|i| i.bytes()).sum::<u64>().max(1);
    let stage = PathBuf::from(opts.stage_dir.clone().unwrap_or_else(|| "/tmp".into())).join(format!("rustydisc_stick_{}_{}", std::process::id(), STAGE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&stage);
    let convert: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, i)| matches!(i, Item::Track { plan, .. } if plan.transcode))
        .map(|(n, _)| n)
        .collect();
    if !convert.is_empty() {
        crate::backend::transcode::ensure_ffmpeg()?;
        std::fs::create_dir_all(&stage)?;
    }
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(stage.clone());

    let pipe = Pipeline { ready: Mutex::new(HashMap::new()), consumed: Mutex::new(0), cv: Condvar::new() };
    let next_job = std::sync::atomic::AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).div_ceil(2).clamp(1, 4);
    let spec: Option<TranscodeSpec> = match plan.transcode_spec.as_deref() {
        Some(s) => Some(TranscodeSpec::parse(s)?),
        None => None,
    };
    if spec.is_none() && !convert.is_empty() {
        return Err(Error::backend("Internal error: the conversion setting was lost"));
    }

    let result: Result<(), Error> = std::thread::scope(|scope| {
        // Converters run ahead of the writer, a few files at a time.
        for _ in 0..threads.min(convert.len()) {
            scope.spawn(|| loop {
                let k = next_job.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let Some(&item_no) = convert.get(k) else { break };
                {
                    // don't get too far ahead of the writer
                    let mut consumed = pipe.consumed.lock().unwrap();
                    while k >= *consumed + LOOKAHEAD {
                        consumed = pipe.cv.wait(consumed).unwrap();
                    }
                }
                let Item::Track { idx, plan: t } = &items[item_no] else { continue };
                let ext = Path::new(&t.dest).extension().and_then(|e| e.to_str()).unwrap_or("dat").to_string();
                let spec = spec.as_ref().expect("spec present");
                let art = opts.keep_art && t.art_bytes > 0;
                let key = opts.cache.as_ref().and_then(|_| crate::backend::cache::Cache::key(&t.src, plan.transcode_spec.as_deref().unwrap_or(""), art));
                let r = match (&opts.cache, key) {
                    (Some(c), Some(key)) => match c.get(&key, &ext) {
                        Some(hit) => Ok((hit, true)),
                        None => {
                            let part = c.begin(&key, &ext);
                            transcode_file_art(&t.src, &part.to_string_lossy(), spec, art, opts.debug)
                                .and_then(|_| c.commit(&part, &key, &ext).map_err(Error::from))
                                .map(|done| (done, true))
                                .map_err(|e| e.to_string())
                        }
                    },
                    _ => {
                        let out = stage.join(format!("{idx}.{ext}"));
                        transcode_file_art(&t.src, &out.to_string_lossy(), spec, art, opts.debug).map(|_| (out, false)).map_err(|e| e.to_string())
                    }
                };
                pipe.ready.lock().unwrap().insert(k, r);
                pipe.cv.notify_all();
            });
        }

        // The writer: strictly in order.
        let mut done_bytes = 0u64;
        let total_items = items.len();
        let mut convert_pos = 0usize;
        let mut result = Ok(());
        for (n, item) in items.iter().enumerate() {
            let dest = dest_root.join(item.dest());
            let name = item.dest().to_string();
            say(opts, &format!("Writing {} of {} — {}", n + 1, total_items, name));
            let r: Result<u64, Error> = (|| {
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| io_error("Can't create a folder on the stick", e))?;
                }
                let part = PathBuf::from(format!("{}{PART}", dest.to_string_lossy()));
                let written = match item {
                    Item::Track { plan: t, .. } if t.transcode => {
                        // wait for the converter to finish this file
                        let k = convert_pos;
                        convert_pos += 1;
                        let converted = {
                            let mut ready = pipe.ready.lock().unwrap();
                            loop {
                                if let Some(r) = ready.remove(&k) {
                                    break r;
                                }
                                ready = pipe.cv.wait(ready).unwrap();
                            }
                        };
                        {
                            *pipe.consumed.lock().unwrap() = k + 1;
                            pipe.cv.notify_all();
                        }
                        let (staged, cached) = converted.map_err(Error::backend)?;
                        let len = copy_file(&staged, &part)?;
                        if !cached {
                            let _ = std::fs::remove_file(&staged);
                        }
                        summary.converted += 1;
                        len
                    }
                    Item::Track { plan: t, .. } => {
                        let len = copy_file(Path::new(&t.src), &part)?;
                        if len != t.original_bytes {
                            return Err(Error::device(format!("'{}' changed while it was being copied", t.src)));
                        }
                        len
                    }
                    Item::Cover(c) => copy_file(Path::new(&c.src), &part)?,
                };
                std::fs::rename(&part, &dest).map_err(|e| io_error("Can't finish a file on the stick", e))?;
                if matches!(item, Item::Cover(_)) {
                    summary.covers += 1;
                } else {
                    summary.written += 1;
                    if matches!(item, Item::Track { plan: t, .. } if t.action == Action::Replace) {
                        summary.replaced += 1;
                    }
                }
                Ok(written)
            })();
            match r {
                Ok(len) => {
                    summary.bytes += len;
                    done_bytes += item.bytes();
                    progress(opts, done_bytes as f32 / total_bytes as f32 * 98.0);
                }
                Err(e) => {
                    result = Err(e);
                    break;
                }
            }
        }
        // Let any waiting converter finish so the scope can end.
        if result.is_err() {
            next_job.store(usize::MAX / 2, std::sync::atomic::Ordering::SeqCst);
            *pipe.consumed.lock().unwrap() = usize::MAX / 2;
            pipe.cv.notify_all();
        }
        result
    });
    result?;

    finish_deletes(plan, &dest_root, opts, &mut summary);
    if let Some(c) = &opts.cache {
        let p = c.prune();
        if p.removed > 0 {
            say(opts, &format!("Cleared {} old converted file(s) from the cache ({:.0} MB).", p.removed, p.freed_bytes as f64 / 1e6));
        }
    }
    say(opts, "Flushing to the stick — wait for this before pulling it out...");
    let _ = Command::new("sync").arg("-f").arg(&dest_root).status();
    remove_partials(&dest_root);
    progress(opts, 99.0);
    summary.seconds = started.elapsed().as_secs_f64();
    Ok(summary)
}

/// Remove a folder if nothing but cover pictures are left in it, then its parents likewise.
fn prune_empty(dir: &Path, root: &Path) {
    let mut d = dir.to_path_buf();
    while d.starts_with(root) && d != root {
        let Ok(rd) = std::fs::read_dir(&d) else { return };
        let left: Vec<_> = rd.filter_map(|e| e.ok()).collect();
        if !left.is_empty() {
            return;
        }
        if std::fs::remove_dir(&d).is_err() {
            return;
        }
        match d.parent() {
            Some(p) => d = p.to_path_buf(),
            None => return,
        }
    }
}

/// Delete the older copies that a written file replaced (at another path).
fn finish_deletes(plan: &StickPlan, dest_root: &Path, opts: &WriteOptions, summary: &mut Summary) {
    if plan.deletes.is_empty() {
        return;
    }
    say(opts, &format!("Removing {} older copies...", plan.deletes.len()));
    for rel in &plan.deletes {
        let p = dest_root.join(rel);
        if p.is_file() && std::fs::remove_file(&p).is_ok() {
            summary.deleted += 1;
            if let Some(d) = p.parent() {
                prune_empty(d, dest_root);
            }
        }
    }
}

fn copy_file(src: &Path, dst: &Path) -> Result<u64, Error> {
    std::fs::copy(src, dst).map_err(|e| {
        let _ = std::fs::remove_file(dst);
        io_error(&format!("Can't copy '{}'", src.display()), e)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stick::{
        layout::{Layout, LayoutOptions, TrackMeta},
        plan::{build_plan, StickOptions, TargetInfo},
        scan::{Scan, SourceTrack},
    };

    fn have_ffmpeg() -> bool {
        Command::new("ffmpeg").arg("-version").output().is_ok()
    }

    fn wav(path: &Path, freq: u32, secs: u32) {
        let ok = Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", &format!("sine=frequency={freq}:duration={secs}"), "-ar", "44100", "-ac", "2"])
            .arg(path)
            .status()
            .unwrap()
            .success();
        assert!(ok);
    }

    fn src(path: &Path, artist: &str, album: &str, title: &str, n: u32, secs: f64) -> SourceTrack {
        SourceTrack {
            path: path.to_string_lossy().to_string(),
            size: std::fs::metadata(path).unwrap().len(),
            meta: TrackMeta { artist: artist.into(), album_artist: artist.into(), album: album.into(), title: title.into(), track: Some(n), disc: Some(1), ..Default::default() },
            duration_secs: Some(secs),
            art_bytes: 0,
            cover: None,
        }
    }

    fn quiet() -> WriteOptions {
        WriteOptions { keep_art: false, clear: false, confirm_clear: None, dry_run: false, debug: false, progress_json: false, stage_dir: None, cache: None }
    }

    /// Copy and convert real (tiny) files onto a "stick" and check where everything lands.
    #[test]
    fn writes_copies_and_conversions_in_the_planned_layout() {
        if !have_ffmpeg() {
            eprintln!("skipping: ffmpeg not installed");
            return;
        }
        let root = std::env::temp_dir().join(format!("rd_stick_write_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (music, stick) = (root.join("music"), root.join("stick"));
        std::fs::create_dir_all(&music).unwrap();
        std::fs::create_dir_all(&stick).unwrap();
        let mut tracks = Vec::new();
        for (i, (artist, title)) in [("The Band", "One"), ("The Band", "Two"), ("Zed", "Three")].iter().enumerate() {
            let p = music.join(format!("{i}.wav"));
            wav(&p, 300 + i as u32 * 100, 3);
            tracks.push(src(&p, artist, "Album", title, i as u32 + 1, 3.0));
        }
        let scan = Scan { tracks, skipped: vec![] };
        let target = TargetInfo { mount_point: stick.clone(), total_bytes: 1 << 30, free_bytes: 1 << 30, block_size: 4096, max_file_bytes: None };

        // 1. plain copy
        let mut o = StickOptions::default();
        o.layout = Layout { template: "{initial}/{albumartist}/{album}/{track} - {title}".into(), options: LayoutOptions::default() };
        let plan = build_plan(&scan, &[], &o, &target).unwrap();
        let s = write(&plan, &quiet()).unwrap();
        assert_eq!((s.written, s.converted), (3, 0));
        assert!(stick.join("B/The Band/Album/01 - One.wav").is_file());
        assert!(stick.join("Z/Zed/Album/03 - Three.wav").is_file());
        assert!(!stick.join("B/The Band/Album/01 - One.wav.rustydisc-part").exists());

        // 2. a second run finds everything already there
        let again = build_plan(&scan, &crate::stick::existing::read_existing(&stick), &o, &target).unwrap();
        assert_eq!((again.already_there, again.to_write), (3, 0));
        assert_eq!(write(&again, &quiet()).unwrap().written, 0);

        // 3. converting to mp3 into a fresh folder, in order, with a progress-free run
        let mut c = o.clone();
        c.transcode = Some("mp3:128".into());
        c.dest_subfolder = "Converted".into();
        let plan = build_plan(&scan, &[], &c, &target).unwrap();
        assert_eq!(plan.converted, 3);
        let s = write(&plan, &quiet()).unwrap();
        assert_eq!((s.written, s.converted), (3, 3));
        let out = stick.join("Converted/B/The Band/Album/02 - Two.mp3");
        assert!(out.is_file() && std::fs::metadata(&out).unwrap().len() > 1000);

        // 4. emptying needs the right confirmation and removes the old content
        let mut e = o.clone();
        e.clear = true;
        let plan = build_plan(&scan, &[], &e, &target).unwrap();
        let mut wo = quiet();
        wo.clear = true;
        assert!(write(&plan, &wo).unwrap_err().to_string().contains("confirm"));
        wo.confirm_clear = Some("stick".into());
        std::fs::write(stick.join("old.txt"), b"old").unwrap();
        write(&plan, &wo).unwrap();
        assert!(!stick.join("old.txt").exists() && !stick.join("Converted").exists());
        assert!(stick.join("B/The Band/Album/01 - One.wav").is_file());
        std::fs::remove_dir_all(&root).ok();
    }

    /// Re-file what is on the stick, and replace an old copy with a better one.
    #[test]
    fn reorganizes_existing_music_and_replaces_by_quality() {
        use crate::stick::existing::{read_existing, Conflict, ExistingMode};
        if !have_ffmpeg() {
            eprintln!("skipping: ffmpeg not installed");
            return;
        }
        let root = std::env::temp_dir().join(format!("rd_stick_reorg_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (music, stick) = (root.join("music"), root.join("stick"));
        std::fs::create_dir_all(&music).unwrap();
        std::fs::create_dir_all(&stick).unwrap();
        let mp3 = |path: &Path, kbps: u32, secs: u32| {
            assert!(Command::new("ffmpeg")
                .args(["-v", "error", "-y", "-f", "lavfi", "-i", &format!("sine=frequency=440:duration={secs}"), "-b:a", &format!("{kbps}k"), "-metadata", "artist=Band", "-metadata", "album=Rec", "-metadata", "title=Song", "-metadata", "track=1"])
                .arg(path).status().unwrap().success());
        };
        // the stick has a low-quality copy filed as Artist/Album/Song
        std::fs::create_dir_all(stick.join("Old/Stuff")).unwrap();
        mp3(&stick.join("Old/Stuff/song.mp3"), 64, 20);
        std::fs::write(stick.join("Old/Stuff/cover.jpg"), b"jpg").unwrap();
        // a better copy is on the computer
        let better = music.join("song.mp3");
        mp3(&better, 256, 20);
        let mut t = src(&better, "Band", "Rec", "Song", 1, 20.0);
        t.size = std::fs::metadata(&better).unwrap().len();
        let scan = Scan { tracks: vec![t], skipped: vec![] };
        let target = TargetInfo { mount_point: stick.clone(), total_bytes: 1 << 30, free_bytes: 1 << 30, block_size: 4096, max_file_bytes: None };
        let mut o = StickOptions::default();
        o.layout = Layout { template: "{albumartist}/{album}/{track} - {title}".into(), options: LayoutOptions::default() };
        o.existing_mode = ExistingMode::Reorganize;
        o.conflict = Conflict::HigherQuality;

        let on_stick = read_existing(&stick);
        assert_eq!(on_stick.len(), 1);
        let plan = build_plan(&scan, &on_stick, &o, &target).unwrap();
        assert_eq!(plan.moved, 1, "{:?}", plan.moves);
        assert_eq!(plan.moves.iter().filter(|m| m.kind == "cover").count(), 1);
        assert_eq!(plan.replacing, 1);
        assert!(plan.tracks[0].note.as_deref().unwrap_or("").contains("Better quality"));
        assert!(plan.freed_bytes > 0);
        let s = write(&plan, &quiet()).unwrap();
        assert_eq!((s.moved, s.replaced, s.written), (1, 1, 1));
        assert!(stick.join("Band/Rec/01 - Song.mp3").is_file());
        assert!(stick.join("Band/Rec/cover.jpg").is_file(), "the cover moves with the album");
        assert!(!stick.join("Old").exists(), "emptied folders are removed");
        let now = std::fs::metadata(stick.join("Band/Rec/01 - Song.mp3")).unwrap().len();
        assert_eq!(now, std::fs::metadata(&better).unwrap().len(), "the better copy is what is there now");

        // now the stick's copy is better than a lower-quality candidate: it is kept
        let worse = music.join("worse.mp3");
        mp3(&worse, 64, 20);
        let mut w = src(&worse, "Band", "Rec", "Song", 1, 20.0);
        w.size = std::fs::metadata(&worse).unwrap().len();
        let scan2 = Scan { tracks: vec![w.clone()], skipped: vec![] };
        let on_stick = read_existing(&stick);
        let kept = build_plan(&scan2, &on_stick, &o, &target).unwrap();
        assert_eq!((kept.already_there, kept.to_write), (1, 0));
        // ...unless we ask for lower quality (to save space), or keep both
        o.conflict = Conflict::LowerQuality;
        assert_eq!(build_plan(&scan2, &on_stick, &o, &target).unwrap().replacing, 1);
        o.conflict = Conflict::KeepBoth;
        let both = build_plan(&scan2, &on_stick, &o, &target).unwrap();
        assert_eq!(both.tracks[0].dest, "Band/Rec/01 - Song (2).mp3");
        std::fs::remove_dir_all(&root).ok();
    }

    /// With a cache, converted files are kept and the next job reuses them instead of converting again.
    #[test]
    fn converted_files_are_kept_and_reused_when_a_cache_is_set() {
        use crate::backend::cache::{stats, Cache, CacheArgs};
        if !have_ffmpeg() {
            eprintln!("skipping: ffmpeg not installed");
            return;
        }
        let root = std::env::temp_dir().join(format!("rd_stick_cache_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (music, stick, cache_dir) = (root.join("music"), root.join("stick"), root.join("cache"));
        std::fs::create_dir_all(&music).unwrap();
        std::fs::create_dir_all(&stick).unwrap();
        let p = music.join("a.wav");
        wav(&p, 440, 2);
        let scan = Scan { tracks: vec![src(&p, "Band", "Rec", "Song", 1, 2.0)], skipped: vec![] };
        let target = TargetInfo { mount_point: stick.clone(), total_bytes: 1 << 30, free_bytes: 1 << 30, block_size: 4096, max_file_bytes: None };
        let mut o = StickOptions::default();
        o.layout = Layout { template: "{albumartist}/{title}".into(), options: LayoutOptions::default() };
        o.transcode = Some("mp3:128".into());
        let cached = || WriteOptions { cache: Cache::from_args(&CacheArgs { convert_cache: Some(cache_dir.to_string_lossy().to_string()), ..Default::default() }), ..quiet() };

        o.dest_subfolder = "One".into();
        write(&build_plan(&scan, &[], &o, &target).unwrap(), &cached()).unwrap();
        assert_eq!(stats(&cache_dir).files, 1, "the converted file is kept");
        assert!(stick.join("One/Band/Song.mp3").is_file());

        // Mark the kept file: if the second job reuses it, the marker shows up on the stick.
        let kept = std::fs::read_dir(&cache_dir).unwrap().next().unwrap().unwrap().path();
        std::fs::write(&kept, b"REUSED").unwrap();
        o.dest_subfolder = "Two".into();
        write(&build_plan(&scan, &[], &o, &target).unwrap(), &cached()).unwrap();
        assert_eq!(std::fs::read(stick.join("Two/Band/Song.mp3")).unwrap(), b"REUSED");
        assert_eq!(stats(&cache_dir).files, 1);

        // Without a cache nothing is kept.
        std::fs::remove_dir_all(&cache_dir).unwrap();
        o.dest_subfolder = "Three".into();
        write(&build_plan(&scan, &[], &o, &target).unwrap(), &quiet()).unwrap();
        assert!(!cache_dir.exists());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_dry_run_writes_nothing() {
        let plan = StickPlan {
            dest_root: "/definitely/not/here".into(), layout: "x".into(), transcode: None, transcode_spec: None, tracks: vec![], covers: vec![], moves: vec![], deletes: vec![], conflict: "skip".into(), replacing: 0, moved: 0, freed_bytes: 0,
            original_bytes: 0, write_bytes: 0, needed_bytes: 0, to_write: 0, already_there: 0, duplicates: 0, converted: 0, total_bytes: 0,
            free_bytes: 0, clear_bytes: 0, available_bytes: 0, fits: true, percent_after: 0.0, suggestions: vec![], skipped: vec![], warnings: vec![],
        };
        let mut o = quiet();
        o.dry_run = true;
        write(&plan, &o).unwrap();
        assert!(!Path::new("/definitely/not/here").exists());
    }
}
