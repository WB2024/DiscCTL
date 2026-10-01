//! Loudness measurement with ffmpeg's EBU R128 meter.

use std::{path::Path, process::Command};

use serde::Serialize;

#[derive(Debug, Clone, Serialize, Default)]
pub struct Loudness {
    /// Integrated loudness in LUFS.
    pub lufs: Option<f64>,
    /// Loudness range in LU: how much the loudness moves about.
    pub lra: Option<f64>,
    /// True peak in dBFS. Above 0 means it clips.
    pub true_peak_db: Option<f64>,
}

impl Loudness {
    /// ReplayGain 2.0 gain (dB) that brings this to the -18 LUFS reference.
    pub fn gain_db(&self) -> Option<f64> {
        self.lufs.map(|l| -18.0 - l)
    }
    /// The true peak as a linear sample value (1.0 = full scale).
    pub fn peak_linear(&self) -> Option<f64> {
        self.true_peak_db.map(|db| 10f64.powf(db / 20.0))
    }
}

pub(crate) fn parse_ebur128(stderr: &str) -> Loudness {
    // The last "Summary:" block holds the whole-file numbers.
    let tail = stderr.rsplit("Summary:").next().unwrap_or(stderr);
    let grab = |key: &str| -> Option<f64> {
        tail.lines().find_map(|l| {
            let l = l.trim();
            l.strip_prefix(key).and_then(|r| r.split_whitespace().next()).and_then(|n| n.parse::<f64>().ok())
        })
    };
    Loudness { lufs: grab("I:"), lra: grab("LRA:"), true_peak_db: grab("Peak:") }
}

fn run_ebur128(inputs: &[&Path]) -> Result<Loudness, String> {
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-nostats", "-nostdin", "-hide_banner"]);
    for p in inputs {
        cmd.arg("-i").arg(p);
    }
    if inputs.len() == 1 {
        cmd.args(["-af", "ebur128=peak=true"]);
    } else {
        let labels: String = (0..inputs.len()).map(|i| format!("[{i}:a]")).collect();
        cmd.args(["-filter_complex", &format!("{labels}concat=n={}:v=0:a=1,ebur128=peak=true", inputs.len())]);
    }
    let out = cmd.args(["-f", "null", "-"]).output().map_err(|e| e.to_string())?;
    let l = parse_ebur128(&String::from_utf8_lossy(&out.stderr));
    if l.lufs.is_none() {
        return Err("The loudness couldn't be measured (is the file silent or unreadable?)".into());
    }
    Ok(l)
}

pub fn loudness(path: &Path) -> Result<Loudness, String> {
    run_ebur128(&[path])
}

/// The loudness of the files played back to back, for the album's ReplayGain.
pub fn album_loudness(paths: &[&Path]) -> Result<Loudness, String> {
    run_ebur128(paths)
}

