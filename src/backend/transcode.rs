use std::path::Path;
use std::process::Command;
use crate::error::Error;

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

    /// Short description, e.g. "MP3 320k".
    pub fn label(&self) -> String {
        let name = match self.format {
            OutputFormat::Mp3 => "MP3",
            OutputFormat::Aac => "AAC",
            OutputFormat::Opus => "Opus",
            OutputFormat::Flac => "FLAC",
            OutputFormat::Wav => "WAV",
        };
        match self.bitrate_kbps {
            Some(k) => format!("{name} {k}k"),
            None => name.to_string(),
        }
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

// ── Public entry points ───────────────────────────────────────────────────────

/// Convert one audio file. Audio only (embedded cover art is dropped) with tags kept.
pub fn transcode_file(input: &str, output: &str, spec: &TranscodeSpec, debug: bool) -> Result<(), Error> {
    transcode_file_art(input, output, spec, false, debug)
}

/// Like [`transcode_file`], optionally keeping the embedded cover picture (MP3, M4A and FLAC
/// can carry it; other formats ignore the request).
pub fn transcode_file_art(input: &str, output: &str, spec: &TranscodeSpec, keep_art: bool, debug: bool) -> Result<(), Error> {
    let mut cmd = Command::new("ffmpeg");
    // Never read the terminal: another part of the program may be waiting on it for a keypress.
    cmd.arg("-nostdin").stdin(std::process::Stdio::null());
    let art = keep_art && matches!(spec.format, OutputFormat::Mp3 | OutputFormat::Aac | OutputFormat::Flac);
    if art {
        // Audio plus the first picture stream (if there is one), copied as it is.
        cmd.arg("-y").arg("-i").arg(input)
            .args(["-map", "0:a:0", "-map", "0:v:0?", "-c:v", "copy", "-disposition:v:0", "attached_pic", "-map_metadata", "0"]);
        if spec.format == OutputFormat::Mp3 {
            cmd.args(["-id3v2_version", "3"]);
        }
    } else {
        // Audio only, tags kept. Embedded cover art is dropped: it would be copied into every
        // file and make sizes unpredictable when planning how many discs are needed.
        cmd.arg("-y").arg("-i").arg(input).arg("-vn").arg("-map_metadata").arg("0");
    }

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

pub fn ensure_ffmpeg() -> Result<(), Error> {
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

pub fn is_audio(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase().as_str(),
        "flac" | "mp3" | "m4a" | "aac" | "ogg" | "opus" | "wma" | "wav" | "aiff" | "ape" | "wv"
    )
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

}
