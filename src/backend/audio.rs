use std::io::Read;
use std::process::{Command, Stdio};
use crate::{error::Error, model::disc::{AudioSession, CdText, TrackTitle}};
use super::convert;

/// Convert the session's tracks to disc audio: links are fetched first, and a track can name the
/// audio stream to use (see `source`).
pub fn prepare_tracks(session: &AudioSession, debug: bool) -> Result<PreparedSession, Error> {
    let mut prepared_tracks = Vec::new();
    let mut temp_files = Vec::new();

    for track in &session.tracks {
        let src = super::source::parse(track);
        let mut local = src.location.to_string();
        if super::source::is_url(&local) {
            let downloaded = super::source::fetch(&local, debug)?;
            temp_files.push(downloaded.to_string_lossy().to_string());
            if !super::source::has_audio(&downloaded) {
                return Err(Error::validation(format!("{local} doesn't contain audio ffmpeg can read")));
            }
            local = downloaded.to_string_lossy().to_string();
        }
        let converted = convert::convert_track(&local, src.stream, 0.0, debug)?;
        if converted != local {
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

impl PreparedSession {
    /// Apply a gain (dB) to each track, one per track; 0 leaves a track as it is.
    pub fn apply_gains(&mut self, gains: &[f64], debug: bool) -> Result<(), Error> {
        for (i, gain) in gains.iter().enumerate() {
            if *gain == 0.0 || i >= self.tracks.len() {
                continue;
            }
            let old = self.tracks[i].clone();
            let new = convert::convert_track(&old, None, *gain, debug)?;
            // The audio before the gain was a temporary file of ours, unless it was the user's own WAV.
            if let Some(pos) = self._temp_files.iter().position(|t| *t == old) {
                if new != old {
                    let _ = std::fs::remove_file(&old);
                    self._temp_files.remove(pos);
                }
            }
            if new != old {
                self._temp_files.push(new.clone());
            }
            self.tracks[i] = new;
        }
        Ok(())
    }
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

/// The `cdrdao write` arguments for an audio session.
pub(crate) fn cdrdao_write_args(device: &str, keep_open: bool, speed: Option<u32>, toc_path: &str) -> Vec<String> {
    let mut a: Vec<String> = ["write", "--device", device, "--driver", "generic-mmc-raw"].iter().map(|s| s.to_string()).collect();
    if keep_open {
        a.push("--multi".into());
    }
    if let Some(x) = speed {
        a.extend(["--speed".into(), x.to_string()]);
    }
    a.push(toc_path.to_string());
    a
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
    cmd.args(cdrdao_write_args(device, keep_open, super::speed::get(), &toc_path));

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
mod source_tests {
    use super::*;
    use std::{io::{Read, Write}, net::TcpListener, process::Command};

    fn have_ffmpeg() -> bool {
        Command::new("ffmpeg").arg("-version").output().is_ok()
    }

    /// Sign changes in the PCM of a CD WAV, which count the tone's frequency (2 per cycle).
    fn crossings(path: &str) -> usize {
        let b = std::fs::read(path).unwrap();
        let pcm = &b[44..];
        let left: Vec<i16> = pcm.chunks_exact(4).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
        left.windows(2).filter(|w| (w[0] < 0) != (w[1] < 0)).count()
    }

    #[test]
    fn a_chosen_stream_and_a_fetched_link_become_disc_audio() {
        if !have_ffmpeg() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("rustydisc_prep_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A file with two audio streams: 300 Hz first, 900 Hz second, 2 seconds each.
        let mkv = dir.join("two.mkv");
        assert!(Command::new("ffmpeg")
            .args(["-loglevel", "error", "-y", "-f", "lavfi", "-i", "sine=f=300:d=2", "-f", "lavfi", "-i", "sine=f=900:d=2", "-map", "0:a", "-map", "1:a"])
            .arg(&mkv).status().unwrap().success());
        // A 600 Hz WAV served over HTTP.
        let wav = dir.join("link.wav");
        assert!(Command::new("ffmpeg")
            .args(["-loglevel", "error", "-y", "-f", "lavfi", "-i", "sine=f=600:d=2", "-ac", "2", "-ar", "44100", "-sample_fmt", "s16"])
            .arg(&wav).status().unwrap().success());
        let body = std::fs::read(&wav).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/tone.wav", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf);
                let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes());
                let _ = s.write_all(&body);
            }
        });

        let session = AudioSession {
            tracks: vec![format!("{}#stream=1", mkv.display()), url, mkv.display().to_string()],
            cd_text: None,
            track_titles: None,
        };
        let prepared = prepare_tracks(&session, false).unwrap();
        assert_eq!(prepared.tracks.len(), 3);
        // Stream 1 is the 900 Hz tone: about 3600 sign changes in 2 s; stream 0 (the default) is 300 Hz: about 1200.
        let (c1, c2, c3) = (crossings(&prepared.tracks[0]), crossings(&prepared.tracks[1]), crossings(&prepared.tracks[2]));
        assert!((3500..3700).contains(&c1), "chosen stream is 900 Hz: {c1}");
        assert!((2300..2500).contains(&c2), "the fetched link is 600 Hz: {c2}");
        assert!((1100..1300).contains(&c3), "no choice means the default stream, 300 Hz: {c3}");
        let temps = prepared.tracks.clone();
        drop(prepared);
        assert!(temps.iter().all(|t| !std::path::Path::new(t).exists()), "temporary files are cleaned up");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn cdrdao_gets_the_speed_before_the_toc() {
        let a = super::cdrdao_write_args("/dev/sr0", true, Some(8), "x.toc");
        assert_eq!(a, ["write", "--device", "/dev/sr0", "--driver", "generic-mmc-raw", "--multi", "--speed", "8", "x.toc"]);
        let auto = super::cdrdao_write_args("/dev/sr0", false, None, "x.toc");
        assert!(!auto.iter().any(|x| x == "--speed"), "auto sends no speed");
    }

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
