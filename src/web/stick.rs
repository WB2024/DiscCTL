//! Rusty Stick endpoints: finding sticks, planning what goes on them, and writing.

use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    extract::State,
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{path_str, safe_join, start_job, spawn_cli, ApiError, ApiResult, AppState, S};
use crate::{
    error::Error,
    stick::{
        devices::{self, Target},
        layout::{self, Layout, LayoutOptions},
        existing::{self, Conflict, ExistingMode, ExistingTrack},
        format as stick_format,
        plan::{build_plan, sanitize_subfolder, StickOptions, TargetInfo},
        scan::{self, Scan, SourceSpec},
    },
};

/// Folders (beyond auto-detected USB sticks) that may be written to.
pub fn extra_folders(st: &AppState) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = st.cfg.stick_dirs.clone();
    for f in st.settings.get().stick_extra_folders {
        let p = PathBuf::from(f.trim());
        if p.is_absolute() && !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

fn mock_targets(st: &AppState) -> Vec<Target> {
    let base = st.cfg.config_dir.join("mock-usb");
    [("SANDISK_ULTRA", "SanDisk", "Ultra Fit", 64_000_000_000u64, "exfat", 131072u64, "/dev/sdz1"), ("KINGSTON_8GB", "Kingston", "DataTraveler", 8_000_000_000, "vfat", 32768, "/dev/sdy1")]
        .iter()
        .map(|(name, vendor, model, total, fs, block, dev)| {
            let dir = base.join(name);
            let _ = std::fs::create_dir_all(&dir);
            // Reformatting a mock stick just remembers the new filesystem and name.
            let fs = std::fs::read_to_string(base.join(format!(".fs-{name}"))).map(|s| s.trim().to_string()).unwrap_or_else(|_| fs.to_string());
            let label = std::fs::read_to_string(base.join(format!(".label-{name}"))).map(|s| s.trim().to_string()).unwrap_or_else(|_| name.to_string());
            let block = &(if fs == "vfat" { 32768u64 } else { *block });
            let used = crate::stick::plan::dir_size(&dir);
            Target {
                id: path_str(&dir),
                mount_point: path_str(&dir),
                label,
                device: Some(dev.to_string()),
                fs_type: fs.clone(),
                total_bytes: *total,
                free_bytes: total.saturating_sub(used),
                block_size: *block,
                read_only: false,
                kind: "usb".into(),
                vendor: vendor.to_string(),
                model: model.to_string(),
                max_file_bytes: if fs == "vfat" { Some(4 * 1024 * 1024 * 1024 - 1) } else { None },
                windows_names: true,
            }
        })
        .collect()
}

fn all_targets(st: &AppState) -> Vec<Target> {
    if st.cfg.mock {
        mock_targets(st)
    } else {
        devices::list_targets(&extra_folders(st))
    }
}

pub async fn targets(State(st): S) -> ApiResult<Json<Value>> {
    let st2 = st.clone();
    let (targets, unmounted) = tokio::task::spawn_blocking(move || {
        let unmounted = if st2.cfg.mock {
            vec![devices::Unmounted { device: "/dev/sdz2".into(), label: "BACKUP_STICK".into(), size_bytes: 16_000_000_000, fs_type: "vfat".into(), model: "Mock Drive".into() }]
        } else {
            devices::list_unmounted()
        };
        (all_targets(&st2), unmounted)
    })
    .await
    .map_err(|e| Error::backend(e.to_string()))?;
    Ok(Json(json!({
        "targets": targets,
        "unmounted": unmounted,
        "presets": layout::PRESETS.iter().map(|p| json!({"id": p.id, "label": p.label, "template": p.template})).collect::<Vec<_>>(),
        "tokens": layout::TOKENS.iter().map(|(n, d)| json!({"name": n, "description": d})).collect::<Vec<_>>(),
        "extra_folders": extra_folders(&st).iter().map(|p| path_str(p)).collect::<Vec<_>>(),
        "filesystems": stick_format::choices(),
        "conflicts": [
            {"id": "skip", "label": "Skip files that are already there", "note": "Leave what is on the stick alone"},
            {"id": "higher_quality", "label": "Replace if the new file is higher quality", "note": "e.g. FLAC over MP3, or 320k over 128k"},
            {"id": "lower_quality", "label": "Replace if the new file is lower quality", "note": "Shrinks the stick's copy to save space"},
            {"id": "newer", "label": "Replace if the new file is newer", "note": "By modification time"},
            {"id": "replace", "label": "Always replace", "note": "Overwrite every file that is already there"},
            {"id": "keep_both", "label": "Keep both", "note": "Write the new file next to the old one"}
        ],
        "suggested_transcodes": ["", "mp3:320", "mp3:256", "mp3:192", "mp3:128", "aac:256", "opus:128", "flac"],
    })))
}

#[derive(Deserialize)]
#[serde(default)]
pub struct StickReq {
    /// Mount point of the stick, as listed by /api/stick/targets.
    target: String,
    /// Sources, relative to the media directory.
    folders: Vec<String>,
    files: Vec<String>,
    playlists: Vec<String>,
    layout: Option<String>,
    preset: Option<String>,
    transcode: Option<String>,
    keep_art: bool,
    copy_covers: bool,
    /// skip | replace | higher_quality | lower_quality | newer | keep_both
    conflict: Option<String>,
    /// leave | reorganize
    existing: Option<String>,
    clear: bool,
    confirm_clear: Option<String>,
    subfolder: Option<String>,
    /// "auto", "on" or "off"
    windows_names: Option<String>,
    keep_the: bool,
    dry_run: bool,
    debug: bool,
    force: bool,
}

impl Default for StickReq {
    fn default() -> Self {
        StickReq {
            target: String::new(), folders: vec![], files: vec![], playlists: vec![], layout: None, preset: None, transcode: None,
            keep_art: true, copy_covers: true, conflict: None, existing: None, clear: false, confirm_clear: None, subfolder: None,
            windows_names: None, keep_the: false, dry_run: false, debug: false, force: false,
        }
    }
}

struct Resolved {
    target: Target,
    sources: SourceSpec,
    options: StickOptions,
}

fn resolve(st: &AppState, req: &StickReq) -> ApiResult<Resolved> {
    let target = all_targets(st)
        .into_iter()
        .find(|t| t.mount_point == req.target)
        .ok_or_else(|| ApiError::not_found("That stick isn't available (unplug and re-plug it, or refresh the list)"))?;
    if target.read_only {
        return Err(ApiError::bad(format!("{} is mounted read-only", target.label)));
    }

    let media = &st.cfg.media_dir;
    let join = |v: &[String]| -> ApiResult<Vec<String>> { v.iter().map(|f| safe_join(media, f).map(|p| path_str(&p))).collect() };
    let sources = SourceSpec {
        folders: join(&req.folders)?,
        files: join(&req.files)?,
        playlists: join(&req.playlists)?,
        playlist_root: Some(media.clone()),
    };
    if sources.is_empty() {
        return Err(ApiError::bad("Choose what to put on the stick: a folder, some files or a playlist"));
    }

    let template = match (&req.layout, &req.preset) {
        (Some(t), _) if !t.trim().is_empty() => t.trim().to_string(),
        (_, Some(id)) => layout::PRESETS
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.template.to_string())
            .ok_or_else(|| ApiError::bad(format!("Unknown layout preset '{id}'")))?,
        _ => layout::PRESETS[0].template.to_string(),
    };
    layout::validate_template(&template)?;
    let windows_safe = match req.windows_names.as_deref() {
        Some("on") => true,
        Some("off") => false,
        _ => target.windows_names,
    };
    let options = StickOptions {
        layout: Layout { template, options: LayoutOptions { windows_safe, ignore_the: !req.keep_the } },
        transcode: req.transcode.clone().filter(|t| !t.trim().is_empty()),
        keep_art: req.keep_art,
        copy_covers: req.copy_covers,
        conflict: req.conflict.as_deref().unwrap_or("").parse::<Conflict>().map_err(ApiError::bad)?,
        existing_mode: if req.clear { ExistingMode::Leave } else { req.existing.as_deref().unwrap_or("").parse::<ExistingMode>().map_err(ApiError::bad)? },
        clear: req.clear,
        dest_subfolder: req.subfolder.clone().unwrap_or_default(),
    };
    Ok(Resolved { target, sources, options })
}

fn cache_key(spec: &SourceSpec) -> String {
    format!("{:?}|{:?}|{:?}", spec.folders, spec.files, spec.playlists)
}

/// Reading tags for a big library takes a moment, so a scan is remembered briefly: changing the
/// layout or the conversion then re-plans instantly.
async fn cached_scan(st: &Arc<AppState>, spec: &SourceSpec) -> ApiResult<Arc<Scan>> {
    let key = cache_key(spec);
    if let Some((k, at, scan)) = st.stick_scan.lock().unwrap().as_ref() {
        if *k == key && at.elapsed() < Duration::from_secs(180) {
            return Ok(scan.clone());
        }
    }
    let spec2 = spec.clone();
    let scanned = tokio::task::spawn_blocking(move || scan::scan(&spec2))
        .await
        .map_err(|e| Error::backend(e.to_string()))??;
    let scanned = Arc::new(scanned);
    *st.stick_scan.lock().unwrap() = Some((key, Instant::now(), scanned.clone()));
    Ok(scanned)
}

/// Reading the tags of everything on a stick takes a moment, so the result is kept until the
/// stick's contents change.
async fn cached_existing(st: &Arc<AppState>, root: PathBuf) -> Arc<Vec<ExistingTrack>> {
    let key = path_str(&root);
    let root2 = root.clone();
    let fp = tokio::task::spawn_blocking(move || existing::fingerprint(&root2)).await.unwrap_or((0, 0, 0));
    if let Some((f, tracks)) = st.stick_existing.lock().unwrap().get(&key) {
        if *f == fp {
            return tracks.clone();
        }
    }
    let tracks = Arc::new(tokio::task::spawn_blocking(move || existing::read_existing(&root)).await.unwrap_or_default());
    let mut cache = st.stick_existing.lock().unwrap();
    if cache.len() > 8 {
        cache.clear();
    }
    cache.insert(key, (fp, tracks.clone()));
    tracks
}

pub async fn plan(State(st): S, Json(req): Json<StickReq>) -> ApiResult<Json<Value>> {
    let r = resolve(&st, &req)?;
    let scanned = cached_scan(&st, &r.sources).await?;
    let info = TargetInfo {
        mount_point: PathBuf::from(&r.target.mount_point),
        total_bytes: r.target.total_bytes,
        free_bytes: r.target.free_bytes,
        block_size: r.target.block_size,
        max_file_bytes: r.target.max_file_bytes,
    };
    let opts = r.options.clone();
    let dest_root = if opts.dest_subfolder.trim().is_empty() { info.mount_point.clone() } else { info.mount_point.join(sanitize_subfolder(&opts.dest_subfolder)?) };
    let on_stick = if opts.clear { Arc::new(Vec::new()) } else { cached_existing(&st, dest_root).await };
    let plan = tokio::task::spawn_blocking(move || build_plan(&scanned, &on_stick, &opts, &info))
        .await
        .map_err(|e| Error::backend(e.to_string()))??;
    Ok(Json(json!({"plan": plan, "target": r.target})))
}

#[derive(Deserialize)]
pub struct MountReq {
    device: String,
}

pub async fn mount(State(st): S, Json(req): Json<MountReq>) -> ApiResult<Json<Value>> {
    if st.cfg.mock {
        return Err(ApiError::bad("Mock mode: nothing to mount"));
    }
    let dev = req.device.clone();
    let mp = tokio::task::spawn_blocking(move || devices::mount_device(&dev))
        .await
        .map_err(|e| Error::backend(e.to_string()))??;
    Ok(Json(json!({"mount_point": mp})))
}

#[derive(Deserialize)]
pub struct EjectReq {
    target: String,
}

pub async fn eject(State(st): S, Json(req): Json<EjectReq>) -> ApiResult<Json<Value>> {
    let target = all_targets(&st)
        .into_iter()
        .find(|t| t.mount_point == req.target)
        .ok_or_else(|| ApiError::not_found("That stick isn't available"))?;
    if st.jobs.all().iter().any(|j| j.kind == "stick" && !j.status().is_terminal()) {
        return Err(ApiError::bad("A stick job is still running"));
    }
    if st.cfg.mock {
        return Ok(Json(json!({"ok": true})));
    }
    tokio::task::spawn_blocking(move || devices::eject(&target))
        .await
        .map_err(|e| Error::backend(e.to_string()))??;
    Ok(Json(json!({"ok": true})))
}

pub async fn start(State(st): S, Json(req): Json<StickReq>) -> ApiResult<Json<Value>> {
    let r = resolve(&st, &req)?;
    layout::validate_template(&r.options.layout.template)?;
    if st.jobs.all().iter().any(|j| j.kind == "stick" && !j.status().is_terminal()) {
        return Err(ApiError::bad("A stick job is already running"));
    }

    let mut a: Vec<String> = vec!["stick".into(), "--target".into(), r.target.mount_point.clone(), "--progress-json".into()];
    for f in &r.sources.folders { a.extend(["--folder".into(), f.clone()]); }
    for f in &r.sources.files { a.extend(["--file".into(), f.clone()]); }
    for f in &r.sources.playlists { a.extend(["--playlist".into(), f.clone()]); }
    a.extend(["--playlist-root".into(), path_str(&st.cfg.media_dir)]);
    a.extend(["--layout".into(), r.options.layout.template.clone()]);
    a.extend(["--windows-names".into(), if r.options.layout.options.windows_safe { "on" } else { "off" }.into()]);
    if let Some(t) = &r.options.transcode { a.extend(["--transcode".into(), t.clone()]); }
    if !r.options.keep_art { a.push("--no-keep-art".into()); }
    if !r.options.copy_covers { a.push("--no-covers".into()); }
    a.extend(["--on-conflict".into(), r.options.conflict.id().into()]);
    if r.options.existing_mode == ExistingMode::Reorganize { a.extend(["--existing".into(), "reorganize".into()]); }
    if req.keep_the { a.push("--keep-the".into()); }
    if !r.options.dest_subfolder.trim().is_empty() {
        a.extend(["--subfolder".into(), r.options.dest_subfolder.clone()]);
    }
    if r.options.clear {
        // Emptying a stick is destructive: the folder's name has to be typed to confirm.
        let dest_name = if r.options.dest_subfolder.trim().is_empty() {
            PathBuf::from(&r.target.mount_point).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
        } else {
            sanitize_subfolder(&r.options.dest_subfolder)?.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
        };
        if req.confirm_clear.as_deref() != Some(dest_name.as_str()) || dest_name.is_empty() {
            return Err(ApiError::bad(format!("Type “{dest_name}” to confirm emptying the stick first")));
        }
        a.extend(["--clear".into(), "--confirm-clear".into(), dest_name]);
    }
    if req.dry_run { a.push("--dry-run".into()); }
    if req.debug { a.push("--debug".into()); }
    if req.force { a.push("--force".into()); }
    if st.cfg.mock {
        // The mock sticks are ordinary folders; make them behave like their pretend capacity.
        a.extend(["--assume-capacity".into(), r.target.free_bytes.to_string()]);
    }

    let job = start_job(&st, "stick", &format!("Rusty Stick → {}", r.target.label), false)?;
    spawn_cli(&st, job.clone(), a, None);
    Ok(Json(json!(job.summary())))
}

// ── Identify, reformat ───────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct IdentifyReq {
    target: String,
}

fn fs_name(fs: &str) -> &'static str {
    match fs {
        "vfat" | "msdos" => "FAT32",
        "exfat" => "exFAT",
        "ntfs" | "ntfs3" => "NTFS",
        "fuseblk" => "NTFS / exFAT (FUSE)",
        "ext4" => "ext4",
        "ext3" => "ext3",
        "ext2" => "ext2",
        "btrfs" => "Btrfs",
        "xfs" => "XFS",
        "hfsplus" => "HFS+",
        "apfs" => "APFS",
        _ => "unknown",
    }
}

fn fs_limits(t: &Target) -> Vec<String> {
    let mut v = Vec::new();
    match t.fs_type.as_str() {
        "vfat" | "msdos" => {
            v.push("Files can be at most 4 GB.".to_string());
            v.push("Names can't contain \\ / : * ? \" < > | (they are cleaned up for you).".to_string());
            v.push("The most widely compatible choice, including older car stereos and TVs.".to_string());
        }
        "exfat" => {
            v.push("No practical file-size limit.".to_string());
            v.push("Readable by Windows, macOS, Linux and most modern players and cars.".to_string());
        }
        "ntfs" | "ntfs3" | "fuseblk" => v.push("Windows and Linux can write to it; macOS and most players and cars only read it, or can't at all.".to_string()),
        "ext2" | "ext3" | "ext4" | "btrfs" | "xfs" => v.push("A Linux filesystem: Windows, macOS, most players and cars can't read it.".to_string()),
        "" => v.push("The filesystem couldn't be recognised.".to_string()),
        _ => {}
    }
    if t.read_only {
        v.push("Mounted read-only, so nothing can be written until it is remounted.".to_string());
    }
    v
}

pub async fn identify(State(st): S, Json(req): Json<IdentifyReq>) -> ApiResult<Json<Value>> {
    let target = all_targets(&st)
        .into_iter()
        .find(|t| t.mount_point == req.target)
        .ok_or_else(|| ApiError::not_found("That stick isn't available (refresh the list)"))?;
    let root = PathBuf::from(&target.mount_point);
    let tracks = cached_existing(&st, root.clone()).await;
    let (t2, tracks2, mock) = (target.clone(), tracks.clone(), st.cfg.mock);
    let (content, detected, facts) = tokio::task::spawn_blocking(move || {
        let content = existing::summarize(&root, &tracks2);
        let detected = existing::detect_layout(&tracks2);
        let facts = match (&t2.device, mock) {
            (Some(d), false) => devices::device_facts(d),
            _ => devices::DeviceFacts { fs_type: t2.fs_type.clone(), label: t2.label.clone(), ..Default::default() },
        };
        (content, detected, facts)
    })
    .await
    .map_err(|e| Error::backend(e.to_string()))?;

    let (can_format, why_not) = match (&target.device, target.kind.as_str()) {
        (Some(_), "usb") => (true, String::new()),
        (_, "folder") => (false, "This is a folder, not a whole stick, so it can't be reformatted here.".to_string()),
        _ => (false, "The stick's device isn't known, so it can't be reformatted here.".to_string()),
    };
    Ok(Json(json!({
        "target": target,
        "fs_name": fs_name(&target.fs_type),
        "facts": facts,
        "limits": fs_limits(&target),
        "content": content,
        "detected_layout": detected,
        "can_format": can_format,
        "format_blocked": why_not,
        "filesystems": stick_format::choices(),
    })))
}

#[derive(Deserialize)]
pub struct FormatReq {
    /// The partition, e.g. /dev/sdb1 (from the stick listing).
    device: String,
    fs: String,
    label: String,
    /// Must be the device's short name, e.g. "sdb1".
    confirm: String,
}

pub async fn format(State(st): S, Json(req): Json<FormatReq>) -> ApiResult<Json<Value>> {
    if st.jobs.all().iter().any(|j| j.kind == "stick" && !j.status().is_terminal()) {
        return Err(ApiError::bad("A stick job is running; wait for it to finish first"));
    }
    let short = req.device.rsplit('/').next().unwrap_or("").to_string();
    if req.confirm.trim() != short {
        return Err(ApiError::bad(format!("Type “{short}” to confirm erasing this stick")));
    }
    let label = stick_format::clean_label(&req.fs, &req.label)?;

    if st.cfg.mock {
        let base = st.cfg.config_dir.join("mock-usb");
        let target = mock_targets(&st).into_iter().find(|t| t.device.as_deref() == Some(req.device.as_str())).ok_or_else(|| ApiError::not_found("No such stick"))?;
        let name = std::path::Path::new(&target.mount_point).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let dir = PathBuf::from(&target.mount_point);
        for e in std::fs::read_dir(&dir).map_err(Error::from)?.filter_map(|e| e.ok()) {
            let p = e.path();
            let _ = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
        }
        let fs = if req.fs == "fat32" { "vfat" } else { req.fs.as_str() };
        std::fs::write(base.join(format!(".fs-{name}")), fs).map_err(Error::from)?;
        std::fs::write(base.join(format!(".label-{name}")), if label.is_empty() { name.as_str() } else { label.as_str() }).map_err(Error::from)?;
        return Ok(Json(json!({"ok": true, "mount_point": target.mount_point})));
    }

    // Only devices this program itself lists as sticks can be formatted.
    let dev = req.device.clone();
    let known = tokio::task::spawn_blocking(move || {
        let mut all: Vec<String> = devices::list_targets(&[]).into_iter().filter(|t| t.kind == "usb").filter_map(|t| t.device).collect();
        all.extend(devices::list_unmounted().into_iter().map(|u| u.device));
        all.contains(&dev)
    })
    .await
    .map_err(|e| Error::backend(e.to_string()))?;
    if !known {
        return Err(ApiError::bad("That isn't one of the USB sticks that were found"));
    }
    let (device, fs, confirm) = (req.device.clone(), req.fs.clone(), req.confirm.clone());
    tokio::task::spawn_blocking(move || stick_format::format_device(&device, &fs, &label, &confirm))
        .await
        .map_err(|e| Error::backend(e.to_string()))??;
    // Mount the fresh, empty stick again so it can be used straight away.
    let device = req.device.clone();
    let mounted = tokio::task::spawn_blocking(move || devices::mount_device(&device).ok()).await.ok().flatten();
    Ok(Json(json!({"ok": true, "mount_point": mounted})))
}
