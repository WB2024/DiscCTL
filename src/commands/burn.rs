use clap::Args;
use crate::{backend, backend::transcode::{StagedDir, TranscodeSpec}, error::Error, parser, planner};

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
    // Transcoding is a preprocessing step that may redirect what the disc graph sees
    // as its data source. We hold `_staged` so the StagedDir lives until end of burn.
    let (graph, _staged) = if let Some(ref path) = args.input {
        (parser::from_file(path)?, None)
    } else if let Some(ref spec_str) = args.transcode {
        build_graph_with_transcode(&args, spec_str)?
    } else {
        let g = parser::from_cli(
            &args.format,
            args.audio.as_deref(),
            args.playlist.as_deref(),
            args.data.as_deref(),
            &args.label,
            args.cd_text,
        )?;
        (g, None)
    };

    let plan = planner::plan(&graph)?;

    if args.debug || args.dry_run {
        println!("{}", serde_json::to_string_pretty(&plan)?);
    }

    if !args.dry_run {
        backend::execute(&graph, &plan, &args.device, args.debug)?;
        println!("Disc burn complete.");
    } else {
        println!("Dry run complete. No disc was written.");
    }

    Ok(())
    // _staged drops here, deleting the stage dir unless --keep-staged
}

fn build_graph_with_transcode(
    args: &BurnArgs,
    spec_str: &str,
) -> Result<(crate::model::disc::DiscGraph, Option<StagedDir>), Error> {
    let spec = TranscodeSpec::parse(spec_str)?;

    // Resolve staging directory
    let (stage_path, auto_created) = match &args.stage_dir {
        Some(p) => (p.clone(), false),
        None => (
            format!("/tmp/discctl_stage_{}", std::process::id()),
            true,
        ),
    };

    let staged = StagedDir::new(stage_path.clone(), args.keep_staged, auto_created);

    // ── Case 1: playlist + transcode → DataCD from staged dir ──────────────
    if let Some(ref pl_path) = args.playlist {
        let entries = parser::playlist::parse(pl_path)?;
        eprintln!(
            "Transcoding {} tracks to {} → {}",
            entries.len(),
            spec_str,
            stage_path
        );
        backend::transcode::transcode_playlist(&entries, &spec, &stage_path, args.debug)?;

        let graph = parser::from_cli(
            "datacd",
            None,
            None,
            Some(&stage_path),
            &args.label,
            false, // CD-Text doesn't apply to data sessions
        )?;
        return Ok((graph, Some(staged)));
    }

    // ── Case 2: data dir + transcode → DataCD from staged dir ──────────────
    if let Some(ref data_dir) = args.data {
        eprintln!(
            "Transcoding '{}' to {} → {}",
            data_dir, spec_str, stage_path
        );
        backend::transcode::transcode_dir(data_dir, &stage_path, &spec, args.debug)?;

        let graph = parser::from_cli(
            &args.format,
            None,
            None,
            Some(&stage_path),
            &args.label,
            false,
        )?;
        return Ok((graph, Some(staged)));
    }

    // ── Case 3: audio tracks + transcode (not a common path — convert.rs
    //    already handles CDDA conversion implicitly for Red Book burns)
    let graph = parser::from_cli(
        &args.format,
        args.audio.as_deref(),
        args.playlist.as_deref(),
        args.data.as_deref(),
        &args.label,
        args.cd_text,
    )?;
    Ok((graph, Some(staged)))
}
