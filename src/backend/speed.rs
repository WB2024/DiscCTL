//! Write speed.
//!
//! Speeds are given as the "x" multiple people know (8x CD, 4x DVD). The tools want them in
//! their own way: cdrdao takes the CD multiple, and `xorriso -as cdrecord` takes a number with a
//! unit (`8c` for CD, `4d` for DVD), so the choice can't be misread as the wrong media's speed.
//! "Auto" sends nothing and lets the drive pick, which is what happened before this existed.

use std::{
    process::Command,
    sync::Mutex,
};

use crate::error::Error;

/// The largest multiple accepted for any media (a sanity limit; the drive's own list is the real one).
pub const MAX_X: u32 = 100;

/// What a CD / DVD / BD "1x" is in kB/s.
pub const CD_KBPS: f64 = 176.4;
pub const DVD_KBPS: f64 = 1385.0;

static CHOSEN: Mutex<Option<u32>> = Mutex::new(None);

/// Remember the speed for this run (the backends read it when they build their commands).
pub fn set(x: Option<u32>) {
    *CHOSEN.lock().unwrap() = x;
}

pub fn get() -> Option<u32> {
    *CHOSEN.lock().unwrap()
}

/// `auto` (or nothing) means the drive decides; otherwise a whole number of x.
pub fn parse(s: &str) -> Result<Option<u32>, String> {
    let t = s.trim().to_lowercase();
    let t = t.trim_end_matches('x');
    if t.is_empty() || t == "auto" || t == "max" || t == "0" {
        return Ok(None);
    }
    t.parse::<u32>()
        .ok()
        .filter(|n| (1..=MAX_X).contains(n))
        .map(Some)
        .ok_or_else(|| format!("'{s}' is not a write speed (use auto, or a number like 8 for 8x, up to {MAX_X})"))
}

/// The argument for `xorriso -as cdrecord`.
pub fn xorriso_arg(x: u32, dvd: bool) -> String {
    format!("speed={}{}", x, if dvd { 'd' } else { 'c' })
}

/// A speed the drive offers for the disc that is in it.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct DriveSpeed {
    pub kbps: u32,
    /// The "x" multiple, to one decimal.
    pub x: f64,
    /// Which media the multiple is for: CD, DVD or BD.
    pub media: String,
}

/// Read the write speeds out of `xorriso -list_speeds`, e.g. `Write speed  :   4234k , 24.0xC`.
pub fn parse_list_speeds(text: &str) -> Vec<DriveSpeed> {
    let mut out: Vec<DriveSpeed> = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.trim().strip_prefix("Write speed") else { continue };
        // "Write speed  :", "Write speed L:" and "Write speed H:" (lowest / highest) all count.
        let Some((_, value)) = rest.split_once(':') else { continue };
        let Some((k, x)) = value.split_once(',') else { continue };
        let Some(kbps) = k.trim().strip_suffix('k').and_then(|n| n.trim().parse::<u32>().ok()) else { continue };
        let x = x.trim();
        let Some(media) = x.chars().last().filter(|c| matches!(c, 'C' | 'D' | 'B')) else { continue };
        let Ok(factor) = x[..x.len() - 1].trim_end_matches('x').parse::<f64>() else { continue };
        if out.iter().any(|s| s.kbps == kbps) {
            continue;
        }
        out.push(DriveSpeed { kbps, x: (factor * 10.0).round() / 10.0, media: match media { 'C' => "CD", 'D' => "DVD", _ => "BD" }.into() });
    }
    out.sort_by_key(|s| s.kbps);
    out
}

/// The write speeds the drive offers for the disc in it.
pub fn query(device: &str) -> Result<Vec<DriveSpeed>, Error> {
    let out = Command::new("xorriso")
        .args(["-outdev", device, "-list_speeds"])
        .output()
        .map_err(|e| Error::device(format!("Can't ask the drive for its speeds (is xorriso installed?): {e}")))?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    Ok(parse_list_speeds(&text))
}

/// Refuse a speed above what the drive can do for this disc. A speed between two the drive
/// offers is fine: the drive uses the nearest, as it treats the number as an upper limit.
pub fn check_supported(x: u32, speeds: &[DriveSpeed]) -> Result<(), Error> {
    let Some(max) = speeds.iter().map(|s| s.x).fold(None, |m: Option<f64>, v| Some(m.map_or(v, |m| m.max(v)))) else {
        return Ok(()); // the drive didn't say; let the burn tool decide
    };
    if (x as f64) > max + 0.5 {
        let list: Vec<String> = speeds.iter().map(|s| format!("{}x", s.x)).collect();
        return Err(Error::validation(format!(
            "This drive can write this disc at up to {max}x, not {x}x (it offers {}). Pick a lower speed or Auto.",
            list.join(", ")
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
Media current: CD-R\n\
Write speed  :   1411k , 8.0xC\n\
Write speed  :   2822k , 16.0xC\n\
Write speed  :   4234k , 24.0xC\n\
Write speed L:   1411k , 8.0xC\n\
Write speed H:   4234k , 24.0xC\n\
Read speed   :   4234k , 24.0xC\n";

    #[test]
    fn parses_values() {
        assert_eq!(parse("auto"), Ok(None));
        assert_eq!(parse(""), Ok(None));
        assert_eq!(parse("8"), Ok(Some(8)));
        assert_eq!(parse("16x"), Ok(Some(16)));
        assert!(parse("0.5").is_err());
        assert!(parse("101").is_err());
        assert!(parse("fast").is_err());
    }

    #[test]
    fn builds_tool_arguments_with_the_right_unit() {
        assert_eq!(xorriso_arg(8, false), "speed=8c");
        assert_eq!(xorriso_arg(4, true), "speed=4d");
    }

    #[test]
    fn reads_the_drives_speed_list() {
        let s = parse_list_speeds(SAMPLE);
        assert_eq!(s.iter().map(|x| x.x).collect::<Vec<_>>(), vec![8.0, 16.0, 24.0]);
        assert!(s.iter().all(|x| x.media == "CD"));
        assert_eq!(s[0].kbps, 1411);
    }

    #[test]
    fn a_dvd_list_is_recognised() {
        let s = parse_list_speeds("Write speed  :   5540k , 4.0xD\nWrite speed  :  11080k , 8.0xD\n");
        assert_eq!((s[1].x, s[1].media.as_str()), (8.0, "DVD"));
    }

    #[test]
    fn too_fast_is_refused_and_in_between_is_fine() {
        let s = parse_list_speeds(SAMPLE);
        assert!(check_supported(24, &s).is_ok());
        assert!(check_supported(10, &s).is_ok());
        let err = check_supported(48, &s).unwrap_err().to_string();
        assert!(err.contains("up to 24x") && err.contains("8x, 16x, 24x"), "{err}");
        assert!(check_supported(48, &[]).is_ok(), "no list from the drive means no opinion");
    }

    #[test]
    fn the_closed_pressed_disc_in_the_real_drive_parses() {
        // What the drive reported with a pressed CD in it (no choice of speeds).
        let s = parse_list_speeds("Write speed  :   4234k , 24.0xC\nWrite speed L:   4234k , 24.0xC\nWrite speed H:   4234k , 24.0xC\n");
        assert_eq!(s.len(), 1);
    }
}
