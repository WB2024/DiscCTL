use clap::Args;
use crate::{error::Error, parser, planner};

#[derive(Args, Debug)]
pub struct PlanArgs {
    /// Disc format: redbook, datacd, bluebook
    #[arg(long)]
    pub format: Option<String>,
    /// Load disc graph from JSON file
    #[arg(long)]
    pub input: Option<String>,
    /// Audio track files or glob patterns
    #[arg(long, num_args = 1..)]
    pub audio: Option<Vec<String>>,
    /// M3U/M3U8 playlist file to use as the track list
    #[arg(long)]
    pub playlist: Option<String>,
    /// Source directory for data session
    #[arg(long)]
    pub data: Option<String>,
    /// Write speed to show in the plan: auto (default) or a multiple like 8 for 8x
    #[arg(long, default_value = "auto", value_name = "auto|N")]
    pub speed: String,
    /// Disc label
    #[arg(long, default_value = "Untitled")]
    pub label: String,
    /// Read CD-Text (title, artist) from embedded audio file tags
    #[arg(long)]
    pub cd_text: bool,
    /// Data CD only: individual files
    #[arg(long, num_args = 1..)]
    pub files: Option<Vec<String>>,
    /// Data CD only: convert audio first, e.g. mp3:320 — the plan counts the converted sizes
    #[arg(long)]
    pub transcode: Option<String>,
    /// Blank disc size, in MB or by name: cd700 (default for CDs), cd650, cd800, dvd (default for
    /// DVDs), dvd-dl, bd, bd-dl
    #[arg(long)]
    pub disc_size: Option<String>,
    /// Music DVD: Dolby Digital bitrate in kbps (192, 256, 384 or 448)
    #[arg(long, default_value_t = 448)]
    pub dvd_audio_kbps: u32,
    /// Only accept playlist entries inside this folder (used by the web UI)
    #[arg(long, hide = true)]
    pub playlist_root: Option<String>,
}

pub fn run(args: PlanArgs) -> Result<(), Error> {
    // A hand-written disc graph is a single disc: show its steps as before.
    if let Some(ref path) = args.input {
        let mut graph = parser::from_file(path)?;
        if let Some(x) = crate::backend::speed::parse(&args.speed).map_err(Error::validation)? {
            graph.speed = Some(x);
        }
        let plan = planner::plan(&graph)?;
        println!("{}", serde_json::to_string_pretty(&plan)?);
        return Ok(());
    }

    let format = args.format.clone().unwrap_or_else(|| "redbook".into());

    // How many discs, and what goes on each (counting the size after any transcoding).
    let mut out = planner::discs::plan_request(&planner::discs::PlanRequest {
        format: format.clone(),
        audio: args.audio.clone().unwrap_or_default(),
        playlist: args.playlist.clone(),
        files: args.files.clone(),
        data: args.data.clone(),
        playlist_root: args.playlist_root.clone().map(std::path::PathBuf::from),
        transcode: args.transcode.clone(),
        disc_size_mb: args.disc_size.as_deref().map(planner::discs::DiscSize::parse).transpose()?.map(|s| s.mb),
        dvd_audio_kbps: Some(args.dvd_audio_kbps),
    })?;

    if let Some(x) = crate::backend::speed::parse(&args.speed).map_err(Error::validation)? {
        out["speed"] = serde_json::json!(x);
    }

    // The burn steps for one disc, when they can be worked out from the flags alone.
    let single_source = args.files.is_none() && (args.playlist.is_none() || format != "datacd");
    if single_source {
        if let Ok(graph) = parser::from_cli(&format, args.audio.as_deref(), args.playlist.as_deref(), args.data.as_deref(), &args.label, args.cd_text) {
            if let Ok(steps) = planner::build_steps(&graph) {
                out["format"] = serde_json::json!(graph.format.to_string());
                out["label"] = serde_json::json!(graph.label);
                out["steps"] = serde_json::to_value(steps)?;
            }
        }
    }

    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}
