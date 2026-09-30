use std::path::PathBuf;

use clap::Args;

use crate::{
    error::Error,
    stick::{
        devices, existing,
        layout::{self, Layout, LayoutOptions},
        plan::{build_plan, StickOptions, TargetInfo},
        scan::{self, SourceSpec},
        write::{self, WriteOptions},
    },
};

#[derive(Args, Debug)]
pub struct StickArgs {
    /// The stick: the folder it is mounted at (e.g. /run/media/you/STICK)
    #[arg(long)]
    pub target: String,

    /// A folder of music (sub-folders are included). Repeat for several.
    #[arg(long, value_name = "DIR")]
    pub folder: Vec<String>,
    /// An audio file. Repeat for several.
    #[arg(long, value_name = "FILE")]
    pub file: Vec<String>,
    /// An M3U/M3U8 playlist. Repeat for several.
    #[arg(long, value_name = "FILE")]
    pub playlist: Vec<String>,

    /// Folder layout, e.g. "{initial}/{albumartist}/{album}/{discfolder}/{track} - {title}"
    /// Tokens: initial, albumartist, artist, album, year, genre, disc, discfolder, track, dtrack, title
    #[arg(long, conflicts_with = "preset")]
    pub layout: Option<String>,
    /// A ready-made layout: initial-artist-album, artist-album, artist-album-flat,
    /// artist-year-album, artist-album-onefolder, flat
    #[arg(long, default_value = "artist-album")]
    pub preset: String,

    /// Convert audio while writing, e.g. mp3:320, mp3:256, aac:256, opus:128 (only files that would shrink)
    #[arg(long)]
    pub transcode: Option<String>,
    /// Don't keep the embedded cover picture in converted files
    #[arg(long)]
    pub no_keep_art: bool,
    /// Don't put cover.jpg in each album folder
    #[arg(long)]
    pub no_covers: bool,
    /// What to do when a track is already on the stick: skip (default), replace, higher-quality,
    /// lower-quality, newer, keep-both
    #[arg(long, default_value = "skip")]
    pub on_conflict: String,
    /// Same as --on-conflict replace
    #[arg(long)]
    pub no_skip_existing: bool,
    /// Music already on the stick but filed differently: leave it, or reorganize it into the layout
    #[arg(long, default_value = "leave")]
    pub existing: String,
    /// Delete everything in the destination first (needs --confirm-clear with the folder's name)
    #[arg(long)]
    pub clear: bool,
    #[arg(long, value_name = "NAME")]
    pub confirm_clear: Option<String>,
    /// Write into this folder on the stick instead of its root
    #[arg(long, default_value = "")]
    pub subfolder: String,
    /// Make names safe for Windows filesystems: auto (default, from the stick's filesystem), on, off
    #[arg(long, default_value = "auto")]
    pub windows_names: String,
    /// File "The Beatles" under T instead of B
    #[arg(long)]
    pub keep_the: bool,

    /// Print the plan as JSON and stop
    #[arg(long)]
    pub plan: bool,
    /// Work everything out but write nothing
    #[arg(long)]
    pub dry_run: bool,
    /// Write even if the plan says it won't fit
    #[arg(long)]
    pub force: bool,
    /// Where converted files wait before they are written (default /tmp)
    #[arg(long)]
    pub stage_dir: Option<String>,
    /// Pretend the stick has this many bytes (testing)
    #[arg(long, hide = true)]
    pub assume_capacity: Option<u64>,
    /// Only accept playlist entries inside this folder (used by the web UI)
    #[arg(long, hide = true)]
    pub playlist_root: Option<String>,
    #[arg(long)]
    pub debug: bool,
    /// Emit newline-delimited JSON progress events to stdout
    #[arg(long)]
    pub progress_json: bool,
}

fn note(args: &StickArgs, msg: &str) {
    if args.progress_json {
        println!("{}", serde_json::json!({"type": "step", "msg": msg}));
    } else {
        eprintln!("{msg}");
    }
}

pub fn run(args: StickArgs) -> Result<(), Error> {
    let target = PathBuf::from(&args.target);
    if !target.is_dir() {
        return Err(Error::validation(format!("The stick isn't there: {} is not a folder. Is it mounted?", args.target)));
    }

    let sources = SourceSpec {
        folders: args.folder.clone(),
        files: args.file.clone(),
        playlists: args.playlist.clone(),
        playlist_root: args.playlist_root.clone().map(PathBuf::from),
    };
    if sources.is_empty() {
        return Err(Error::validation("Choose what to put on the stick: --folder, --file or --playlist"));
    }

    // Filesystem facts decide file-name rules and the size limit for one file.
    let stats = devices::fs_stats(&target)?;
    let fs = devices::filesystem_of(&target).unwrap_or_default();
    let (max_file, fs_windows) = match fs.as_str() {
        "vfat" | "msdos" => (Some(4u64 * 1024 * 1024 * 1024 - 1), true),
        "exfat" | "ntfs" | "ntfs3" | "fuseblk" | "hfsplus" | "apfs" => (None, true),
        "" => (None, true),
        _ => (None, false),
    };
    let windows_safe = match args.windows_names.as_str() {
        "on" | "true" => true,
        "off" | "false" => false,
        "auto" => fs_windows,
        other => return Err(Error::validation(format!("--windows-names must be auto, on or off, not '{other}'"))),
    };

    let template = match &args.layout {
        Some(t) => t.clone(),
        None => layout::PRESETS
            .iter()
            .find(|p| p.id == args.preset)
            .map(|p| p.template.to_string())
            .ok_or_else(|| Error::validation(format!(
                "Unknown preset '{}'. Choose one of: {}",
                args.preset,
                layout::PRESETS.iter().map(|p| p.id).collect::<Vec<_>>().join(", ")
            )))?,
    };
    let opts = StickOptions {
        layout: Layout { template, options: LayoutOptions { windows_safe, ignore_the: !args.keep_the } },
        transcode: args.transcode.clone().filter(|t| !t.trim().is_empty()),
        keep_art: !args.no_keep_art,
        copy_covers: !args.no_covers,
        conflict: if args.no_skip_existing { existing::Conflict::Replace } else { args.on_conflict.parse().map_err(Error::validation)? },
        existing_mode: args.existing.parse().map_err(Error::validation)?,
        clear: args.clear,
        dest_subfolder: args.subfolder.clone(),
    };

    note(&args, "Reading the music and its tags...");
    let scanned = scan::scan(&sources)?;
    let info = TargetInfo {
        mount_point: target.clone(),
        total_bytes: args.assume_capacity.unwrap_or(stats.total_bytes),
        free_bytes: args.assume_capacity.unwrap_or(stats.free_bytes),
        block_size: stats.block_size,
        max_file_bytes: max_file,
    };
    note(&args, "Working out where everything goes and whether it fits...");
    let dest_root = if opts.dest_subfolder.trim().is_empty() { target.clone() } else { target.join(crate::stick::plan::sanitize_subfolder(&opts.dest_subfolder)?) };
    let on_stick = if opts.clear { Vec::new() } else { note(&args, "Looking at what is already on the stick..."); existing::read_existing(&dest_root) };
    let plan = build_plan(&scanned, &on_stick, &opts, &info)?;

    if args.plan {
        println!("{}", serde_json::to_string_pretty(&plan)?);
        return Ok(());
    }

    note(&args, &format!(
        "{} file(s), {:.0} MB to write{}; {:.0} MB free on the stick",
        plan.to_write,
        plan.write_bytes as f64 / 1e6,
        plan.transcode.as_ref().map(|t| format!(" (converted to {t})")).unwrap_or_default(),
        plan.available_bytes as f64 / 1e6,
    ));
    if !plan.fits && !args.force {
        return Err(Error::validation(format!(
            "This won't fit: it needs about {:.0} MB but the stick has {:.0} MB free. Convert to a lower bitrate, choose less music, or use --force to try anyway.",
            plan.needed_bytes as f64 / 1e6,
            plan.available_bytes as f64 / 1e6,
        )));
    }

    let summary = write::write(&plan, &WriteOptions {
        keep_art: opts.keep_art,
        clear: args.clear,
        confirm_clear: args.confirm_clear.clone(),
        dry_run: args.dry_run,
        debug: args.debug,
        progress_json: args.progress_json,
        stage_dir: args.stage_dir.clone(),
    })?;

    let result = serde_json::json!({
        "type": "stick_done", "written": summary.written, "converted": summary.converted, "covers": summary.covers,
        "bytes": summary.bytes, "already_there": summary.skipped_existing,
        "moved": summary.moved, "replaced": summary.replaced, "deleted": summary.deleted, "seconds": summary.seconds,
        "dest": plan.dest_root, "dry_run": args.dry_run,
    });
    if args.progress_json {
        println!("{result}");
    } else {
        eprintln!(
            "Done: {} file(s) written ({} converted), {:.0} MB, {} already there, in {:.0}s.",
            summary.written, summary.converted, summary.bytes as f64 / 1e6, summary.skipped_existing, summary.seconds
        );
    }
    Ok(())
}
