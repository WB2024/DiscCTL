pub mod cdtext;
pub mod playlist;

use std::fs;
use crate::{error::Error, model::disc::*};

pub fn from_file(path: &str) -> Result<DiscGraph, Error> {
    let content = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&content)?)
}

pub fn from_cli(
    format: &str,
    audio: Option<&[String]>,
    playlist: Option<&str>,
    data: Option<&str>,
    label: &str,
    use_cd_text: bool,
) -> Result<DiscGraph, Error> {
    let format = parse_format(format)?;
    let mut sessions = Vec::new();

    // Playlist takes precedence over --audio glob patterns when both are given
    let audio_tracks: Option<Vec<String>> = if let Some(pl_path) = playlist {
        let entries = playlist::parse(pl_path)?;
        Some(entries.into_iter().map(|e| e.path).collect())
    } else if let Some(patterns) = audio {
        Some(expand_audio_patterns(patterns)?)
    } else {
        None
    };

    if let Some(tracks) = audio_tracks {
        let (cd_text, track_titles) = if use_cd_text {
            cdtext::from_tags(&tracks)
        } else {
            (None, None)
        };

        sessions.push(Session::Audio(AudioSession {
            tracks,
            cd_text,
            track_titles,
        }));
    }

    if let Some(dir) = data {
        sessions.push(Session::Data(DataSession {
            source_dir: dir.to_string(),
            filesystem: Filesystem::Iso9660,
            joliet: true,
            rock_ridge: true,
        }));
    }

    Ok(DiscGraph {
        format,
        label: label.to_string(),
        sessions,
    })
}

fn parse_format(s: &str) -> Result<DiscFormat, Error> {
    match s.to_lowercase().as_str() {
        "redbook" | "red-book" | "audio" => Ok(DiscFormat::RedBook),
        "datacd" | "data-cd" | "data" => Ok(DiscFormat::DataCD),
        "bluebook" | "blue-book" | "cdextra" | "cd-extra" => Ok(DiscFormat::BlueBook),
        other => Err(Error::validation(format!(
            "Unknown format: '{}'. Valid values: redbook, datacd, bluebook",
            other
        ))),
    }
}

pub fn expand_audio_globs(patterns: &[String]) -> Result<Vec<String>, Error> {
    expand_audio_patterns(patterns)
}

fn expand_audio_patterns(patterns: &[String]) -> Result<Vec<String>, Error> {
    let mut tracks = Vec::new();
    for pattern in patterns {
        let path = std::path::Path::new(pattern);
        if path.exists() {
            tracks.push(pattern.clone());
        } else {
            let mut matched: Vec<String> = glob::glob(pattern)
                .map_err(Error::Glob)?
                .filter_map(|r| r.ok())
                .map(|p| p.to_string_lossy().to_string())
                .collect();
            if matched.is_empty() {
                tracks.push(pattern.clone());
            } else {
                matched.sort();
                tracks.extend(matched);
            }
        }
    }
    Ok(tracks)
}
