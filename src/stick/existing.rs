//! What is already on a stick: reading it, working out how it is organised, and comparing
//! quality so a new copy of a track can replace an old one only when that is an improvement.

use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashMap},
    path::Path,
    str::FromStr,
};

use serde::Serialize;

use super::{
    layout::{self, Layout, LayoutOptions, TrackMeta},
    scan::{self, SourceSpec},
};
use crate::backend::transcode::{OutputFormat, TranscodeSpec};

// ── Quality ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Quality {
    pub lossless: bool,
    pub kbps: Option<f64>,
    /// "FLAC", "MP3 ~320k", ...
    pub label: String,
    /// Higher is better; only comparable between two lossy files.
    pub score: f64,
}

const LOSSLESS_EXTS: &[&str] = &["flac", "wav", "aiff", "aif", "ape", "wv", "alac"];

/// How much better than MP3 a codec sounds at the same bitrate (roughly).
fn codec_factor(ext: &str) -> f64 {
    match ext {
        "opus" => 1.5,
        "m4a" | "aac" => 1.25,
        "ogg" => 1.15,
        "wma" => 0.9,
        _ => 1.0,
    }
}

pub fn quality_of(ext: &str, size: u64, duration: Option<f64>) -> Quality {
    let ext = ext.to_lowercase();
    let kbps = duration.filter(|d| *d > 1.0).map(|d| size as f64 * 8.0 / d / 1000.0);
    // An .m4a holding ALAC is lossless; it is far bigger than any AAC.
    let lossless = LOSSLESS_EXTS.contains(&ext.as_str()) || (ext == "m4a" && kbps.is_some_and(|k| k > 600.0));
    let name = ext.to_uppercase();
    if lossless {
        return Quality { lossless: true, kbps, label: name, score: 1e6 };
    }
    let label = match kbps {
        Some(k) => format!("{name} ~{}k", (k / 16.0).round() as u32 * 16),
        None => name,
    };
    Quality { lossless: false, kbps, label, score: kbps.map(|k| k * codec_factor(&ext)).unwrap_or(0.0) }
}

/// The quality a file will have after being converted.
pub fn quality_of_spec(spec: &TranscodeSpec) -> Quality {
    match spec.format {
        OutputFormat::Flac | OutputFormat::Wav => Quality { lossless: true, kbps: None, label: spec.label(), score: 1e6 },
        _ => {
            let kbps = spec.bitrate_kbps.unwrap_or(192) as f64;
            let factor = codec_factor(spec.extension());
            Quality { lossless: false, kbps: Some(kbps), label: spec.label(), score: kbps * factor }
        }
    }
}

/// `Greater` when `new` is better than `old`. Lossy files within 4% of each other are equal.
pub fn compare(new: &Quality, old: &Quality) -> Ordering {
    match (new.lossless, old.lossless) {
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (true, true) => Ordering::Equal,
        (false, false) => {
            if new.score <= 0.0 || old.score <= 0.0 {
                return Ordering::Equal; // can't tell
            }
            let ratio = new.score / old.score;
            if ratio > 1.04 {
                Ordering::Greater
            } else if ratio < 0.96 {
                Ordering::Less
            } else {
                Ordering::Equal
            }
        }
    }
}

// ── What conflicts to do about ────────────────────────────────────────────────

/// What to do when a track is already on the stick.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Conflict {
    /// Leave what is there.
    #[default]
    Skip,
    /// Always write the new file over the old one.
    Replace,
    /// Replace only when the new file is better quality.
    HigherQuality,
    /// Replace only when the new file is lower quality (to save space).
    LowerQuality,
    /// Replace only when the new file is more recently modified.
    Newer,
    /// Write the new file next to the old one.
    KeepBoth,
}

impl FromStr for Conflict {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        Ok(match s.trim().to_lowercase().replace('-', "_").as_str() {
            "" | "skip" => Conflict::Skip,
            "replace" | "overwrite" => Conflict::Replace,
            "higher_quality" | "higher" | "better" => Conflict::HigherQuality,
            "lower_quality" | "lower" | "smaller" => Conflict::LowerQuality,
            "newer" => Conflict::Newer,
            "keep_both" | "both" => Conflict::KeepBoth,
            other => return Err(format!("Unknown conflict rule '{other}'. Choose skip, replace, higher-quality, lower-quality, newer or keep-both")),
        })
    }
}

impl Conflict {
    pub fn id(self) -> &'static str {
        match self {
            Conflict::Skip => "skip",
            Conflict::Replace => "replace",
            Conflict::HigherQuality => "higher_quality",
            Conflict::LowerQuality => "lower_quality",
            Conflict::Newer => "newer",
            Conflict::KeepBoth => "keep_both",
        }
    }
}

/// What to do with music that is already on the stick but filed differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExistingMode {
    /// Leave it where it is.
    #[default]
    Leave,
    /// Move it into the chosen layout (renaming and re-filing, no copying).
    Reorganize,
}

impl FromStr for ExistingMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s.trim().to_lowercase().as_str() {
            "" | "leave" | "ignore" => Ok(ExistingMode::Leave),
            "reorganize" | "reorganise" => Ok(ExistingMode::Reorganize),
            other => Err(format!("Unknown option '{other}': use leave or reorganize")),
        }
    }
}

// ── Reading a stick ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ExistingTrack {
    /// Path relative to the folder that was read.
    pub rel: String,
    pub size: u64,
    pub mtime: i64,
    pub meta: TrackMeta,
    pub quality: Quality,
}

pub fn mtime_of(p: &Path) -> i64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Read every audio file under `root` (hidden folders and system folders are skipped).
pub fn read_existing(root: &Path) -> Vec<ExistingTrack> {
    if !root.is_dir() {
        return Vec::new();
    }
    let spec = SourceSpec { folders: vec![root.to_string_lossy().to_string()], ..Default::default() };
    let Ok((paths, _)) = scan::gather_paths(&spec) else { return Vec::new() };
    if paths.is_empty() {
        return Vec::new();
    }
    let results: std::sync::Mutex<Vec<Option<ExistingTrack>>> = std::sync::Mutex::new(vec![None; paths.len()]);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).clamp(2, 8);
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(p) = paths.get(i) else { break };
                let t = scan::read_track(p);
                let rel = p.strip_prefix(root).unwrap_or(p).to_string_lossy().to_string();
                let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
                results.lock().unwrap()[i] = Some(ExistingTrack {
                    quality: quality_of(ext, t.size, t.duration_secs),
                    mtime: mtime_of(p),
                    rel,
                    size: t.size,
                    meta: t.meta,
                });
            });
        }
    });
    results.into_inner().unwrap().into_iter().flatten().collect()
}

/// A cheap fingerprint of a folder (file count, total size, newest change) that changes whenever
/// something is added, removed or rewritten. Lets a slow tag scan be reused until then.
pub fn fingerprint(root: &Path) -> (u64, u64, i64) {
    fn walk(dir: &Path, acc: &mut (u64, u64, i64), depth: usize) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.filter_map(|e| e.ok()) {
            let Ok(m) = e.metadata() else { continue };
            let mt = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
            acc.2 = acc.2.max(mt);
            if m.is_dir() {
                if depth < 12 {
                    walk(&e.path(), acc, depth + 1);
                }
            } else {
                acc.0 += 1;
                acc.1 += m.len();
            }
        }
    }
    let mut acc = (0, 0, 0);
    walk(root, &mut acc, 0);
    acc
}

/// Identity of a track across files: the same song is found even when it is filed differently or
/// in another format.
pub fn ident(m: &TrackMeta) -> Option<(String, String, u32, u32, String)> {
    let track = m.track?;
    let key = layout::album_key(m);
    Some((key.0, key.1, m.disc.unwrap_or(1), track, m.title.to_lowercase()))
}

// ── How the stick is organised ────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct DetectedLayout {
    pub preset: String,
    pub label: String,
    pub template: String,
    /// Folder holding all the music, when it isn't the root ("" for the root).
    pub subfolder: String,
    /// Share of the tracks that are filed exactly the way this layout would file them.
    pub match_pct: u32,
}

/// `Artist/Album/01 - Song (2).mp3` → `artist/album/01 - song`
fn normalize(rel: &str) -> String {
    let no_ext = rel.rsplit_once('.').map(|(s, _)| s).unwrap_or(rel);
    let mut s = no_ext.to_lowercase();
    if s.ends_with(')') {
        if let Some(i) = s.rfind(" (") {
            if s[i + 2..s.len() - 1].chars().all(|c| c.is_ascii_digit()) {
                s.truncate(i);
            }
        }
    }
    s
}

pub fn detect_layout(tracks: &[ExistingTrack]) -> Option<DetectedLayout> {
    let tagged: Vec<&ExistingTrack> = tracks.iter().filter(|t| t.meta.track.is_some() && !(t.meta.artist.is_empty() && t.meta.album_artist.is_empty())).collect();
    if tagged.len() < 3 {
        return None;
    }
    // Is all the music inside one folder?
    let firsts: Vec<&str> = tagged.iter().filter_map(|t| t.rel.split_once('/').map(|(a, _)| a)).collect();
    let common = if firsts.len() == tagged.len() && firsts.iter().all(|f| *f == firsts[0]) { Some(firsts[0].to_string()) } else { None };

    let metas: Vec<&TrackMeta> = tagged.iter().map(|t| &t.meta).collect();
    let infos = layout::album_infos(&metas);
    let mut best: Option<(f64, &layout::Preset, String)> = None;
    for sub in std::iter::once(String::new()).chain(common.clone()) {
        for preset in layout::PRESETS {
            for the in [true, false] {
                let l = Layout { template: preset.template.into(), options: LayoutOptions { windows_safe: true, ignore_the: the } };
                let mut hits = 0usize;
                for t in &tagged {
                    let info = infos.get(&layout::album_key(&t.meta)).copied().unwrap_or_default();
                    let ext = t.rel.rsplit('.').next().unwrap_or("");
                    let Ok(want) = layout::render(&t.meta, info, &l, ext) else { continue };
                    let want = if sub.is_empty() { want } else { format!("{sub}/{want}") };
                    if normalize(&want) == normalize(&t.rel) {
                        hits += 1;
                    }
                }
                let score = hits as f64 / tagged.len() as f64;
                if best.as_ref().is_none_or(|b| score > b.0 + 1e-9) {
                    best = Some((score, preset, sub.clone()));
                }
            }
        }
    }
    let (score, preset, subfolder) = best?;
    (score >= 0.6).then(|| DetectedLayout {
        preset: preset.id.into(),
        label: preset.label.into(),
        template: preset.template.into(),
        subfolder,
        match_pct: (score * 100.0).round() as u32,
    })
}

// ── Summary for the "Identify" panel ──────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Default)]
pub struct Content {
    pub audio_files: usize,
    pub audio_bytes: u64,
    pub other_files: usize,
    pub other_bytes: u64,
    pub albums: usize,
    pub artists: usize,
    pub formats: BTreeMap<String, usize>,
    pub lossless_files: usize,
    /// Top-level folders and how many audio files are in each.
    pub folders: Vec<(String, usize)>,
    pub untagged: usize,
}

const SYSTEM_NAMES: &[&str] = &["system volume information", "$recycle.bin", "lost+found", ".trashes", ".spotlight-v100", ".fseventsd"];

fn other_files(root: &Path) -> (usize, u64) {
    fn walk(dir: &Path, acc: &mut (usize, u64), depth: usize) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.filter_map(|e| e.ok()) {
            let name = e.file_name().to_string_lossy().to_lowercase();
            if name.starts_with('.') || SYSTEM_NAMES.contains(&name.as_str()) {
                continue;
            }
            let p = e.path();
            let Ok(m) = e.metadata() else { continue };
            if m.is_dir() {
                if depth < 12 {
                    walk(&p, acc, depth + 1);
                }
            } else if !scan::is_audio_path(&p) {
                acc.0 += 1;
                acc.1 += m.len();
            }
        }
    }
    let mut acc = (0, 0);
    walk(root, &mut acc, 0);
    acc
}

pub fn summarize(root: &Path, tracks: &[ExistingTrack]) -> Content {
    let mut c = Content { audio_files: tracks.len(), ..Default::default() };
    let mut albums = std::collections::HashSet::new();
    let mut artists = std::collections::HashSet::new();
    let mut folders: HashMap<String, usize> = HashMap::new();
    for t in tracks {
        c.audio_bytes += t.size;
        *c.formats.entry(t.rel.rsplit('.').next().unwrap_or("?").to_lowercase()).or_insert(0) += 1;
        if t.quality.lossless {
            c.lossless_files += 1;
        }
        if t.meta.artist.is_empty() && t.meta.album_artist.is_empty() {
            c.untagged += 1;
        } else {
            artists.insert(layout::album_key(&t.meta).0);
            albums.insert(layout::album_key(&t.meta));
        }
        let top = t.rel.split_once('/').map(|(a, _)| a.to_string()).unwrap_or_else(|| "(top level)".into());
        *folders.entry(top).or_insert(0) += 1;
    }
    c.albums = albums.len();
    c.artists = artists.len();
    let mut f: Vec<(String, usize)> = folders.into_iter().collect();
    f.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    f.truncate(40);
    c.folders = f;
    let (n, b) = other_files(root);
    c.other_files = n;
    c.other_bytes = b;
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn et(rel: &str, artist: &str, album: &str, n: u32, title: &str) -> ExistingTrack {
        ExistingTrack {
            rel: rel.into(),
            size: 4_000_000,
            mtime: 0,
            meta: TrackMeta { artist: artist.into(), album_artist: artist.into(), album: album.into(), title: title.into(), track: Some(n), disc: Some(1), ..Default::default() },
            quality: quality_of("mp3", 4_000_000, Some(100.0)),
        }
    }

    #[test]
    fn lossless_beats_lossy_and_bitrates_compare() {
        let flac = quality_of("flac", 30_000_000, Some(200.0));
        let mp3_320 = quality_of("mp3", 8_000_000, Some(200.0));
        let mp3_128 = quality_of("mp3", 3_200_000, Some(200.0));
        assert!(flac.lossless && !mp3_320.lossless);
        assert_eq!(compare(&flac, &mp3_320), Ordering::Greater);
        assert_eq!(compare(&mp3_128, &flac), Ordering::Less);
        assert_eq!(compare(&mp3_320, &mp3_128), Ordering::Greater);
        assert_eq!(compare(&mp3_320, &mp3_320.clone()), Ordering::Equal);
        // opus 128 is treated as better than mp3 128, but not better than mp3 320
        let opus = quality_of("opus", 3_200_000, Some(200.0));
        assert_eq!(compare(&opus, &mp3_128), Ordering::Greater);
        assert_eq!(compare(&opus, &mp3_320), Ordering::Less);
        // an ALAC .m4a is lossless, an AAC one isn't
        assert!(quality_of("m4a", 40_000_000, Some(200.0)).lossless);
        assert!(!quality_of("m4a", 5_000_000, Some(200.0)).lossless);
        // unknown length can't be compared
        assert_eq!(compare(&quality_of("mp3", 1, None), &mp3_320), Ordering::Equal);
    }

    #[test]
    fn a_conversion_has_the_quality_of_its_target() {
        let q = quality_of_spec(&TranscodeSpec::parse("mp3:192").unwrap());
        assert!(!q.lossless);
        assert_eq!(q.kbps, Some(192.0));
        assert!(quality_of_spec(&TranscodeSpec::parse("flac").unwrap()).lossless);
    }

    #[test]
    fn conflict_rules_parse() {
        assert_eq!("higher-quality".parse::<Conflict>().unwrap(), Conflict::HigherQuality);
        assert_eq!("".parse::<Conflict>().unwrap(), Conflict::Skip);
        assert!("bogus".parse::<Conflict>().is_err());
        assert_eq!("reorganize".parse::<ExistingMode>().unwrap(), ExistingMode::Reorganize);
    }

    #[test]
    fn detects_how_a_stick_is_organised() {
        let mut tracks = Vec::new();
        for (i, album) in ["Alpha", "Beta"].iter().enumerate() {
            for n in 1..=3u32 {
                tracks.push(et(&format!("Music/The Band/{album}/{n:02} - Song {i}{n}.mp3"), "The Band", album, n, &format!("Song {i}{n}")));
            }
        }
        let d = detect_layout(&tracks).unwrap();
        assert_eq!(d.preset, "artist-album");
        assert_eq!(d.subfolder, "Music");
        assert_eq!(d.match_pct, 100);

        // a mess isn't a layout
        let mess: Vec<ExistingTrack> = (1..=5).map(|n| et(&format!("dump/{n}.mp3"), "X", "Y", n, "z")).collect();
        assert!(detect_layout(&mess).is_none());
        assert!(detect_layout(&tracks[..2]).is_none());
    }

    #[test]
    fn normalizing_ignores_extension_case_and_duplicate_suffix() {
        assert_eq!(normalize("A/B/01 - Song (2).MP3"), "a/b/01 - song");
        assert_eq!(normalize("A/B/01 - Song (live).mp3"), "a/b/01 - song (live)");
    }
}
