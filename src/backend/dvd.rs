//! DVD and Blu-ray discs.
//!
//! * A **Data DVD** is an ISO 9660 + Joliet + UDF image of a folder, written in one go.
//! * A **Music DVD** is a DVD-Video disc: every track becomes a chapter that plays in any DVD
//!   player (Dolby Digital audio over a still picture), and an optional data folder sits in
//!   the disc root next to `VIDEO_TS`, so one disc holds both the music and the files.
//!
//! Images are built with genisoimage (xorriso can't make UDF or DVD-Video layouts) and written
//! with xorriso, which handles DVD±R, DVD±R DL and BD-R alike. The media in the drive is checked
//! with xorriso too: CD tools like `cdrecord -atip` don't understand DVDs.

use std::{
    io::Write as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
};

use super::data::{
    disc_capacity, drain_with_progress, emit_progress, emit_step, parse_xorriso_pct,
    stdbuf_cmd, write_iso_image,
};
use crate::{
    error::Error,
    model::disc::{DataSession, DvdOptions, VideoStandard},
};

// ── Saving the image instead of burning ──────────────────────────────────────

static ISO_OUT: Mutex<Option<String>> = Mutex::new(None);

/// Save the finished disc image to this file instead of touching a drive.
pub fn set_iso_out(path: Option<String>) {
    *ISO_OUT.lock().unwrap() = path;
}

pub fn iso_out() -> Option<String> {
    ISO_OUT.lock().unwrap().clone()
}

// ── Media in the drive ────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum MediaStatus {
    Blank,
    /// Written, but more can be added.
    Appendable,
    /// Written and closed.
    Closed,
    NoMedia,
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Media {
    /// e.g. "DVD+R", "DVD-RW sequential", "BD-R", "CD-R"
    pub profile: String,
    pub status: MediaStatus,
    /// Room left on the disc, when the drive reports it.
    pub free_bytes: Option<u64>,
}

/// Read what `xorriso -outdev DEV -toc` says about the disc.
pub fn parse_media_report(text: &str) -> Media {
    let mut profile = String::new();
    let mut status = MediaStatus::Unknown;
    let mut free_bytes = None;
    let mut saw_status = false;

    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Media current:") {
            profile = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("Media status :") {
            saw_status = true;
            let r = rest.to_lowercase();
            status = if r.contains("is blank") {
                MediaStatus::Blank
            } else if r.contains("is appendable") {
                MediaStatus::Appendable
            } else if r.contains("is closed") {
                MediaStatus::Closed
            } else if r.contains("not present") || r.contains("no medium") {
                MediaStatus::NoMedia
            } else {
                MediaStatus::Unknown
            };
        } else if line.starts_with("Media summary:") {
            // "0 sessions, 0 data blocks, 0 data, 4482m free"
            if let Some(idx) = line.rfind(" free") {
                let head = &line[..idx];
                if let Some(tok) = head.split(|c: char| c == ',' || c.is_whitespace()).filter(|t| !t.is_empty()).last() {
                    free_bytes = parse_size(tok);
                }
            }
        }
    }
    let lower = text.to_lowercase();
    if !saw_status && (lower.contains("no medium") || lower.contains("not present") || lower.contains("cannot acquire drive")) {
        status = MediaStatus::NoMedia;
    }
    Media { profile, status, free_bytes }
}

/// xorriso sizes: a number with an optional k, m, g or t suffix (powers of 1024).
fn parse_size(tok: &str) -> Option<u64> {
    let (num, mult) = match tok.chars().last()? {
        'k' | 'K' => (&tok[..tok.len() - 1], 1024u64),
        'm' | 'M' => (&tok[..tok.len() - 1], 1024 * 1024),
        'g' | 'G' => (&tok[..tok.len() - 1], 1024 * 1024 * 1024),
        't' | 'T' => (&tok[..tok.len() - 1], 1024u64.pow(4)),
        c if c.is_ascii_digit() => (tok, 1),
        _ => return None,
    };
    let n: f64 = num.parse().ok()?;
    Some((n * mult as f64) as u64)
}

pub fn query_media(device: &str) -> Result<Media, Error> {
    let out = Command::new("xorriso")
        .args(["-outdev", device, "-toc"])
        .output()
        .map_err(|e| Error::device(format!("Could not run xorriso to look at the disc: {e}")))?;
    let text = format!("{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    Ok(parse_media_report(&text))
}

/// Refuse anything but a blank DVD/BD that has room for the image.
pub fn check_media(device: &str, media: &Media, image_bytes: Option<u64>) -> Result<(), Error> {
    if media.profile.to_uppercase().starts_with("CD") {
        return Err(Error::device(format!(
            "A CD ({}) is in {} but this job is for a DVD or Blu-ray disc. Insert a blank DVD.",
            media.profile, device
        )));
    }
    match media.status {
        MediaStatus::Blank => {}
        MediaStatus::NoMedia => {
            return Err(Error::device(format!("No disc found in {}. Insert a blank DVD or Blu-ray disc.", device)));
        }
        MediaStatus::Appendable | MediaStatus::Closed => {
            return Err(Error::device(format!(
                "The {} in {} already has data on it. Insert a blank disc (a rewritable disc must be erased first).",
                if media.profile.is_empty() { "disc" } else { &media.profile },
                device
            )));
        }
        MediaStatus::Unknown => {
            eprintln!("Warning: could not tell whether the disc in {} is blank; trying anyway.", device);
        }
    }
    if let (Some(free), Some(need)) = (media.free_bytes, image_bytes) {
        if need > free {
            return Err(Error::validation(format!(
                "The disc image is {:.0} MB but the {} in {} has only {:.0} MB free. Use a bigger disc or split the files across more discs.",
                need as f64 / 1_048_576.0,
                if media.profile.is_empty() { "disc" } else { &media.profile },
                device,
                free as f64 / 1_048_576.0
            )));
        }
    }
    Ok(())
}

// ── Tools ─────────────────────────────────────────────────────────────────────

fn have(tool: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|d| d.join(tool).is_file()))
        .unwrap_or(false)
}

/// The programs a DVD job needs that aren't installed.
pub fn missing_tools(music: bool) -> Vec<String> {
    let mut missing = Vec::new();
    if !have("genisoimage") && !have("mkisofs") {
        missing.push("genisoimage (sudo apt install genisoimage)".to_string());
    }
    if !have("xorriso") {
        missing.push("xorriso (sudo apt install xorriso)".to_string());
    }
    if music {
        if !have("ffmpeg") {
            missing.push("ffmpeg (sudo apt install ffmpeg)".to_string());
        }
        if !have("dvdauthor") {
            missing.push("dvdauthor (sudo apt install dvdauthor)".to_string());
        }
    }
    missing
}

fn iso_tool() -> &'static str {
    if have("genisoimage") { "genisoimage" } else { "mkisofs" }
}

// ── Building the image ────────────────────────────────────────────────────────

fn volume_label(label: &str) -> String {
    label.chars().take(32).map(|c| c.to_ascii_uppercase()).collect()
}

/// Build an ISO 9660 + Joliet + UDF image of `source_dir`. `dvd_video` lays the disc out as
/// DVD-Video (`VIDEO_TS` first). Progress runs from `from_pct` to `to_pct`.
pub fn build_iso(
    source_dir: &str,
    label: &str,
    dvd_video: bool,
    iso_path: &str,
    debug: bool,
    progress_json: bool,
    from_pct: f32,
    to_pct: f32,
) -> Result<(), Error> {
    let mut cmd = stdbuf_cmd(iso_tool());
    cmd.arg("-V").arg(volume_label(label))
        .args(["-r", "-J", "-udf", "-iso-level", "3", "-allow-limited-size", "-f"]);
    if dvd_video {
        cmd.arg("-dvd-video");
    }
    cmd.arg("-o").arg(iso_path).arg(source_dir);
    if debug { eprintln!("Running: {:?}", cmd); }

    if progress_json { emit_step("Building the disc image..."); }
    cmd.stdout(Stdio::null()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| Error::backend(format!("Could not start {}: {e}", iso_tool())))?;
    let mut stderr_bytes = Vec::new();
    if let Some(mut stderr) = child.stderr.take() {
        drain_with_progress(&mut stderr, &mut stderr_bytes, |line| {
            if progress_json {
                if let Some(pct) = parse_xorriso_pct(line) {
                    emit_progress(from_pct + pct * (to_pct - from_pct) / 100.0);
                }
            }
        });
    }
    let status = child.wait()?;
    if !status.success() {
        let _ = std::fs::remove_file(iso_path);
        let msg = String::from_utf8_lossy(&stderr_bytes);
        let detail: Vec<&str> = msg.lines().filter(|l| !l.contains("% done")).rev().take(4).collect();
        return Err(Error::backend(format!(
            "{} failed (exit {:?}): {}",
            iso_tool(),
            status.code(),
            detail.into_iter().rev().collect::<Vec<_>>().join("; ")
        )));
    }
    Ok(())
}

/// Write the image to the disc, or save it if `--iso-out` was given.
fn finish_image(
    iso_path: &str,
    device: &str,
    media_needed: bool,
    debug: bool,
    progress_json: bool,
    write_from_pct: f32,
) -> Result<(), Error> {
    let bytes = std::fs::metadata(iso_path)?.len();

    if let Some(out) = iso_out() {
        std::fs::rename(iso_path, &out).or_else(|_| std::fs::copy(iso_path, &out).map(|_| ()).and_then(|_| std::fs::remove_file(iso_path)))?;
        if progress_json {
            emit_step(&format!("Saved the disc image: {} ({:.0} MB)", out, bytes as f64 / 1_048_576.0));
        } else {
            eprintln!("Saved the disc image: {} ({:.0} MB)", out, bytes as f64 / 1_048_576.0);
        }
        return Ok(());
    }

    if bytes > disc_capacity() {
        let _ = std::fs::remove_file(iso_path);
        return Err(Error::validation(format!(
            "The disc image is {:.1} MB, which is more than a {:.0} MB disc holds. Split the files across more discs.",
            bytes as f64 / 1_048_576.0,
            disc_capacity() as f64 / 1_048_576.0,
        )));
    }
    if media_needed {
        let media = query_media(device)?;
        if let Err(e) = check_media(device, &media, Some(bytes)) {
            let _ = std::fs::remove_file(iso_path);
            return Err(e);
        }
    }
    write_iso_image(iso_path, device, true, false, debug, progress_json, write_from_pct)
}

/// Is this job going to a file rather than a drive?
pub fn writing_to_file() -> bool {
    iso_out().is_some()
}

// ── Data DVD ──────────────────────────────────────────────────────────────────

pub fn burn_data_dvd(
    session: &DataSession,
    device: &str,
    label: &str,
    debug: bool,
    progress_json: bool,
) -> Result<(), Error> {
    let missing = missing_tools(false);
    if !missing.is_empty() {
        return Err(Error::validation(format!("Missing required tools:\n  - {}", missing.join("\n  - "))));
    }
    if !writing_to_file() {
        // Fail before spending minutes building an image for the wrong disc.
        let media = query_media(device)?;
        check_media(device, &media, None)?;
    }
    let iso = format!("/tmp/rustydisc_dvd_{}.iso", std::process::id());
    build_iso(&session.source_dir, label, false, &iso, debug, progress_json, 0.0, 45.0)?;
    finish_image(&iso, device, true, debug, progress_json, 45.0)
}

// ── Music DVD ─────────────────────────────────────────────────────────────────

struct Geometry {
    width: u32,
    height: u32,
    /// Frame rate as ffmpeg writes it
    fps: &'static str,
    gop: u32,
    /// Square-pixel canvas the still is fitted into before being squeezed to the DVD frame
    canvas: (u32, u32),
    sar: &'static str,
    dvdauthor_format: &'static str,
}

fn geometry(standard: VideoStandard) -> Geometry {
    match standard {
        VideoStandard::Pal => Geometry { width: 720, height: 576, fps: "25", gop: 12, canvas: (768, 576), sar: "16/15", dvdauthor_format: "pal" },
        VideoStandard::Ntsc => Geometry { width: 720, height: 480, fps: "30000/1001", gop: 15, canvas: (640, 480), sar: "8/9", dvdauthor_format: "ntsc" },
    }
}

fn run_ffmpeg(args: &[String], what: &str, debug: bool) -> Result<(), Error> {
    if debug { eprintln!("Running: ffmpeg {}", args.join(" ")); }
    let out = Command::new("ffmpeg")
        .args(args)
        .output()
        .map_err(|e| Error::backend(format!("Could not run ffmpeg: {e}")))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let tail: Vec<&str> = err.lines().rev().take(3).collect();
        return Err(Error::backend(format!(
            "ffmpeg failed {}: {}",
            what,
            tail.into_iter().rev().collect::<Vec<_>>().join("; ")
        )));
    }
    Ok(())
}

/// The picture shown while the music plays, at DVD size: the chosen image, else cover art next
/// to the tracks, else a plain dark background.
fn prepare_still(opts: &DvdOptions, first_track: &str, work: &Path, geo: &Geometry, debug: bool) -> Result<PathBuf, Error> {
    let out = work.join("still.png");
    let source: Option<PathBuf> = opts.still.as_ref().map(PathBuf::from).or_else(|| find_cover(first_track));

    let args: Vec<String> = match source {
        Some(src) => {
            let (cw, ch) = geo.canvas;
            let vf = format!(
                "scale={cw}:{ch}:force_original_aspect_ratio=decrease,pad={cw}:{ch}:(ow-iw)/2:(oh-ih)/2:color=black,scale={}:{},setsar={}",
                geo.width, geo.height, geo.sar
            );
            vec!["-v".into(), "error".into(), "-y".into(), "-i".into(), src.to_string_lossy().to_string(), "-vf".into(), vf, "-frames:v".into(), "1".into(), out.to_string_lossy().to_string()]
        }
        None => vec![
            "-v".into(), "error".into(), "-y".into(), "-f".into(), "lavfi".into(), "-i".into(),
            format!("color=c=0x1b1d24:s={}x{}", geo.width, geo.height),
            "-frames:v".into(), "1".into(), out.to_string_lossy().to_string(),
        ],
    };
    run_ffmpeg(&args, "preparing the picture", debug)?;
    Ok(out)
}

/// Cover art next to a track (or one folder up, for `Album/CD 1/track.flac`), whatever the case
/// of its name (`cover.jpg`, `Folder.JPG`, ...).
pub fn find_cover(track: &str) -> Option<PathBuf> {
    let dir = Path::new(track).parent()?;
    let names = ["cover", "folder", "front", "albumart", "album"];
    for d in [Some(dir), dir.parent()].into_iter().flatten() {
        let mut found: Vec<(usize, PathBuf)> = Vec::new();
        for entry in std::fs::read_dir(d).ok()?.filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let stem = path.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
            let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            if let (Some(rank), true) = (names.iter().position(|n| *n == stem), ["jpg", "jpeg", "png"].contains(&ext.as_str())) {
                found.push((rank, path));
            }
        }
        found.sort();
        if let Some((_, p)) = found.into_iter().next() {
            return Some(p);
        }
    }
    None
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Most chapters a DVD-Video title can have.
const MAX_CHAPTERS: usize = 99;

/// The dvdauthor description of the disc: one title with a chapter per track (more titles, played
/// one after the other, past 99 tracks), starting to play as soon as the disc is inserted.
pub fn dvdauthor_xml(dest: &str, mpgs: &[String], standard: VideoStandard) -> String {
    let geo = geometry(standard);
    let fmt = geo.dvdauthor_format;
    let mut pgcs = String::new();
    let groups: Vec<&[String]> = mpgs.chunks(MAX_CHAPTERS).collect();
    for (i, group) in groups.iter().enumerate() {
        pgcs.push_str("      <pgc>\n");
        for f in *group {
            pgcs.push_str(&format!("        <vob file=\"{}\" chapters=\"0\"/>\n", xml_escape(f)));
        }
        if i + 1 < groups.len() {
            pgcs.push_str(&format!("        <post>jump title {};</post>\n", i + 2));
        }
        pgcs.push_str("      </pgc>\n");
    }
    format!(
        "<dvdauthor dest=\"{dest}\">\n  <vmgm>\n    <fpc>jump title 1;</fpc>\n    <menus><video format=\"{fmt}\" aspect=\"4:3\"/></menus>\n  </vmgm>\n  <titleset>\n    <titles>\n      <video format=\"{fmt}\" aspect=\"4:3\"/>\n      <audio format=\"ac3\" channels=\"2\"/>\n{pgcs}    </titles>\n  </titleset>\n</dvdauthor>\n",
        dest = xml_escape(dest),
    )
}

/// Encode each track as a DVD-compliant clip: the still picture with the audio as Dolby Digital.
fn encode_tracks(
    tracks: &[String],
    still: &Path,
    opts: &DvdOptions,
    geo: &Geometry,
    work: &Path,
    debug: bool,
    progress_json: bool,
) -> Result<Vec<String>, Error> {
    let total = tracks.len();
    let outputs: Vec<String> = (0..total).map(|i| work.join(format!("track{:03}.mpg", i + 1)).to_string_lossy().to_string()).collect();
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let failure: Mutex<Option<Error>> = Mutex::new(None);
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).clamp(1, 4);

    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                if i >= total || failure.lock().unwrap().is_some() {
                    break;
                }
                let args: Vec<String> = [
                    "-v", "error", "-y", "-loop", "1", "-framerate", geo.fps, "-i", &still.to_string_lossy(),
                    "-i", &tracks[i], "-map", "0:v:0", "-map", "1:a:0",
                    "-c:v", "mpeg2video", "-b:v", "300k", "-maxrate", "8000k", "-minrate", "0", "-bufsize", "1835008",
                    "-g", &geo.gop.to_string(), "-r", geo.fps, "-s", &format!("{}x{}", geo.width, geo.height),
                    "-aspect", "4:3", "-pix_fmt", "yuv420p",
                    "-c:a", "ac3", "-b:a", &format!("{}k", opts.audio_kbps), "-ar", "48000", "-ac", "2",
                    "-shortest", "-f", "dvd", &outputs[i],
                ]
                .iter()
                .map(|s| s.to_string())
                .collect();
                match run_ffmpeg(&args, &format!("encoding '{}'", tracks[i]), debug) {
                    Ok(()) => {
                        let n = done.fetch_add(1, Ordering::SeqCst) + 1;
                        let name = Path::new(&tracks[i]).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                        if progress_json {
                            emit_step(&format!("Encoded track {} of {} — {}", n, total, name));
                            emit_progress(n as f32 / total as f32 * 35.0);
                        } else {
                            eprintln!("Encoded track {} of {} — {}", n, total, name);
                        }
                    }
                    Err(e) => {
                        *failure.lock().unwrap() = Some(e);
                    }
                }
            });
        }
    });

    match failure.into_inner().unwrap() {
        Some(e) => Err(e),
        None => Ok(outputs),
    }
}

/// Author the VIDEO_TS folder for `tracks` into `stage`.
pub fn author_video_ts(
    tracks: &[String],
    opts: &DvdOptions,
    stage: &Path,
    work: &Path,
    debug: bool,
    progress_json: bool,
) -> Result<(), Error> {
    let geo = geometry(opts.standard);
    if progress_json { emit_step("Preparing the picture..."); }
    let still = prepare_still(opts, &tracks[0], work, &geo, debug)?;

    if progress_json { emit_step(&format!("Encoding {} track(s) for DVD...", tracks.len())); }
    let mpgs = encode_tracks(tracks, &still, opts, &geo, work, debug, progress_json)?;

    if progress_json { emit_step("Authoring the DVD-Video structure..."); emit_progress(36.0); }
    let xml = dvdauthor_xml(&stage.to_string_lossy(), &mpgs, opts.standard);
    let xml_path = work.join("dvd.xml");
    std::fs::write(&xml_path, xml)?;

    let mut cmd = Command::new("dvdauthor");
    cmd.arg("-x").arg(&xml_path).env("VIDEO_TS_DIR", stage);
    if debug { eprintln!("Running: {:?}", cmd); }
    let out = cmd.output().map_err(|e| Error::backend(format!("Could not run dvdauthor: {e}")))?;
    let log = String::from_utf8_lossy(&out.stderr);
    let errors: Vec<&str> = log.lines().filter(|l| l.starts_with("ERR:")).collect();
    if !out.status.success() || !errors.is_empty() {
        return Err(Error::backend(format!(
            "dvdauthor failed: {}",
            if errors.is_empty() { format!("exit {:?}", out.status.code()) } else { errors.join("; ") }
        )));
    }
    if !stage.join("VIDEO_TS").join("VIDEO_TS.IFO").exists() {
        return Err(Error::backend("dvdauthor did not produce a VIDEO_TS folder"));
    }
    let _ = std::fs::create_dir_all(stage.join("AUDIO_TS"));
    for f in std::fs::read_dir(work)? {
        let f = f?;
        if f.path().extension().map(|e| e == "mpg").unwrap_or(false) {
            let _ = std::fs::remove_file(f.path()); // free the disk space as soon as authoring is done
        }
    }
    Ok(())
}

/// Put everything in `data_dir` in the disc root, next to VIDEO_TS (as links; the image follows them).
fn link_data(data_dir: &str, stage: &Path) -> Result<(), Error> {
    for entry in std::fs::read_dir(data_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let upper = name.to_string_lossy().to_uppercase();
        if upper == "VIDEO_TS" || upper == "AUDIO_TS" {
            return Err(Error::validation(format!(
                "The data folder contains '{}', which clashes with the DVD-Video folders. Rename or remove it.",
                name.to_string_lossy()
            )));
        }
        let dest = stage.join(&name);
        let source = std::fs::canonicalize(entry.path())?;
        std::os::unix::fs::symlink(&source, &dest)?;
    }
    Ok(())
}

pub fn burn_music_dvd(
    tracks: &[String],
    data_dir: Option<&str>,
    opts: &DvdOptions,
    device: &str,
    label: &str,
    debug: bool,
    progress_json: bool,
) -> Result<(), Error> {
    let missing = missing_tools(true);
    if !missing.is_empty() {
        return Err(Error::validation(format!("Missing required tools:\n  - {}", missing.join("\n  - "))));
    }
    opts.validate().map_err(Error::validation)?;
    if tracks.is_empty() {
        return Err(Error::validation("A Music DVD needs at least one audio track"));
    }
    if !writing_to_file() {
        let media = query_media(device)?;
        check_media(device, &media, None)?;
    }

    let root = PathBuf::from(format!("/tmp/rustydisc_musicdvd_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let stage = root.join("disc");
    let work = root.join("work");
    std::fs::create_dir_all(&stage)?;
    std::fs::create_dir_all(&work)?;
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());

    author_video_ts(tracks, opts, &stage, &work, debug, progress_json)?;
    if let Some(dir) = data_dir {
        link_data(dir, &stage)?;
    }

    let iso = format!("/tmp/rustydisc_musicdvd_{}.iso", std::process::id());
    build_iso(&stage.to_string_lossy(), label, true, &iso, debug, progress_json, 40.0, 60.0)?;
    finish_image(&iso, device, true, debug, progress_json, 60.0)?;
    let _ = std::io::stdout().flush();
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const BLANK_DVD: &str = "xorriso 1.5.6 : RockRidge filesystem manipulator, libburnia project.\n\nDrive current: -outdev '/dev/sr0'\nMedia current: DVD+R\nMedia status : is blank\nMedia summary: 0 sessions, 0 data blocks, 0 data, 4482m free\n";
    const USED_DVD: &str = "Media current: DVD-R sequential recording\nMedia status : is written , is appendable\nMedia summary: 1 session, 100 data blocks, 200k data, 4000m free\n";
    const CLOSED: &str = "Media current: DVD-ROM\nMedia status : is written , is closed\nMedia summary: 1 session, 10 data blocks, 20k data, 0k free\n";
    const CD: &str = "Media current: CD-R\nMedia status : is blank\nMedia summary: 0 sessions, 0 data blocks, 0 data, 702m free\n";

    #[test]
    fn reads_the_media_report() {
        let m = parse_media_report(BLANK_DVD);
        assert_eq!(m.profile, "DVD+R");
        assert_eq!(m.status, MediaStatus::Blank);
        assert_eq!(m.free_bytes, Some(4482 * 1024 * 1024));

        let m = parse_media_report(USED_DVD);
        assert_eq!(m.status, MediaStatus::Appendable);
        assert_eq!(m.free_bytes, Some(4000 * 1024 * 1024));
        assert_eq!(parse_media_report(CLOSED).status, MediaStatus::Closed);
        assert_eq!(parse_media_report("xorriso : FAILURE : Cannot acquire drive '/dev/sr0'\nno medium present").status, MediaStatus::NoMedia);
        assert_eq!(parse_size("46g"), Some(46 * 1024 * 1024 * 1024));
        assert_eq!(parse_size("bogus"), None);
    }

    #[test]
    fn only_blank_dvds_with_room_are_accepted() {
        let blank = parse_media_report(BLANK_DVD);
        assert!(check_media("/dev/sr0", &blank, Some(4_000_000_000)).is_ok());
        let too_big = check_media("/dev/sr0", &blank, Some(5_000_000_000)).unwrap_err().to_string();
        assert!(too_big.contains("free"), "{too_big}");
        assert!(check_media("/dev/sr0", &parse_media_report(USED_DVD), None).unwrap_err().to_string().contains("already has data"));
        assert!(check_media("/dev/sr0", &parse_media_report(CD), None).unwrap_err().to_string().contains("CD"));
        assert!(check_media("/dev/sr0", &parse_media_report("no medium present"), None).is_err());
    }

    #[test]
    fn dvdauthor_xml_has_a_chapter_per_track_and_plays_straight_away() {
        let mpgs: Vec<String> = (1..=3).map(|i| format!("/w/t{i}.mpg")).collect();
        let xml = dvdauthor_xml("/w/out", &mpgs, VideoStandard::Pal);
        assert!(xml.contains("<fpc>jump title 1;</fpc>"));
        assert_eq!(xml.matches("<vob ").count(), 3);
        assert_eq!(xml.matches("<pgc>").count(), 1);
        assert!(xml.contains("format=\"pal\"") && xml.contains("format=\"ac3\""));
        let ntsc = dvdauthor_xml("/w/out", &mpgs, VideoStandard::Ntsc);
        assert!(ntsc.contains("format=\"ntsc\""));
        // paths are escaped
        assert!(dvdauthor_xml("/w", &["/a&b/\"c\".mpg".into()], VideoStandard::Pal).contains("/a&amp;b/&quot;c&quot;.mpg"));
    }

    #[test]
    fn more_than_99_tracks_become_more_titles_that_play_on() {
        let mpgs: Vec<String> = (1..=150).map(|i| format!("/w/t{i}.mpg")).collect();
        let xml = dvdauthor_xml("/w/out", &mpgs, VideoStandard::Pal);
        assert_eq!(xml.matches("<pgc>").count(), 2);
        assert_eq!(xml.matches("<vob ").count(), 150);
        assert!(xml.contains("<post>jump title 2;</post>"));
        assert_eq!(xml.matches("<post>").count(), 1); // the last title just ends
    }

    #[test]
    fn finds_cover_art_beside_the_tracks_or_one_folder_up() {
        let root = std::env::temp_dir().join(format!("rd_cover_{}", std::process::id()));
        std::fs::create_dir_all(root.join("Album/CD 1")).unwrap();
        std::fs::write(root.join("Album/Folder.JPG"), b"x").unwrap(); // any case
        std::fs::write(root.join("Album/notes.txt"), b"x").unwrap();
        let track = root.join("Album/CD 1/01.flac").to_string_lossy().to_string();
        assert_eq!(find_cover(&track), Some(root.join("Album/Folder.JPG")));
        std::fs::write(root.join("Album/CD 1/cover.png"), b"x").unwrap();
        assert_eq!(find_cover(&track), Some(root.join("Album/CD 1/cover.png")));
        std::fs::remove_dir_all(&root).ok();
    }

    /// Author a real (tiny) Music DVD and check its structure. Skipped when the tools aren't installed.
    #[test]
    fn authors_a_music_dvd_image() {
        if !missing_tools(true).is_empty() || !have("isoinfo") {
            eprintln!("skipping: dvdauthor/ffmpeg/genisoimage/isoinfo not installed");
            return;
        }
        let root = std::env::temp_dir().join(format!("rd_musicdvd_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (stage, work, music, data) = (root.join("stage"), root.join("work"), root.join("music"), root.join("data"));
        for d in [&stage, &work, &music, &data] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(data.join("readme.txt"), b"hello").unwrap();
        let mut tracks = Vec::new();
        for i in 1..=3 {
            let path = music.join(format!("{i}.wav")).to_string_lossy().to_string();
            let ok = Command::new("ffmpeg")
                .args(["-v", "error", "-y", "-f", "lavfi", "-i", &format!("sine=frequency={}:duration=2", 300 + i * 100), "-ar", "44100", "-ac", "2", &path])
                .status()
                .unwrap()
                .success();
            assert!(ok);
            tracks.push(path);
        }
        author_video_ts(&tracks, &DvdOptions::default(), &stage, &work, false, false).unwrap();
        assert!(stage.join("VIDEO_TS/VIDEO_TS.IFO").exists() && stage.join("VIDEO_TS/VTS_01_1.VOB").exists());
        assert!(stage.join("AUDIO_TS").is_dir());
        link_data(&data.to_string_lossy(), &stage).unwrap();

        let iso = root.join("out.iso").to_string_lossy().to_string();
        build_iso(&stage.to_string_lossy(), "Test", true, &iso, false, false, 0.0, 100.0).unwrap();
        let listing = Command::new("isoinfo").args(["-l", "-i", &iso]).output().unwrap();
        let text = String::from_utf8_lossy(&listing.stdout).to_string();
        for want in ["VIDEO_TS.IFO", "VTS_01_1.VOB", "README.TXT"] {
            assert!(text.contains(want), "{want} missing from:\n{text}");
        }
        // a data folder can't shadow the DVD-Video folders
        std::fs::create_dir_all(data.join("video_ts")).unwrap();
        let clash = link_data(&data.to_string_lossy(), &root.join("stage2").tap_create()).unwrap_err().to_string();
        assert!(clash.contains("clashes"), "{clash}");
        std::fs::remove_dir_all(&root).ok();
    }

    trait TapCreate {
        fn tap_create(self) -> PathBuf;
    }
    impl TapCreate for PathBuf {
        fn tap_create(self) -> PathBuf {
            std::fs::create_dir_all(&self).unwrap();
            self
        }
    }
}

