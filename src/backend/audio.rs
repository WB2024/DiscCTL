use std::io::Read;
use std::process::{Command, Stdio};
use crate::{error::Error, model::disc::{AudioSession, CdText, TrackTitle}};
use super::convert;

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
    progress_json: bool,
) -> Result<(), Error> {
    let toc = generate_toc(session);
    let toc_path = format!("/tmp/rustydisc_{}.toc", std::process::id());
    std::fs::write(&toc_path, &toc)?;

    if debug {
        eprintln!("=== TOC ({}) ===\n{}", toc_path, toc);
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
        eprintln!("Running: {:?}", cmd);
    }

    let total_tracks = session.tracks.len().max(1) as f32;

    if progress_json {
        emit_step("Writing audio session...");
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::piped());
        let mut child = cmd.spawn()?;

        let mut tracks_done = 0.0f32;
        let mut stderr_bytes: Vec<u8> = Vec::new();
        if let Some(mut stderr) = child.stderr.take() {
            drain_with_progress(&mut stderr, &mut stderr_bytes, |line| {
                let lower = line.to_lowercase();
                if lower.contains("writing track") {
                    emit_step(line);
                } else if lower.contains("done.") || line.contains(": DONE") {
                    tracks_done += 1.0;
                } else if let Some(pct) = parse_pct(line) {
                    let overall = ((tracks_done + pct / 100.0) / total_tracks) * 100.0;
                    emit_progress(overall.min(99.0));
                } else if lower.contains("fixating") {
                    emit_step("Fixating disc...");
                    emit_progress(99.5);
                }
            });
        }

        let status = child.wait()?;
        let _ = std::fs::remove_file(&toc_path);
        if !status.success() {
            let stderr_msg = String::from_utf8_lossy(&stderr_bytes);
            let detail = stderr_msg.lines()
                .filter(|l| l.contains("ERROR") || l.contains("error") || l.contains("failed"))
                .collect::<Vec<_>>()
                .join("; ");
            let msg = if detail.is_empty() {
                format!("cdrdao failed (exit {:?})", status.code())
            } else {
                format!("cdrdao failed: {}", detail)
            };
            return Err(Error::backend(msg));
        }
    } else {
        let output = cmd.output()?;
        let _ = std::fs::remove_file(&toc_path);
        if !output.status.success() {
            let stderr_msg = String::from_utf8_lossy(&output.stderr);
            let detail = stderr_msg.lines()
                .filter(|l| l.contains("ERROR") || l.contains("error") || l.contains("failed"))
                .collect::<Vec<_>>()
                .join("; ");
            let msg = if detail.is_empty() {
                format!("cdrdao failed (exit {:?})", output.status.code())
            } else {
                format!("cdrdao failed: {}", detail)
            };
            return Err(Error::backend(msg));
        }
    }

    Ok(())
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn drain_with_progress<F>(reader: &mut impl Read, buf_out: &mut Vec<u8>, mut on_line: F)
where
    F: FnMut(&str),
{
    let mut buf = [0u8; 4096];
    let mut line_buf: Vec<u8> = Vec::new();

    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                buf_out.extend_from_slice(&buf[..n]);
                for &byte in &buf[..n] {
                    if byte == b'\n' {
                        let line = String::from_utf8_lossy(&line_buf);
                        on_line(line.trim_end_matches('\r'));
                        line_buf.clear();
                    } else {
                        line_buf.push(byte);
                    }
                }
            }
            Err(_) => break,
        }
    }
    if !line_buf.is_empty() {
        let line = String::from_utf8_lossy(&line_buf);
        on_line(line.trim_end_matches('\r'));
    }
}

fn parse_pct(line: &str) -> Option<f32> {
    // Matches "  45% done." or just "45%"
    let trimmed = line.trim();
    let pct_pos = trimmed.find('%')?;
    let before = trimmed[..pct_pos].trim();
    // Take the last whitespace-delimited token before '%'
    before.split_whitespace().last()?.parse::<f32>().ok()
}

fn emit_progress(pct: f32) {
    println!("{{\"type\":\"progress\",\"pct\":{:.1}}}", pct);
}

fn emit_step(msg: &str) {
    let escaped = msg.replace('\\', "\\\\").replace('"', "\\\"");
    println!("{{\"type\":\"step\",\"msg\":\"{}\"}}", escaped);
}

// ── TOC generation ────────────────────────────────────────────────────────────

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
        let after_t2 = toc.split("TITLE \"Song Two\"").nth(1).unwrap();
        assert!(after_t2.contains("PERFORMER \"Band\""));
    }

    #[test]
    fn toc_partial_track_titles() {
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

    #[test]
    fn parse_pct_cdrdao_style() {
        assert_eq!(parse_pct(" 45% done."), Some(45.0));
        assert_eq!(parse_pct("  0% done."), Some(0.0));
        assert_eq!(parse_pct("100% done."), Some(100.0));
        assert_eq!(parse_pct("no pct here"), None);
    }
}
