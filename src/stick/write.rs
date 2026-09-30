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

pub struct WriteOptions {
    pub keep_art: bool,
    pub clear: bool,
    /// Must equal the destination folder's name to allow `clear`.
    pub confirm_clear: Option<String>,
    pub dry_run: bool,
    pub debug: bool,
    pub progress_json: bool,
    pub stage_dir: Option<String>,
}

#[derive(Debug, Default)]
pub struct Summary {
    pub written: usize,
    pub converted: usize,
    pub covers: usize,
    pub bytes: u64,
    pub skipped_existing: usize,
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
    ready: Mutex<HashMap<usize, Result<PathBuf, String>>>,
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
        .filter(|(_, t)| t.action == Action::Write)
        .map(|(idx, plan)| Item::Track { idx, plan })
        .chain(plan.covers.iter().map(Item::Cover))
        .collect();
    items.sort_by(|a, b| super::scan::natural_cmp(a.dest(), b.dest()));
    summary.skipped_existing = plan.already_there;

    if opts.dry_run {
        say(opts, &format!("Dry run: would write {} file(s) ({:.0} MB) to {}", items.len(), plan.write_bytes as f64 / 1e6, plan.dest_root));
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

    if items.is_empty() {
        say(opts, "Nothing new to write: everything is already on the stick.");
        progress(opts, 99.0);
        return Ok(summary);
    }

    let total_bytes: u64 = items.iter().map(|i| i.bytes()).sum::<u64>().max(1);
    let stage = PathBuf::from(opts.stage_dir.clone().unwrap_or_else(|| "/tmp".into())).join(format!("rustydisc_stick_{}", std::process::id()));
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
                let out = stage.join(format!("{idx}.{}", Path::new(&t.dest).extension().and_then(|e| e.to_str()).unwrap_or("dat")));
                let r = transcode_file_art(&t.src, &out.to_string_lossy(), spec.as_ref().expect("spec present"), opts.keep_art && t.art_bytes > 0, opts.debug)
                    .map(|_| out)
                    .map_err(|e| e.to_string());
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
                        let staged = converted.map_err(Error::backend)?;
                        let len = copy_file(&staged, &part)?;
                        let _ = std::fs::remove_file(&staged);
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

    say(opts, "Flushing to the stick — wait for this before pulling it out...");
    let _ = Command::new("sync").arg("-f").arg(&dest_root).status();
    remove_partials(&dest_root);
    progress(opts, 99.0);
    summary.seconds = started.elapsed().as_secs_f64();
    Ok(summary)
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
        WriteOptions { keep_art: false, clear: false, confirm_clear: None, dry_run: false, debug: false, progress_json: false, stage_dir: None }
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
        let plan = build_plan(&scan, &o, &target).unwrap();
        let s = write(&plan, &quiet()).unwrap();
        assert_eq!((s.written, s.converted), (3, 0));
        assert!(stick.join("B/The Band/Album/01 - One.wav").is_file());
        assert!(stick.join("Z/Zed/Album/03 - Three.wav").is_file());
        assert!(!stick.join("B/The Band/Album/01 - One.wav.rustydisc-part").exists());

        // 2. a second run finds everything already there
        let again = build_plan(&scan, &o, &target).unwrap();
        assert_eq!((again.already_there, again.to_write), (3, 0));
        assert_eq!(write(&again, &quiet()).unwrap().written, 0);

        // 3. converting to mp3 into a fresh folder, in order, with a progress-free run
        let mut c = o.clone();
        c.transcode = Some("mp3:128".into());
        c.dest_subfolder = "Converted".into();
        let plan = build_plan(&scan, &c, &target).unwrap();
        assert_eq!(plan.converted, 3);
        let s = write(&plan, &quiet()).unwrap();
        assert_eq!((s.written, s.converted), (3, 3));
        let out = stick.join("Converted/B/The Band/Album/02 - Two.mp3");
        assert!(out.is_file() && std::fs::metadata(&out).unwrap().len() > 1000);

        // 4. emptying needs the right confirmation and removes the old content
        let mut e = o.clone();
        e.clear = true;
        let plan = build_plan(&scan, &e, &target).unwrap();
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

    #[test]
    fn a_dry_run_writes_nothing() {
        let plan = StickPlan {
            dest_root: "/definitely/not/here".into(), layout: "x".into(), transcode: None, transcode_spec: None, tracks: vec![], covers: vec![],
            original_bytes: 0, write_bytes: 0, needed_bytes: 0, to_write: 0, already_there: 0, duplicates: 0, converted: 0, total_bytes: 0,
            free_bytes: 0, clear_bytes: 0, available_bytes: 0, fits: true, percent_after: 0.0, suggestions: vec![], skipped: vec![], warnings: vec![],
        };
        let mut o = quiet();
        o.dry_run = true;
        write(&plan, &o).unwrap();
        assert!(!Path::new("/definitely/not/here").exists());
    }
}
