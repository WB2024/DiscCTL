use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum DiscFormat {
    RedBook,
    DataCD,
    BlueBook,
    /// Files on a DVD or Blu-ray (ISO 9660 + UDF).
    DataDvd,
    /// A DVD-Video disc that plays the music in any DVD player, optionally with a data folder.
    MusicDvd,
}

impl DiscFormat {
    pub fn is_dvd(&self) -> bool {
        matches!(self, DiscFormat::DataDvd | DiscFormat::MusicDvd)
    }
}

impl std::fmt::Display for DiscFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiscFormat::RedBook => write!(f, "redbook"),
            DiscFormat::DataCD => write!(f, "datacd"),
            DiscFormat::BlueBook => write!(f, "bluebook"),
            DiscFormat::DataDvd => write!(f, "datadvd"),
            DiscFormat::MusicDvd => write!(f, "musicdvd"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Session {
    Audio(AudioSession),
    Data(DataSession),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioSession {
    pub tracks: Vec<String>,
    /// Disc-level CD-Text (album title, artist).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cd_text: Option<CdText>,
    /// Per-track CD-Text. Index aligns with tracks[]. Missing entries fall back
    /// to auto-generated "Track NN" titles.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_titles: Option<Vec<TrackTitle>>,
}

/// Disc-level CD-Text metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CdText {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
}

/// Per-track CD-Text metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackTitle {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataSession {
    pub source_dir: String,
    #[serde(default)]
    pub filesystem: Filesystem,
    #[serde(default)]
    pub joliet: bool,
    #[serde(default)]
    pub rock_ridge: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Filesystem {
    #[default]
    Iso9660,
}

impl std::fmt::Display for Filesystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Filesystem::Iso9660 => write!(f, "iso9660"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscGraph {
    pub format: DiscFormat,
    pub label: String,
    pub sessions: Vec<Session>,
    /// Settings for `musicdvd` discs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dvd: Option<DvdOptions>,
    /// Write speed as an "x" multiple (8 = 8x). Absent: the drive chooses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<u32>,
    /// Level the audio before burning (audio CDs only). Absent: the audio is burned as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub normalize: Option<crate::backend::normalize::Spec>,
}

/// How the audio on a Music DVD is encoded and presented.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DvdOptions {
    /// Dolby Digital (AC-3) stereo bitrate: 192, 256, 384 or 448.
    pub audio_kbps: u32,
    /// Picture standard of the disc: PAL (25 fps, Europe) or NTSC (29.97 fps, Americas/Japan).
    pub standard: VideoStandard,
    /// Picture shown while a track plays. Empty: the cover art next to the tracks, or a plain background.
    pub still: Option<String>,
}

impl Default for DvdOptions {
    fn default() -> Self {
        DvdOptions { audio_kbps: 448, standard: VideoStandard::Pal, still: None }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum VideoStandard {
    #[default]
    Pal,
    Ntsc,
}

impl std::str::FromStr for VideoStandard {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "pal" => Ok(VideoStandard::Pal),
            "ntsc" => Ok(VideoStandard::Ntsc),
            other => Err(format!("unknown video standard '{other}' (use pal or ntsc)")),
        }
    }
}

impl DvdOptions {
    pub fn validate(&self) -> Result<(), String> {
        if ![192, 256, 384, 448].contains(&self.audio_kbps) {
            return Err(format!("audio bitrate {} kbps isn't supported (use 192, 256, 384 or 448)", self.audio_kbps));
        }
        Ok(())
    }
}
