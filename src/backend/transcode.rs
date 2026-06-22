use std::path::Path;
use std::process::Command;
use crate::error::Error;
use crate::parser::playlist::PlaylistEntry;

// ── Spec ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TranscodeSpec {
    pub format: OutputFormat,
    pub bitrate_kbps: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OutputFormat {
    Mp3,
    Aac,
    Opus,
    Flac,
    Wav,
}

impl TranscodeSpec {
    /// Parse "mp3:256", "aac:320", "opus:192", "flac", "wav"
    pub fn parse(s: &str) -> Result<Self, Error> {
        let mut parts = s.splitn(2, ':');
        let fmt_str = parts.next().unwrap_or("").trim().to_lowercase();
        let bitrate_kbps = parts
            .next()
            .and_then(|b| b.trim().trim_end_matches('k').parse::<u32>().ok());

        let format = match fmt_str.as_str() {
            "mp3"  => OutputFormat::Mp3,
            "aac"  => OutputFormat::Aac,
            "opus" => OutputFormat::Opus,
            "flac" => OutputFormat::Flac,
            "wav"  => OutputFormat::Wav,
            other  => return Err(Error::validation(format!(
                "Unknown transcode format '{}'. Valid options: mp3, aac, opus, flac, wav", other
            ))),
        };

        if matches!(format, OutputFormat::Mp3 | OutputFormat::Aac | OutputFormat::Opus)
            && bitrate_kbps.is_none()
        {
            return Err(Error::validation(format!(
                "Lossy format '{}' requires a bitrate, e.g. {}:256", fmt_str, fmt_str
            )));
        }

        Ok(TranscodeSpec { format, bitrate_kbps })
    }

    pub fn extension(&self) -> &'static str {
        match self.format {
            OutputFormat::Mp3  => "mp3",
            OutputFormat::Aac  => "m4a",
            OutputFormat::Opus => "opus",
            OutputFormat::Flac => "flac",
            OutputFormat::Wav  => "wav",
        }
    }
}

// ── Public entry points ───────────────────────────────────────────────────────

/// Transcode a list of playlist entries into a flat staging directory.
/// Output files are named "<nn> - <display_name>.<ext>".
pub fn transcode_playlist(
    entries: &[PlaylistEntry],
    spec: &TranscodeSpec,
    stage_dir: &str,
    debug: bool,
) -> Result<(), Error> {
    ensure_ffmpeg()?;
    std::fs::create_dir_all(stage_dir)?;

    let total = entries.len();
    for (i, entry) in entries.iter().enumerate() {
        let track_num = i + 1;

        let display = entry
            .display
            .as_deref()
            .unwrap_or_else(|| stem(&entry.path));

        let safe = sanitize(display);
        let filename = format!("{:02} - {}.{}", track_num, safe, spec.extension());
        let out_path = Path::new(stage_dir).join(&filename);

        eprintln!("[{}/{}] {}", track_num, total, filename);

        transcode_file(&entry.path, out_path.to_str().unwrap(), spec, debug)?;
    }

    Ok(())
}

/// Transcode all audio files under source_dir into stage_dir, preserving the
/// directory tree. Non-audio files are copied verbatim.
pub fn transcode_dir(
    source_dir: &str,
    stage_dir: &str,
    spec: &TranscodeSpec,
    debug: bool,
) -> Result<(), Error> {
    ensure_ffmpeg()?;
    let src = Path::new(source_dir);
    let dst = Path::new(stage_dir);
    std::fs::create_dir_all(dst)?;
    transcode_tree(src, src, dst, spec, debug)
}

// ── Internal ──────────────────────────────────────────────────────────────────

fn transcode_tree(
    root: &Path,
    dir: &Path,
    dest_root: &Path,
    spec: &TranscodeSpec,
    debug: bool,
) -> Result<(), Error> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let path = entry.path();
        let rel = path.strip_prefix(root).map_err(|_| {
            Error::backend("Internal error: path strip failed during transcode")
        })?;

        if path.is_dir() {
            std::fs::create_dir_all(dest_root.join(rel))?;
            transcode_tree(root, &path, dest_root, spec, debug)?;
        } else if is_audio(&path) {
            let dest = dest_root.join(rel).with_extension(spec.extension());
            if let Some(p) = dest.parent() { std::fs::create_dir_all(p)?; }
            eprintln!("Transcoding: {}", rel.display());
            transcode_file(
                &path.to_string_lossy(),
                &dest.to_string_lossy(),
                spec,
                debug,
            )?;
        } else {
            // Non-audio: copy as-is (cover art, cue sheets, etc.)
            let dest = dest_root.join(rel);
            if let Some(p) = dest.parent() { std::fs::create_dir_all(p)?; }
            std::fs::copy(&path, &dest)?;
        }
    }
    Ok(())
}

fn transcode_file(input: &str, output: &str, spec: &TranscodeSpec, debug: bool) -> Result<(), Error> {
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y").arg("-i").arg(input);

    match spec.format {
        OutputFormat::Mp3 => {
            cmd.arg("-codec:a").arg("libmp3lame");
            if let Some(kbps) = spec.bitrate_kbps {
                // -b:a gives CBR for libmp3lame
                cmd.arg("-b:a").arg(format!("{}k", kbps));
            }
        }
        OutputFormat::Aac => {
            cmd.arg("-codec:a").arg("aac");
            if let Some(kbps) = spec.bitrate_kbps {
                cmd.arg("-b:a").arg(format!("{}k", kbps));
            }
        }
        OutputFormat::Opus => {
            cmd.arg("-codec:a").arg("libopus");
            if let Some(kbps) = spec.bitrate_kbps {
                cmd.arg("-b:a").arg(format!("{}k", kbps));
            }
        }
        OutputFormat::Flac => {
            cmd.arg("-codec:a").arg("flac");
        }
        OutputFormat::Wav => {
            cmd.arg("-ar").arg("44100")
                .arg("-ac").arg("2")
                .arg("-sample_fmt").arg("s16");
        }
    }

    if !debug {
        cmd.arg("-loglevel").arg("error");
    }
    cmd.arg(output);

    if debug { println!("Running: {:?}", cmd); }

    let status = cmd.status()?;
    if !status.success() {
        return Err(Error::backend(format!(
            "ffmpeg failed transcoding '{}': exit code {:?}",
            input, status.code()
        )));
    }

    Ok(())
}

fn ensure_ffmpeg() -> Result<(), Error> {
    let ok = Command::new("which").arg("ffmpeg").output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !ok {
        return Err(Error::backend(
            "ffmpeg is required for transcoding but was not found in PATH. \
             Install ffmpeg and try again."
        ));
    }
    Ok(())
}

fn is_audio(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase().as_str(),
        "flac" | "mp3" | "m4a" | "aac" | "ogg" | "opus" | "wma" | "wav" | "aiff" | "ape" | "wv"
    )
}

fn stem(path: &str) -> &str {
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("track")
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c => c,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

// ── Staged directory RAII ─────────────────────────────────────────────────────

/// Wraps a staging directory. Deletes it on drop unless `keep` is set.
pub struct StagedDir {
    pub path: String,
    pub keep: bool,
    pub auto_created: bool,
}

impl StagedDir {
    pub fn new(path: String, keep: bool, auto_created: bool) -> Self {
        StagedDir { path, keep, auto_created }
    }
}

impl Drop for StagedDir {
    fn drop(&mut self) {
        if !self.keep && self.auto_created {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mp3_with_bitrate() {
        let spec = TranscodeSpec::parse("mp3:256").unwrap();
        assert_eq!(spec.format, OutputFormat::Mp3);
        assert_eq!(spec.bitrate_kbps, Some(256));
        assert_eq!(spec.extension(), "mp3");
    }

    #[test]
    fn parse_mp3_with_k_suffix() {
        let spec = TranscodeSpec::parse("mp3:320k").unwrap();
        assert_eq!(spec.bitrate_kbps, Some(320));
    }

    #[test]
    fn parse_aac() {
        let spec = TranscodeSpec::parse("aac:192").unwrap();
        assert_eq!(spec.format, OutputFormat::Aac);
        assert_eq!(spec.extension(), "m4a");
    }

    #[test]
    fn parse_opus() {
        let spec = TranscodeSpec::parse("opus:128").unwrap();
        assert_eq!(spec.format, OutputFormat::Opus);
    }

    #[test]
    fn parse_flac_no_bitrate() {
        let spec = TranscodeSpec::parse("flac").unwrap();
        assert_eq!(spec.format, OutputFormat::Flac);
        assert!(spec.bitrate_kbps.is_none());
    }

    #[test]
    fn rejects_mp3_without_bitrate() {
        assert!(TranscodeSpec::parse("mp3").is_err());
    }

    #[test]
    fn rejects_unknown_format() {
        assert!(TranscodeSpec::parse("wma:256").is_err());
    }

    #[test]
    fn sanitize_strips_path_chars() {
        assert_eq!(sanitize("AC/DC: Highway to Hell"), "AC_DC_ Highway to Hell");
    }
}
