//! Where each track goes on the stick: a folder template such as
//! `{initial}/{albumartist}/{album}/{discfolder}/{track} - {title}` filled in from the tags,
//! with names made safe for the stick's filesystem.

use std::collections::HashMap;

use serde::Serialize;

use crate::error::Error;

/// What we know about a track.
#[derive(Debug, Clone, Default, Serialize)]
pub struct TrackMeta {
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    pub title: String,
    pub track: Option<u32>,
    pub track_total: Option<u32>,
    pub disc: Option<u32>,
    pub disc_total: Option<u32>,
    pub year: Option<u32>,
    pub genre: String,
}

/// Facts about a track's album that change how it is filed.
#[derive(Debug, Clone, Copy, Default)]
pub struct AlbumInfo {
    /// The album has more than one disc.
    pub multi_disc: bool,
    /// Track numbers are padded to this many digits (2, or 3 for long albums).
    pub track_digits: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct LayoutOptions {
    /// Make names valid on FAT, exFAT and NTFS (most sticks): no `: * ? " < > |`, no trailing dots.
    pub windows_safe: bool,
    /// File "The Beatles" under B.
    pub ignore_the: bool,
}

impl Default for LayoutOptions {
    fn default() -> Self {
        LayoutOptions { windows_safe: true, ignore_the: true }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Layout {
    pub template: String,
    #[serde(flatten)]
    pub options: LayoutOptions,
}

pub struct Preset {
    pub id: &'static str,
    pub label: &'static str,
    pub template: &'static str,
}

pub const PRESETS: &[Preset] = &[
    Preset { id: "initial-artist-album", label: "A–Z › Artist › Album › Disc › Tracks", template: "{initial}/{albumartist}/{album}/{discfolder}/{track} - {title}" },
    Preset { id: "artist-album", label: "Artist › Album › Disc › Tracks", template: "{albumartist}/{album}/{discfolder}/{track} - {title}" },
    Preset { id: "artist-album-flat", label: "Artist › Album › Tracks (disc in the track number)", template: "{albumartist}/{album}/{dtrack} - {title}" },
    Preset { id: "artist-year-album", label: "Artist › Year - Album › Tracks", template: "{albumartist}/{year} - {album}/{discfolder}/{track} - {title}" },
    Preset { id: "artist-album-onefolder", label: "Artist - Album › Tracks", template: "{albumartist} - {album}/{dtrack} - {title}" },
    Preset { id: "flat", label: "One folder: Artist - Title", template: "{albumartist} - {title}" },
];

pub const TOKENS: &[(&str, &str)] = &[
    ("initial", "First letter of the artist (A–Z, or # for anything else)"),
    ("albumartist", "Album artist (falls back to the track artist)"),
    ("artist", "Track artist"),
    ("album", "Album title"),
    ("year", "Release year"),
    ("genre", "Genre"),
    ("disc", "Disc number"),
    ("discfolder", "\"Disc 2\" — only for albums with more than one disc, otherwise nothing"),
    ("track", "Track number, e.g. 05"),
    ("dtrack", "Track number with the disc for multi-disc albums, e.g. 2-05"),
    ("title", "Track title"),
];

/// Check that a template only uses known tokens and names a file.
pub fn validate_template(template: &str) -> Result<(), Error> {
    let t = template.trim();
    if t.is_empty() {
        return Err(Error::validation("The folder layout is empty"));
    }
    if t.starts_with('/') || t.contains("..") || t.contains("//") && t.trim_matches('/').is_empty() {
        return Err(Error::validation("The layout must be a relative path such as {artist}/{album}/{track} - {title}"));
    }
    let mut rest = t;
    while let Some(start) = rest.find('{') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else {
            return Err(Error::validation("A { in the layout has no closing }"));
        };
        let name = &after[..end];
        if !TOKENS.iter().any(|(n, _)| *n == name) {
            return Err(Error::validation(format!(
                "Unknown layout token {{{name}}}. Available: {}",
                TOKENS.iter().map(|(n, _)| format!("{{{n}}}")).collect::<Vec<_>>().join(", ")
            )));
        }
        rest = &after[end + 1..];
    }
    let last = t.rsplit('/').next().unwrap_or("");
    if last.trim().is_empty() {
        return Err(Error::validation("The layout must end with a file name, e.g. {track} - {title}"));
    }
    if !last.contains("{title}") && !last.contains("{track}") && !last.contains("{dtrack}") {
        return Err(Error::validation("The file name part of the layout needs {title} or {track}, or tracks would overwrite each other"));
    }
    Ok(())
}

// ── Names ─────────────────────────────────────────────────────────────────────

const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9",
    "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Make one name (a folder or a file name without its extension) safe.
pub fn sanitize_component(s: &str, windows_safe: bool, max_chars: usize) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let bad = c.is_control()
            || c == '/'
            || c == '\\'
            || (windows_safe && matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|'));
        out.push(if bad { '_' } else { c });
    }
    // collapse runs of spaces
    let mut collapsed = String::with_capacity(out.len());
    let mut prev_space = false;
    for c in out.chars() {
        if c == ' ' {
            if !prev_space {
                collapsed.push(c);
            }
            prev_space = true;
        } else {
            collapsed.push(c);
            prev_space = false;
        }
    }
    let mut name: String = collapsed.trim().chars().take(max_chars).collect();
    if windows_safe {
        name = name.trim_end_matches(['.', ' ']).to_string();
        let stem = name.split('.').next().unwrap_or("").to_uppercase();
        if RESERVED.contains(&stem.as_str()) {
            name.insert(0, '_');
        }
    } else {
        name = name.trim_end().to_string();
    }
    // never "." or ".." or empty
    if name.is_empty() || name == "." || name == ".." {
        return "_".to_string();
    }
    name
}

fn strip_accents(c: char) -> char {
    match c {
        'À'..='Å' | 'à'..='å' => 'A',
        'Ç' | 'ç' => 'C',
        'È'..='Ë' | 'è'..='ë' => 'E',
        'Ì'..='Ï' | 'ì'..='ï' => 'I',
        'Ñ' | 'ñ' => 'N',
        'Ò'..='Ö' | 'Ø' | 'ò'..='ö' | 'ø' => 'O',
        'Ù'..='Ü' | 'ù'..='ü' => 'U',
        'Ý' | 'ý' | 'ÿ' => 'Y',
        other => other,
    }
}

/// `The Beatles` → `Beatles` for filing.
fn sort_name(s: &str, ignore_the: bool) -> &str {
    if ignore_the {
        let lower = s.to_lowercase();
        if lower.starts_with("the ") && s.len() > 4 {
            return &s[4..];
        }
    }
    s
}

/// The letter a name is filed under: A–Z, or `#` for digits and everything else.
pub fn initial_of(artist: &str, ignore_the: bool) -> String {
    let name = sort_name(artist.trim(), ignore_the);
    match name.chars().find(|c| c.is_alphanumeric()).map(strip_accents) {
        Some(c) if c.is_ascii_alphabetic() => c.to_ascii_uppercase().to_string(),
        _ => "#".to_string(),
    }
}

// ── Rendering ─────────────────────────────────────────────────────────────────

/// The pieces of an album's tracks that decide how they are filed.
pub fn album_infos(metas: &[&TrackMeta]) -> HashMap<(String, String), AlbumInfo> {
    let mut discs: HashMap<(String, String), std::collections::BTreeSet<u32>> = HashMap::new();
    let mut declared: HashMap<(String, String), u32> = HashMap::new();
    let mut max_track: HashMap<(String, String), u32> = HashMap::new();
    let mut count: HashMap<(String, String), usize> = HashMap::new();
    for m in metas {
        let key = album_key(m);
        discs.entry(key.clone()).or_default().insert(m.disc.unwrap_or(1));
        let d = declared.entry(key.clone()).or_insert(0);
        *d = (*d).max(m.disc_total.unwrap_or(0));
        let t = max_track.entry(key.clone()).or_insert(0);
        *t = (*t).max(m.track.unwrap_or(0)).max(m.track_total.unwrap_or(0));
        *count.entry(key).or_insert(0) += 1;
    }
    discs
        .into_iter()
        .map(|(key, set)| {
            let multi = set.len() > 1 || declared.get(&key).copied().unwrap_or(0) > 1;
            let biggest = max_track.get(&key).copied().unwrap_or(0).max(count.get(&key).copied().unwrap_or(0) as u32);
            (key, AlbumInfo { multi_disc: multi, track_digits: if biggest > 99 { 3 } else { 2 } })
        })
        .collect()
}

pub fn album_key(m: &TrackMeta) -> (String, String) {
    (album_artist_of(m).to_lowercase(), m.album.to_lowercase())
}

fn album_artist_of(m: &TrackMeta) -> &str {
    if !m.album_artist.trim().is_empty() {
        &m.album_artist
    } else if !m.artist.trim().is_empty() {
        &m.artist
    } else {
        "Unknown Artist"
    }
}

/// Fill in the template for one track. `ext` is the extension of the file as it will be written.
pub fn render(meta: &TrackMeta, album: AlbumInfo, layout: &Layout, ext: &str) -> Result<String, Error> {
    validate_template(&layout.template)?;
    let ws = layout.options.windows_safe;
    let album_artist = album_artist_of(meta).to_string();
    let artist = if meta.artist.trim().is_empty() { album_artist.clone() } else { meta.artist.clone() };
    let album_title = if meta.album.trim().is_empty() { "Unknown Album".to_string() } else { meta.album.clone() };
    let title = if meta.title.trim().is_empty() { "Untitled".to_string() } else { meta.title.clone() };
    let digits = album.track_digits.max(2);
    let track = meta.track.map(|t| format!("{:0width$}", t, width = digits)).unwrap_or_default();
    let disc = meta.disc.unwrap_or(1);
    let dtrack = if album.multi_disc {
        format!("{}-{}", disc, if track.is_empty() { "00".to_string() } else { track.clone() })
    } else {
        track.clone()
    };

    let value = |name: &str| -> String {
        match name {
            "initial" => initial_of(&album_artist, layout.options.ignore_the),
            "albumartist" => album_artist.clone(),
            "artist" => artist.clone(),
            "album" => album_title.clone(),
            "year" => meta.year.map(|y| y.to_string()).unwrap_or_default(),
            "genre" => meta.genre.clone(),
            "disc" => disc.to_string(),
            "discfolder" => if album.multi_disc { format!("Disc {disc}") } else { String::new() },
            "track" => track.clone(),
            "dtrack" => dtrack.clone(),
            "title" => title.clone(),
            _ => String::new(),
        }
    };

    let segments: Vec<&str> = layout.template.trim().split('/').collect();
    let mut parts: Vec<String> = Vec::new();
    for (i, seg) in segments.iter().enumerate() {
        let is_file = i + 1 == segments.len();
        let mut filled = String::new();
        let mut rest = *seg;
        while let Some(start) = rest.find('{') {
            filled.push_str(&rest[..start]);
            let after = &rest[start + 1..];
            let end = after.find('}').unwrap_or(0);
            filled.push_str(&value(&after[..end]));
            rest = &after[end + 1..];
        }
        filled.push_str(rest);
        // A leftover " - " or "()" from an empty token looks untidy: tidy the obvious cases.
        let filled = tidy(&filled);
        if filled.trim().is_empty() {
            if is_file {
                return Err(Error::validation("The layout produced an empty file name"));
            }
            continue; // e.g. {discfolder} on a single-disc album drops the whole folder level
        }
        parts.push(sanitize_component(&filled, ws, if is_file { 180 } else { 120 }));
    }
    let file = parts.pop().unwrap_or_else(|| "track".to_string());
    let ext = ext.trim_start_matches('.');
    let name = if ext.is_empty() { file } else { format!("{file}.{ext}") };
    parts.push(name);
    Ok(parts.join("/"))
}

/// Remove separators left dangling by empty tokens: `" - Song"` → `"Song"`, `"2001 - "` → `"2001"`.
fn tidy(s: &str) -> String {
    let mut t = s.trim().to_string();
    for sep in [" - ", " – ", "- ", " -"] {
        if let Some(r) = t.strip_prefix(sep) {
            t = r.trim().to_string();
        }
        if let Some(r) = t.strip_suffix(sep) {
            t = r.trim().to_string();
        }
    }
    t.replace("()", "").replace("[]", "").trim().to_string()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(artist: &str, album: &str, title: &str, track: u32) -> TrackMeta {
        TrackMeta {
            artist: artist.into(), album_artist: artist.into(), album: album.into(), title: title.into(),
            track: Some(track), disc: Some(1), year: Some(1994), ..Default::default()
        }
    }

    fn layout(t: &str) -> Layout {
        Layout { template: t.into(), options: LayoutOptions::default() }
    }

    #[test]
    fn presets_are_valid_and_render() {
        let m = meta("The Verve", "Urban Hymns", "Bitter Sweet Symphony", 1);
        for p in PRESETS {
            validate_template(p.template).unwrap_or_else(|e| panic!("{}: {e}", p.id));
            let out = render(&m, AlbumInfo { multi_disc: false, track_digits: 2 }, &layout(p.template), "mp3").unwrap();
            assert!(out.ends_with(".mp3") && !out.contains("//"), "{}: {out}", p.id);
        }
    }

    #[test]
    fn a_to_z_layout_files_under_the_first_letter_ignoring_the() {
        let m = meta("The Verve", "Urban Hymns", "Bitter Sweet Symphony", 1);
        let l = layout(PRESETS[0].template);
        let out = render(&m, AlbumInfo { multi_disc: false, track_digits: 2 }, &l, "flac").unwrap();
        assert_eq!(out, "V/The Verve/Urban Hymns/01 - Bitter Sweet Symphony.flac");
        let mut keep_the = l.clone();
        keep_the.options.ignore_the = false;
        assert!(render(&m, AlbumInfo::default(), &keep_the, "flac").unwrap().starts_with("T/"));
        assert_eq!(initial_of("2Pac", true), "#");
        assert_eq!(initial_of("Édith Piaf", true), "E");
        assert_eq!(initial_of("", true), "#");
    }

    #[test]
    fn disc_folder_only_appears_for_multi_disc_albums() {
        let mut m = meta("Artist", "Album", "Song", 5);
        m.disc = Some(2);
        let l = layout("{albumartist}/{album}/{discfolder}/{track} - {title}");
        assert_eq!(render(&m, AlbumInfo { multi_disc: true, track_digits: 2 }, &l, "mp3").unwrap(), "Artist/Album/Disc 2/05 - Song.mp3");
        assert_eq!(render(&m, AlbumInfo { multi_disc: false, track_digits: 2 }, &l, "mp3").unwrap(), "Artist/Album/05 - Song.mp3");
        // the flat layout puts the disc in the track number instead
        let flat = layout("{albumartist}/{album}/{dtrack} - {title}");
        assert_eq!(render(&m, AlbumInfo { multi_disc: true, track_digits: 2 }, &flat, "mp3").unwrap(), "Artist/Album/2-05 - Song.mp3");
        assert_eq!(render(&m, AlbumInfo { multi_disc: false, track_digits: 2 }, &flat, "mp3").unwrap(), "Artist/Album/05 - Song.mp3");
    }

    #[test]
    fn names_are_made_safe_for_fat_and_ntfs() {
        let m = meta("AC/DC", "Who Made Who?", "Hells Bells: Live \"Rock\" <1>", 1);
        let out = render(&m, AlbumInfo::default(), &layout("{albumartist}/{album}/{track} - {title}"), "mp3").unwrap();
        assert_eq!(out, "AC_DC/Who Made Who_/01 - Hells Bells_ Live _Rock_ _1_.mp3");
        // a linux-only stick keeps the punctuation but never a path separator
        let mut unix = layout("{albumartist}/{album}/{track} - {title}");
        unix.options.windows_safe = false;
        assert_eq!(render(&m, AlbumInfo::default(), &unix, "mp3").unwrap(), "AC_DC/Who Made Who?/01 - Hells Bells: Live \"Rock\" <1>.mp3");
        // trailing dots and reserved names
        assert_eq!(sanitize_component("Album...", true, 100), "Album");
        assert_eq!(sanitize_component("CON", true, 100), "_CON");
        assert_eq!(sanitize_component("con.txt", true, 100), "_con.txt");
        assert_eq!(sanitize_component("..", false, 100), "_");
        assert_eq!(sanitize_component("  lots   of   space  ", true, 100), "lots of space");
    }

    #[test]
    fn missing_tags_get_sensible_names_and_empty_tokens_are_tidied() {
        let m = TrackMeta { title: "Song".into(), ..Default::default() };
        let out = render(&m, AlbumInfo::default(), &layout("{albumartist}/{album}/{track} - {title}"), "mp3").unwrap();
        assert_eq!(out, "Unknown Artist/Unknown Album/Song.mp3");
        let mut y = meta("A", "B", "C", 1);
        y.year = None;
        assert_eq!(render(&y, AlbumInfo::default(), &layout("{albumartist}/{year} - {album}/{title}"), "flac").unwrap(), "A/B/C.flac");
    }

    #[test]
    fn templates_are_validated() {
        assert!(validate_template("{artist}/{album}/{track} - {title}").is_ok());
        assert!(validate_template("").is_err());
        assert!(validate_template("/abs/{title}").is_err());
        assert!(validate_template("../{title}").is_err());
        assert!(validate_template("{artist}/{bogus}/{title}").is_err());
        assert!(validate_template("{artist}/{album").is_err());
        assert!(validate_template("{artist}/{album}/").is_err());
        assert!(validate_template("{artist}/{album}/cover").is_err()); // would overwrite itself
    }

    #[test]
    fn album_info_detects_multi_disc_and_track_padding() {
        let mut a = meta("A", "Box", "x", 1);
        let mut b = meta("A", "Box", "y", 120);
        a.disc = Some(1);
        b.disc = Some(2);
        let other = meta("B", "Single", "z", 1);
        let infos = album_infos(&[&a, &b, &other]);
        let box_info = infos[&album_key(&a)];
        assert!(box_info.multi_disc && box_info.track_digits == 3);
        assert!(!infos[&album_key(&other)].multi_disc);
        // a disc total tag alone also marks an album as multi-disc, even if one disc is selected
        let mut only = meta("A", "Part", "p", 1);
        only.disc_total = Some(2);
        assert!(album_infos(&[&only])[&album_key(&only)].multi_disc);
    }

    #[test]
    fn long_names_are_shortened() {
        let long = "x".repeat(400);
        let m = meta("A", &long, &long, 1);
        let out = render(&m, AlbumInfo::default(), &layout("{albumartist}/{album}/{track} - {title}"), "flac").unwrap();
        assert!(out.split('/').all(|c| c.chars().count() <= 190), "{out}");
    }
}
