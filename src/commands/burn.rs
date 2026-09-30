use std::{
    collections::{HashSet, VecDeque},
    io::Write as _,
    path::{Path, PathBuf},
};
use clap::Args;
use crate::{
    backend,
    backend::transcode::{StagedDir, TranscodeSpec},
    error::Error,
    parser,
    planner,
    planner::{
        discs::{self, DataSpec, DiscSize, PlannedFile},
        split::{self, AudioItem, AudioSlice, DataItem, AUDIO_MAX_TRACKS},
    },
};

#[derive(Args, Debug)]
pub struct BurnArgs {
    /// Disc format: redbook, datacd, bluebook
    #[arg(long, default_value = "redbook")]
    pub format: String,
    /// Audio track files or glob patterns (WAV/FLAC/MP3/M4A)
    #[arg(long, num_args = 1..)]
    pub audio: Option<Vec<String>>,
    /// M3U/M3U8 playlist file to use as the track list
    #[arg(long)]
    pub playlist: Option<String>,
    /// Source directory for data session (Data CD, or the data session of an Enhanced CD)
    #[arg(long)]
    pub data: Option<String>,
    /// Individual files for a Data CD (they go in the disc root)
    #[arg(long, num_args = 1..)]
    pub files: Option<Vec<String>>,
    /// Disc capacity in MB: 650 (74 min), 700 (80 min, default) or 800 (90 min)
    #[arg(long, default_value_t = 700)]
    pub disc_size: u64,
    /// Only accept playlist entries inside this folder (used by the web UI)
    #[arg(long, hide = true)]
    pub playlist_root: Option<String>,
    /// Disc label
    #[arg(long, default_value = "Untitled")]
    pub label: String,
    /// Load disc graph from JSON file instead of flags
    #[arg(long)]
    pub input: Option<String>,
    /// Target optical drive device
    #[arg(long, default_value = "/dev/sr0")]
    pub device: String,
    /// Print debug information and backend calls
    #[arg(long)]
    pub debug: bool,
    /// Plan without writing to disc
    #[arg(long)]
    pub dry_run: bool,
    /// Read CD-Text (title, artist) from embedded audio file tags
    #[arg(long)]
    pub cd_text: bool,
    /// Transcode audio to a target format before burning, e.g. mp3:256, aac:320, opus:192, flac
    #[arg(long)]
    pub transcode: Option<String>,
    /// Directory to stage transcoded files (auto-temp if omitted)
    #[arg(long)]
    pub stage_dir: Option<String>,
    /// Keep staged files after burn (default: delete)
    #[arg(long)]
    pub keep_staged: bool,
    /// Emit newline-delimited JSON progress events to stdout (for machine consumers)
    #[arg(long)]
    pub progress_json: bool,
}

pub fn run(args: BurnArgs) -> Result<(), Error> {
    // JSON graph path: single disc, no multi-disc support
    if let Some(ref path) = args.input {
        let graph = parser::from_file(path)?;
        return burn_graph(&graph, &args, None);
    }

    let is_data = args.format == "datacd" || args.format == "data-cd" || args.format == "data";

    if is_data {
        run_data(&args)
    } else {
        run_audio(&args)
    }
}

// ── Data CD path ─────────────────────────────────────────────────────────────

fn disc_size(args: &BurnArgs) -> Result<DiscSize, Error> {
    let size = DiscSize::new(args.disc_size)?;
    // Lets the ISO builder and validator refuse anything that can't fit this disc.
    backend::data::set_disc_capacity(size.capacity_bytes());
    Ok(size)
}

fn playlist_root(args: &BurnArgs) -> Option<PathBuf> {
    args.playlist_root.as_ref().map(PathBuf::from)
}

/// A progress line: a JSON event for machine readers, plain text otherwise.
fn note(args: &BurnArgs, msg: &str) {
    if args.progress_json {
        println!("{}", serde_json::json!({"type": "step", "msg": msg}));
    } else {
        eprintln!("{}", msg);
    }
}

fn run_data(args: &BurnArgs) -> Result<(), Error> {
    let size = disc_size(args)?;
    let spec = match args.transcode.as_deref().filter(|t| !t.trim().is_empty()) {
        Some(t) => Some(TranscodeSpec::parse(t)?),
        None => None,
    };

    let resolved = discs::resolve_data(&DataSpec {
        playlist: args.playlist.clone(),
        files: args.files.clone(),
        data: args.data.clone(),
        playlist_root: playlist_root(args),
    })?;

    // How big will everything be (after transcoding), and how many discs is that?
    note(args, "Working out how many discs are needed...");
    let (planned, _) = discs::plan_files(&resolved.items, spec.as_ref());
    let (est_discs, too_big) = discs::pack_data(&planned, size);
    for &i in &too_big {
        eprintln!(
            "Warning: '{}' ({:.1}MB) is larger than a whole disc — skipping.",
            planned[i].path,
            planned[i].est_bytes as f64 / 1_048_576.0
        );
    }
    let usable: Vec<PlannedFile> = planned
        .iter()
        .enumerate()
        .filter(|(i, _)| !too_big.contains(i))
        .map(|(_, f)| f.clone())
        .collect();
    if usable.is_empty() {
        return Err(Error::validation("Nothing to burn: every file is larger than a whole disc"));
    }

    let total_est: u64 = usable.iter().map(|f| f.est_bytes).sum();
    let total_orig: u64 = usable.iter().map(|f| f.original_bytes).sum();
    if let Some(s) = &spec {
        note(args, &format!(
            "{} files: {:.0} MB as they are, about {:.0} MB after converting to {} → {} disc{}",
            usable.len(),
            total_orig as f64 / 1_048_576.0,
            total_est as f64 / 1_048_576.0,
            s.label(),
            est_discs.len(),
            if est_discs.len() == 1 { "" } else { "s" },
        ));
    } else if est_discs.len() > 1 {
        note(args, &format!(
            "{} files, {:.0} MB → {} discs required.",
            usable.len(), total_est as f64 / 1_048_576.0, est_discs.len()
        ));
    }

    // One disc and nothing to convert: burn straight from the folder, or from a folder of
    // links to the chosen files.
    if est_discs.len() <= 1 && spec.is_none() && too_big.is_empty() {
        let (dir, _guard) = match &resolved.ready_root {
            Some(root) => (root.clone(), None),
            None => {
                let dir = format!("/tmp/rustydisc_stage_{}", std::process::id());
                stage_items(&resolved.items, &dir)?;
                (dir.clone(), Some(StagedDir::new(dir, false, true)))
            }
        };
        let graph = parser::from_cli("datacd", None, None, Some(&dir), &args.label, false)?;
        return burn_graph(&graph, args, None);
    }

    burn_data_discs(args, size, spec, usable, est_discs.len())
}

/// A file waiting for a disc; `cached` is a copy that was already converted for an earlier
/// disc that turned out to be full.
struct Queued {
    file: PlannedFile,
    cached: Option<PathBuf>,
}

/// Fill and burn discs one at a time. Files are converted disc by disc as they are staged,
/// so only one disc's worth of converted files exists at a time, and the real converted size
/// (not the estimate) decides when a disc is full.
fn burn_data_discs(
    args: &BurnArgs,
    size: DiscSize,
    spec: Option<TranscodeSpec>,
    files: Vec<PlannedFile>,
    estimated_discs: usize,
) -> Result<(), Error> {
    if spec.as_ref().is_some_and(|_| files.iter().any(|f| f.transcode)) && !args.dry_run {
        backend::transcode::ensure_ffmpeg()?;
    }
    let usable = size.data_usable_bytes();
    let pid = std::process::id();
    // Converted files are written here one disc at a time; --stage-dir puts them somewhere with room.
    let base = args.stage_dir.clone().unwrap_or_else(|| "/tmp".to_string());
    let carry_dir = format!("{}/rustydisc_carry_{}", base, pid);
    let _carry_guard = StagedDir::new(carry_dir.clone(), false, true);

    let mut queue: VecDeque<Queued> = files.into_iter().map(|file| Queued { file, cached: None }).collect();
    let mut disc_num = 0usize;
    let mut expected = estimated_discs.max(1);

    while !queue.is_empty() {
        disc_num += 1;
        let stage = format!("{}/rustydisc_disc{:02}_{}", base, disc_num, pid);
        let _ = std::fs::remove_dir_all(&stage);
        std::fs::create_dir_all(&stage)?;
        let staged = StagedDir::new(stage.clone(), args.keep_staged, true);

        note(args, &format!("Disc {} of ~{}: preparing files...", disc_num, expected.max(disc_num)));

        let mut used = 0u64;
        let mut payload = 0u64;
        let mut count = 0usize;
        let mut dirs: HashSet<String> = HashSet::new();
        let mut convert_no = 0usize;

        while let Some(front) = queue.front() {
            let dir = Path::new(&front.file.rel_path).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
            let new_dir = if dirs.contains(&dir) { 0 } else { 8 * 1024 };
            // The estimate says whether it is worth converting this file for this disc at all.
            if count > 0 && used + discs::file_cost(front.file.est_bytes) + new_dir > usable {
                break;
            }
            let item = queue.pop_front().expect("front exists");
            let dest = Path::new(&stage).join(&item.file.rel_path);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }

            let actual = if let Some(cached) = &item.cached {
                std::fs::rename(cached, &dest).or_else(|_| std::fs::copy(cached, &dest).map(|_| ()))?;
                std::fs::metadata(&dest)?.len()
            } else if item.file.transcode && !args.dry_run {
                convert_no += 1;
                note(args, &format!("Disc {}: converting {} — {}", disc_num, convert_no, item.file.rel_path));
                backend::transcode::transcode_file(
                    &item.file.path,
                    &dest.to_string_lossy(),
                    spec.as_ref().expect("transcode implies a spec"),
                    args.debug,
                )?;
                std::fs::metadata(&dest)?.len()
            } else {
                let source = std::fs::canonicalize(&item.file.path)?;
                std::os::unix::fs::symlink(&source, &dest)
                    .or_else(|_| std::fs::hard_link(&source, &dest))
                    .or_else(|_| std::fs::copy(&source, &dest).map(|_| ()))?;
                if item.file.transcode { item.file.est_bytes } else { std::fs::metadata(&source)?.len() }
            };

            if count > 0 && used + discs::file_cost(actual) + new_dir > usable {
                // The estimate was too low and this file doesn't fit after all: it starts the next
                // disc, and if it was already converted that result is kept for it.
                let cached = if item.file.transcode && !args.dry_run {
                    std::fs::create_dir_all(&carry_dir)?;
                    let keep = Path::new(&carry_dir).join(format!("{}_{}", disc_num, queue.len()));
                    std::fs::rename(&dest, &keep).or_else(|_| std::fs::copy(&dest, &keep).map(|_| ()))?;
                    let _ = std::fs::remove_file(&dest);
                    Some(keep)
                } else {
                    let _ = std::fs::remove_file(&dest);
                    None
                };
                queue.push_front(Queued { file: item.file, cached });
                break;
            }

            used += discs::file_cost(actual) + new_dir;
            payload += actual;
            dirs.insert(dir);
            count += 1;
        }

        // The real number of discs is only known once the files are converted; keep the count honest.
        if queue.is_empty() {
            expected = disc_num; // that was the last one
        } else {
            let remaining: u64 = queue.iter().map(|q| q.file.est_bytes).sum();
            expected = expected.max(disc_num + 1).max(disc_num + remaining.div_ceil(usable.max(1)) as usize);
        }
        let label = disc_label(&args.label, disc_num, expected);

        if !args.dry_run {
            prompt_insert(disc_num, expected, &args.device, payload, count, None)?;
        }

        let graph = parser::from_cli("datacd", None, None, Some(&stage), &label, false)?;
        // A dry run doesn't convert anything, so the staged links point at the originals and
        // would look too big; the plan above already used the converted sizes.
        if args.dry_run {
            backend::data::set_disc_capacity(u64::MAX / 2);
        }
        let result = burn_graph(&graph, args, Some(&staged));
        if args.dry_run {
            backend::data::set_disc_capacity(size.capacity_bytes());
        }
        result?;

        if !args.dry_run && !queue.is_empty() {
            eject(&args.device);
            eprintln!("Disc {} complete. Remove the disc.", disc_num);
        }
    }

    if !args.dry_run {
        eprintln!("All {} disc{} burned successfully.", disc_num, if disc_num == 1 { "" } else { "s" });
    }
    Ok(())
}

// ── Audio (Red Book / Blue Book) path ─────────────────────────────────────────

fn run_audio(args: &BurnArgs) -> Result<(), Error> {
    let is_bluebook = matches!(args.format.to_lowercase().as_str(), "bluebook" | "blue-book" | "cdextra" | "cd-extra");
    if is_bluebook && args.data.is_none() {
        return Err(Error::validation(
            "An Enhanced (Blue Book) CD needs a data folder for its second session: add --data <dir>",
        ));
    }
    if !is_bluebook && args.data.is_some() {
        return Err(Error::validation(
            "--data only applies to Enhanced CDs (--format bluebook); an Audio CD has no data session",
        ));
    }
    if args.files.is_some() {
        return Err(Error::validation("--files only applies to Data CDs; use --audio or --playlist for audio tracks"));
    }

    let size = disc_size(args)?;
    let capacity_secs = size.audio_capacity_secs();

    // Collect tracks with durations
    let audio_items = collect_audio_items(args)?;

    let total_secs: u64 = audio_items.iter().map(|t| t.duration_secs).sum();
    let total_tracks = audio_items.len();

    let needs_split = total_secs > capacity_secs
        || total_tracks > AUDIO_MAX_TRACKS;

    if !needs_split {
        // Single disc
        let tracks: Vec<String> = audio_items.into_iter().map(|t| t.path).collect();
        let graph = parser::from_cli(
            &args.format,
            Some(&tracks),
            None, args.data.as_deref(),
            &args.label,
            args.cd_text,
        )?;
        return burn_graph(&graph, args, None);
    }

    if is_bluebook {
        return Err(Error::validation(format!(
            "An Enhanced CD must fit on one disc, but these {} tracks ({}:{:02}) need more than one. \
             Use fewer tracks, or burn them as separate Audio CDs.",
            total_tracks, total_secs / 60, total_secs % 60,
        )));
    }

    // Multi-disc
    let slices = split::split_audio(audio_items, capacity_secs, AUDIO_MAX_TRACKS);

    eprintln!(
        "\n{} tracks ({}:{:02} total) → {} discs required.",
        total_tracks,
        total_secs / 60,
        total_secs % 60,
        slices.len(),
    );

    burn_audio_discs(&slices, args)
}

fn collect_audio_items(args: &BurnArgs) -> Result<Vec<AudioItem>, Error> {
    let audio = args.audio.clone().unwrap_or_default();
    let (items, _skipped) = discs::resolve_audio(&audio, args.playlist.as_deref(), playlist_root(args).as_deref())?;
    Ok(items)
}

fn burn_audio_discs(slices: &[AudioSlice], args: &BurnArgs) -> Result<(), Error> {
    let total = slices.len();

    for (i, slice) in slices.iter().enumerate() {
        let disc_num = i + 1;
        let label = disc_label(&args.label, disc_num, total);

        if !args.dry_run {
            prompt_insert(disc_num, total, &args.device, 0, slice.items.len(), Some(slice.total_secs))?;
        }

        let tracks: Vec<String> = slice.items.iter().map(|t| t.path.clone()).collect();
        let graph = parser::from_cli(
            &args.format,
            Some(&tracks),
            None, None,
            &label,
            args.cd_text,
        )?;

        burn_graph(&graph, args, None)?;

        if !args.dry_run && disc_num < total {
            eject(&args.device);
            eprintln!("Disc {}/{} complete. Remove the disc.", disc_num, total);
        }
    }

    if !args.dry_run {
        eprintln!("All {} discs burned successfully.", total);
    }

    Ok(())
}

// ── Common burn logic ─────────────────────────────────────────────────────────

fn burn_graph(
    graph: &crate::model::disc::DiscGraph,
    args: &BurnArgs,
    _staged: Option<&StagedDir>,
) -> Result<(), Error> {
    let plan = planner::plan(graph)?;

    if args.debug || args.dry_run {
        println!("{}", serde_json::to_string_pretty(&plan)?);
    }

    if !args.dry_run {
        backend::execute(graph, &plan, &args.device, args.debug, args.progress_json)?;
        if args.progress_json {
            println!("{{\"type\":\"done\"}}");
        } else {
            eprintln!("Disc burn complete.");
        }
    } else {
        eprintln!("Dry run complete. No disc was written.");
    }

    Ok(())
}

// ── Staging helpers ───────────────────────────────────────────────────────────

/// Build a folder that holds `items` at their `rel_path`s, using symlinks (no copying).
/// The ISO is built following symlinks, so the disc gets real files.
fn stage_items(items: &[DataItem], dir: &str) -> Result<(), Error> {
    std::fs::create_dir_all(dir)?;
    for item in items {
        let dest = std::path::Path::new(dir).join(&item.rel_path);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if dest.exists() { std::fs::remove_file(&dest).ok(); }
        let source = std::fs::canonicalize(&item.path)?;
        // Prefer symlinks (zero copy); fall back to hard link then copy
        std::os::unix::fs::symlink(&source, &dest)
            .or_else(|_| std::fs::hard_link(&source, &dest))
            .or_else(|_| std::fs::copy(&source, &dest).map(|_| ()))?;
    }
    Ok(())
}


// ── User interaction ──────────────────────────────────────────────────────────

fn prompt_insert(
    disc_num: usize,
    total: usize,
    device: &str,
    bytes: u64,
    file_count: usize,
    duration_secs: Option<u64>,
) -> Result<(), Error> {
    eprintln!("\n══ Disc {} of {} ═══════════════════════════════════════", disc_num, total);
    if let Some(secs) = duration_secs {
        eprintln!("  {} tracks  |  {}:{:02}", file_count, secs / 60, secs % 60);
    } else {
        eprintln!("  {} files  |  {:.1}MB", file_count, bytes as f64 / 1_048_576.0);
    }
    eprint!("Insert blank disc {} into {} and press ENTER to burn... ", disc_num, device);
    std::io::stderr().flush().ok();
    let mut buf = String::new();
    std::io::stdin().read_line(&mut buf)?;

    // Give the drive a moment to recognise the disc
    std::thread::sleep(std::time::Duration::from_secs(4));
    Ok(())
}

fn eject(device: &str) {
    let _ = std::process::Command::new("eject").arg(device).status();
}

/// The label for disc `disc_num` of `total`: the base label as it is for a single disc,
/// otherwise "Base - Disc N". ISO volume labels are at most 32 characters, so a long base
/// is shortened to leave room for the suffix.
fn disc_label(base: &str, disc_num: usize, total: usize) -> String {
    if total <= 1 {
        return base.to_string();
    }
    let suffix = format!(" - Disc {}", disc_num);
    let room = 32usize.saturating_sub(suffix.chars().count());
    let truncated: String = base.chars().take(room).collect();
    format!("{}{}", truncated.trim_end(), suffix)
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn have(tool: &str) -> bool {
        Command::new(tool).arg("--version").output().is_ok() || Command::new(tool).arg("-version").output().is_ok()
    }

    /// A Data CD built from a chosen list of files and from a nested folder must keep every
    /// file, at the right place, as a real file rather than a link.
    #[test]
    fn staged_data_becomes_real_files_in_the_iso() {
        if !have("xorriso") || !have("isoinfo") {
            eprintln!("skipping: xorriso/isoinfo not installed");
            return;
        }
        let root = std::env::temp_dir().join(format!("rd_stage_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src/sub")).unwrap();
        std::fs::create_dir_all(root.join("other")).unwrap();
        std::fs::write(root.join("src/one.txt"), b"11").unwrap();
        std::fs::write(root.join("src/sub/two.txt"), b"222").unwrap();
        std::fs::write(root.join("other/one.txt"), b"3333").unwrap(); // same name as src/one.txt

        let build = |items: &[DataItem], name: &str| -> String {
            let stage = root.join(format!("stage_{name}"));
            stage_items(items, stage.to_str().unwrap()).unwrap();
            let iso = root.join(format!("{name}.iso"));
            let ok = Command::new("xorriso")
                .args(["-as", "mkisofs", "-r", "-J", "-f", "-o"])
                .arg(&iso)
                .arg(&stage)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "xorriso failed");
            let out = Command::new("isoinfo").args(["-R", "-f", "-i"]).arg(&iso).output().unwrap();
            String::from_utf8_lossy(&out.stdout).to_string()
        };

        // Individually chosen files: flat, same-named files told apart.
        let files = split::flat_items(&[
            root.join("src/one.txt").to_string_lossy().to_string(),
            root.join("other/one.txt").to_string_lossy().to_string(),
        ]);
        let listing = build(&files, "files");
        assert!(listing.contains("/one.txt") && listing.contains("/one (2).txt"), "{listing}");

        // A nested folder: structure preserved.
        let tree = split::enumerate_dir(root.join("src").to_str().unwrap()).unwrap();
        let listing = build(&tree, "tree");
        assert!(listing.contains("/sub/two.txt") && listing.contains("/one.txt"), "{listing}");

        // Files carry their contents (not zero-byte links).
        let iso = root.join("tree.iso");
        let out = Command::new("isoinfo").args(["-R", "-l", "-i"]).arg(&iso).output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(!text.contains(" -> "), "the ISO holds symlinks:\n{text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn data_sources_are_exclusive() {
        let args = |files: Option<Vec<String>>, data: Option<String>| BurnArgs {
            format: "datacd".into(), audio: None, playlist: None, data, files, playlist_root: None, disc_size: 700,
            label: "T".into(), input: None, device: "/dev/null".into(), debug: false, dry_run: true,
            cd_text: false, transcode: None, stage_dir: None, keep_staged: false, progress_json: false,
        };
        let resolve = |a: BurnArgs| discs::resolve_data(&DataSpec { playlist: a.playlist, files: a.files, data: a.data, playlist_root: None });
        assert!(resolve(args(None, None)).is_err());
        assert!(resolve(args(Some(vec!["/etc/hostname".into()]), Some("/tmp".into()))).is_err());
        assert!(resolve(args(Some(vec!["/definitely/not/here".into()]), None)).is_err());
    }

    #[test]
    fn disc_labels() {
        // a single disc keeps its label as it is
        assert_eq!(disc_label("Magnum Opus", 1, 1), "Magnum Opus");
        // several discs get "- Disc N"
        assert_eq!(disc_label("Magnum Opus", 1, 7), "Magnum Opus - Disc 1");
        assert_eq!(disc_label("Magnum Opus", 12, 38), "Magnum Opus - Disc 12");
        // a long label is shortened so the whole thing stays within 32 characters
        let long = disc_label("A very long playlist name that goes on and on", 3, 9);
        assert_eq!(long, "A very long playlist na - Disc 3");
        assert!(long.chars().count() <= 32);
        assert!(disc_label(&"x".repeat(40), 123, 200).chars().count() <= 32);
    }
}
