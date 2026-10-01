//! Working out how many discs a job needs, and what goes on each.
//!
//! * A **Data CD** is packed by size. Files that will be transcoded are counted at their
//!   *estimated size after transcoding* (bitrate × duration), so converting FLAC to MP3 first
//!   correctly needs far fewer discs. Every file also costs its ISO 9660 overhead (sector
//!   padding and directory records), and each disc keeps a small safety margin.
//! * An **Audio CD** is packed by playing time, an **Enhanced CD** must fit both sessions on
//!   one disc.
//!
//! The same numbers drive `plan` (the preview) and `burn`, so the preview matches what happens.
//! When the burn actually transcodes, it re-checks against the real file sizes.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
    time::UNIX_EPOCH,
};

use serde::Serialize;
use serde_json::{json, Value};

use super::split::{self, AudioItem, DataItem};
use crate::{
    backend::transcode::{is_audio, OutputFormat, TranscodeSpec},
    error::Error,
    parser::{self, playlist::Skipped},
};

const MIB: u64 = 1024 * 1024;

// ── Disc sizes ────────────────────────────────────────────────────────────────

/// A blank CD-R: 650 MB / 74 min, 700 MB / 80 min or 800 MB / 90 min.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscSize {
    pub mb: u64,
}

impl Default for DiscSize {
    fn default() -> Self {
        DiscSize { mb: 700 }
    }
}

/// Room kept free for lead-out and differences between drives and media.
const SAFETY_BYTES: u64 = 6 * MIB;
/// ISO 9660 system area, volume descriptors and path tables.
const ISO_BASE_BYTES: u64 = MIB;
/// Per file: the directory records (ISO 9660, Rock Ridge and Joliet) it needs.
const ISO_RECORD_BYTES: u64 = 700;
/// Per directory: its own records and a sector or two of padding.
const ISO_DIR_BYTES: u64 = 8 * 1024;
/// A second session (Enhanced CD) costs this many sectors of lead-out and lead-in.
const SESSION_GAP_SECTORS: u64 = 11_400;

/// The blank discs we know about.
pub struct Preset {
    pub id: &'static str,
    pub label: &'static str,
    pub mb: u64,
    /// "cd" or "dvd" (DVD and Blu-ray)
    pub media: &'static str,
}

/// Capacities are what the disc really holds (sector counts), rounded down.
pub const PRESETS: &[Preset] = &[
    Preset { id: "cd700", label: "CD-R 700 MB · 80 min", mb: 700, media: "cd" },
    Preset { id: "cd650", label: "CD-R 650 MB · 74 min", mb: 650, media: "cd" },
    Preset { id: "cd800", label: "CD-R 800 MB · 90 min", mb: 800, media: "cd" },
    Preset { id: "dvd5", label: "DVD±R 4.7 GB", mb: 4482, media: "dvd" },
    Preset { id: "dvd9", label: "DVD±R DL 8.5 GB", mb: 8147, media: "dvd" },
    Preset { id: "bd25", label: "BD-R 25 GB", mb: 23866, media: "dvd" },
    Preset { id: "bd50", label: "BD-R DL 50 GB", mb: 47732, media: "dvd" },
];

impl DiscSize {
    pub fn new(mb: u64) -> Result<Self, Error> {
        if !(100..=200_000).contains(&mb) {
            return Err(Error::validation(format!("Disc size {mb} MB is outside the supported range (100-200000)")));
        }
        Ok(DiscSize { mb })
    }

    /// A size in MB, or a name such as `dvd`, `dvd-dl`, `bd`, `bd-dl`, `cd700`.
    pub fn parse(s: &str) -> Result<Self, Error> {
        let t = s.trim().to_ascii_lowercase().replace(['-', '_', ' '], "");
        if let Ok(mb) = t.parse::<u64>() {
            return Self::new(mb);
        }
        let mb = match t.as_str() {
            "cd" | "cd700" | "cd80" => 700,
            "cd650" | "cd74" => 650,
            "cd800" | "cd90" => 800,
            "dvd" | "dvd5" | "dvd47" => 4482,
            "dvddl" | "dvd9" | "dvd85" => 8147,
            "bd" | "bd25" => 23866,
            "bddl" | "bd50" => 47732,
            _ => return Err(Error::validation(format!(
                "Unknown disc size '{s}'. Use a size in MB or one of: cd700, cd650, cd800, dvd, dvd-dl, bd, bd-dl"
            ))),
        };
        Self::new(mb)
    }

    /// DVD and Blu-ray discs (bigger than any CD) use UDF and need a little more room.
    pub fn is_dvd_class(self) -> bool {
        self.mb > 1000
    }

    /// Bytes on the whole disc (as marketed: 700 MB = 700 MiB).
    pub fn capacity_bytes(self) -> u64 {
        self.mb * MIB
    }

    /// Playing time of the matching audio CD.
    pub fn minutes(self) -> u64 {
        match self.mb {
            0..=650 => 74,
            651..=700 => 80,
            _ => 90,
        }
    }

    /// Seconds of audio that safely fit on a CD (30 s kept free).
    pub fn audio_capacity_secs(self) -> u64 {
        self.minutes() * 60 - 30
    }

    fn safety_bytes(self) -> u64 {
        if self.is_dvd_class() { 16 * MIB } else { SAFETY_BYTES }
    }

    /// Filesystem structures that exist once per disc.
    pub fn iso_base(self) -> u64 {
        if self.is_dvd_class() { 3 * MIB } else { ISO_BASE_BYTES }
    }

    /// Directory records per file (UDF adds a second set on DVDs).
    fn record_bytes(self) -> u64 {
        if self.is_dvd_class() { 1500 } else { ISO_RECORD_BYTES }
    }

    /// What a file costs on the disc: its data rounded up to whole 2048-byte sectors, plus records.
    pub fn file_cost(self, bytes: u64) -> u64 {
        bytes.div_ceil(2048) * 2048 + self.record_bytes()
    }

    /// Bytes of file content that safely fit on one disc.
    pub fn data_usable_bytes(self) -> u64 {
        self.capacity_bytes().saturating_sub(self.safety_bytes() + self.iso_base())
    }
}

/// What a file costs on a CD.
pub fn file_cost(bytes: u64) -> u64 {
    DiscSize::default().file_cost(bytes)
}

// ── Reading durations ─────────────────────────────────────────────────────────

static DURATIONS: Mutex<Option<HashMap<(String, u64, u64), Option<f64>>>> = Mutex::new(None);

fn cache_key(path: &str) -> (String, u64, u64) {
    let meta = std::fs::metadata(path).ok();
    let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
    let mtime = meta
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    (path.to_string(), size, mtime)
}

/// Length of an audio file in seconds, from ffprobe. Results are remembered.
pub fn probe_duration(path: &str) -> Option<f64> {
    let key = cache_key(path);
    if let Some(hit) = DURATIONS.lock().unwrap().get_or_insert_with(HashMap::new).get(&key) {
        return *hit;
    }
    let out = Command::new("ffprobe")
        .args(["-v", "quiet", "-show_entries", "format=duration", "-of", "default=noprint_wrappers=1:nokey=1", crate::backend::source::parse(path).location])
        .output()
        .ok()
        .filter(|o| o.status.success());
    let secs = out
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|d| *d > 0.0);
    DURATIONS.lock().unwrap().get_or_insert_with(HashMap::new).insert(key, secs);
    secs
}

/// Probe many files at once.
pub fn probe_many(paths: &[String]) -> HashMap<String, Option<f64>> {
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).clamp(2, 8);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let out = Mutex::new(HashMap::new());
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(p) = paths.get(i) else { break };
                let d = probe_duration(p);
                out.lock().unwrap().insert(p.clone(), d);
            });
        }
    });
    out.into_inner().unwrap()
}

// ── Transcoding: what happens to each file, and how big it ends up ───────────

fn ext_of(path: &str) -> String {
    Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase()
}

fn is_lossless(ext: &str) -> bool {
    matches!(ext, "flac" | "wav" | "aiff" | "aif" | "ape" | "wv")
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Decision {
    /// Convert this file (otherwise it is used as it is).
    pub transcode: bool,
    /// Expected size on the disc.
    pub est_bytes: u64,
    /// The file should be converted but its length is unknown, so the size is the original's.
    pub unknown_duration: bool,
}

/// Decide whether a file is converted and estimate its size afterwards.
///
/// Files are only converted when that helps: lossless files become the target format, but a
/// lossy file is left alone unless it is clearly bigger than the target bitrate, so an MP3 at
/// 128k is never "upgraded" to 320k.
pub fn decide(path: &str, size: u64, duration: Option<f64>, spec: Option<&TranscodeSpec>) -> Decision {
    let copy = Decision { transcode: false, est_bytes: size, unknown_duration: false };
    let Some(spec) = spec else { return copy };
    if !is_audio(Path::new(path)) {
        return copy;
    }
    let ext = ext_of(path);
    let lossless = is_lossless(&ext);

    let wanted = match spec.format {
        OutputFormat::Mp3 | OutputFormat::Aac | OutputFormat::Opus => {
            let target = spec.bitrate_kbps.unwrap_or(0) as f64;
            if lossless {
                true
            } else {
                match duration {
                    Some(d) => (size as f64 * 8.0 / d / 1000.0) > target * 1.1,
                    None => false,
                }
            }
        }
        OutputFormat::Flac => lossless && ext != "flac",
        OutputFormat::Wav => ext != "wav",
    };
    if !wanted {
        return copy;
    }
    let Some(d) = duration else {
        return Decision { transcode: true, est_bytes: size, unknown_duration: true };
    };
    let est = match spec.format {
        OutputFormat::Mp3 | OutputFormat::Aac | OutputFormat::Opus => {
            (d * spec.bitrate_kbps.unwrap_or(0) as f64 * 125.0 * 1.03) as u64 + 16 * 1024
        }
        OutputFormat::Flac => (d * 176_400.0 * 0.65) as u64,
        OutputFormat::Wav => (d * 176_400.0) as u64 + 44,
    };
    Decision { transcode: true, est_bytes: est, unknown_duration: false }
}

/// Where a file ends up on the disc: same place, new extension if it is converted.
pub fn final_rel_path(rel: &str, transcoded: bool, spec: Option<&TranscodeSpec>) -> String {
    match (transcoded, spec) {
        (true, Some(s)) => Path::new(rel).with_extension(s.extension()).to_string_lossy().to_string(),
        _ => rel.to_string(),
    }
}

// ── Sources ───────────────────────────────────────────────────────────────────

/// Where a Data CD's files come from: exactly one of a playlist, a list of files or a folder.
#[derive(Default, Clone)]
pub struct DataSpec {
    pub playlist: Option<String>,
    pub files: Option<Vec<String>>,
    pub data: Option<String>,
    /// Playlist entries must live inside this folder.
    pub playlist_root: Option<PathBuf>,
}

pub struct ResolvedData {
    pub items: Vec<DataItem>,
    /// A folder that already holds exactly `items` at their relative paths.
    pub ready_root: Option<String>,
    /// Playlist lines that couldn't be used.
    pub skipped: Vec<Skipped>,
}

/// Individually chosen files, checked and made absolute.
pub fn resolve_files(files: &[String]) -> Result<Vec<String>, Error> {
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

pub fn resolve_data(spec: &DataSpec) -> Result<ResolvedData, Error> {
    let chosen = [spec.playlist.is_some(), spec.files.is_some(), spec.data.is_some()];
    match chosen.iter().filter(|c| **c).count() {
        0 => {
            return Err(Error::validation(
                "Data CD burn requires --data <dir>, --files <files>, --playlist <file>, or --input <graph.json>",
            ))
        }
        1 => {}
        _ => {
            return Err(Error::validation(
                "Choose one source for a Data CD: --data <dir>, --files <files>, or --playlist <file>",
            ))
        }
    }

    if let Some(pl) = &spec.playlist {
        let parsed = parser::playlist::parse_detailed(pl, spec.playlist_root.as_deref())?;
        for s in &parsed.skipped {
            eprintln!("Warning: playlist entry skipped ({}): {}", s.reason, s.entry);
        }
        if parsed.entries.is_empty() {
            return Err(Error::validation(format!("Playlist '{}' contains no resolvable track paths", pl)));
        }
        let paths: Vec<String> = parsed.entries.into_iter().map(|e| e.path).collect();
        return Ok(ResolvedData { items: split::flat_items(&paths), ready_root: None, skipped: parsed.skipped });
    }
    if let Some(files) = &spec.files {
        let paths = resolve_files(files)?;
        return Ok(ResolvedData { items: split::flat_items(&paths), ready_root: None, skipped: Vec::new() });
    }
    let dir = spec.data.as_ref().expect("checked above");
    let items = split::enumerate_dir(dir)?;
    Ok(ResolvedData { items, ready_root: Some(dir.clone()), skipped: Vec::new() })
}

// ── Data plan ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct PlannedFile {
    pub path: String,
    /// Where it goes on the disc (the extension changes when it is converted).
    pub rel_path: String,
    pub original_bytes: u64,
    pub est_bytes: u64,
    pub transcode: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DataDisc {
    pub number: usize,
    pub files: usize,
    /// Estimated size of the files themselves.
    pub payload_bytes: u64,
    /// Including ISO 9660 overhead.
    pub used_bytes: u64,
    pub percent: f32,
    /// Indices into the plan's `files`.
    pub items: Vec<usize>,
}

#[derive(Debug, Serialize)]
pub struct DataPlan {
    pub disc_size_mb: u64,
    pub usable_bytes: u64,
    pub disc_count: usize,
    pub discs: Vec<DataDisc>,
    pub files: Vec<PlannedFile>,
    pub original_bytes: u64,
    pub estimated_bytes: u64,
    /// e.g. "MP3 320k"
    pub transcode: Option<String>,
    pub transcoded_files: usize,
    /// Files bigger than a whole disc: left out.
    pub too_big: Vec<String>,
    pub skipped: Vec<Value>,
    pub warnings: Vec<String>,
}

/// Estimate what each file becomes.
pub fn plan_files(items: &[DataItem], spec: Option<&TranscodeSpec>) -> (Vec<PlannedFile>, usize) {
    let audio_paths: Vec<String> = if spec.is_some() {
        items.iter().filter(|i| is_audio(Path::new(&i.path))).map(|i| i.path.clone()).collect()
    } else {
        Vec::new()
    };
    let durations = probe_many(&audio_paths);

    let mut unknown = 0;
    let files = items
        .iter()
        .map(|i| {
            let d = durations.get(&i.path).copied().flatten();
            let dec = decide(&i.path, i.size_bytes, d, spec);
            if dec.unknown_duration {
                unknown += 1;
            }
            PlannedFile {
                path: i.path.clone(),
                rel_path: final_rel_path(&i.rel_path, dec.transcode, spec),
                original_bytes: i.size_bytes,
                est_bytes: dec.est_bytes,
                transcode: dec.transcode,
            }
        })
        .collect();
    (files, unknown)
}

/// Fill discs one after another, in order, so albums and playlists stay together.
pub fn pack_data(files: &[PlannedFile], size: DiscSize) -> (Vec<DataDisc>, Vec<usize>) {
    let usable = size.data_usable_bytes();
    let capacity = size.capacity_bytes() as f32;
    let mut discs: Vec<DataDisc> = Vec::new();
    let mut too_big = Vec::new();

    let mut items: Vec<usize> = Vec::new();
    let mut payload = 0u64;
    let mut used = 0u64;
    let mut dirs: HashSet<String> = HashSet::new();

    let base = size.iso_base();
    let close = |discs: &mut Vec<DataDisc>, items: &mut Vec<usize>, payload: &mut u64, used: &mut u64, dirs: &mut HashSet<String>| {
        if items.is_empty() {
            return;
        }
        let total = *used + base;
        discs.push(DataDisc {
            number: discs.len() + 1,
            files: items.len(),
            payload_bytes: *payload,
            used_bytes: total,
            percent: (total as f32 / capacity * 100.0).min(999.0),
            items: std::mem::take(items),
        });
        *payload = 0;
        *used = 0;
        dirs.clear();
    };

    for (i, f) in files.iter().enumerate() {
        let dir = Path::new(&f.rel_path).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
        let new_dir = if dirs.contains(&dir) { 0 } else { ISO_DIR_BYTES };
        let cost = size.file_cost(f.est_bytes) + new_dir;
        if size.file_cost(f.est_bytes) + ISO_DIR_BYTES > usable {
            too_big.push(i);
            continue;
        }
        if !items.is_empty() && used + cost > usable {
            close(&mut discs, &mut items, &mut payload, &mut used, &mut dirs);
        }
        let new_dir = if dirs.contains(&dir) { 0 } else { ISO_DIR_BYTES };
        dirs.insert(dir);
        used += size.file_cost(f.est_bytes) + new_dir;
        payload += f.est_bytes;
        items.push(i);
    }
    close(&mut discs, &mut items, &mut payload, &mut used, &mut dirs);
    (discs, too_big)
}

pub fn plan_data(resolved: ResolvedData, spec: Option<&TranscodeSpec>, size: DiscSize) -> DataPlan {
    let (files, unknown) = plan_files(&resolved.items, spec);
    let (discs, too_big) = pack_data(&files, size);

    let mut warnings = Vec::new();
    if unknown > 0 {
        warnings.push(format!(
            "The length of {unknown} audio file(s) couldn't be read (is ffprobe installed?), so they are counted at their original size and the real result may need fewer discs."
        ));
    }
    if !too_big.is_empty() {
        warnings.push(format!(
            "{} file(s) are larger than a whole {} MB disc and would be left out.",
            too_big.len(),
            size.mb
        ));
    }
    let transcoded_files = files.iter().filter(|f| f.transcode).count();
    if let Some(s) = spec {
        if transcoded_files == 0 && files.iter().any(|f| is_audio(Path::new(&f.path))) {
            warnings.push(format!("None of the audio files would change with {} (already as small or smaller), so they are used as they are.", s.label()));
        }
    }

    let original_bytes = files.iter().map(|f| f.original_bytes).sum();
    let estimated_bytes = files.iter().map(|f| f.est_bytes).sum();
    DataPlan {
        disc_size_mb: size.mb,
        usable_bytes: size.data_usable_bytes(),
        disc_count: discs.len(),
        too_big: too_big.iter().map(|&i| files[i].path.clone()).collect(),
        discs,
        files,
        original_bytes,
        estimated_bytes,
        transcode: spec.map(|s| s.label()),
        transcoded_files,
        skipped: resolved.skipped.iter().map(|s| json!({"entry": s.entry, "reason": s.reason})).collect(),
        warnings,
    }
}

// ── Audio plan ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct AudioTrack {
    pub path: String,
    pub secs: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AudioDisc {
    pub number: usize,
    pub tracks: usize,
    pub secs: u64,
    pub percent: f32,
    pub items: Vec<usize>,
}

#[derive(Debug, Serialize)]
pub struct AudioPlan {
    pub disc_size_mb: u64,
    pub capacity_secs: u64,
    pub disc_count: usize,
    pub discs: Vec<AudioDisc>,
    pub tracks: Vec<AudioTrack>,
    pub total_secs: u64,
    pub skipped: Vec<Value>,
    pub warnings: Vec<String>,
}

pub fn plan_audio(items: Vec<AudioItem>, skipped: &[Skipped], size: DiscSize) -> AudioPlan {
    let capacity = size.audio_capacity_secs();
    let total_secs = items.iter().map(|i| i.duration_secs).sum();
    let tracks: Vec<AudioTrack> = items.iter().map(|i| AudioTrack { path: i.path.clone(), secs: i.duration_secs }).collect();

    let mut discs = Vec::new();
    let mut idx = 0usize;
    for slice in split::split_audio(items, capacity, split::AUDIO_MAX_TRACKS) {
        discs.push(AudioDisc {
            number: discs.len() + 1,
            tracks: slice.items.len(),
            secs: slice.total_secs,
            percent: slice.total_secs as f32 / (size.minutes() * 60) as f32 * 100.0,
            items: (idx..idx + slice.items.len()).collect(),
        });
        idx += slice.items.len();
    }
    AudioPlan {
        disc_size_mb: size.mb,
        capacity_secs: capacity,
        disc_count: discs.len(),
        discs,
        tracks,
        total_secs,
        skipped: skipped.iter().map(|s| json!({"entry": s.entry, "reason": s.reason})).collect(),
        warnings: Vec::new(),
    }
}

/// Audio items for `--audio` files or a playlist. Playlist durations from `#EXTINF` are used
/// when every entry has one; otherwise files are measured.
pub fn resolve_audio(
    audio: &[String],
    playlist: Option<&str>,
    playlist_root: Option<&Path>,
) -> Result<(Vec<AudioItem>, Vec<Skipped>), Error> {
    if let Some(pl) = playlist {
        let parsed = parser::playlist::parse_detailed(pl, playlist_root)?;
        for s in &parsed.skipped {
            eprintln!("Warning: playlist entry skipped ({}): {}", s.reason, s.entry);
        }
        if parsed.entries.is_empty() {
            return Err(Error::validation(format!("Playlist '{}' contains no resolvable track paths", pl)));
        }
        let paths: Vec<String> = parsed.entries.iter().filter(|e| e.duration_secs.is_none()).map(|e| e.path.clone()).collect();
        let measured = probe_many(&paths);
        let items = parsed
            .entries
            .into_iter()
            .map(|e| {
                let secs = e.duration_secs.unwrap_or_else(|| {
                    measured.get(&e.path).copied().flatten().map(|d| d.ceil() as u64).unwrap_or_else(|| split::duration_secs(&e.path))
                });
                AudioItem { path: e.path, duration_secs: secs }
            })
            .collect();
        return Ok((items, parsed.skipped));
    }
    if audio.is_empty() {
        return Err(Error::validation("Audio burn requires --audio <files>, --playlist <file>, or --input <graph.json>"));
    }
    let tracks = parser::expand_audio_globs(audio)?;
    let measured = probe_many(&tracks);
    let items = tracks
        .into_iter()
        .map(|p| {
            let secs = measured.get(&p).copied().flatten().map(|d| d.ceil() as u64).unwrap_or_else(|| split::duration_secs(&p));
            AudioItem { path: p, duration_secs: secs }
        })
        .collect();
    Ok((items, Vec::new()))
}

// ── The whole request ─────────────────────────────────────────────────────────

#[derive(Default, Clone)]
pub struct PlanRequest {
    /// "redbook", "datacd", "bluebook", "datadvd" or "musicdvd"
    pub format: String,
    pub audio: Vec<String>,
    pub playlist: Option<String>,
    pub files: Option<Vec<String>>,
    pub data: Option<String>,
    pub playlist_root: Option<PathBuf>,
    pub transcode: Option<String>,
    pub disc_size_mb: Option<u64>,
    /// Music DVD: Dolby Digital bitrate (192, 256, 384 or 448; default 448)
    pub dvd_audio_kbps: Option<u32>,
}

/// Roughly what the still picture and the DVD's packaging add to a track, in kbit/s (measured
/// on real DVD-Video output).
pub const DVD_OVERHEAD_KBPS: u64 = 260;

/// Size of one track on a Music DVD: Dolby Digital audio plus the still picture and packaging.
pub fn music_dvd_track_bytes(secs: u64, audio_kbps: u32) -> u64 {
    let bytes = secs as f64 * (audio_kbps as f64 + DVD_OVERHEAD_KBPS as f64) * 125.0 * 1.02;
    bytes as u64
}

#[derive(Debug, Serialize)]
pub struct MusicDvdPlan {
    pub disc_size_mb: u64,
    pub capacity_bytes: u64,
    pub tracks: Vec<AudioTrack>,
    pub total_secs: u64,
    pub audio_kbps: u32,
    pub audio_bytes: u64,
    pub data_bytes: u64,
    pub data_files: usize,
    pub used_bytes: u64,
    pub percent: f32,
    pub fits: bool,
    pub skipped: Vec<Value>,
    pub warnings: Vec<String>,
}

/// Work out the plan for a burn request. The result says how many discs are needed and what
/// goes on each; `kind` is "data", "audio" or "enhanced".
pub fn plan_request(req: &PlanRequest) -> Result<Value, Error> {
    let is_dvd_format = matches!(req.format.to_lowercase().as_str(), "datadvd" | "data-dvd" | "dvd" | "musicdvd" | "music-dvd" | "enhanceddvd" | "enhanced-dvd" | "dvd-video");
    let size = match req.disc_size_mb {
        Some(mb) => DiscSize::new(mb)?,
        None if is_dvd_format => DiscSize::new(4482)?,
        None => DiscSize::default(),
    };
    let spec = match req.transcode.as_deref().filter(|t| !t.trim().is_empty()) {
        Some(t) => Some(TranscodeSpec::parse(t)?),
        None => None,
    };

    match req.format.to_lowercase().as_str() {
        "datacd" | "data-cd" | "data" | "datadvd" | "data-dvd" | "dvd" => {
            let resolved = resolve_data(&DataSpec {
                playlist: req.playlist.clone(),
                files: req.files.clone(),
                data: req.data.clone(),
                playlist_root: req.playlist_root.clone(),
            })?;
            let plan = plan_data(resolved, spec.as_ref(), size);
            Ok(json!({"kind": "data", "plan": plan}))
        }
        "redbook" | "red-book" | "audio" => {
            let (items, skipped) = resolve_audio(&req.audio, req.playlist.as_deref(), req.playlist_root.as_deref())?;
            let plan = plan_audio(items, &skipped, size);
            Ok(json!({"kind": "audio", "plan": plan}))
        }
        "bluebook" | "blue-book" | "cdextra" | "cd-extra" => {
            let (items, skipped) = resolve_audio(&req.audio, req.playlist.as_deref(), req.playlist_root.as_deref())?;
            let mut plan = plan_audio(items, &skipped, size);
            let data_dir = req
                .data
                .as_deref()
                .ok_or_else(|| Error::validation("An Enhanced (Blue Book) CD needs a data folder for its second session"))?;
            let data_items = split::enumerate_dir(data_dir)?;
            let data_bytes: u64 = data_items.iter().map(|i| file_cost(i.size_bytes)).sum::<u64>() + ISO_BASE_BYTES;

            // Everything shares one disc: audio, the gap between sessions, and the data.
            let total_sectors = plan.total_secs * 75 + SESSION_GAP_SECTORS + data_bytes.div_ceil(2048);
            let disc_sectors = size.minutes() * 60 * 75;
            let percent = total_sectors as f32 / disc_sectors as f32 * 100.0;
            let fits = plan.disc_count <= 1 && total_sectors <= disc_sectors.saturating_sub(150);
            if !fits {
                plan.warnings.push(format!(
                    "This Enhanced CD needs about {:.0}% of a {}-minute disc ({:.0} MB of data plus {}:{:02} of audio), so it can't be burned on one disc. Use fewer tracks or less data, or a bigger disc.",
                    percent,
                    size.minutes(),
                    data_bytes as f64 / MIB as f64,
                    plan.total_secs / 60,
                    plan.total_secs % 60
                ));
            }
            Ok(json!({
                "kind": "enhanced",
                "plan": plan,
                "data_bytes": data_bytes,
                "data_files": data_items.len(),
                "percent": percent,
                "fits": fits,
            }))
        }
        "musicdvd" | "music-dvd" | "enhanceddvd" | "enhanced-dvd" | "dvd-video" => {
            let kbps = req.dvd_audio_kbps.unwrap_or(448);
            crate::model::disc::DvdOptions { audio_kbps: kbps, ..Default::default() }.validate().map_err(Error::validation)?;
            let (items, skipped) = resolve_audio(&req.audio, req.playlist.as_deref(), req.playlist_root.as_deref())?;
            let tracks: Vec<AudioTrack> = items.iter().map(|i| AudioTrack { path: i.path.clone(), secs: i.duration_secs }).collect();
            let total_secs: u64 = tracks.iter().map(|t| t.secs).sum();
            let audio_bytes: u64 = tracks.iter().map(|t| music_dvd_track_bytes(t.secs, kbps)).sum();

            // The data folder is optional: it shares the disc with the music.
            let (data_bytes, data_files) = match req.data.as_deref() {
                Some(dir) => {
                    let items = split::enumerate_dir(dir)?;
                    (items.iter().map(|i| size.file_cost(i.size_bytes)).sum::<u64>() + ISO_DIR_BYTES, items.len())
                }
                None => (0, 0),
            };
            let used = audio_bytes + data_bytes + size.iso_base();
            let usable = size.capacity_bytes().saturating_sub(size.safety_bytes());
            let fits = used <= usable;
            let percent = used as f32 / size.capacity_bytes() as f32 * 100.0;
            let mut warnings = Vec::new();
            if !fits {
                warnings.push(format!(
                    "This needs about {:.0} MB ({} of music + {}) but a {} MB disc holds {:.0} MB. Use fewer tracks, less data, a lower audio bitrate or a bigger disc.",
                    used as f64 / MIB as f64,
                    format!("{:.0} MB", audio_bytes as f64 / MIB as f64),
                    format!("{:.0} MB of files", data_bytes as f64 / MIB as f64),
                    size.mb,
                    usable as f64 / MIB as f64,
                ));
            }
            if tracks.len() > 99 {
                warnings.push(format!("{} tracks: DVD-Video allows 99 chapters per title, so the disc will have {} titles that play one after the other.", tracks.len(), tracks.len().div_ceil(99)));
            }
            Ok(json!({"kind": "music_dvd", "plan": MusicDvdPlan {
                disc_size_mb: size.mb,
                capacity_bytes: size.capacity_bytes(),
                tracks,
                total_secs,
                audio_kbps: kbps,
                audio_bytes,
                data_bytes,
                data_files,
                used_bytes: used,
                percent,
                fits,
                skipped: skipped.iter().map(|s| json!({"entry": s.entry, "reason": s.reason})).collect(),
                warnings,
            }}))
        }
        other => Err(Error::validation(format!("Unknown format: '{}'. Valid values: redbook, datacd, bluebook, datadvd, musicdvd", other))),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn mp3_320() -> TranscodeSpec {
        TranscodeSpec::parse("mp3:320").unwrap()
    }

    fn item(name: &str, mb: f64) -> DataItem {
        DataItem { path: format!("/x/{name}"), rel_path: name.to_string(), size_bytes: (mb * MIB as f64) as u64 }
    }

    #[test]
    fn disc_sizes() {
        assert_eq!(DiscSize::new(700).unwrap().minutes(), 80);
        assert_eq!(DiscSize::new(650).unwrap().minutes(), 74);
        assert_eq!(DiscSize::new(800).unwrap().minutes(), 90);
        assert_eq!(DiscSize::new(700).unwrap().audio_capacity_secs(), 80 * 60 - 30);
        assert!(DiscSize::new(5).is_err());
        assert_eq!(file_cost(1), 2048 + ISO_RECORD_BYTES);
        assert_eq!(file_cost(2048), 2048 + ISO_RECORD_BYTES);
        assert_eq!(file_cost(2049), 4096 + ISO_RECORD_BYTES);
    }

    #[test]
    fn flac_becomes_mp3_at_bitrate_times_duration() {
        // 10 minutes of FLAC (~45 MB) -> 320 kbps MP3 ≈ 24 MB
        let d = decide("/x/a.flac", 45 * MIB, Some(600.0), Some(&mp3_320()));
        assert!(d.transcode);
        let mb = d.est_bytes as f64 / MIB as f64;
        assert!((23.0..=25.5).contains(&mb), "{mb}");
        // no transcoding: size unchanged
        let d = decide("/x/a.flac", 45 * MIB, Some(600.0), None);
        assert!(!d.transcode && d.est_bytes == 45 * MIB);
    }

    #[test]
    fn only_converts_when_it_helps() {
        let spec = mp3_320();
        // a 128k MP3 stays as it is instead of being "upgraded" to 320k
        let size = (128_000.0 / 8.0 * 200.0) as u64;
        let d = decide("/x/low.mp3", size, Some(200.0), Some(&spec));
        assert!(!d.transcode && d.est_bytes == size);
        // a 1000k lossy file (unusually big) is shrunk
        let big = (1_000_000.0 / 8.0 * 200.0) as u64;
        assert!(decide("/x/big.ogg", big, Some(200.0), Some(&spec)).transcode);
        // non-audio is never touched
        let d = decide("/x/cover.jpg", 500_000, None, Some(&spec));
        assert!(!d.transcode && d.est_bytes == 500_000);
        // FLAC target leaves FLAC and lossy sources alone, converts WAV
        let flac = TranscodeSpec::parse("flac").unwrap();
        assert!(!decide("/x/a.flac", 10, Some(1.0), Some(&flac)).transcode);
        assert!(!decide("/x/a.mp3", 10, Some(1.0), Some(&flac)).transcode);
        assert!(decide("/x/a.wav", 10 * MIB, Some(60.0), Some(&flac)).transcode);
        // unknown duration: converted, but counted at the original size
        let d = decide("/x/a.flac", 30 * MIB, None, Some(&spec));
        assert!(d.transcode && d.unknown_duration && d.est_bytes == 30 * MIB);
    }

    #[test]
    fn converted_files_change_extension() {
        let s = mp3_320();
        assert_eq!(final_rel_path("Album/01 - a.flac", true, Some(&s)), "Album/01 - a.mp3");
        assert_eq!(final_rel_path("Album/01 - a.flac", false, Some(&s)), "Album/01 - a.flac");
        assert_eq!(final_rel_path("a.flac", true, None), "a.flac");
    }

    #[test]
    fn packing_is_sequential_and_respects_capacity() {
        let size = DiscSize::new(100).unwrap(); // usable ≈ 93 MiB
        let files: Vec<PlannedFile> = (0..10)
            .map(|i| PlannedFile { path: format!("/x/{i}"), rel_path: format!("{i:02}.mp3"), original_bytes: 30 * MIB, est_bytes: 30 * MIB, transcode: false })
            .collect();
        let (discs, too_big) = pack_data(&files, size);
        assert!(too_big.is_empty());
        // 3 files (90 MiB) per disc -> 4 discs, in order
        assert_eq!(discs.len(), 4);
        assert_eq!(discs[0].items, vec![0, 1, 2]);
        assert_eq!(discs[3].items, vec![9]);
        assert!(discs.iter().all(|d| d.used_bytes <= size.capacity_bytes()));
        assert!(discs[0].percent > 85.0 && discs[0].percent < 100.0, "{}", discs[0].percent);
    }

    #[test]
    fn oversized_files_are_reported_not_packed() {
        let size = DiscSize::new(100).unwrap();
        let files = vec![
            PlannedFile { path: "/x/huge".into(), rel_path: "huge.iso".into(), original_bytes: 500 * MIB, est_bytes: 500 * MIB, transcode: false },
            PlannedFile { path: "/x/ok".into(), rel_path: "ok.txt".into(), original_bytes: 1000, est_bytes: 1000, transcode: false },
        ];
        let (discs, too_big) = pack_data(&files, size);
        assert_eq!(too_big, vec![0]);
        assert_eq!(discs.len(), 1);
        assert_eq!(discs[0].items, vec![1]);
    }

    /// The scenario that motivated this module: lots of FLAC, transcoded to MP3 320k, needs far
    /// fewer discs than the FLAC files as they are.
    #[test]
    fn transcoding_reduces_the_disc_count() {
        // 200 FLAC tracks of 5 minutes, ~35 MiB each = ~6.8 GiB
        let items: Vec<DataItem> = (0..200)
            .map(|i| DataItem { path: format!("/x/{i:03}.flac"), rel_path: format!("{i:03}.flac"), size_bytes: 35 * MIB })
            .collect();
        let spec = mp3_320();
        let size = DiscSize::default();

        let plain: Vec<PlannedFile> = items.iter().map(|i| PlannedFile { path: i.path.clone(), rel_path: i.rel_path.clone(), original_bytes: i.size_bytes, est_bytes: i.size_bytes, transcode: false }).collect();
        let (plain_discs, _) = pack_data(&plain, size);

        let converted: Vec<PlannedFile> = items
            .iter()
            .map(|i| {
                let d = decide(&i.path, i.size_bytes, Some(300.0), Some(&spec));
                PlannedFile { path: i.path.clone(), rel_path: final_rel_path(&i.rel_path, d.transcode, Some(&spec)), original_bytes: i.size_bytes, est_bytes: d.est_bytes, transcode: d.transcode }
            })
            .collect();
        let (mp3_discs, _) = pack_data(&converted, size);

        assert!(plain_discs.len() >= 10, "{}", plain_discs.len()); // ~6.8 GiB / ~0.68 GiB
        assert!(mp3_discs.len() <= 4, "{}", mp3_discs.len()); // 200 × ~12 MiB ≈ 2.4 GiB
        assert!(mp3_discs.len() < plain_discs.len());
    }

    #[test]
    fn audio_plan_splits_by_playing_time() {
        let items: Vec<AudioItem> = (0..30).map(|i| AudioItem { path: format!("/x/{i}.wav"), duration_secs: 240 }).collect();
        let plan = plan_audio(items, &[], DiscSize::default()); // 79:30 = 4770 s -> 19 tracks of 4:00
        assert_eq!(plan.disc_count, 2);
        assert_eq!(plan.discs[0].tracks, 19);
        assert_eq!(plan.discs[1].tracks, 11);
        assert!(plan.discs[0].percent < 100.0);
    }

    #[test]
    fn disc_sizes_by_name() {
        assert_eq!(DiscSize::parse("dvd").unwrap().mb, 4482);
        assert_eq!(DiscSize::parse("DVD-DL").unwrap().mb, 8147);
        assert_eq!(DiscSize::parse("bd").unwrap().mb, 23866);
        assert_eq!(DiscSize::parse("bd_dl").unwrap().mb, 47732);
        assert_eq!(DiscSize::parse("cd650").unwrap().mb, 650);
        assert_eq!(DiscSize::parse("1234").unwrap().mb, 1234);
        assert!(DiscSize::parse("floppy").is_err());
        assert!(DiscSize::new(4482).unwrap().is_dvd_class() && !DiscSize::new(700).unwrap().is_dvd_class());
        // a DVD keeps a bit more free than a CD
        let dvd = DiscSize::new(4482).unwrap();
        assert!(dvd.data_usable_bytes() < dvd.capacity_bytes() - 16 * MIB);
        assert!(dvd.data_usable_bytes() > dvd.capacity_bytes() - 30 * MIB);
    }

    /// Estimates are checked against real DVD-Video output: 20 s at 448 kbps came to 1.74 MB.
    #[test]
    fn music_dvd_track_size_estimate() {
        let est = music_dvd_track_bytes(20, 448);
        assert!((1_738_752..=1_950_000).contains(&est), "{est}");
        assert!(music_dvd_track_bytes(20, 192) < music_dvd_track_bytes(20, 448));
        // 4 hours of music at the top quality still fits a 4.7 GB DVD
        assert!(music_dvd_track_bytes(4 * 3600, 448) < DiscSize::new(4482).unwrap().data_usable_bytes());
    }

    /// The case that prompted DVD support: FLAC that becomes MP3 320k fits one DVD, and it only
    /// needs one disc even where the FLAC needs several CDs.
    #[test]
    fn a_dvd_holds_what_takes_several_cds() {
        let items: Vec<DataItem> = (0..120)
            .map(|i| DataItem { path: format!("/x/{i:03}.flac"), rel_path: format!("{i:03}.flac"), size_bytes: 35 * MIB })
            .collect(); // 4.1 GiB of FLAC
        let spec = mp3_320();
        let convert = |size: DiscSize| {
            let files: Vec<PlannedFile> = items
                .iter()
                .map(|i| {
                    let d = decide(&i.path, i.size_bytes, Some(300.0), Some(&spec));
                    PlannedFile { path: i.path.clone(), rel_path: final_rel_path(&i.rel_path, d.transcode, Some(&spec)), original_bytes: i.size_bytes, est_bytes: d.est_bytes, transcode: d.transcode }
                })
                .collect::<Vec<_>>();
            pack_data(&files, size).0.len()
        };
        assert!(convert(DiscSize::default()) >= 3);            // CDs
        assert_eq!(convert(DiscSize::new(4482).unwrap()), 1);  // one DVD
    }
}
