use std::io::Write as _;
use clap::Args;
use crate::{
    backend,
    backend::transcode::{StagedDir, TranscodeSpec},
    error::Error,
    parser,
    planner,
    planner::split::{
        self, AudioItem, AudioSlice, DataItem, DataSlice,
        AUDIO_DISC_CAPACITY_SECS, AUDIO_MAX_TRACKS, DATA_DISC_CAPACITY_BYTES,
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

fn run_data(args: &BurnArgs) -> Result<(), Error> {
    // Step 1: resolve the source file list (with optional transcoding)
    let source = prepare_data_items(args)?;

    let total_bytes: u64 = source.items.iter().map(|f| f.size_bytes).sum();
    let disc_count = estimate_disc_count_data(total_bytes);

    if disc_count <= 1 {
        // Single disc: burn from the folder directly when it already holds exactly the
        // right files, otherwise stage the chosen files into a temporary folder.
        let (dir, _guard) = match &source.ready_root {
            Some(root) => (root.clone(), None),
            None => {
                let dir = format!("/tmp/rustydisc_stage_{}", std::process::id());
                stage_items(&source.items, &dir)?;
                (dir.clone(), Some(StagedDir::new(dir, false, true)))
            }
        };
        let graph = parser::from_cli("datacd", None, None, Some(&dir), &args.label, false)?;
        return burn_graph(&graph, args, None);
    }

    // Multi-disc
    eprintln!(
        "\n{} total ({:.0}MB) → {} discs required.",
        source.items.len(),
        total_bytes as f64 / 1_048_576.0,
        disc_count,
    );

    let slices = split::split_data(source.items, DATA_DISC_CAPACITY_BYTES);
    burn_data_discs(&slices, args)
}

/// What a Data CD will contain.
struct DataSource {
    items: Vec<DataItem>,
    /// A folder that already contains exactly `items` at their `rel_path`s.
    ready_root: Option<String>,
    _staged: Option<StagedDir>,
}

fn playlist_root(args: &BurnArgs) -> Option<std::path::PathBuf> {
    args.playlist_root.as_ref().map(std::path::PathBuf::from)
}

/// Individually chosen files, checked and made absolute.
fn resolve_files(files: &[String]) -> Result<Vec<String>, Error> {
    files
        .iter()
        .map(|f| {
            let p = std::fs::canonicalize(f).map_err(|_| Error::validation(format!("File not found: {}", f)))?;
            if p.is_dir() {
                return Err(Error::validation(format!("'{}' is a folder — use --data for folders", f)));
            }
            Ok(p.to_string_lossy().to_string())
        })
        .collect()
}

fn prepare_data_items(args: &BurnArgs) -> Result<DataSource, Error> {
    let chosen = [args.playlist.is_some(), args.files.is_some(), args.data.is_some()];
    match chosen.iter().filter(|c| **c).count() {
        0 => return Err(Error::validation(
            "Data CD burn requires --data <dir>, --files <files>, --playlist <file>, or --input <graph.json>",
        )),
        1 => {}
        _ => return Err(Error::validation(
            "Choose one source for a Data CD: --data <dir>, --files <files>, or --playlist <file>",
        )),
    }

    // The individual files (from --files or a playlist), if that is the source.
    let entries: Option<Vec<parser::playlist::PlaylistEntry>> = if let Some(ref pl) = args.playlist {
        Some(parser::playlist::parse_within(pl, playlist_root(args).as_deref())?)
    } else if let Some(ref files) = args.files {
        Some(resolve_files(files)?
            .into_iter()
            .map(|path| parser::playlist::PlaylistEntry { path, duration_secs: None, display: None })
            .collect())
    } else {
        None
    };

    if let Some(ref spec_str) = args.transcode {
        let spec = TranscodeSpec::parse(spec_str)?;
        let (stage_path, auto) = stage_path(args);
        let staged = StagedDir::new(stage_path.clone(), args.keep_staged, auto);

        if let Some(ref entries) = entries {
            eprintln!("Transcoding {} files → {} ...", entries.len(), stage_path);
            backend::transcode::transcode_playlist(entries, &spec, &stage_path, args.debug)?;
        } else if let Some(ref data_dir) = args.data {
            eprintln!("Transcoding '{}' → {} ...", data_dir, stage_path);
            backend::transcode::transcode_dir(data_dir, &stage_path, &spec, args.debug)?;
        }

        let items = split::enumerate_dir(&stage_path)?;
        return Ok(DataSource { items, ready_root: Some(stage_path), _staged: Some(staged) });
    }

    if let Some(entries) = entries {
        let paths: Vec<String> = entries.into_iter().map(|e| e.path).collect();
        return Ok(DataSource { items: split::flat_items(&paths), ready_root: None, _staged: None });
    }

    let data_dir = args.data.as_ref().expect("checked above");
    let items = split::enumerate_dir(data_dir)?;
    Ok(DataSource { items, ready_root: Some(data_dir.clone()), _staged: None })
}

fn burn_data_discs(slices: &[DataSlice], args: &BurnArgs) -> Result<(), Error> {
    let total = slices.len();

    for (i, slice) in slices.iter().enumerate() {
        let disc_num = i + 1;
        let label = disc_label(&args.label, disc_num, total);

        // Stage this disc's files into a sub-directory using symlinks
        let disc_stage = format!("/tmp/rustydisc_disc{:02}_{}", disc_num, std::process::id());
        stage_items(&slice.items, &disc_stage)?;
        let disc_staged = StagedDir::new(disc_stage.clone(), false, true);

        // Prompt
        if !args.dry_run {
            prompt_insert(disc_num, total, &args.device, slice.total_bytes, slice.items.len(), None)?;
        }

        let graph = parser::from_cli(
            "datacd", None, None, Some(&disc_stage), &label, false,
        )?;

        burn_graph(&graph, args, Some(&disc_staged))?;

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

    // Collect tracks with durations
    let audio_items = collect_audio_items(args)?;

    let total_secs: u64 = audio_items.iter().map(|t| t.duration_secs).sum();
    let total_tracks = audio_items.len();

    let needs_split = total_secs > AUDIO_DISC_CAPACITY_SECS
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
    let slices = split::split_audio(audio_items, AUDIO_DISC_CAPACITY_SECS, AUDIO_MAX_TRACKS);

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
    // Playlist path: use EXTINF durations where available
    if let Some(ref pl) = args.playlist {
        let entries = parser::playlist::parse_within(pl, playlist_root(args).as_deref())?;
        return Ok(entries
            .into_iter()
            .map(|e| AudioItem {
                duration_secs: e.duration_secs.unwrap_or_else(|| split::duration_secs(&e.path)),
                path: e.path,
            })
            .collect());
    }

    // Audio flag / glob patterns
    if let Some(ref patterns) = args.audio {
        let tracks = parser::expand_audio_globs(patterns)?;
        return Ok(tracks
            .into_iter()
            .map(|p| AudioItem {
                duration_secs: split::duration_secs(&p),
                path: p,
            })
            .collect());
    }

    Err(Error::validation(
        "Audio burn requires --audio <files>, --playlist <file>, or --input <graph.json>",
    ))
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

fn stage_path(args: &BurnArgs) -> (String, bool) {
    match &args.stage_dir {
        Some(p) => (p.clone(), false),
        None => (format!("/tmp/rustydisc_stage_{}", std::process::id()), true),
    }
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

fn disc_label(base: &str, disc_num: usize, total: usize) -> String {
    if total > 1 {
        // ISO volume labels: uppercase, max 32 chars — keep base short
        let max_base = 26; // leaves room for " (X/Y)"
        let truncated: String = base.chars().take(max_base).collect();
        format!("{} ({}/{})", truncated, disc_num, total)
    } else {
        base.to_string()
    }
}

fn estimate_disc_count_data(total_bytes: u64) -> usize {
    ((total_bytes + DATA_DISC_CAPACITY_BYTES - 1) / DATA_DISC_CAPACITY_BYTES).max(1) as usize
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
            format: "datacd".into(), audio: None, playlist: None, data, files, playlist_root: None,
            label: "T".into(), input: None, device: "/dev/null".into(), debug: false, dry_run: true,
            cd_text: false, transcode: None, stage_dir: None, keep_staged: false, progress_json: false,
        };
        assert!(prepare_data_items(&args(None, None)).is_err());
        assert!(prepare_data_items(&args(Some(vec!["/etc/hostname".into()]), Some("/tmp".into()))).is_err());
        assert!(prepare_data_items(&args(Some(vec!["/definitely/not/here".into()]), None)).is_err());
    }
}
