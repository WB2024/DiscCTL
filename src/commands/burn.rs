use clap::Args;
use crate::{backend, error::Error, parser, planner};

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
}

pub fn run(args: BurnArgs) -> Result<(), Error> {
    let graph = if let Some(ref path) = args.input {
        parser::from_file(path)?
    } else {
        parser::from_cli(
            &args.format,
            args.audio.as_deref(),
            args.playlist.as_deref(),
            args.data.as_deref(),
            &args.label,
            args.cd_text,
        )?
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
}
