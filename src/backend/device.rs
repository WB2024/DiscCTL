use std::path::Path;
use std::process::Command;
use crate::error::Error;

#[derive(Debug, PartialEq)]
pub enum DiscMediaType {
    CdR,
    CdRw,
    Unknown,
}

pub fn check_device(device: &str) -> Result<(), Error> {
    if !Path::new(device).exists() {
        return Err(Error::device(format!("Device not found: {}", device)));
    }
    Ok(())
}

/// Detect whether the loaded disc is CD-R or CD-RW.
/// CD-RW requires blanking before reuse; CD-R is write-once.
pub fn detect_media_type(device: &str) -> Result<DiscMediaType, Error> {
    let output = Command::new("cdrecord")
        .arg(format!("dev={}", device))
        .arg("-v")
        .arg("-checkdrive")
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{}{}", stdout, stderr);

    if combined.contains("ReWritable") || combined.contains("CD-RW") {
        Ok(DiscMediaType::CdRw)
    } else if combined.contains("Recordable") || combined.contains("CD-R") {
        Ok(DiscMediaType::CdR)
    } else {
        Ok(DiscMediaType::Unknown)
    }
}

/// Returns true if the drive reports buffer underrun protection (BURN-Proof / SMART-BURN).
pub fn has_buffer_underrun_protection(device: &str) -> Result<bool, Error> {
    let output = Command::new("cdrecord")
        .arg(format!("dev={}", device))
        .arg("-v")
        .arg("-checkdrive")
        .output()?;

    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    Ok(text.contains("BURN-Proof")
        || text.contains("SMART-BURN")
        || text.contains("buffer underrun")
        || text.contains("Burnfree"))
}

pub fn get_msinfo(device: &str) -> Result<String, Error> {
    let output = Command::new("cdrecord")
        .arg(format!("dev={}", device))
        .arg("-msinfo")
        .output()?;

    if !output.status.success() {
        return Err(Error::device(format!(
            "Failed to read multisession info from {}: {}",
            device,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    let msinfo = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if msinfo.is_empty() {
        return Err(Error::device(format!(
            "Empty multisession info from {}. Is the disc appendable?",
            device
        )));
    }

    Ok(msinfo)
}

#[allow(dead_code)]
pub fn supports_multisession(device: &str) -> Result<bool, Error> {
    let output = Command::new("cdrecord")
        .arg(format!("dev={}", device))
        .arg("-checkdrive")
        .output()?;
    Ok(output.status.success())
}

pub fn finalize_disc(device: &str, debug: bool) -> Result<(), Error> {
    let mut cmd = Command::new("cdrecord");
    cmd.arg(format!("dev={}", device)).arg("-fix");

    if debug {
        println!("Running: {:?}", cmd);
    }

    let status = cmd.status()?;
    if !status.success() {
        return Err(Error::device(format!(
            "Failed to finalize disc on {}: exit code {:?}",
            device,
            status.code()
        )));
    }

    Ok(())
}
