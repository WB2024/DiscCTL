use std::path::Path;
use std::process::Command;
use crate::error::Error;

/// Converts an audio file to 44100Hz 16-bit stereo PCM WAV.
/// Tries ffmpeg first, falls back to sox.
/// Returns the path to the converted file (caller must clean up if it differs from input).
pub fn to_cdda_wav(input: &str, debug: bool) -> Result<String, Error> {
    let ext = Path::new(input)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    // Already CDDA-spec WAV — no conversion needed (planner already validated)
    if ext == "wav" {
        return Ok(input.to_string());
    }

    let output_path = format!("/tmp/discctl_conv_{}.wav", sanitize_name(input));

    if try_ffmpeg(input, &output_path, debug)? {
        return Ok(output_path);
    }
    if try_sox(input, &output_path, debug)? {
        return Ok(output_path);
    }

    Err(Error::backend(format!(
        "Cannot convert '{}' to CDDA WAV: neither ffmpeg nor sox is available. \
         Install one of them or supply pre-converted 44100Hz 16-bit stereo WAV files.",
        input
    )))
}

fn try_ffmpeg(input: &str, output: &str, debug: bool) -> Result<bool, Error> {
    if which("ffmpeg").is_none() {
        return Ok(false);
    }

    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y")
        .arg("-i").arg(input)
        .arg("-ar").arg("44100")
        .arg("-ac").arg("2")
        .arg("-sample_fmt").arg("s16")
        .arg(output);

    if debug {
        println!("Running: {:?}", cmd);
    } else {
        cmd.arg("-loglevel").arg("error");
    }

    let status = cmd.status()?;
    if !status.success() {
        return Err(Error::backend(format!(
            "ffmpeg failed converting '{}': exit code {:?}",
            input,
            status.code()
        )));
    }

    Ok(true)
}

fn try_sox(input: &str, output: &str, debug: bool) -> Result<bool, Error> {
    if which("sox").is_none() {
        return Ok(false);
    }

    let mut cmd = Command::new("sox");
    cmd.arg(input)
        .arg("-r").arg("44100")
        .arg("-c").arg("2")
        .arg("-b").arg("16")
        .arg("-e").arg("signed-integer")
        .arg(output);

    if debug {
        println!("Running: {:?}", cmd);
    }

    let status = cmd.status()?;
    if !status.success() {
        return Err(Error::backend(format!(
            "sox failed converting '{}': exit code {:?}",
            input,
            status.code()
        )));
    }

    Ok(true)
}

fn which(tool: &str) -> Option<()> {
    Command::new("which")
        .arg(tool)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|_| ())
}

fn sanitize_name(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("track")
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' })
        .collect()
}
