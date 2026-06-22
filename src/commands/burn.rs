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
    /// Source directory for data session
    #[arg(long)]
    pub data: Option<String>,
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
    let (items, _staged) = prepare_data_items(args)?;

    let total_bytes: u64 = items.iter().map(|f| f.size_bytes).sum();
    let disc_count = estimate_disc_count_data(total_bytes);

    if disc_count <= 1 {
        // Single disc — use the normal single-graph path
        let source_dir = items_to_stage_dir(&items, args, None)?;
        let graph = parser::from_cli(
            "datacd", None, None, Some(&source_dir.path), &args.label, false,
        )?;
        burn_graph(&graph, args, Some(&source_dir))?;
        return Ok(());
    }

    // Multi-disc
    eprintln!(
        "\n{} total ({:.0}MB) → {} discs required.",
        items.len(),
        total_bytes as f64 / 1_048_576.0,
        disc_count,
    );

    let slices = split::split_data(items, DATA_DISC_CAPACITY_BYTES);
    burn_data_discs(&slices, args)
}

fn prepare_data_items(args: &BurnArgs) -> Result<(Vec<DataItem>, Option<StagedDir>), Error> {
    if let Some(ref spec_str) = args.transcode {
        let spec = TranscodeSpec::parse(spec_str)?;
        let (stage_path, auto) = stage_path(args);
        let staged = StagedDir::new(stage_path.clone(), args.keep_staged, auto);

        if let Some(ref pl) = args.playlist {
            let entries = parser::playlist::parse(pl)?;
            eprintln!("Transcoding {} tracks → {} ...", entries.len(), stage_path);
            backend::transcode::transcode_playlist(&entries, &spec, &stage_path, args.debug)?;
        } else if let Some(ref data_dir) = args.data {
            eprintln!("Transcoding '{}' → {} ...", data_dir, stage_path);
            backend::transcode::transcode_dir(data_dir, &stage_path, &spec, args.debug)?;
        }

        let items = split::enumerate_dir(&stage_path)?;
        return Ok((items, Some(staged)));
    }

    // No transcode
    if let Some(ref pl) = args.playlist {
        let entries = parser::playlist::parse(pl)?;
        let items = entries
            .into_iter()
            .map(|e| DataItem {
                size_bytes: std::fs::metadata(&e.path).map(|m| m.len()).unwrap_or(0),
                path: e.path,
            })
            .collect();
        return Ok((items, None));
    }

    if let Some(ref data_dir) = args.data {
        let items = split::enumerate_dir(data_dir)?;
        return Ok((items, None));
    }

    Err(Error::validation(
        "Data CD burn requires --data <dir>, --playlist <file>, or --input <graph.json>",
    ))
}

fn burn_data_discs(slices: &[DataSlice], args: &BurnArgs) -> Result<(), Error> {
    let total = slices.len();

    for (i, slice) in slices.iter().enumerate() {
        let disc_num = i + 1;
        let label = disc_label(&args.label, disc_num, total);

        // Stage this disc's files into a sub-directory using symlinks
        let disc_stage = format!("/tmp/rustydisc_disc{:02}_{}", disc_num, std::process::id());
        stage_files_with_symlinks(&slice.items, &disc_stage)?;
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
            None, None,
            &args.label,
            args.cd_text,
        )?;
        return burn_graph(&graph, args, None);
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
        let entries = parser::playlist::parse(pl)?;
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
        backend::execute(graph, &plan, &args.device, args.debug)?;
        println!("Disc burn complete.");
    } else {
        println!("Dry run complete. No disc was written.");
    }

    Ok(())
}

// ── Staging helpers ───────────────────────────────────────────────────────────

/// Create a single-disc staging dir from a DataItem list.
/// Used for the single-disc data path where we have a file list but need a dir.
fn items_to_stage_dir(
    items: &[DataItem],
    args: &BurnArgs,
    suffix: Option<&str>,
) -> Result<StagedDir, Error> {
    // If all items are already under a common directory (e.g. transcoded to stage_dir),
    // find the common prefix. Otherwise, create a new symlink staging directory.
    if let Some(common) = common_parent(items) {
        // All files live under one directory already; use it directly without copying
        return Ok(StagedDir::new(common, true, false)); // don't auto-delete, not auto-created
    }

    let path = match suffix {
        Some(s) => format!("/tmp/rustydisc_stage_{}_{}", s, std::process::id()),
        None => format!("/tmp/rustydisc_stage_{}", std::process::id()),
    };
    stage_files_with_symlinks(items, &path)?;
    let (_, auto) = stage_path(args);
    Ok(StagedDir::new(path, args.keep_staged, auto))
}

fn common_parent(items: &[DataItem]) -> Option<String> {
    if items.is_empty() { return None; }
    let first = std::path::Path::new(&items[0].path).parent()?;
    let all_same = items.iter().all(|i| {
        std::path::Path::new(&i.path).parent().map(|p| p == first).unwrap_or(false)
    });
    if all_same { Some(first.to_string_lossy().to_string()) } else { None }
}

fn stage_files_with_symlinks(items: &[DataItem], dir: &str) -> Result<(), Error> {
    std::fs::create_dir_all(dir)?;
    for item in items {
        let fname = std::path::Path::new(&item.path)
            .file_name()
            .unwrap_or_default();
        let dest = std::path::Path::new(dir).join(fname);
        if dest.exists() { std::fs::remove_file(&dest).ok(); }
        // Prefer symlinks (zero copy); fall back to hard link then copy
        std::os::unix::fs::symlink(&item.path, &dest)
            .or_else(|_| std::fs::hard_link(&item.path, &dest))
            .or_else(|_| std::fs::copy(&item.path, &dest).map(|_| ()))?;
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
