use std::process::Command;
use serde::{Deserialize, Serialize};
use sha1::Digest as _;
use base64::Engine as _;
use crate::error::Error;

// ── Public types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscInfo {
    pub format: DiscFormat,
    pub sessions: Vec<SessionInfo>,
    pub is_writable: bool,
    pub device: String,
    /// MusicBrainz DiscID — SHA-1 hash of the audio TOC, base64 with MB substitutions.
    /// None for data-only discs or if the TOC could not be parsed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discid: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum DiscFormat {
    RedBook,
    DataCD,
    BlueBook,
    Unknown,
}

impl std::fmt::Display for DiscFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiscFormat::RedBook  => write!(f, "Red Book Audio CD"),
            DiscFormat::DataCD   => write!(f, "Data CD"),
            DiscFormat::BlueBook => write!(f, "Blue Book (Enhanced CD / CD Extra)"),
            DiscFormat::Unknown  => write!(f, "Unknown"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub index: usize,
    pub kind: SessionKind,
    pub tracks: Vec<TrackInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cd_text: Option<CdTextBlock>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SessionKind {
    Audio,
    Data {
        #[serde(skip_serializing_if = "Option::is_none")]
        volume_label: Option<String>,
        size_mb: f64,
        filesystem: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackInfo {
    pub number: usize,
    pub kind: TrackKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_secs: Option<f64>,
    pub lba_start: u32,
    pub lba_end: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cd_text: Option<CdTextBlock>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum TrackKind {
    Audio,
    Data,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CdTextBlock {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub songwriter: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub composer: Option<String>,
}

impl CdTextBlock {
    fn empty() -> Self {
        CdTextBlock { title: None, artist: None, songwriter: None, composer: None }
    }
    fn is_empty(&self) -> bool {
        self.title.is_none() && self.artist.is_none()
            && self.songwriter.is_none() && self.composer.is_none()
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

pub fn analyze(device: &str) -> Result<DiscInfo, Error> {
    let is_writable = detect_writable(device);
    let (raw_tracks, disc_cd_type) = read_toc_cdrecord(device)?;

    if raw_tracks.is_empty() {
        return Ok(DiscInfo {
            format: DiscFormat::Unknown,
            sessions: vec![],
            is_writable,
            device: device.to_string(),
            discid: None,
        });
    }

    let has_audio = raw_tracks.iter().any(|t| t.kind == TrackKind::Audio);
    let has_data  = raw_tracks.iter().any(|t| t.kind == TrackKind::Data);

    let format = if disc_cd_type == "CD_ROM" && !has_audio {
        DiscFormat::DataCD
    } else if has_audio && has_data {
        DiscFormat::BlueBook
    } else if has_audio {
        DiscFormat::RedBook
    } else {
        DiscFormat::DataCD
    };

    // Compute DiscID from audio tracks only (data tracks are excluded — this is
    // what libdiscid and every other tool does for CD Extra / Blue Book).
    let discid = compute_discid(&raw_tracks);

    let sessions = build_sessions(&raw_tracks, &format, device);
    let sessions = overlay_cdtext(sessions, device);

    Ok(DiscInfo { format, sessions, is_writable, device: device.to_string(), discid })
}

// ── DiscID (MusicBrainz) ─────────────────────────────────────────────────────

/// Compute the MusicBrainz DiscID from the raw TOC.
///
/// Algorithm: SHA-1 of a 804-character string built from:
///   first_track (2 hex) + last_track (2 hex) + leadout (8 hex) +
///   track1_offset (8 hex) + ... + track99_offset (8 hex, zero-padded)
///
/// All offsets are LBA + 150 (absolute sectors from the physical disc start).
/// Only audio tracks are included; data tracks are excluded per the MB spec.
///
/// Reference: https://musicbrainz.org/doc/Disc_ID_Calculation
fn compute_discid(raw: &[RawTrack]) -> Option<String> {
    let audio: Vec<&RawTrack> = raw.iter().filter(|t| t.kind == TrackKind::Audio).collect();
    if audio.is_empty() {
        return None;
    }

    let first: u8 = 1;
    let last: u8  = audio.len() as u8;
    // Leadout = lba_end of the last audio track (next track / session start).
    let leadout: u32 = audio.last()?.lba_end + 150;

    let mut s = format!("{:02X}{:02X}{:08X}", first, last, leadout);

    for i in 0..99usize {
        let offset = if i < audio.len() {
            audio[i].lba_start + 150
        } else {
            0
        };
        s.push_str(&format!("{:08X}", offset));
    }

    let hash = sha1::Sha1::digest(s.as_bytes());
    let b64  = base64::engine::general_purpose::STANDARD.encode(hash);
    Some(b64.replace('+', ".").replace('/', "_").replace('=', "-"))
}

// ── TOC reading via cdrecord ──────────────────────────────────────────────────

struct RawTrack {
    number: usize,
    kind: TrackKind,
    lba_start: u32,
    lba_end: u32,   // filled in from the next track's lba_start
}

fn read_toc_cdrecord(device: &str) -> Result<(Vec<RawTrack>, String), Error> {
    let output = Command::new("cdrecord")
        .arg(format!("dev={}", device))
        .arg("-v")
        .arg("-toc")
        .output()?;

    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let mut raw: Vec<(usize, TrackKind, u32)> = Vec::new();  // (number, kind, lba_start)
    let mut leadout_lba: Option<u32> = None;
    let mut disc_cd_type = String::from("CD_ROM");

    for line in text.lines() {
        let lower = line.to_lowercase();

        if lower.contains("toc type") {
            if lower.contains("cd-da") || lower.contains("audio") {
                disc_cd_type = "CD_DA".to_string();
            }
        }

        // "track:  1 lba:      0 (0x000000) 00:02:00.000 adr:  1 ctrl:  0 mode:   -1"
        // "track:   1 lba:         0 (         0) 00:02:00.000 adr:  1 ctrl:  4 mode:   1"
        if lower.trim_start().starts_with("track:") || lower.trim_start().starts_with("track ") {
            if let Some((num, kind, lba)) = parse_track_line(line) {
                raw.push((num, kind, lba));
            }
            continue;
        }

        // Leadout line
        if lower.trim_start().starts_with("lout:") || lower.trim_start().starts_with("leadout") {
            leadout_lba = parse_lba_from_line(line);
        }
    }

    // Fill in lba_end for each track from next track's lba_start / leadout.
    let mut tracks: Vec<RawTrack> = Vec::with_capacity(raw.len());
    for i in 0..raw.len() {
        let (number, ref kind, lba_start) = raw[i];
        let lba_end = if i + 1 < raw.len() {
            raw[i + 1].2
        } else {
            leadout_lba.unwrap_or(lba_start + 1)
        };
        tracks.push(RawTrack { number, kind: kind.clone(), lba_start, lba_end });
    }

    Ok((tracks, disc_cd_type))
}

fn parse_track_line(line: &str) -> Option<(usize, TrackKind, u32)> {
    // Handles: "track:  1 lba:      0 ..." and "track   1: 00 LBA:   0 ..."
    let lower = line.to_lowercase();

    // Extract track number — first integer after "track"
    let after_track = lower.split("track").nth(1)?;
    let num_str = after_track.split_whitespace().next()?.trim_end_matches(':');
    let number: usize = num_str.parse().ok()?;

    // Extract LBA — first integer after "lba"
    let after_lba = lower.split("lba:").nth(1)
        .or_else(|| lower.split("lba ").nth(1))?;
    let lba_str = after_lba.split_whitespace().next()?.trim_start_matches('(');
    let lba: u32 = lba_str.trim_end_matches(')').parse().ok()?;

    // Determine track type from ctrl field: ctrl: 0 = audio, ctrl: 4 = data
    let kind = if let Some(after_ctrl) = lower.split("ctrl:").nth(1)
        .or_else(|| lower.split("ctrl ").nth(1)) {
        let ctrl_str = after_ctrl.split_whitespace().next().unwrap_or("0");
        let ctrl: u8 = ctrl_str.parse().unwrap_or(0);
        if ctrl & 4 != 0 { TrackKind::Data } else { TrackKind::Audio }
    } else {
        TrackKind::Audio
    };

    Some((number, kind, lba))
}

fn parse_lba_from_line(line: &str) -> Option<u32> {
    let lower = line.to_lowercase();
    let after_lba = lower.split("lba:").nth(1)
        .or_else(|| lower.split("lba ").nth(1))?;
    let lba_str = after_lba.split_whitespace().next()?.trim_start_matches('(');
    lba_str.trim_end_matches(')').parse().ok()
}

// ── Session builder ───────────────────────────────────────────────────────────

fn build_sessions(raw: &[RawTrack], format: &DiscFormat, device: &str) -> Vec<SessionInfo> {
    if raw.is_empty() {
        return vec![];
    }

    match format {
        DiscFormat::BlueBook => {
            // Split at the first data track — everything before is Session 1 (audio),
            // everything from the data track onward is Session 2 (data).
            let split = raw.iter().position(|t| t.kind == TrackKind::Data)
                .unwrap_or(raw.len());

            let audio_tracks = &raw[..split];
            let data_tracks  = &raw[split..];

            let mut sessions = Vec::new();

            if !audio_tracks.is_empty() {
                sessions.push(SessionInfo {
                    index: 1,
                    kind: SessionKind::Audio,
                    tracks: audio_tracks.iter().map(raw_to_track_info).collect(),
                    cd_text: None,
                });
            }

            if !data_tracks.is_empty() {
                let (label, size_mb) = query_data_session(device);
                sessions.push(SessionInfo {
                    index: 2,
                    kind: SessionKind::Data {
                        volume_label: label,
                        size_mb,
                        filesystem: "ISO9660".to_string(),
                    },
                    tracks: data_tracks.iter().map(raw_to_track_info).collect(),
                    cd_text: None,
                });
            }

            sessions
        }
        DiscFormat::DataCD => {
            let (label, size_mb) = query_data_session(device);
            vec![SessionInfo {
                index: 1,
                kind: SessionKind::Data {
                    volume_label: label,
                    size_mb,
                    filesystem: "ISO9660".to_string(),
                },
                tracks: raw.iter().map(raw_to_track_info).collect(),
                cd_text: None,
            }]
        }
        _ => {
            vec![SessionInfo {
                index: 1,
                kind: SessionKind::Audio,
                tracks: raw.iter().map(raw_to_track_info).collect(),
                cd_text: None,
            }]
        }
    }
}

fn raw_to_track_info(t: &RawTrack) -> TrackInfo {
    let duration_secs = if t.kind == TrackKind::Audio && t.lba_end > t.lba_start {
        // CD sectors = 75 frames per second; subtract 150 sector pregap for track 1
        Some((t.lba_end - t.lba_start) as f64 / 75.0)
    } else {
        None
    };
    TrackInfo {
        number: t.number,
        kind: t.kind.clone(),
        duration_secs,
        lba_start: t.lba_start,
        lba_end: t.lba_end,
        cd_text: None,
    }
}

// ── Data session info via isoinfo ─────────────────────────────────────────────

fn query_data_session(device: &str) -> (Option<String>, f64) {
    let output = Command::new("isoinfo")
        .arg("-d")
        .arg("-i")
        .arg(device)
        .output();

    let Ok(out) = output else { return (None, 0.0) };
    let text = String::from_utf8_lossy(&out.stdout).to_string()
        + &String::from_utf8_lossy(&out.stderr);

    let mut label: Option<String> = None;
    let mut size_blocks: u64 = 0;
    let mut block_size: u64 = 2048;

    for line in text.lines() {
        if line.starts_with("Volume id:") {
            let v = line["Volume id:".len()..].trim().to_string();
            if !v.is_empty() { label = Some(v); }
        } else if line.starts_with("Volume size is:") {
            size_blocks = line["Volume size is:".len()..].trim().parse().unwrap_or(0);
        } else if line.starts_with("Logical block size is:") {
            block_size = line["Logical block size is:".len()..].trim().parse().unwrap_or(2048);
        }
    }

    let size_mb = (size_blocks * block_size) as f64 / 1_048_576.0;
    (label, size_mb)
}

// ── CD-Text overlay via cdrdao read-toc ───────────────────────────────────────

fn overlay_cdtext(mut sessions: Vec<SessionInfo>, device: &str) -> Vec<SessionInfo> {
    let toc_path = format!("/tmp/rustydisc_toc_{}.toc", std::process::id());

    let ok = Command::new("cdrdao")
        .arg("read-toc")
        .arg("--device").arg(device)
        .arg("--quiet")
        .arg(&toc_path)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if !ok {
        return sessions;
    }

    let toc_text = std::fs::read_to_string(&toc_path).unwrap_or_default();
    let _ = std::fs::remove_file(&toc_path);

    let (disc_cd_text, track_cd_texts) = parse_cdrdao_toc_cdtext(&toc_text);

    for session in &mut sessions {
        if let SessionKind::Audio = session.kind {
            session.cd_text = disc_cd_text.clone().filter(|c| !c.is_empty());
            for track in &mut session.tracks {
                if let Some(ct) = track_cd_texts.get(track.number.saturating_sub(1)) {
                    if !ct.is_empty() {
                        track.cd_text = Some(ct.clone());
                    }
                }
            }
        }
    }

    sessions
}

fn parse_cdrdao_toc_cdtext(toc: &str) -> (Option<CdTextBlock>, Vec<CdTextBlock>) {
    let mut disc_ct = CdTextBlock::empty();
    let mut track_cts: Vec<CdTextBlock> = Vec::new();
    let mut current_ct = CdTextBlock::empty();

    // State
    let mut in_cdtext = false;
    let mut in_language = false;
    let mut in_track = false;
    let mut in_track_cdtext = false;

    for line in toc.lines() {
        let trimmed = line.trim();

        if trimmed == "CD_TEXT {" {
            if in_track { in_track_cdtext = true; }
            in_cdtext = true;
            current_ct = CdTextBlock::empty();
            continue;
        }
        if trimmed.starts_with("LANGUAGE 0 {") || trimmed.starts_with("LANGUAGE 0{") {
            in_language = true;
            continue;
        }
        if trimmed == "}" {
            if in_language {
                in_language = false;
                if in_track_cdtext {
                    track_cts.push(current_ct.clone());
                    current_ct = CdTextBlock::empty();
                    in_track_cdtext = false;
                } else if in_cdtext {
                    if !in_track {
                        disc_ct = current_ct.clone();
                    }
                    current_ct = CdTextBlock::empty();
                }
                in_cdtext = false;
            }
            continue;
        }

        if trimmed.starts_with("TRACK ") {
            in_track = true;
            in_track_cdtext = false;
            continue;
        }

        if in_language {
            if let Some(v) = extract_cdtext_field(trimmed, "TITLE") {
                current_ct.title = Some(v);
            } else if let Some(v) = extract_cdtext_field(trimmed, "PERFORMER") {
                current_ct.artist = Some(v);
            } else if let Some(v) = extract_cdtext_field(trimmed, "SONGWRITER") {
                current_ct.songwriter = Some(v);
            } else if let Some(v) = extract_cdtext_field(trimmed, "COMPOSER") {
                current_ct.composer = Some(v);
            }
        }
    }

    let disc = if disc_ct.is_empty() { None } else { Some(disc_ct) };
    (disc, track_cts)
}

fn extract_cdtext_field(line: &str, field: &str) -> Option<String> {
    let prefix = format!("{} \"", field);
    if line.starts_with(&prefix) && line.ends_with('"') {
        let inner = &line[prefix.len()..line.len() - 1];
        if inner.is_empty() { None } else { Some(inner.to_string()) }
    } else {
        None
    }
}

// ── Writable detection ────────────────────────────────────────────────────────

fn detect_writable(device: &str) -> bool {
    let out = Command::new("cdrecord")
        .arg(format!("dev={}", device))
        .arg("-atip")
        .output();
    let Ok(o) = out else { return false };
    let text = String::from_utf8_lossy(&o.stdout).to_string()
        + &String::from_utf8_lossy(&o.stderr);
    text.to_lowercase().contains("atip start of lead in")
}

// ── Human-readable display ────────────────────────────────────────────────────

pub fn display(info: &DiscInfo) {
    println!("Disc Type:  {}", info.format);
    println!("Sessions:   {}", info.sessions.len());
    if info.is_writable { println!("Media:      Writable (CD-R / CD-RW)"); }
    if let Some(ref id) = info.discid {
        println!("DiscID:     {}", id);
    }
    println!();

    for session in &info.sessions {
        match &session.kind {
            SessionKind::Audio => {
                let total_secs: f64 = session.tracks.iter()
                    .filter_map(|t| t.duration_secs)
                    .sum();
                let (m, s) = (total_secs as u64 / 60, total_secs as u64 % 60);
                print!("Session {} — Audio  ({} track{}, {}:{:02})",
                    session.index, session.tracks.len(),
                    if session.tracks.len() == 1 { "" } else { "s" }, m, s);
                if let Some(ct) = &session.cd_text {
                    if let Some(t) = &ct.title  { print!("  «{}»", t); }
                    if let Some(a) = &ct.artist { print!(" — {}", a); }
                }
                println!();
                for track in &session.tracks {
                    let dur = track.duration_secs.unwrap_or(0.0);
                    let (tm, ts) = (dur as u64 / 60, dur as u64 % 60);
                    print!("  Track {:2}  {:2}:{:02}", track.number, tm, ts);
                    if let Some(ct) = &track.cd_text {
                        if let Some(t) = &ct.title  { print!("  \"{}\"", t); }
                        if let Some(a) = &ct.artist { print!(" — {}", a); }
                    }
                    println!();
                }
            }
            SessionKind::Data { volume_label, size_mb, filesystem } => {
                print!("Session {} — Data   ({}, {:.1} MB)",
                    session.index, filesystem, size_mb);
                if let Some(label) = volume_label {
                    print!("  Volume: {}", label);
                }
                println!();
            }
        }
        println!();
    }
}
