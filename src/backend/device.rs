use std::path::Path;
use std::process::Command;
use crate::error::Error;

#[derive(Debug, PartialEq)]
pub enum DiscMediaType {
    CdR,
    CdRw,
    Unknown,
}

/// State of the disc currently loaded in the drive.
#[derive(Debug, PartialEq)]
pub enum DiscState {
    /// No disc or blank disc — nothing written yet.
    Blank,
    /// All sessions closed and disc is finalized. No further writing possible.
    Finalized,
    /// One or more sessions closed, disc is still appendable (multisession in progress).
    Appendable { msinfo: String },
    /// A session was started but not closed — likely an interrupted burn.
    OpenSession,
    /// Could not determine state (no disc, unrecognized drive response, etc.).
    Unknown(String),
}

pub fn check_device(device: &str) -> Result<(), Error> {
    if !Path::new(device).exists() {
        return Err(Error::device(format!("Device not found: {}", device)));
    }
    Ok(())
}

/// Query the state of the disc in the drive.
pub fn query_disc_state(device: &str) -> Result<DiscState, Error> {
    // ATIP is only present on blank/writable CD-R and CD-RW discs.
    // A finalized disc's lead-in is closed, so ATIP is not readable.
    let atip = Command::new("cdrecord")
        .arg(format!("dev={}", device))
        .arg("-atip")
        .output()?;

    let atip_text = format!(
        "{}{}",
        String::from_utf8_lossy(&atip.stdout),
        String::from_utf8_lossy(&atip.stderr)
    )
    .to_lowercase();

    if atip_text.contains("atip start of lead in") {
        // Writable disc detected. Check if any sessions have been written.
        let msinfo = Command::new("cdrecord")
            .arg(format!("dev={}", device))
            .arg("-msinfo")
            .output()?;

        if msinfo.status.success() {
            let info = String::from_utf8_lossy(&msinfo.stdout).trim().to_string();
            if !info.is_empty() {
                return Ok(DiscState::Appendable { msinfo: info });
            }
        }

        // ATIP present but no sessions written yet — blank disc.
        return Ok(DiscState::Blank);
    }

    // No ATIP — disc is finalized, a pressed CD, or no disc present.
    // Check TOC for actual track data to distinguish finalized from empty drive.
    let toc = Command::new("cdrecord")
        .arg(format!("dev={}", device))
        .arg("-toc")
        .output()?;

    let toc_text = format!(
        "{}{}",
        String::from_utf8_lossy(&toc.stdout),
        String::from_utf8_lossy(&toc.stderr)
    )
    .to_lowercase();

    if toc_text.contains("open session") || toc_text.contains("incomplete") {
        return Ok(DiscState::OpenSession);
    }

    // Look for actual track entries (e.g. "track   1") not just the word "toc"
    if toc_text.contains("track   1") || toc_text.contains("track  1") || toc_text.contains("lba") {
        return Ok(DiscState::Finalized);
    }

    if toc_text.contains("no disc") || toc_text.contains("no medium") {
        return Ok(DiscState::Unknown("No disc detected".to_string()));
    }

    Ok(DiscState::Unknown(toc_text.trim().chars().take(200).collect()))
}

/// Attempt to recover a disc with an open (interrupted) session by closing it.
/// Returns true if the recovery command ran successfully.
pub fn recover_open_session(device: &str, debug: bool) -> Result<bool, Error> {
    let state = query_disc_state(device)?;

    match state {
        DiscState::OpenSession => {
            eprintln!("Detected open session on {}. Attempting to close it...", device);
            let mut cmd = Command::new("cdrecord");
            cmd.arg(format!("dev={}", device)).arg("-fix");

            if debug {
                eprintln!("Running: {:?}", cmd);
            }

            let output = cmd.output()?;
            if output.status.success() {
                eprintln!("Session closed successfully.");
                Ok(true)
            } else {
                Err(Error::device(format!(
                    "cdrecord -fix failed on {}: {}",
                    device,
                    String::from_utf8_lossy(&output.stderr).trim()
                )))
            }
        }
        DiscState::Blank => {
            eprintln!("Disc on {} is blank — nothing to recover.", device);
            Ok(false)
        }
        DiscState::Appendable { .. } => {
            eprintln!("Disc on {} is appendable — no recovery needed.", device);
            Ok(false)
        }
        DiscState::Finalized => {
            Err(Error::device(format!(
                "Disc on {} is already finalized. Cannot recover or write further.",
                device
            )))
        }
        DiscState::Unknown(detail) => Err(Error::device(format!(
            "Cannot determine disc state on {}: {}",
            device, detail
        ))),
    }
}

/// Blank a CD-RW disc. `mode` is "fast" or "all".
pub fn blank_cdrw(device: &str, mode: &str, debug: bool) -> Result<(), Error> {
    match detect_media_type(device)? {
        DiscMediaType::CdR => {
            return Err(Error::device(
                "Cannot blank a CD-R disc — blanking is only supported on CD-RW.",
            ));
        }
        DiscMediaType::CdRw | DiscMediaType::Unknown => {}
    }

    let mut cmd = Command::new("cdrecord");
    cmd.arg(format!("dev={}", device))
        .arg(format!("blank={}", mode));

    if debug {
        eprintln!("Running: {:?}", cmd);
    }

    let output = cmd.output()?;
    if !output.status.success() {
        return Err(Error::device(format!(
            "cdrecord blank={} failed on {}: {}",
            mode,
            device,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    Ok(())
}

/// Detect whether the loaded disc is CD-R or CD-RW.
pub fn detect_media_type(device: &str) -> Result<DiscMediaType, Error> {
    let output = Command::new("cdrecord")
        .arg(format!("dev={}", device))
        .arg("-v")
        .arg("-checkdrive")
        .output()?;

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

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

    // Case-insensitive search covers: BurnProof (cdrdao), BURNFREE (cdrecord/wodim),
    // BURN-Proof, SMART-BURN, JustLink, buffer underrun protection variants
    let lower = text.to_lowercase();
    Ok(lower.contains("burnproof")
        || lower.contains("burnfree")
        || lower.contains("burn-proof")
        || lower.contains("smart-burn")
        || lower.contains("justlink")
        || lower.contains("buffer underrun"))
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
        eprintln!("Running: {:?}", cmd);
    }

    let output = cmd.output()?;
    if !output.status.success() {
        return Err(Error::device(format!(
            "Failed to finalize disc on {}: {}",
            device,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    Ok(())
}
