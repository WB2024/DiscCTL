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

/// Like `to_cdda_wav`, with a gain (dB) applied while converting, even to a file that is already
/// in the right format. A gain of 0 is the plain conversion.
pub fn to_cdda_wav_with_gain(input: &str, gain_db: f64, debug: bool) -> Result<String, Error> {
    convert_track(input, None, gain_db, debug)
}

/// Convert one track to disc audio, optionally choosing one of its audio streams (counting audio
/// streams from 0) and applying a gain in dB. The gain is applied in floating point and dithered
/// back to 16 bits so a cut doesn't leave quantization noise. With no stream choice and no gain
/// this is `to_cdda_wav`.
pub fn convert_track(input: &str, stream: Option<usize>, gain_db: f64, debug: bool) -> Result<String, Error> {
    if stream.is_none() && gain_db == 0.0 {
        return to_cdda_wav(input, debug);
    }
    if which("ffmpeg").is_none() {
        return Err(Error::backend("Choosing an audio stream or levelling the audio needs ffmpeg, which is not installed."));
    }
    let output = format!("/tmp/discctl_conv_{}{}.wav", sanitize_name(input), stream.map(|n| format!("_s{n}")).unwrap_or_default());
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y").arg("-i").arg(input);
    if let Some(n) = stream {
        cmd.arg("-map").arg(format!("0:a:{n}")).arg("-vn");
    }
    if gain_db != 0.0 {
        cmd.arg("-af").arg(format!("volume={gain_db}dB,aresample=44100:dither_method=triangular_hp"));
    }
    cmd.arg("-ar").arg("44100").arg("-ac").arg("2").arg("-sample_fmt").arg("s16").arg(&output);
    if debug {
        println!("Running: {:?}", cmd);
    } else {
        cmd.arg("-loglevel").arg("error");
    }
    let status = cmd.status()?;
    if !status.success() {
        return Err(Error::backend(format!("ffmpeg failed converting '{input}'{}: exit code {:?}", stream.map(|n| format!(" (audio stream {n})")).unwrap_or_default(), status.code())));
    }
    Ok(output)
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
