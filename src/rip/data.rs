use std::process::Command;
use crate::error::Error;

/// Extract a data session from an optical disc.
///
/// `device` — source drive
/// `output_dir` — destination directory; will be created if it does not exist
/// `as_iso` — if true, produce a single `session.iso` file; if false, extract
///   the filesystem contents as a directory tree using xorriso
pub fn extract_data_session(
    device: &str,
    output_dir: &str,
    as_iso: bool,
    debug: bool,
) -> Result<(), Error> {
    std::fs::create_dir_all(output_dir)?;

    if as_iso {
        extract_as_iso(device, output_dir, debug)
    } else {
        extract_as_tree(device, output_dir, debug)
    }
}

fn extract_as_iso(device: &str, output_dir: &str, debug: bool) -> Result<(), Error> {
    let iso_path = format!("{}/session.iso", output_dir);

    // Use ddrescue if available for best results on worn media; fall back to dd.
    let tool = if std::path::Path::new("/usr/bin/ddrescue").exists() {
        "ddrescue"
    } else {
        "dd"
    };

    let output = if tool == "ddrescue" {
        Command::new("ddrescue")
            .arg("--no-scrape")
            .arg(device)
            .arg(&iso_path)
            .arg(format!("{}/ddrescue.log", output_dir))
            .output()?
    } else {
        Command::new("dd")
            .arg(format!("if={}", device))
            .arg(format!("of={}", iso_path))
            .arg("bs=2048")
            .arg("conv=noerror,sync")
            .output()?
    };

    if debug {
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
    }

    if !output.status.success() && !std::path::Path::new(&iso_path).exists() {
        return Err(Error::backend(format!(
            "Failed to extract ISO from {}: {}",
            device,
            String::from_utf8_lossy(&output.stderr).lines().last().unwrap_or("")
        )));
    }

    Ok(())
}

fn extract_as_tree(device: &str, output_dir: &str, debug: bool) -> Result<(), Error> {
    // xorriso -osirrox copies the ISO filesystem contents to a directory.
    let mut cmd = Command::new("xorriso");
    cmd.arg("-osirrox").arg("on")
       .arg("-indev").arg(device)
       .arg("-extract").arg("/").arg(output_dir)
       .arg("--");

    if debug { eprintln!("Running: {:?}", cmd); }

    let output = cmd.output()?;

    // xorriso exits non-zero on warnings; check stderr for actual failures.
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let has_failure = stderr.contains("FAILURE") || stderr.contains("FATAL");
        if has_failure {
            let detail: String = stderr.lines()
                .filter(|l| l.contains("FAILURE") || l.contains("FATAL") || l.contains("Error"))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(Error::backend(format!("xorriso extract failed: {}", detail)));
        }
    }

    Ok(())
}
