use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum DiscFormat {
    RedBook,
    DataCD,
    BlueBook,
}

impl std::fmt::Display for DiscFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiscFormat::RedBook => write!(f, "redbook"),
            DiscFormat::DataCD => write!(f, "datacd"),
            DiscFormat::BlueBook => write!(f, "bluebook"),
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cd_text: Option<CdText>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CdText {
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
}
