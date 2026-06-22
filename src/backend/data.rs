use std::io::{BufRead, BufReader};
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
        // stdout: " 5.00% done, estimate finish..."  stderr: errors + diagnostics
        mkiso.stdout(Stdio::piped());
        mkiso.stderr(Stdio::piped());
        let mut child = mkiso.spawn()?;

        // Drain stdout for progress (blocking read — stderr buffered by OS until wait)
        if let Some(stdout) = child.stdout.take() {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Some(pct) = parse_xorriso_pct(&line) {
                    emit_progress(pct * 0.45);
                }
            }
        }
        let output = child.wait_with_output()?;
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
        // stderr carries both progress updates and errors; no separate stdout needed
        write_cmd.stdout(Stdio::null());
        write_cmd.stderr(Stdio::piped());
        let mut child = write_cmd.spawn()?;

        let mut stderr_lines: Vec<String> = Vec::new();
        if let Some(stderr) = child.stderr.take() {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if line.to_lowercase().contains("closing") || line.contains("Fixating") {
                    emit_step("Closing disc...");
                    emit_progress(99.0);
                } else if let Some(pct) = parse_xorriso_pct(&line) {
                    emit_progress(45.0 + pct * 0.54);
                }
                stderr_lines.push(line);
            }
        }
        let status = child.wait()?;
        let _ = std::fs::remove_file(&iso_path);
        if !status.success() {
            let detail = stderr_lines.iter()
                .filter(|l| l.contains("FAILURE") || l.contains("FATAL") || l.contains("Error"))
                .cloned()
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
