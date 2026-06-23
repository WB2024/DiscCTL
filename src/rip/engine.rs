use std::io::Read;
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
///
/// Output files are named: track01.cdda.wav, track02.cdda.wav, etc. — the
/// standard cdparanoia batch-mode naming convention.
///
/// Calls `on_progress` with (0.0..=1.0) once the rip is complete (cdparanoia
/// does not expose per-sector progress in a parseable form, so we report
/// completion as a single event).
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
    }

    let mut cmd = Command::new("cdparanoia");
    cmd.arg("-d").arg(device)
       .arg("-B")           // batch mode: one WAV per track
       .arg("-w");          // force WAV output
    // cdparanoia writes to the current directory; we change the working dir.
    cmd.current_dir(output_dir);

    if !debug {
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::piped());
    }

    if debug { eprintln!("Running: {:?}", cmd); }

    let mut child = cmd.spawn()?;

    // Drain stderr so the child never blocks on a full pipe.
    let mut _stderr_bytes: Vec<u8> = Vec::new();
    if !debug {
        if let Some(mut stderr) = child.stderr.take() {
            let mut buf = [0u8; 4096];
            loop {
                match stderr.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => _stderr_bytes.extend_from_slice(&buf[..n]),
                }
            }
        }
    }

    let status = child.wait()?;
    if !status.success() {
        let msg = String::from_utf8_lossy(&_stderr_bytes);
        return Err(Error::backend(format!(
            "cdparanoia failed (exit {:?}): {}",
            status.code(),
            msg.lines().last().unwrap_or("see debug output")
        )));
    }

    if progress_json {
        emit_progress(5.0); // rip done, encoding starts
    }

    // Collect files in the order cdparanoia created them.
    let mut tracks: Vec<(usize, String)> = Vec::new();
    for i in 1..=track_count {
        let name = format!("track{:02}.cdda.wav", i);
        let path = format!("{}/{}", output_dir, name);
        if Path::new(&path).exists() {
            tracks.push((i, path));
        } else {
            // cdparanoia sometimes zero-pads differently; try alternate naming.
            let alt = format!("{}/track{:02}.cdda.wav", output_dir, i);
            if Path::new(&alt).exists() {
                tracks.push((i, alt));
            }
        }
    }

    if tracks.is_empty() {
        return Err(Error::backend(
            "cdparanoia produced no output files — check disc and device",
        ));
    }

    Ok(tracks)
}

pub(super) fn emit_progress(pct: f32) {
    println!("{{\"type\":\"progress\",\"pct\":{:.1}}}", pct);
}

pub(super) fn emit_step(msg: &str) {
    let escaped = msg.replace('\\', "\\\\").replace('"', "\\\"");
    println!("{{\"type\":\"step\",\"msg\":\"{}\"}}", escaped);
}
