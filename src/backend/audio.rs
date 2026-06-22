use std::process::Command;
use crate::{error::Error, model::disc::AudioSession};

pub fn write_audio_session(
    session: &AudioSession,
    device: &str,
    keep_open: bool,
    debug: bool,
) -> Result<(), Error> {
    let toc = generate_toc(session);
    let toc_path = format!("/tmp/discctl_{}.toc", std::process::id());
    std::fs::write(&toc_path, &toc)?;

    if debug {
        println!("=== TOC ({}) ===\n{}", toc_path, toc);
    }

    let mut cmd = Command::new("cdrdao");
    cmd.arg("write")
        .arg("--device").arg(device)
        .arg("--driver").arg("generic-mmc-raw");

    if keep_open {
        cmd.arg("--multi");
    }

    cmd.arg(&toc_path);

    if debug {
        println!("Running: {:?}", cmd);
    }

    let status = cmd.status()?;
    let _ = std::fs::remove_file(&toc_path);

    if !status.success() {
        return Err(Error::backend(format!(
            "cdrdao failed with exit code: {:?}",
            status.code()
        )));
    }

    Ok(())
}

fn generate_toc(session: &AudioSession) -> String {
    let mut toc = String::from("CD_DA\n\n");

    if let Some(cd_text) = &session.cd_text {
        toc.push_str("CD_TEXT {\n  LANGUAGE_MAP { 0:EN }\n  LANGUAGE 0 {\n");
        if let Some(title) = &cd_text.title {
            toc.push_str(&format!("    TITLE \"{}\"\n", title));
        }
        if let Some(artist) = &cd_text.artist {
            toc.push_str(&format!("    PERFORMER \"{}\"\n", artist));
        }
        toc.push_str("  }\n}\n\n");
    }

    for (i, track) in session.tracks.iter().enumerate() {
        toc.push_str("TRACK AUDIO\n");
        if let Some(cd_text) = &session.cd_text {
            toc.push_str("CD_TEXT {\n  LANGUAGE 0 {\n");
            toc.push_str(&format!("    TITLE \"Track {:02}\"\n", i + 1));
            if let Some(artist) = &cd_text.artist {
                toc.push_str(&format!("    PERFORMER \"{}\"\n", artist));
            }
            toc.push_str("  }\n}\n");
        }
        toc.push_str(&format!("FILE \"{}\" 0\n\n", track));
    }

    toc
}
