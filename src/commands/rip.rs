use clap::Args;
use crate::{
    error::Error,
    rip::{self, RipLayout, encoder::AudioFormat},
};

#[derive(Args, Debug)]
pub struct RipArgs {
    /// Optical drive to rip from
    #[arg(long, default_value = "/dev/sr0")]
    pub device: String,
    /// Output directory for ripped files
    #[arg(long, default_value = "disc_rip")]
    pub output: String,
    /// Audio format: wav, flac, alac, aiff, ogg, mp3, opus
    #[arg(long, default_value = "flac")]
    pub format: String,
    /// Archive mode: include disc.json, cdtext.json, checksums.json for reconstruction
    #[arg(long)]
    pub archive: bool,
    /// Skip MusicBrainz lookup (for offline use or when the disc is not in the database)
    #[arg(long)]
    pub no_musicbrainz: bool,
    /// Print debug information
    #[arg(long)]
    pub debug: bool,
    /// Emit machine-readable JSON progress events to stdout
    #[arg(long)]
    pub progress_json: bool,
}

pub fn run(args: RipArgs) -> Result<(), Error> {
    let format = args.format.parse::<AudioFormat>().map_err(|e| Error::validation(e))?;

    let missing = rip::check_dependencies(&format);
    if !missing.is_empty() {
        return Err(Error::validation(format!(
            "Missing required tools:\n{}",
            missing.iter().map(|m| format!("  - {}", m)).collect::<Vec<_>>().join("\n")
        )));
    }

    let layout = if args.archive {
        RipLayout::Archive
    } else {
        RipLayout::Exploded
    };

    let opts = rip::RipOptions {
        device: args.device,
        output_dir: args.output.clone(),
        format,
        layout,
        debug: args.debug,
        progress_json: args.progress_json,
        no_musicbrainz: args.no_musicbrainz,
    };

    rip::rip(&opts)?;

    if args.progress_json {
        println!("{{\"type\":\"done\"}}");
    } else {
        eprintln!("Rip complete → {}", args.output);
    }

    Ok(())
}
