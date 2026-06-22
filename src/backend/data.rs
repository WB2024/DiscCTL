use std::io::Read;
use std::process::{Command, Stdio};
use crate::{error::Error, model::disc::DataSession};

pub fn append_data_session(
    session: &DataSession,
    device: &str,
    msinfo: Option<&str>,
    label: &str,
    debug: bool,
    progress_json: bool,
) -> Result<(), Error> {
    let iso_path = format!("/tmp/rustydisc_{}.iso", std::process::id());

    let vol_label: String = label
        .chars()
        .take(32)
        .map(|c| c.to_ascii_uppercase())
        .collect();

    // ── Phase 1: build ISO image ──────────────────────────────────────────────
    let mut mkiso = Command::new("xorriso");
    mkiso.arg("-as").arg("mkisofs")
         .arg("-V").arg(&vol_label);

    if session.joliet     { mkiso.arg("-J"); }
    if session.rock_ridge { mkiso.arg("-r"); }

    if let Some(ms) = msinfo {
        mkiso.arg("-C").arg(ms).arg("-M").arg(device);
    }

    mkiso.arg("-o").arg(&iso_path).arg(&session.source_dir);

    if debug { eprintln!("Running: {:?}", mkiso); }

    if progress_json {
        emit_step("Building ISO image...");
        // xorriso mkisofs writes progress to stderr, not stdout.
        // Stdout is empty; pipe it so it doesn't inherit ours.
        mkiso.stdout(Stdio::null());
        mkiso.stderr(Stdio::piped());
        let mut child = mkiso.spawn()?;

        // Drain stderr with raw-byte reads so invalid UTF-8 never stops us early.
        let mut stderr_bytes: Vec<u8> = Vec::new();
        if let Some(mut stderr) = child.stderr.take() {
            let mut buf = [0u8; 4096];
            loop {
                match stderr.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let chunk = &buf[..n];
                        stderr_bytes.extend_from_slice(chunk);
                        // Emit progress from any "X% done" lines seen so far
                        let text = String::from_utf8_lossy(chunk);
                        for line in text.lines() {
                            if let Some(pct) = parse_xorriso_pct(line) {
                                emit_progress(pct * 0.45);
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
        }
        let status = child.wait()?;
        if !status.success() {
            let _ = std::fs::remove_file(&iso_path);
            let stderr_msg = String::from_utf8_lossy(&stderr_bytes);
            let detail = stderr_msg.lines()
                .filter(|l| l.contains("FAILURE") || l.contains("FATAL") || l.contains("Error"))
                .collect::<Vec<_>>()
                .join("; ");
            let msg = if detail.is_empty() {
                format!("xorriso mkisofs failed (exit {:?})", status.code())
            } else {
                format!("xorriso mkisofs failed: {}", detail)
            };
            return Err(Error::backend(msg));
        }
    } else {
        let output = mkiso.output()?;
        if !output.status.success() {
            let _ = std::fs::remove_file(&iso_path);
            let stderr_msg = String::from_utf8_lossy(&output.stderr);
            let detail = stderr_msg.lines()
                .filter(|l| l.contains("FAILURE") || l.contains("FATAL") || l.contains("Error"))
                .collect::<Vec<_>>()
                .join("; ");
            let msg = if detail.is_empty() {
                format!("xorriso mkisofs failed (exit {:?})", output.status.code())
            } else {
                format!("xorriso mkisofs failed: {}", detail)
            };
            return Err(Error::backend(msg));
        }
    }

    // ── Phase 2: write ISO to disc ────────────────────────────────────────────
    let mut write_cmd = Command::new("xorriso");
    write_cmd
        .arg("-as").arg("cdrecord")
        .arg(format!("dev={}", device))
        .arg("-data");

    if msinfo.is_some() {
        write_cmd.arg("-multi");
    }
    write_cmd.arg(&iso_path);

    if debug { eprintln!("Running: {:?}", write_cmd); }

    if progress_json {
        emit_step("Writing to disc...");
        write_cmd.stdout(Stdio::null());
        write_cmd.stderr(Stdio::piped());
        let mut child = write_cmd.spawn()?;

        // Raw-byte drain: BufReader::lines() stops on non-UTF-8, which closes the pipe
        // early and deadlocks child.wait(). Read raw bytes to always drain fully.
        let mut stderr_bytes: Vec<u8> = Vec::new();
        if let Some(mut stderr) = child.stderr.take() {
            let mut buf = [0u8; 4096];
            loop {
                match stderr.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let chunk = &buf[..n];
                        stderr_bytes.extend_from_slice(chunk);
                        let text = String::from_utf8_lossy(chunk);
                        for line in text.lines() {
                            let lower = line.to_lowercase();
                            if lower.contains("closing") || lower.contains("fixating") {
                                emit_step("Closing disc...");
                                emit_progress(99.0);
                            } else if let Some(pct) = parse_xorriso_pct(line) {
                                emit_progress(45.0 + pct * 0.54);
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
        }
        let status = child.wait()?;
        let _ = std::fs::remove_file(&iso_path);
        if !status.success() {
            let stderr_msg = String::from_utf8_lossy(&stderr_bytes);
            let detail = stderr_msg.lines()
                .filter(|l| l.contains("FAILURE") || l.contains("FATAL") || l.contains("Error"))
                .collect::<Vec<_>>()
                .join("; ");
            let msg = if detail.is_empty() {
                format!("xorriso cdrecord failed (exit {:?})", status.code())
            } else {
                format!("xorriso cdrecord failed: {}", detail)
            };
            return Err(Error::backend(msg));
        }
    } else {
        let output = write_cmd.output()?;
        let _ = std::fs::remove_file(&iso_path);
        if !output.status.success() {
            let stderr_msg = String::from_utf8_lossy(&output.stderr);
            let detail = stderr_msg.lines()
                .filter(|l| l.contains("FAILURE") || l.contains("FATAL") || l.contains("Error"))
                .collect::<Vec<_>>()
                .join("; ");
            let msg = if detail.is_empty() {
                format!("xorriso cdrecord failed (exit {:?})", output.status.code())
            } else {
                format!("xorriso cdrecord failed: {}", detail)
            };
            return Err(Error::backend(msg));
        }
    }

    Ok(())
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn parse_xorriso_pct(line: &str) -> Option<f32> {
    // Matches:
    //   "xorriso : UPDATE :  5.00% done"
    //   " 5.00% done, estimate finish ..."
    let pos = line.find('%')?;
    let before = line[..pos].trim();
    before.split_whitespace().last()?.parse::<f32>().ok()
}

fn emit_progress(pct: f32) {
    println!("{{\"type\":\"progress\",\"pct\":{:.1}}}", pct);
}

fn emit_step(msg: &str) {
    let escaped = msg.replace('\\', "\\\\").replace('"', "\\\"");
    println!("{{\"type\":\"step\",\"msg\":\"{}\"}}", escaped);
}
