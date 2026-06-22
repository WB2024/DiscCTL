use std::process::Command;
use crate::{error::Error, model::disc::DataSession};

pub fn append_data_session(
    session: &DataSession,
    device: &str,
    msinfo: Option<&str>,
    label: &str,
    debug: bool,
) -> Result<(), Error> {
    let iso_path = format!("/tmp/discctl_{}.iso", std::process::id());

    let mut mkiso = Command::new("xorriso");
    mkiso.arg("-as").arg("mkisofs");

    // ISO 9660 volume label: uppercase, max 32 chars
    let vol_label: String = label
        .chars()
        .take(32)
        .map(|c| c.to_ascii_uppercase())
        .collect();
    mkiso.arg("-V").arg(&vol_label)
         .arg("--dereference"); // follow symlinks rather than recording them

    if session.joliet {
        mkiso.arg("-J");
    }
    if session.rock_ridge {
        mkiso.arg("-r");
    }

    // Multisession: reference previous session so the new one links to it
    if let Some(ms) = msinfo {
        mkiso.arg("-C").arg(ms).arg("-M").arg(device);
    }

    mkiso.arg("-o").arg(&iso_path).arg(&session.source_dir);

    if debug {
        println!("Running: {:?}", mkiso);
    }

    let status = mkiso.status()?;
    if !status.success() {
        return Err(Error::backend(format!(
            "xorriso mkisofs failed with exit code: {:?}",
            status.code()
        )));
    }

    // Write the ISO to disc
    let mut write_cmd = Command::new("xorriso");
    write_cmd
        .arg("-as").arg("cdrecord")
        .arg(format!("dev={}", device))
        .arg("-data");

    if msinfo.is_some() {
        write_cmd.arg("-multi");
    }

    write_cmd.arg(&iso_path);

    if debug {
        println!("Running: {:?}", write_cmd);
    }

    let status = write_cmd.status()?;
    let _ = std::fs::remove_file(&iso_path);

    if !status.success() {
        return Err(Error::backend(format!(
            "xorriso cdrecord failed with exit code: {:?}",
            status.code()
        )));
    }

    Ok(())
}
