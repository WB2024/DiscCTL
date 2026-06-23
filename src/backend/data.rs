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
    // Wrap in stdbuf -eL to force line-buffered stderr so progress arrives in
    // real time rather than buffered until the process exits.
    let mut mkiso = stdbuf_cmd("xorriso");
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
        mkiso.stdout(Stdio::null());
        mkiso.stderr(Stdio::piped());
        let mut child = mkiso.spawn()?;

        let mut stderr_bytes: Vec<u8> = Vec::new();
        if let Some(mut stderr) = child.stderr.take() {
            drain_with_progress(&mut stderr, &mut stderr_bytes, |line| {
                if let Some(pct) = parse_xorriso_pct(line) {
                    emit_progress(pct * 0.45);
                }
            });
        }
        let status = child.wait()?;
        if !status.success() {
            let _ = std::fs::remove_file(&iso_path);
            return Err(Error::backend(format_xorriso_error(
                "mkisofs", status.code(), &stderr_bytes,
            )));
        }
    } else {
        let output = mkiso.output()?;
        if !output.status.success() {
            let _ = std::fs::remove_file(&iso_path);
            return Err(Error::backend(format_xorriso_error(
                "mkisofs", output.status.code(), &output.stderr,
            )));
        }
    }

    // ── Phase 2: write ISO to disc ────────────────────────────────────────────
    let mut write_cmd = stdbuf_cmd("xorriso");
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

        let mut stderr_bytes: Vec<u8> = Vec::new();
        if let Some(mut stderr) = child.stderr.take() {
            drain_with_progress(&mut stderr, &mut stderr_bytes, |line| {
                let lower = line.to_lowercase();
                if lower.contains("closing") || lower.contains("fixating") {
                    emit_step("Closing disc...");
                    emit_progress(99.0);
                } else if let Some(pct) = parse_xorriso_pct(line) {
                    emit_progress(45.0 + pct * 0.54);
                }
            });
        }
        let status = child.wait()?;
        let _ = std::fs::remove_file(&iso_path);
        if !status.success() {
            return Err(Error::backend(format_xorriso_error(
                "cdrecord", status.code(), &stderr_bytes,
            )));
        }
    } else {
        let output = write_cmd.output()?;
        let _ = std::fs::remove_file(&iso_path);
        if !output.status.success() {
            return Err(Error::backend(format_xorriso_error(
                "cdrecord", output.status.code(), &output.stderr,
            )));
        }
    }

    Ok(())
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// Build a Command wrapped in `stdbuf -eL` to force line-buffered stderr.
/// Falls back to running the command directly if stdbuf is not available.
fn stdbuf_cmd(program: &str) -> Command {
    if std::path::Path::new("/usr/bin/stdbuf").exists()
        || std::path::Path::new("/usr/local/bin/stdbuf").exists()
    {
        let mut cmd = Command::new("stdbuf");
        cmd.arg("-eL").arg(program);
        cmd
    } else {
        Command::new(program)
    }
}

/// Drain a pipe handle line by line using raw bytes so non-UTF-8 output never
/// stops the reader early (avoiding a deadlock in child.wait()). Each complete
/// line is passed to `on_line` for progress parsing.
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
    // Flush any trailing partial line (no final newline)
    if !line_buf.is_empty() {
        let line = String::from_utf8_lossy(&line_buf);
        on_line(line.trim_end_matches('\r'));
    }
}

fn format_xorriso_error(phase: &str, code: Option<i32>, stderr: &[u8]) -> String {
    let stderr_msg = String::from_utf8_lossy(stderr);
    let detail = stderr_msg
        .lines()
        .filter(|l| l.contains("FAILURE") || l.contains("FATAL") || l.contains("Error"))
        .collect::<Vec<_>>()
        .join("; ");
    if detail.is_empty() {
        format!("xorriso {} failed (exit {:?})", phase, code)
    } else {
        format!("xorriso {} failed: {}", phase, detail)
    }
}

fn parse_xorriso_pct(line: &str) -> Option<f32> {
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
