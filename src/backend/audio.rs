use std::process::Command;
use crate::{error::Error, model::disc::{AudioSession, CdText, TrackTitle}};
use super::convert;

/// Converts any FLAC tracks to CDDA WAV and returns a session with resolved paths.
/// Converted temp files are removed when `PreparedSession` is dropped.
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
        track_titles: session.track_titles.clone(),
        _temp_files: temp_files,
    })
}

pub struct PreparedSession {
    pub tracks: Vec<String>,
    pub cd_text: Option<CdText>,
    pub track_titles: Option<Vec<TrackTitle>>,
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

    let disc_artist = session.cd_text.as_ref().and_then(|c| c.artist.as_deref());

    for (i, track_path) in session.tracks.iter().enumerate() {
        toc.push_str("TRACK AUDIO\n");

        // Per-track CD-Text: use track_titles[i] if present, fall back to disc-level artist
        // and auto-generated title.
        let per_track = session.track_titles.as_ref().and_then(|v| v.get(i));
        let auto_title = format!("Track {:02}", i + 1);
        let track_title = per_track
            .and_then(|t| t.title.as_deref())
            .unwrap_or(&auto_title);
        let track_artist = per_track
            .and_then(|t| t.artist.as_deref())
            .or(disc_artist);

        if session.cd_text.is_some() || per_track.is_some() {
            toc.push_str("CD_TEXT {\n  LANGUAGE 0 {\n");
            toc.push_str(&format!("    TITLE \"{}\"\n", track_title));
            if let Some(artist) = track_artist {
                toc.push_str(&format!("    PERFORMER \"{}\"\n", artist));
            }
            toc.push_str("  }\n}\n");
        }

        toc.push_str(&format!("FILE \"{}\" 0\n\n", track_path));
    }

    toc
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::disc::CdText;

    fn session(tracks: Vec<&str>, cd_text: Option<CdText>, track_titles: Option<Vec<TrackTitle>>) -> PreparedSession {
        PreparedSession {
            tracks: tracks.iter().map(|s| s.to_string()).collect(),
            cd_text,
            track_titles,
            _temp_files: vec![],
        }
    }

    #[test]
    fn toc_no_cd_text() {
        let s = session(vec!["t1.wav", "t2.wav"], None, None);
        let toc = generate_toc(&s);
        assert!(toc.starts_with("CD_DA\n\n"));
        assert!(toc.contains("FILE \"t1.wav\" 0"));
        assert!(toc.contains("FILE \"t2.wav\" 0"));
        assert!(!toc.contains("CD_TEXT"));
    }

    #[test]
    fn toc_disc_level_cd_text() {
        let s = session(
            vec!["t1.wav"],
            Some(CdText { title: Some("My Album".into()), artist: Some("Artist".into()) }),
            None,
        );
        let toc = generate_toc(&s);
        assert!(toc.contains("TITLE \"My Album\""));
        assert!(toc.contains("PERFORMER \"Artist\""));
        // Auto-generated track title
        assert!(toc.contains("TITLE \"Track 01\""));
    }

    #[test]
    fn toc_per_track_titles_override() {
        let s = session(
            vec!["t1.wav", "t2.wav"],
            Some(CdText { title: Some("Album".into()), artist: Some("Band".into()) }),
            Some(vec![
                TrackTitle { title: Some("Song One".into()), artist: Some("Solo Artist".into()) },
                TrackTitle { title: Some("Song Two".into()), artist: None },
            ]),
        );
        let toc = generate_toc(&s);
        assert!(toc.contains("TITLE \"Song One\""));
        assert!(toc.contains("PERFORMER \"Solo Artist\""));
        assert!(toc.contains("TITLE \"Song Two\""));
        // Track 2 has no per-track artist, falls back to disc artist
        let after_t2 = toc.split("TITLE \"Song Two\"").nth(1).unwrap();
        assert!(after_t2.contains("PERFORMER \"Band\""));
    }

    #[test]
    fn toc_partial_track_titles() {
        // Only one track_title entry for two tracks — second track falls back to auto
        let s = session(
            vec!["t1.wav", "t2.wav"],
            Some(CdText { title: Some("Album".into()), artist: None }),
            Some(vec![
                TrackTitle { title: Some("Opener".into()), artist: None },
            ]),
        );
        let toc = generate_toc(&s);
        assert!(toc.contains("TITLE \"Opener\""));
        assert!(toc.contains("TITLE \"Track 02\""));
    }
}
