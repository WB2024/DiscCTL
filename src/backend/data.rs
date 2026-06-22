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
         .arg("-V").arg(&vol_label)
         .arg("--dereference");

    if session.joliet   { mkiso.arg("-J"); }
    if session.rock_ridge { mkiso.arg("-r"); }

    if let Some(ms) = msinfo {
        mkiso.arg("-C").arg(ms).arg("-M").arg(device);
    }

    mkiso.arg("-o").arg(&iso_path).arg(&session.source_dir);

    if debug { eprintln!("Running: {:?}", mkiso); }

    if progress_json {
        emit_step("Building ISO image...");
        // xorriso mkisofs writes progress to stdout: " 5.00% done, estimate finish..."
        mkiso.stdout(Stdio::piped());
        mkiso.stderr(Stdio::null());
        let mut child = mkiso.spawn()?;

        if let Some(stdout) = child.stdout.take() {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Some(pct) = parse_xorriso_pct(&line) {
                    // Scale mkisofs phase to 0–45%
                    emit_progress(pct * 0.45);
                }
            }
        }
        let status = child.wait()?;
        if !status.success() {
            let _ = std::fs::remove_file(&iso_path);
            return Err(Error::backend(format!(
                "xorriso mkisofs failed with exit code: {:?}", status.code()
            )));
        }
    } else {
        let status = mkiso.status()?;
        if !status.success() {
            let _ = std::fs::remove_file(&iso_path);
            return Err(Error::backend(format!(
                "xorriso mkisofs failed with exit code: {:?}", status.code()
            )));
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
        // xorriso cdrecord writes progress to stderr: "xorriso : UPDATE :  5.00% done"
        write_cmd.stdout(Stdio::null());
        write_cmd.stderr(Stdio::piped());
        let mut child = write_cmd.spawn()?;

        if let Some(stderr) = child.stderr.take() {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if line.to_lowercase().contains("closing") || line.contains("Fixating") {
                    emit_step("Closing disc...");
                    emit_progress(99.0);
                } else if let Some(pct) = parse_xorriso_pct(&line) {
                    // Scale write phase to 45–99%
                    emit_progress(45.0 + pct * 0.54);
                }
            }
        }
        let status = child.wait()?;
        let _ = std::fs::remove_file(&iso_path);
        if !status.success() {
            return Err(Error::backend(format!(
                "xorriso cdrecord failed with exit code: {:?}", status.code()
            )));
        }
    } else {
        let status = write_cmd.status()?;
        let _ = std::fs::remove_file(&iso_path);
        if !status.success() {
            return Err(Error::backend(format!(
                "xorriso cdrecord failed with exit code: {:?}", status.code()
            )));
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
