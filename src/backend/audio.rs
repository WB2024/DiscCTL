use std::process::Command;
use crate::{error::Error, model::disc::AudioSession};
use super::convert;

/// Converts any FLAC tracks to CDDA WAV, returns a session with resolved track paths.
/// Caller is responsible for cleaning up converted temp files.
pub fn prepare_tracks(session: &AudioSession, debug: bool) -> Result<PreparedSession, Error> {
    let mut prepared_tracks = Vec::new();
    let mut temp_files = Vec::new();

    for track in &session.tracks {
        let converted = convert::to_cdda_wav(track, debug)?;
        if converted != *track {
            temp_files.push(converted.clone());
        }
        prepared_tracks.push(converted);
    }

    Ok(PreparedSession {
        tracks: prepared_tracks,
        cd_text: session.cd_text.clone(),
        _temp_files: temp_files,
    })
}

pub struct PreparedSession {
    pub tracks: Vec<String>,
    pub cd_text: Option<crate::model::disc::CdText>,
    /// Temp files to clean up when this struct is dropped
    _temp_files: Vec<String>,
}

impl Drop for PreparedSession {
    fn drop(&mut self) {
        for path in &self._temp_files {
            let _ = std::fs::remove_file(path);
        }
    }
}

pub fn write_audio_session(
    session: &PreparedSession,
    device: &str,
    keep_open: bool,
    debug: bool,
) -> Result<(), Error> {
    let toc = generate_toc(session);
    let toc_path = format!("/tmp/discctl_{}.toc", std::process::id());
    std::fs::write(&toc_path, &toc)?;

    if debug {
        println!("=== TOC ({}) ===\n{}", toc_path, toc);
    }

    let mut cmd = Command::new("cdrdao");
    cmd.arg("write")
        .arg("--device").arg(device)
        .arg("--driver").arg("generic-mmc-raw");

    if keep_open {
        cmd.arg("--multi");
    }

    cmd.arg(&toc_path);

    if debug {
        println!("Running: {:?}", cmd);
    }

    let status = cmd.status()?;
    let _ = std::fs::remove_file(&toc_path);

    if !status.success() {
        return Err(Error::backend(format!(
            "cdrdao failed with exit code: {:?}",
            status.code()
        )));
    }

    Ok(())
}

fn generate_toc(session: &PreparedSession) -> String {
    let mut toc = String::from("CD_DA\n\n");

    if let Some(cd_text) = &session.cd_text {
        toc.push_str("CD_TEXT {\n  LANGUAGE_MAP { 0:EN }\n  LANGUAGE 0 {\n");
        if let Some(title) = &cd_text.title {
            toc.push_str(&format!("    TITLE \"{}\"\n", title));
        }
        if let Some(artist) = &cd_text.artist {
            toc.push_str(&format!("    PERFORMER \"{}\"\n", artist));
        }
        toc.push_str("  }\n}\n\n");
    }

    for (i, track) in session.tracks.iter().enumerate() {
        toc.push_str("TRACK AUDIO\n");
        if let Some(cd_text) = &session.cd_text {
            toc.push_str("CD_TEXT {\n  LANGUAGE 0 {\n");
            toc.push_str(&format!("    TITLE \"Track {:02}\"\n", i + 1));
            if let Some(artist) = &cd_text.artist {
                toc.push_str(&format!("    PERFORMER \"{}\"\n", artist));
            }
            toc.push_str("  }\n}\n");
        }
        toc.push_str(&format!("FILE \"{}\" 0\n\n", track));
    }

    toc
}
