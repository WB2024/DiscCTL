use std::path::Path;
use std::process::Command;
use crate::error::Error;

pub fn check_device(device: &str) -> Result<(), Error> {
    if !Path::new(device).exists() {
        return Err(Error::device(format!("Device not found: {}", device)));
    }
    Ok(())
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
