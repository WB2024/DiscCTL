use std::path::PathBuf;

use clap::Args;

use crate::{
    error::Error,
    library::{
        import::{self, ImportOptions, Mode, Progress},
        script,
    },
};

#[derive(Args, Debug)]
pub struct ImportArgs {
    /// A ripped album folder to import. Repeat for several.
    #[arg(long, value_name = "DIR", required = true)]
    pub rip: Vec<String>,

    /// The music library folder
    #[arg(long, value_name = "DIR")]
    pub library: String,

    /// A Picard naming script file (default: the built-in one)
    #[arg(long, value_name = "FILE")]
    pub script_file: Option<String>,

    /// move (default), copy or hardlink
    #[arg(long, default_value = "move")]
    pub mode: String,

    /// Leave the cover picture behind
    #[arg(long)]
    pub no_cover: bool,

    /// Bring every other file along too (logs, cue sheets, booklets...)
    #[arg(long)]
    pub include_other: bool,

    /// Delete whatever is left in the rip folder afterwards, and the folder itself
    #[arg(long)]
    pub delete_leftovers: bool,

    /// What to do when a file is already in the library: skip (default), replace, higher-quality,
    /// lower-quality, newer, keep-both
    #[arg(long, default_value = "skip")]
    pub on_conflict: String,

    /// Print the plan as JSON and stop
    #[arg(long)]
    pub plan: bool,

    /// Work everything out but move nothing
    #[arg(long)]
    pub dry_run: bool,

    /// Emit newline-delimited JSON progress events to stdout
    #[arg(long)]
    pub progress_json: bool,
}

pub fn run(args: ImportArgs) -> Result<(), Error> {
    let script = match &args.script_file {
        Some(f) => std::fs::read_to_string(f).map_err(|e| Error::validation(format!("Can't read the naming script {f}: {e}")))?,
        None => script::DEFAULT_SCRIPT.to_string(),
    };
    let mut opts = ImportOptions::new(PathBuf::from(&args.library), script);
    opts.mode = args.mode.parse::<Mode>().map_err(Error::validation)?;
    opts.include_cover = !args.no_cover;
    opts.include_other = args.include_other;
    opts.delete_leftovers = args.delete_leftovers;
    opts.conflict = args.on_conflict.parse().map_err(Error::validation)?;

    let json = args.progress_json;
    let say = |m: &str| {
        if json { println!("{}", serde_json::json!({"type": "step", "msg": m})); } else { eprintln!("{m}"); }
    };
    let pct = |p: f32| {
        if json { println!("{{\"type\":\"progress\",\"pct\":{:.1}}}", p.min(99.0)); }
    };

    let mut plans = Vec::new();
    for r in &args.rip {
        say(&format!("Planning {}...", r));
        plans.push((PathBuf::from(r), import::plan(&PathBuf::from(r), &opts)?));
    }
    if args.plan {
        println!("{}", serde_json::to_string_pretty(&plans.iter().map(|(_, p)| p).collect::<Vec<_>>())?);
        return Ok(());
    }

    let (mut imported, mut replaced, mut skipped, mut bytes, mut removed) = (0, 0, 0, 0u64, 0);
    let n = plans.len().max(1);
    for (i, (rip, plan)) in plans.iter().enumerate() {
        say(&format!("Album {} of {}: {} — {}", i + 1, n, plan.artist.as_deref().unwrap_or("Unknown artist"), plan.album.as_deref().unwrap_or("Unknown album")));
        for w in &plan.warnings {
            say(&format!("Note: {w}"));
        }
        if args.dry_run {
            say(&format!("Dry run: would import {} file(s) into {}", plan.to_import + plan.replacing, plan.album_dir.as_deref().unwrap_or("the library")));
            continue;
        }
        let base = i as f32 / n as f32 * 100.0;
        let scale = 1.0 / n as f32;
        let s = import::execute(rip, plan, &opts, &Progress { step: &say, pct: &|p| pct(base + p * scale) })?;
        imported += s.imported;
        replaced += s.replaced;
        skipped += s.skipped;
        bytes += s.bytes;
        removed += usize::from(s.rip_removed);
    }

    let result = serde_json::json!({
        "type": "import_done", "albums": plans.len(), "imported": imported, "replaced": replaced, "skipped": skipped,
        "bytes": bytes, "rips_removed": removed, "dry_run": args.dry_run,
    });
    if json {
        println!("{result}");
    } else {
        eprintln!("Done: {imported} file(s) imported, {replaced} replaced, {skipped} skipped ({:.0} MB).", bytes as f64 / 1e6);
    }
    Ok(())
}
