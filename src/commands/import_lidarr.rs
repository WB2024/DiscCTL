use std::path::PathBuf;

use clap::Args;

use crate::{
    error::Error,
    library::lidarr::{self, Lidarr, Progress},
};

#[derive(Args, Debug)]
pub struct ImportLidarrArgs {
    /// A ripped album folder to import. Repeat for several.
    #[arg(long, value_name = "DIR", required = true)]
    pub rip: Vec<String>,

    /// Lidarr's address, e.g. http://192.168.1.110:8686
    #[arg(long)]
    pub url: String,

    /// Lidarr's API key (or set RUSTYDISC_LIDARR_KEY)
    #[arg(long, env = "RUSTYDISC_LIDARR_KEY", hide_env_values = true)]
    pub api_key: String,

    /// Where a new artist goes (a Lidarr root folder; default: Lidarr's first)
    #[arg(long, default_value = "")]
    pub root_folder: String,
    /// Quality profile ID for a new artist (default: the root folder's)
    #[arg(long)]
    pub quality_profile: Option<u64>,
    /// Metadata profile ID for a new artist (default: the root folder's)
    #[arg(long)]
    pub metadata_profile: Option<u64>,

    /// The rips folder as RustyDisc sees it, when Lidarr sees it under another path...
    #[arg(long, default_value = "")]
    pub path_from: String,
    /// ...and what Lidarr calls it
    #[arg(long, default_value = "")]
    pub path_to: String,

    /// move (default) or copy
    #[arg(long, default_value = "move")]
    pub mode: String,

    /// Wrong match? A MusicBrainz release or release group ID or URL for one rip, as `FOLDER=ID`. Repeatable.
    #[arg(long = "match", value_name = "FOLDER=ID")]
    pub overrides: Vec<String>,

    /// Delete whatever is left in the rip folder afterwards, and the folder itself
    #[arg(long)]
    pub delete_leftovers: bool,

    /// Print what Lidarr would match, as JSON, and stop
    #[arg(long)]
    pub plan: bool,

    /// Work everything out but import nothing
    #[arg(long)]
    pub dry_run: bool,

    /// Emit newline-delimited JSON progress events to stdout
    #[arg(long)]
    pub progress_json: bool,
}

pub fn run(args: ImportLidarrArgs) -> Result<(), Error> {
    let l = Lidarr {
        url: args.url.clone(),
        api_key: args.api_key.clone(),
        root_folder: args.root_folder.clone(),
        quality_profile: args.quality_profile,
        metadata_profile: args.metadata_profile,
        path_from: args.path_from.clone(),
        path_to: args.path_to.clone(),
        mode: if args.mode == "copy" { "copy".into() } else { "move".into() },
    };
    let override_for = |rip: &str| -> Option<String> {
        args.overrides.iter().find_map(|o| o.split_once('=').filter(|(f, _)| rip.trim_end_matches('/').ends_with(f.trim_end_matches('/'))).map(|(_, id)| id.to_string()))
    };
    let json = args.progress_json;
    let say = |m: &str| {
        if json { println!("{}", serde_json::json!({"type": "step", "msg": m})); } else { eprintln!("{m}"); }
    };
    let pct = |p: f32| {
        if json { println!("{{\"type\":\"progress\",\"pct\":{:.1}}}", p.min(99.0)); }
    };

    if args.plan {
        let mut plans = Vec::new();
        for r in &args.rip {
            plans.push(lidarr::plan(&l, &PathBuf::from(r), override_for(r).as_deref())?);
        }
        println!("{}", serde_json::to_string_pretty(&plans)?);
        return Ok(());
    }

    let n = args.rip.len().max(1);
    let (mut imported, mut added, mut removed) = (0usize, 0usize, 0usize);
    for (i, r) in args.rip.iter().enumerate() {
        let rip = PathBuf::from(r);
        say(&format!("Album {} of {}: {}", i + 1, n, r));
        if args.dry_run {
            let p = lidarr::plan(&l, &rip, override_for(r).as_deref())?;
            match (&p.album_match, &p.error) {
                (Some(m), _) => say(&format!("Dry run: Lidarr would import {} of {} file(s) as {} — {}{}", p.matched, p.files.len().max(p.identity.audio_files), m.artist, m.album, if p.will_add_artist { " (adding the artist first)" } else { "" })),
                (_, Some(e)) => say(&format!("Dry run: {e}")),
                _ => {}
            }
            continue;
        }
        let base = i as f32 / n as f32 * 100.0;
        let scale = 1.0 / n as f32;
        let s = lidarr::execute(&l, &rip, override_for(r).as_deref(), args.delete_leftovers, &Progress { step: &say, pct: &|p| pct(base + p * scale) })?;
        imported += s.imported;
        added += usize::from(s.added_artist);
        removed += usize::from(s.rip_removed);
    }
    let result = serde_json::json!({"type": "import_done", "albums": args.rip.len(), "imported": imported, "replaced": 0, "skipped": 0, "bytes": 0, "rips_removed": removed, "artists_added": added, "via": "lidarr", "dry_run": args.dry_run});
    if json { println!("{result}"); } else { eprintln!("Done: {imported} file(s) imported by Lidarr, {added} artist(s) added."); }
    Ok(())
}
