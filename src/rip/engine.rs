use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use crate::error::Error;

/// Verify cdparanoia is available on this system.
pub fn check_available() -> Result<(), Error> {
    if Path::new("/usr/bin/cdparanoia").exists()
        || Path::new("/usr/local/bin/cdparanoia").exists()
    {
        return Ok(());
    }
    Err(Error::backend(
        "cdparanoia is not installed. Run: sudo apt install cdparanoia",
    ))
}

/// Rip all audio tracks from `device` into `output_dir` as numbered WAV files.
/// Returns a Vec of (track_number, wav_path) in track order.
pub fn rip_all_tracks(
    device: &str,
    output_dir: &str,
    track_count: usize,
    debug: bool,
    progress_json: bool,
) -> Result<Vec<(usize, String)>, Error> {
    check_available()?;
    std::fs::create_dir_all(output_dir)?;

    if progress_json {
        emit_step("Ripping audio tracks from disc...");
        emit_progress(0.0);
    } else {
        eprintln!("Ripping {} tracks from disc (this takes a while)...", track_count);
    }

    let mut cmd = Command::new("cdparanoia");
    cmd.arg("-d").arg(device)
       .arg("-B")   // batch mode: one WAV per track
       .arg("-w");  // force WAV output
    cmd.current_dir(output_dir);
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::piped());

    if debug { eprintln!("Running: {:?}", cmd); }

    let mut child = cmd.spawn()?;

    // Parse cdparanoia's stderr so we can report per-track progress.
    // Key line patterns:
    //   "outputting to track01.cdda.wav"  → just started ripping track 1
    //   "Ripping from sector N (track M"  → also contains track number
    if let Some(stderr) = child.stderr.take() {
        let reader = BufReader::new(stderr);
        let mut last_reported: usize = 0;

        for line in reader.lines().map_while(Result::ok) {
            if debug { eprintln!("[cdparanoia] {}", line); }

            // "outputting to track01.cdda.wav"
            let lower = line.to_lowercase();
            if lower.contains("outputting to track") {
                if let Some(track_num) = parse_track_number_from_output_line(&line) {
                    if track_num != last_reported {
                        last_reported = track_num;
                        if progress_json {
                            let pct = (track_num as f32 - 1.0) / track_count as f32 * 5.0;
                            emit_step(&format!("Ripping track {} of {}...", track_num, track_count));
                            emit_progress(pct);
                        } else {
                            eprintln!("  Ripping track {} of {}...", track_num, track_count);
                        }
                    }
                }
            }
        }
    }

    let status = child.wait()?;
    if !status.success() {
        return Err(Error::backend(format!(
            "cdparanoia failed (exit {:?}) — try running with --debug for details",
            status.code(),
        )));
    }

    if progress_json {
        emit_progress(5.0);
    } else {
        eprintln!("  Rip complete — encoding...");
    }

    // Collect the WAV files cdparanoia wrote.
    let mut tracks: Vec<(usize, String)> = Vec::new();
    for i in 1..=track_count {
        let path = format!("{}/track{:02}.cdda.wav", output_dir, i);
        if Path::new(&path).exists() {
            tracks.push((i, path));
        }
    }

    if tracks.is_empty() {
        return Err(Error::backend(
            "cdparanoia produced no output files — check disc and device",
        ));
    }

    Ok(tracks)
}

/// Parse "outputting to track01.cdda.wav" → Some(1)
fn parse_track_number_from_output_line(line: &str) -> Option<usize> {
    // Find "track" then parse the digits that follow.
    let lower = line.to_lowercase();
    let after = lower.split("track").nth(1)?;
    let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

pub(super) fn emit_progress(pct: f32) {
    println!("{{\"type\":\"progress\",\"pct\":{:.1}}}", pct);
}

pub(super) fn emit_step(msg: &str) {
    let escaped = msg.replace('\\', "\\\\").replace('"', "\\\"");
    println!("{{\"type\":\"step\",\"msg\":\"{}\"}}", escaped);
}
