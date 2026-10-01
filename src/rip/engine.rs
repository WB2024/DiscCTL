use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use crate::error::Error;

/// How hard cdparanoia checks what it reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Paranoia {
    /// Reread and verify every sector (cdparanoia's default; the safest).
    #[default]
    Full,
    /// Only the overlap checking that cdda2wav does (`-Y`): faster, catches less.
    Fast,
    /// No verification or correction at all (`-Z`): fastest, no protection.
    Off,
}

impl Paranoia {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_lowercase().as_str() {
            "" | "full" | "max" | "default" => Ok(Paranoia::Full),
            "fast" | "overlap" => Ok(Paranoia::Fast),
            "off" | "none" => Ok(Paranoia::Off),
            other => Err(format!("'{other}' is not a paranoia level (use full, fast or off)")),
        }
    }

    pub fn flag(self) -> Option<&'static str> {
        match self {
            Paranoia::Full => None,
            Paranoia::Fast => Some("-Y"),
            Paranoia::Off => Some("-Z"),
        }
    }

    /// What the rip log says about the read mode.
    pub fn describe(self) -> &'static str {
        match self {
            Paranoia::Full => "full paranoia (rereads and verifies every sector)",
            Paranoia::Fast => "overlap checking only (no sector verification)",
            Paranoia::Off => "no verification or correction",
        }
    }
}

/// What cdparanoia is called: `RUSTYDISC_CDPARANOIA` can point at another program (used by tests).
fn program() -> String {
    std::env::var("RUSTYDISC_CDPARANOIA").ok().filter(|p| !p.is_empty()).unwrap_or_else(|| "cdparanoia".to_string())
}

/// What a rip read, and how cleanly.
pub struct RipRead {
    /// (track number, WAV path) in track order. A kept hidden track is first, numbered 0.
    pub tracks: Vec<(usize, String)>,
    pub events: super::readhealth::Collector,
    /// What became of the audio before track 1 (`None` when it wasn't asked for).
    pub hidden: super::gaps::HiddenOutcome,
}

/// Verify cdparanoia is available on this system.
pub fn check_available() -> Result<(), Error> {
    if std::env::var("RUSTYDISC_CDPARANOIA").is_ok_and(|p| !p.is_empty()) {
        return Ok(());
    }
    if Path::new("/usr/bin/cdparanoia").exists()
        || Path::new("/usr/local/bin/cdparanoia").exists()
    {
        return Ok(());
    }
    Err(Error::backend(
        "cdparanoia is not installed. Run: sudo apt install cdparanoia",
    ))
}

/// Rip all audio tracks from `device` into `output_dir` as numbered WAV files.
/// Returns the tracks in order, and what cdparanoia reported about reading them.
pub fn rip_all_tracks(
    device: &str,
    output_dir: &str,
    track_count: usize,
    paranoia: Paranoia,
    hidden_sectors: Option<u32>,
    debug: bool,
    progress_json: bool,
) -> Result<RipRead, Error> {
    check_available()?;
    std::fs::create_dir_all(output_dir)?;

    if progress_json {
        emit_step("Ripping audio tracks from disc...");
        emit_progress(0.0);
    } else {
        eprintln!("Ripping {} tracks from disc (this takes a while)...", track_count);
    }

    let mut events = super::readhealth::Collector::default();

    // Audio in front of track 1 (a hidden track) is "track 0" to cdparanoia, read on its own first.
    let mut hidden = super::gaps::HiddenOutcome::None;
    let mut hidden_path: Option<String> = None;
    if let Some(sectors) = hidden_sectors {
        let secs = super::gaps::seconds(sectors);
        if progress_json {
            emit_step(&format!("Reading the hidden audio before track 1 ({})...", super::gaps::mmss(sectors)));
        } else {
            eprintln!("Reading the hidden audio before track 1 ({})...", super::gaps::mmss(sectors));
        }
        let path = format!("{}/track00.cdda.wav", output_dir);
        match rip_track_zero(device, &path, paranoia, &mut events, debug) {
            Ok(()) => match super::gaps::is_silent(&path) {
                Ok(true) => {
                    let _ = std::fs::remove_file(&path);
                    hidden = super::gaps::HiddenOutcome::Silent { seconds: secs };
                }
                Ok(false) => {
                    hidden = super::gaps::HiddenOutcome::Kept { seconds: secs };
                    hidden_path = Some(path);
                }
                Err(e) => hidden = super::gaps::HiddenOutcome::Failed { seconds: secs, message: e.to_string() },
            },
            Err(message) => {
                let _ = std::fs::remove_file(&path);
                hidden = super::gaps::HiddenOutcome::Failed { seconds: secs, message };
            }
        }
    }

    let mut cmd = Command::new(program());
    cmd.arg("-d").arg(device)
       .arg("-B")   // batch mode: one WAV per track
       .arg("-w")   // force WAV output
       .arg("-e");  // report every read event on stderr, so the quality of the read can be judged
    if let Some(f) = paranoia.flag() {
        cmd.arg(f);
    }
    cmd.current_dir(output_dir);
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::piped());

    if debug { eprintln!("Running: {:?}", cmd); }

    let mut child = cmd.spawn()?;

    // Parse cdparanoia's stderr so we can report per-track progress.
    // Key line patterns:
    //   "outputting to track01.cdda.wav"  → just started ripping track 1
    //   "Ripping from sector N (track M"  → also contains track number
    if let Some(stderr) = child.stderr.take() {
        let reader = BufReader::new(stderr);
        let mut last_reported: usize = 0;

        for line in reader.lines().map_while(Result::ok) {
            // The per-event lines are collected, not echoed: there is one for every sector.
            if events.feed(&line) {
                continue;
            }
            if debug { eprintln!("[cdparanoia] {}", line); }

            // "outputting to track01.cdda.wav"
            let lower = line.to_lowercase();
            if lower.contains("outputting to track") {
                if let Some(track_num) = parse_track_number_from_output_line(&line) {
                    if track_num != last_reported {
                        last_reported = track_num;
                        if progress_json {
                            let pct = (track_num as f32 - 1.0) / track_count as f32 * 85.0;
                            emit_step(&format!("Ripping track {} of {}...", track_num, track_count));
                            emit_progress(pct);
                        } else {
                            eprintln!("  Ripping track {} of {}...", track_num, track_count);
                        }
                    }
                }
            }
        }
    }

    let status = child.wait()?;
    if !status.success() {
        return Err(Error::backend(format!(
            "cdparanoia failed (exit {:?}) — try running with --debug for details",
            status.code(),
        )));
    }

    if progress_json {
        emit_progress(85.0);
    } else {
        eprintln!("  Rip complete — encoding...");
    }

    // Collect the WAV files cdparanoia wrote.
    let mut tracks: Vec<(usize, String)> = Vec::new();
    for i in 1..=track_count {
        let path = format!("{}/track{:02}.cdda.wav", output_dir, i);
        if Path::new(&path).exists() {
            tracks.push((i, path));
        }
    }

    if tracks.is_empty() {
        return Err(Error::backend(
            "cdparanoia produced no output files — check disc and device",
        ));
    }
    if let Some(p) = hidden_path {
        tracks.insert(0, (0, p));
    }

    Ok(RipRead { tracks, events, hidden })
}

/// Read the audio before track 1 (cdparanoia's "track 0") into `path`.
fn rip_track_zero(device: &str, path: &str, paranoia: Paranoia, events: &mut super::readhealth::Collector, debug: bool) -> Result<(), String> {
    let mut cmd = Command::new(program());
    cmd.arg("-d").arg(device).arg("-w").arg("-e");
    if let Some(f) = paranoia.flag() {
        cmd.arg(f);
    }
    cmd.arg("0").arg(path).stdout(Stdio::null()).stderr(Stdio::piped());
    if debug {
        eprintln!("Running: {:?}", cmd);
    }
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let mut last_error = String::new();
    if let Some(stderr) = child.stderr.take() {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if events.feed(&line) {
                continue;
            }
            if debug {
                eprintln!("[cdparanoia] {}", line);
            }
            let t = line.trim();
            if !t.is_empty() && (t.to_lowercase().contains("error") || t.to_lowercase().contains("not exist") || t.to_lowercase().contains("pregap") || t.to_lowercase().contains("not found")) {
                last_error = t.to_string();
            }
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    if !status.success() || !std::path::Path::new(path).is_file() {
        return Err(if last_error.is_empty() { format!("cdparanoia exited with {:?}", status.code()) } else { last_error });
    }
    Ok(())
}

/// Parse "outputting to track01.cdda.wav" → Some(1)
fn parse_track_number_from_output_line(line: &str) -> Option<usize> {
    // Find "track" then parse the digits that follow.
    let lower = line.to_lowercase();
    let after = lower.split("track").nth(1)?;
    let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

pub(super) fn emit_progress(pct: f32) {
    println!("{{\"type\":\"progress\",\"pct\":{:.1}}}", pct);
}

pub(super) fn emit_step(msg: &str) {
    let escaped = msg.replace('\\', "\\\\").replace('"', "\\\"");
    println!("{{\"type\":\"step\",\"msg\":\"{}\"}}", escaped);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rip::readhealth::Status;
    use std::os::unix::fs::PermissionsExt;

    /// The tests point the ripper at stand-in programs through one environment variable, so they
    /// must not run at the same time.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A stand-in for cdparanoia that writes two tracks and reports what the real one does:
    /// a harmless first-read reset, plain jitter on track 1, and a scratch plus a skip on track 2.
    const FAKE: &str = r###"#!/bin/sh
echo "cdparanoia III release 10.2 (September 11, 2008)" >&2
echo "outputting to track01.cdda.wav" >&2
touch track01.cdda.wav
echo "scsi_read error: sector=5 length=27 retry=0" >&2
echo "                 Sense key: 6 ASC: 29 ASCQ: 0" >&2
echo "##: 12 [transport error] @ 5880" >&2
echo "##: 3 [correction] @ 5880" >&2
echo "##: 2 [jitter] @ 588000" >&2
echo "##: 2 [jitter] @ 600000" >&2
echo "outputting to track02.cdda.wav" >&2
touch track02.cdda.wav
echo "##: 4 [scratch] @ 1470000" >&2
echo "##: 5 [scratch repair] @ 1470000" >&2
echo "##: 3 [correction] @ 1470000" >&2
echo "##: 6 [skip] @ 1764000" >&2
echo "##: -1 [finished] @ 1999000" >&2
"###;

    #[test]
    fn the_rip_collects_what_cdparanoia_reports() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("rustydisc_fakeparanoia_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("cdparanoia");
        std::fs::write(&script, FAKE).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        // SAFETY: only this test spawns the ripper, and it sets the variable before doing so.
        unsafe { std::env::set_var("RUSTYDISC_CDPARANOIA", &script) };

        let out = dir.join("wav");
        let read = rip_all_tracks("/dev/null", out.to_str().unwrap(), 2, Paranoia::Full, None, false, false).unwrap();
        unsafe { std::env::remove_var("RUSTYDISC_CDPARANOIA") };

        assert_eq!(read.tracks.iter().map(|t| t.0).collect::<Vec<_>>(), vec![1, 2]);
        // Track 1 is sectors 0..1000 (588000 words = sector 500), track 2 is 1000..2000.
        let h = read.events.summarize(&[(1, 0, 1000), (2, 1000, 2000)]);
        assert_eq!(h.tracks[0].status, Status::Clean, "{:?}", h.tracks[0]);
        assert_eq!(h.tracks[0].jitter, 2);
        assert_eq!(h.harmless_resets, 1);
        assert_eq!(h.tracks[1].status, Status::Suspect);
        assert_eq!((h.tracks[1].scratches, h.tracks[1].corrections, h.tracks[1].skips), (1, 1, 1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A stand-in cdparanoia that serves "track 0" from a prepared file, and the rest like FAKE.
    fn fake_with_hidden(dir: &std::path::Path, hidden_wav: &std::path::Path) -> std::path::PathBuf {
        let script = dir.join("cdparanoia");
        let body = format!(
            "#!/bin/sh\nfor a in \"$@\"; do prev2=\"$prev\"; prev=\"$a\"; done\nif [ \"$prev2\" = \"0\" ]; then cp '{}' \"$prev\"; exit 0; fi\n{}",
            hidden_wav.display(),
            FAKE.trim_start_matches("#!/bin/sh\n")
        );
        std::fs::write(&script, body).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    fn write_wav(path: &std::path::Path, words: &[u32]) {
        let data = (words.len() * 4) as u32;
        let mut b: Vec<u8> = Vec::new();
        b.extend(b"RIFF");
        b.extend((36 + data).to_le_bytes());
        b.extend(b"WAVEfmt ");
        b.extend(16u32.to_le_bytes());
        b.extend([1, 0, 2, 0]);
        b.extend(44_100u32.to_le_bytes());
        b.extend((44_100u32 * 4).to_le_bytes());
        b.extend([4, 0, 16, 0]);
        b.extend(b"data");
        b.extend(data.to_le_bytes());
        for w in words {
            b.extend(w.to_le_bytes());
        }
        std::fs::write(path, b).unwrap();
    }

    #[test]
    fn hidden_audio_before_track_one_is_ripped_as_track_zero_unless_silent() {
        use crate::rip::gaps::HiddenOutcome;
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("rustydisc_hidden_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let music = dir.join("music.wav");
        let silence = dir.join("silence.wav");
        write_wav(&music, &(0..4000u32).map(|i| i.wrapping_mul(2654435761) & 0x3FFF_3FFF).collect::<Vec<_>>());
        write_wav(&silence, &vec![0u32; 4000]);

        // SAFETY: only these tests spawn the ripper, and they run one after another here.
        let script = fake_with_hidden(&dir, &music);
        unsafe { std::env::set_var("RUSTYDISC_CDPARANOIA", &script) };
        let out = dir.join("a");
        let read = rip_all_tracks("/dev/null", out.to_str().unwrap(), 2, Paranoia::Full, Some(7500), false, false).unwrap();
        assert_eq!(read.tracks.iter().map(|t| t.0).collect::<Vec<_>>(), vec![0, 1, 2], "the hidden track comes first, numbered 0");
        assert_eq!(read.hidden, HiddenOutcome::Kept { seconds: 100.0 });
        assert!(std::path::Path::new(&read.tracks[0].1).is_file());

        let script = fake_with_hidden(&dir, &silence);
        unsafe { std::env::set_var("RUSTYDISC_CDPARANOIA", &script) };
        let out = dir.join("b");
        let read = rip_all_tracks("/dev/null", out.to_str().unwrap(), 2, Paranoia::Full, Some(7500), false, false).unwrap();
        unsafe { std::env::remove_var("RUSTYDISC_CDPARANOIA") };
        assert_eq!(read.tracks.iter().map(|t| t.0).collect::<Vec<_>>(), vec![1, 2], "silence is not kept");
        assert_eq!(read.hidden, HiddenOutcome::Silent { seconds: 100.0 });
        assert!(!out.join("track00.cdda.wav").exists(), "and its file is removed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn paranoia_levels_map_to_cdparanoia_flags() {
        assert_eq!(Paranoia::parse("full").unwrap().flag(), None);
        assert_eq!(Paranoia::parse("fast").unwrap().flag(), Some("-Y"));
        assert_eq!(Paranoia::parse("off").unwrap().flag(), Some("-Z"));
        assert!(Paranoia::parse("extreme").is_err());
    }
}
