use clap::Args;
use crate::{
    error::Error,
    rip::{self, encoder::AudioFormat},
};

#[derive(Args, Debug)]
pub struct RipArgs {
    /// Optical drive to rip from
    #[arg(long, default_value = "/dev/sr0")]
    pub device: String,

    /// Base directory — a subfolder named "Artist - Album (Year)" is created automatically.
    /// Example: --dir /srv/Music/CDRips
    #[arg(long, conflicts_with = "output")]
    pub dir: Option<String>,

    /// Explicit output directory (exact path, no auto-naming).
    /// Example: --output "/srv/Music/CDRips/Morrissey - Bona Drag (2010)"
    #[arg(long, conflicts_with = "dir")]
    pub output: Option<String>,

    /// Audio format: wav, flac, alac, aiff, ogg, mp3, opus  [default: flac]
    #[arg(long, default_value = "flac")]
    pub format: String,

    /// Archive mode: store files in audio/ + metadata/ subdirectories,
    /// include disc.json, cdtext.json, musicbrainz.json, checksums.json.
    #[arg(long)]
    pub archive: bool,

    /// Skip MusicBrainz lookup (for offline use or discs not in the database)
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
    if args.dir.is_none() && args.output.is_none() {
        return Err(Error::validation(
            "Specify a destination:\n  \
             --dir /path/to/base     auto-names the folder from disc metadata\n  \
             --output /path/to/dir   use an exact output path"
        ));
    }

    let format = args.format.parse::<AudioFormat>().map_err(Error::validation)?;

    let missing = rip::check_dependencies(&format);
    if !missing.is_empty() {
        return Err(Error::validation(format!(
            "Missing required tools:\n{}",
            missing.iter().map(|m| format!("  - {}", m)).collect::<Vec<_>>().join("\n")
        )));
    }

    let opts = rip::RipOptions {
        device:         args.device,
        output_dir:     args.output,
        base_dir:       args.dir,
        format,
        archive:        args.archive,
        debug:          args.debug,
        progress_json:  args.progress_json,
        no_musicbrainz: args.no_musicbrainz,
    };

    rip::rip(&opts)?;

    if args.progress_json {
        println!("{{\"type\":\"done\"}}");
    } else {
        eprintln!("Done.");
    }

    Ok(())
}
