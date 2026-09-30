//! Building a Picard naming script from a handful of choices.
//!
//! A port of WBs-Picard-Filenaming-Script-Generator: the same settings, the same six presets and
//! the same script text (the tests compare against output of the original), so a script made
//! there and one made here are identical. The result is a normal Picard script: it can be pasted
//! into Picard, or used by RustyDisc's own library import.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ScriptConfig {
    // Folder structure
    pub use_artist_folder: bool,
    /// standard | sort | first_letter_subfolder
    pub artist_folder_style: String,
    pub use_album_folder: bool,
    pub include_year_in_album: bool,
    /// prefix | suffix
    pub year_position: String,

    // Multi-disc
    pub use_disc_subfolder: bool,
    /// disc | side | cd
    pub disc_folder_format: String,
    pub include_disc_subtitle: bool,

    // Track numbers
    pub pad_track_number: bool,
    pub track_padding_length: u32,
    pub include_disc_in_track: bool,

    // File name
    pub include_artist_in_filename: bool,
    pub artist_separator: String,
    pub include_track_number: bool,
    pub include_title: bool,

    // Various artists
    pub include_track_artist_for_va: bool,
    pub va_artist_separator: String,

    // Featured artists
    pub show_featured_artists: bool,
    /// feat. | ft. | featuring | with
    pub feat_format: String,

    // Release types
    pub separate_singles: bool,
    pub separate_soundtracks: bool,
    pub separate_compilations: bool,

    pub replace_invalid_chars: bool,

    // Length limits
    pub max_album_length: u32,
    pub max_title_length: u32,
    pub max_filename_length: u32,

    pub use_original_year: bool,
    /// standard | sort
    pub format_album_artist: String,
    pub lowercase_extension: bool,

    // Extra information in the album folder name
    pub include_label: bool,
    pub include_catalog: bool,
    pub include_format: bool,
    pub include_disambiguation: bool,
}

impl Default for ScriptConfig {
    fn default() -> Self {
        ScriptConfig {
            use_artist_folder: true,
            artist_folder_style: "standard".into(),
            use_album_folder: true,
            include_year_in_album: true,
            year_position: "prefix".into(),
            use_disc_subfolder: true,
            disc_folder_format: "disc".into(),
            include_disc_subtitle: true,
            pad_track_number: true,
            track_padding_length: 2,
            include_disc_in_track: false,
            include_artist_in_filename: false,
            artist_separator: " - ".into(),
            include_track_number: true,
            include_title: true,
            include_track_artist_for_va: true,
            va_artist_separator: " - ".into(),
            show_featured_artists: true,
            feat_format: "feat.".into(),
            separate_singles: false,
            separate_soundtracks: false,
            separate_compilations: false,
            replace_invalid_chars: true,
            max_album_length: 100,
            max_title_length: 100,
            max_filename_length: 200,
            use_original_year: true,
            format_album_artist: "standard".into(),
            lowercase_extension: false,
            include_label: false,
            include_catalog: false,
            include_format: false,
            include_disambiguation: false,
        }
    }
}

/// The six ready-made configurations: (id, name, description, example, config).
pub fn presets() -> Vec<(&'static str, &'static str, &'static str, &'static str, ScriptConfig)> {
    let base = ScriptConfig { show_featured_artists: false, include_disc_subtitle: false, ..ScriptConfig::default() };
    vec![
        (
            "simple", "Simple", "Basic: Artist/[Year] Album/01. Title",
            "The Beatles/[1969] Abbey Road/01. Come Together.mp3",
            base.clone(),
        ),
        (
            "organized", "Organized", "Alphabetical: A/Artist/[Year] Album/Disc 1/01. Title",
            "T/The Beatles/[1969] Abbey Road (Remaster)/Disc 1/01. Come Together.mp3",
            ScriptConfig {
                artist_folder_style: "first_letter_subfolder".into(), include_disc_subtitle: true, show_featured_artists: true,
                separate_soundtracks: true, max_album_length: 80, max_title_length: 80, max_filename_length: 180, include_disambiguation: true,
                ..ScriptConfig::default()
            },
        ),
        (
            "detailed", "Detailed", "Full info: A/Artist/[Year] Album [Label] {Cat#}/01. Title",
            "T/The Beatles/[1969] Abbey Road [Apple] {PCS7088}/01. Come Together.mp3",
            ScriptConfig {
                artist_folder_style: "first_letter_subfolder".into(), include_disc_subtitle: true, show_featured_artists: true,
                separate_soundtracks: true, include_label: true, include_catalog: true, include_disambiguation: true,
                ..ScriptConfig::default()
            },
        ),
        (
            "flat", "Flat", "No disc folders: Artist/[Year] Album/1-01. Title",
            "The Beatles/[1969] Abbey Road/1-01. Come Together.mp3",
            ScriptConfig { use_disc_subfolder: false, include_disc_in_track: true, ..base.clone() },
        ),
        (
            "minimal", "Minimal", "Album only: Album/01. Artist - Title",
            "Abbey Road/01. The Beatles - Come Together.mp3",
            ScriptConfig {
                use_artist_folder: false, include_year_in_album: false, use_disc_subfolder: false, include_disc_in_track: true,
                include_artist_in_filename: true, include_track_artist_for_va: false, ..base
            },
        ),
        (
            "audiophile", "Audiophile", "With format: A/Artist/[Year] Album [FLAC]/01. Title",
            "B/Beatles, The/[1969] Abbey Road [FLAC]/01. Come Together.flac",
            ScriptConfig {
                artist_folder_style: "first_letter_subfolder".into(), include_disc_subtitle: true, show_featured_artists: true,
                separate_soundtracks: true, format_album_artist: "sort".into(), include_format: true, include_disambiguation: true,
                ..ScriptConfig::default()
            },
        ),
    ]
}

pub fn build(c: &ScriptConfig, timestamp: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    header(&mut parts, timestamp);
    settings(&mut parts, c);
    constants(&mut parts);
    working_variables(&mut parts, c);
    if c.replace_invalid_chars {
        sanitization(&mut parts);
    }
    file_path(&mut parts, c);
    filename(&mut parts, c);
    output(&mut parts);
    parts.join("\n")
}

/// "YYYY-MM-DD HH:MM" (UTC) for the header.
pub fn timestamp_now() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // civil-from-days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", rem / 3600, (rem % 3600) / 60)
}

fn header(parts: &mut Vec<String>, ts: &str) {
    parts.push(format!(
        r##"$noop(
########################################################################
#                                                                      #
#  MusicBrainz Picard File Naming Script                               #
#  Generated: {ts}                               #
#                                                                      #
#  Created with Picard Script Generator                                #
#                                                                      #
########################################################################
)
"##
    ));
}

fn settings(parts: &mut Vec<String>, c: &ScriptConfig) {
    parts.push(
        r##"$noop(
########################################################################
#  USER SETTINGS - Modify these values to customize behavior           #
########################################################################
)"##
        .to_string(),
    );
    parts.push(format!(
        r##"
$noop( Padding minimum lengths )
$set(_PaddedDiscNumMinLength,1)
$set(_PaddedTrackNumMinLength,{})

$noop( Maximum lengths for truncation )
$set(_aTitleMaxLength,{})
$set(_tTitleMaxLength,{})
$set(_tFilenameMaxLength,{})
"##,
        c.track_padding_length, c.max_album_length, c.max_title_length, c.max_filename_length
    ));
}

fn constants(parts: &mut Vec<String>) {
    parts.push(
        r##"
$noop(
########################################################################
#  CONSTANTS                                                           #
########################################################################
)
$set(_cUnknownArtistID,125ec42a-7229-4250-afc5-e057484327fe)
$set(_cVariousArtistID,89ad4ac3-39f7-470e-963a-56509c546377)
$set(_cUnknownArtist,[Unknown Artist])
$set(_cVariousArtist,Various Artists)
$set(_cUnknownAlbum,[Unknown Album])
$set(_cNoTitle,[Unknown Title])
"##
        .to_string(),
    );
}

fn working_variables(parts: &mut Vec<String>, c: &ScriptConfig) {
    parts.push(
        r##"
$noop(
########################################################################
#  WORKING VARIABLES - Set up variables with fallback values           #
########################################################################
)

$noop( Album Artist - with fallbacks )
$set(_nAlbumArtist,$if2(%albumartist%,%artist%,%_cUnknownArtist%))
$set(_nAlbumArtistSort,$if2(%albumartistsort%,%artistsort%,%_nAlbumArtist%))

$noop( Track Artist )
$set(_nTrackArtist,$if2(%artist%,%_cUnknownArtist%))

$noop( Album and Track titles )
$set(_nAlbum,$if2(%album%,%_cUnknownAlbum%))
$set(_nTitle,$if2(%title%,%_cNoTitle%))

$noop( Disc and track numbers with fallbacks )
$set(_nTotalDiscs,$if2(%totaldiscs%,1))
$set(_nDiscNum,$if2(%discnumber%,1))
$set(_nTotalTracks,$if2(%totaltracks%,1))
$set(_nTrackNum,$if2(%tracknumber%,1))
"##
        .to_string(),
    );
    if c.use_original_year {
        parts.push(
            r##"
$noop( Release year - prefer original date )
$set(_nYear,$left($if2(%originaldate%,%originalyear%,%date%,0000),4))
"##
            .to_string(),
        );
    } else {
        parts.push(
            r##"
$noop( Release year )
$set(_nYear,$left($if2(%date%,%originaldate%,%originalyear%,0000),4))
"##
            .to_string(),
        );
    }
    parts.push(
        r##"
$noop( Calculate padding lengths based on totals )
$set(_DiscPadLength,$if($gt($len(%_nTotalDiscs%),%_PaddedDiscNumMinLength%),$len(%_nTotalDiscs%),%_PaddedDiscNumMinLength%))
$set(_TrackPadLength,$if($gt($len(%_nTotalTracks%),%_PaddedTrackNumMinLength%),$len(%_nTotalTracks%),%_PaddedTrackNumMinLength%))

$noop( Padded disc and track numbers )
$set(_nPaddedDiscNum,$num(%_nDiscNum%,%_DiscPadLength%))
$set(_nPaddedTrackNum,$num(%_nTrackNum%,%_TrackPadLength%))
"##
        .to_string(),
    );
}

fn sanitization(parts: &mut Vec<String>) {
    parts.push(
        r##"
$noop(
########################################################################
#  CHARACTER SANITIZATION - Replace invalid characters                 #
########################################################################
)

$noop( Create sanitized versions of metadata for file/folder names )
$set(_nAlbumSafe,$rreplace(%_nAlbum%,[\\/:*?"<>|]+,_))
$set(_nTitleSafe,$rreplace(%_nTitle%,[\\/:*?"<>|]+,_))
$set(_nAlbumArtistSafe,$rreplace(%_nAlbumArtist%,[\\/:*?"<>|]+,_))
$set(_nTrackArtistSafe,$rreplace(%_nTrackArtist%,[\\/:*?"<>|]+,_))
"##
        .to_string(),
    );
}

fn file_path(parts: &mut Vec<String>, c: &ScriptConfig) {
    parts.push(
        r##"
$noop(
########################################################################
#  FOLDER PATH GENERATION                                              #
########################################################################
)
"##
        .to_string(),
    );

    let mut path_parts: Vec<String> = Vec::new();

    if c.use_artist_folder {
        if c.artist_folder_style == "first_letter_subfolder" {
            parts.push(
                r##"
$noop( Get first letter for artist grouping )
$set(_nInitial,$upper($firstalphachar($if2(%_nAlbumArtistSort%,%_nAlbumArtistSafe%),#)))
"##
                .to_string(),
            );
            path_parts.push("%_nInitial%".into());
        }
        if c.format_album_artist == "sort" {
            path_parts.push("%_nAlbumArtistSort%".into());
        } else if c.replace_invalid_chars {
            path_parts.push("%_nAlbumArtistSafe%".into());
        } else {
            path_parts.push("%_nAlbumArtist%".into());
        }
    }

    parts.push(
        r##"
$noop( Check for Various Artists )
$set(_isVA,$eq(%musicbrainz_albumartistid%,%_cVariousArtistID%))
"##
        .to_string(),
    );

    if c.separate_soundtracks {
        parts.push(
            r##"
$noop( Check for Soundtrack )
$set(_isSoundtrack,$in(%_secondaryreleasetype%,soundtrack))
"##
            .to_string(),
        );
    }

    if c.use_album_folder {
        let album = if c.replace_invalid_chars { "%_nAlbumSafe%" } else { "%_nAlbum%" };
        let mut album_parts: Vec<String> = Vec::new();
        if c.include_year_in_album {
            if c.year_position == "prefix" {
                album_parts.push("[%_nYear%]".into());
                if c.include_disambiguation {
                    album_parts.push(format!(" {album}$if(%_releasecomment%, \\(%_releasecomment%\\),)"));
                } else {
                    album_parts.push(format!(" {album}"));
                }
            } else {
                album_parts.push(album.into());
                album_parts.push(" [%_nYear%]".into());
            }
        } else {
            album_parts.push(album.into());
        }
        if c.include_label {
            album_parts.push("$if(%label%, [%label%],)".into());
        }
        if c.include_catalog {
            album_parts.push("$if(%catalognumber%, {%catalognumber%},)".into());
        }
        if c.include_format {
            album_parts.push(" [$upper(%_extension%)]".into());
        }
        path_parts.push(album_parts.concat());
    }

    if !path_parts.is_empty() {
        let joined = path_parts.join("/");
        if c.use_artist_folder && c.separate_soundtracks {
            let sound = if path_parts.len() > 1 { path_parts[1..].join("/") } else { path_parts[0].clone() };
            let last = path_parts.last().cloned().unwrap_or_default();
            parts.push(format!(
                r##"
$noop( Build folder path )
$set(_nFilePath,
    $if(%_isSoundtrack%,
        Soundtracks/{sound},
        $if(%_isVA%,
            %_cVariousArtist%/{last},
            {joined}
        )
    )
)
"##
            ));
        } else if c.use_artist_folder {
            let va = if path_parts.len() > 1 { path_parts.last().cloned().unwrap_or_default() } else { path_parts[0].clone() };
            parts.push(format!(
                r##"
$noop( Build folder path )
$set(_nFilePath,
    $if(%_isVA%,
        %_cVariousArtist%/{va},
        {joined}
    )
)
"##
            ));
        } else {
            parts.push(format!(
                r##"
$noop( Build folder path )
$set(_nFilePath,{joined})
"##
            ));
        }
    } else {
        parts.push(
            r##"
$noop( No folder structure )
$set(_nFilePath,)
"##
            .to_string(),
        );
    }

    if c.use_disc_subfolder {
        let disc = match c.disc_folder_format.as_str() {
            "side" => "Side",
            "cd" => "CD",
            _ => "Disc",
        };
        if c.include_disc_subtitle {
            parts.push(format!(
                r##"
$noop( Add disc subfolder for multi-disc releases )
$if($gt(%_nTotalDiscs%,1),
    $set(_nFilePath,%_nFilePath%/{disc} %_nPaddedDiscNum%$if(%discsubtitle%, - %discsubtitle%,))
)
"##
            ));
        } else {
            parts.push(format!(
                r##"
$noop( Add disc subfolder for multi-disc releases )
$if($gt(%_nTotalDiscs%,1),
    $set(_nFilePath,%_nFilePath%/{disc} %_nPaddedDiscNum%)
)
"##
            ));
        }
    }
}

fn filename(parts: &mut Vec<String>, c: &ScriptConfig) {
    parts.push(
        r##"
$noop(
########################################################################
#  FILENAME GENERATION                                                 #
########################################################################
)
"##
        .to_string(),
    );

    let mut fp: Vec<String> = Vec::new();

    if c.include_track_number {
        if c.include_disc_in_track && !c.use_disc_subfolder {
            fp.push("$if($gt(%_nTotalDiscs%,1),%_nPaddedDiscNum%-,)%_nPaddedTrackNum%".into());
        } else {
            fp.push("%_nPaddedTrackNum%".into());
        }
    }

    if c.include_artist_in_filename || c.include_track_artist_for_va {
        let artist = if c.replace_invalid_chars { "%_nTrackArtistSafe%" } else { "%_nTrackArtist%" };
        if c.include_artist_in_filename {
            fp.push(format!("{}{artist}", c.artist_separator));
        } else if c.include_track_artist_for_va {
            fp.push(format!("$if(%_isVA%,{}{artist},)", c.va_artist_separator));
        }
    }

    if c.include_title {
        let title = if c.replace_invalid_chars { "%_nTitleSafe%" } else { "%_nTitle%" };
        if !fp.is_empty() {
            if c.include_track_number {
                fp.push(format!(". {title}"));
            } else {
                fp.push(format!(" - {title}"));
            }
        } else {
            fp.push(title.into());
        }
    }

    if c.show_featured_artists {
        let feat = match c.feat_format.as_str() {
            "ft." => "ft.",
            "featuring" => "featuring",
            "with" => "with",
            _ => "feat.",
        };
        parts.push(format!(
            r##"
$noop( Determine if there are featured artists )
$set(_nFeat,
    $if($and($ne(%artist%,%albumartist%),$ne(%_isVA%,1)),
        $if($in(%artist%,feat.),
            ,
            $if($ne($lower(%artist%),$lower(%albumartist%)),
                 [{feat} %_nTrackArtist%],
            )
        ),
    )
)
"##
        ));
        fp.push("%_nFeat%".into());
    }

    if !fp.is_empty() {
        parts.push(format!(
            r##"
$noop( Build filename )
$set(_nFileName,{})
"##,
            fp.concat()
        ));
    }

    parts.push(
        r##"
$noop( Truncate filename if too long )
$if($gt($len(%_nFileName%),%_tFilenameMaxLength%),
    $set(_nFileName,$left(%_nFileName%,$sub(%_tFilenameMaxLength%,3))...)
)
"##
        .to_string(),
    );
}

fn output(parts: &mut Vec<String>) {
    parts.push(
        r##"
$noop(
########################################################################
#  OUTPUT - Final path and filename                                    #
########################################################################
)

$noop( Combine path and filename, ensure no double slashes )
$if(%_nFilePath%,
    %_nFilePath%/%_nFileName%,
    %_nFileName%
)
"##
        .to_string(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The script the original Python generator wrote for this configuration (saved in
    /// tests/fixtures/picard, with the time stamp replaced by {TS}).
    fn fixture(name: &str) -> (String, ScriptConfig) {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/picard/");
        let text = std::fs::read_to_string(format!("{dir}{name}.txt")).unwrap();
        let cfg: ScriptConfig = serde_json::from_str(&std::fs::read_to_string(format!("{dir}{name}.json")).unwrap()).unwrap();
        (text, cfg)
    }

    #[test]
    fn output_is_identical_to_the_original_generator() {
        for name in ["simple", "organized", "detailed", "flat", "minimal", "audiophile", "will", "odd", "nofolders"] {
            let (expected, cfg) = fixture(name);
            let got = build(&cfg, "{TS}");
            assert_eq!(got, expected, "the '{name}' script differs from the original generator's");
        }
    }

    #[test]
    fn the_presets_are_the_originals() {
        for (id, _, _, _, cfg) in presets() {
            let (_, original) = fixture(id);
            assert_eq!(cfg, original, "preset {id}");
        }
    }

    #[test]
    fn a_script_made_from_your_choices_is_the_one_in_use() {
        // The naming script RustyDisc ships is what this generator makes for these settings.
        let (_, cfg) = fixture("will");
        let made = build(&cfg, "2026-02-19 10:36");
        assert_eq!(made.trim(), crate::library::script::DEFAULT_SCRIPT.trim());
    }

    #[test]
    fn every_generated_script_parses_and_makes_sensible_paths() {
        use std::collections::HashMap;
        let m = |pairs: &[(&str, &str)]| -> HashMap<String, String> { pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() };
        let album = m(&[("albumartist", "The Beatles"), ("albumartistsort", "Beatles, The"), ("artist", "The Beatles"), ("album", "Abbey Road"), ("date", "1969-09-26"),
            ("tracknumber", "1"), ("totaltracks", "17"), ("title", "Come Together"), ("totaldiscs", "1"), ("_extension", "flac"), ("label", "Apple"),
            ("catalognumber", "PCS7088"), ("_releasecomment", "Remaster")]);
        for (id, _, _, _, cfg) in presets() {
            let script = build(&cfg, "x");
            crate::library::script::check(&script).unwrap_or_else(|e| panic!("{id}: {e}"));
            let out = crate::library::script::run(&script, &album).unwrap_or_else(|e| panic!("{id}: {e}"));
            assert!(out.contains("Come Together"), "{id}: {out}");
        }
        // spot-check two presets against their documented examples
        let run = |id: &str| {
            let cfg = presets().into_iter().find(|p| p.0 == id).unwrap().4;
            crate::library::script::to_components(&crate::library::script::run(&build(&cfg, "x"), &album).unwrap()).join("/")
        };
        assert_eq!(run("simple"), "The Beatles/[1969] Abbey Road/01. Come Together");
        assert_eq!(run("audiophile"), "B/Beatles, The/[1969] Abbey Road (Remaster) [FLAC]/01. Come Together");
    }

    #[test]
    fn the_time_stamp_is_a_real_date() {
        let t = timestamp_now();
        assert_eq!(t.len(), 16);
        assert!(t.starts_with("20"), "{t}");
    }
}
