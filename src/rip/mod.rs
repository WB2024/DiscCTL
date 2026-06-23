pub mod data;
pub mod encoder;
pub mod engine;
pub mod metadata;

use std::path::Path;
use crate::{analyzer::{self, DiscFormat, SessionKind, TrackKind}, error::Error};
use encoder::{AudioFormat, TrackTags, track_filename};
use engine::{emit_progress, emit_step};

/// How to lay out the ripped output.
#[derive(Debug, Clone, PartialEq)]
pub enum RipLayout {
    /// `audio/`, `data/`, `metadata/` subdirectories (default)
    Exploded,
    /// Full archival: exploded + `discgraph.json` + `checksums.json`
    Archive,
}

pub struct RipOptions {
    pub device: String,
    pub output_dir: String,
    pub format: AudioFormat,
    pub layout: RipLayout,
    pub debug: bool,
    pub progress_json: bool,
}

/// Rip a disc according to the provided options.
/// Handles Red Book, Data CD, and Blue Book automatically based on disc contents.
pub fn rip(opts: &RipOptions) -> Result<(), Error> {
    // Step 1: analyze disc
    if opts.progress_json { emit_step("Analyzing disc..."); emit_progress(0.0); }
    let info = analyzer::analyze(&opts.device)?;

    if opts.progress_json {
        emit_step(&format!("Detected: {}", info.format));
    } else {
        eprintln!("Detected: {}", info.format);
        eprintln!("Sessions: {}", info.sessions.len());
    }

    std::fs::create_dir_all(&opts.output_dir)?;

    match info.format {
        DiscFormat::RedBook => rip_redbook(&info, opts),
        DiscFormat::DataCD  => rip_datacd(&info, opts),
        DiscFormat::BlueBook => rip_bluebook(&info, opts),
        DiscFormat::Unknown  => Err(Error::validation(
            "Could not determine disc format. Insert a disc and try again.",
        )),
    }
}

// ── Red Book ──────────────────────────────────────────────────────────────────

fn rip_redbook(
    info: &analyzer::DiscInfo,
    opts: &RipOptions,
) -> Result<(), Error> {
    let Some(session) = info.sessions.first() else {
        return Err(Error::validation("No sessions found on disc"));
    };

    let audio_dir = if matches!(opts.layout, RipLayout::Exploded | RipLayout::Archive) {
        format!("{}/audio", opts.output_dir)
    } else {
        opts.output_dir.clone()
    };
    std::fs::create_dir_all(&audio_dir)?;

    let track_count = session.tracks.len();
    let disc_title  = session.cd_text.as_ref().and_then(|c| c.title.as_deref());
    let disc_artist = session.cd_text.as_ref().and_then(|c| c.artist.as_deref());

    // Rip all tracks to a temp WAV dir using cdparanoia.
    let wav_dir = format!("/tmp/rustydisc_rip_{}", std::process::id());
    let wav_tracks = engine::rip_all_tracks(
        &opts.device, &wav_dir, track_count,
        opts.debug, opts.progress_json,
    )?;

    // Encode each WAV to the target format.
    let ext = opts.format.extension();
    let total = wav_tracks.len();

    for (i, (track_num, wav_path)) in wav_tracks.iter().enumerate() {
        let track_info = session.tracks.iter().find(|t| t.number == *track_num);

        let track_title = track_info
            .and_then(|t| t.cd_text.as_ref())
            .and_then(|c| c.title.as_deref());
        let track_artist = track_info
            .and_then(|t| t.cd_text.as_ref())
            .and_then(|c| c.artist.as_deref())
            .or(disc_artist);

        let filename = track_filename(*track_num, total, track_title, ext);
        let out_path = format!("{}/{}", audio_dir, filename);

        if opts.progress_json {
            emit_step(&format!("Encoding track {} of {} — {}", i + 1, total, filename));
            // Progress: rip took 0-5%, encoding is 5-95% spread across tracks
            let pct = 5.0 + (i as f32 / total as f32) * 90.0;
            emit_progress(pct);
        } else {
            eprintln!("Encoding {} → {}", track_num, filename);
        }

        let tags = TrackTags {
            title:        track_title.map(str::to_string),
            artist:       track_artist.map(str::to_string),
            album:        disc_title.map(str::to_string),
            album_artist: disc_artist.map(str::to_string),
            track_number: Some(*track_num),
            track_total:  Some(total),
            songwriter:   track_info.and_then(|t| t.cd_text.as_ref()).and_then(|c| c.songwriter.as_deref()).map(str::to_string),
            composer:     track_info.and_then(|t| t.cd_text.as_ref()).and_then(|c| c.composer.as_deref()).map(str::to_string),
        };

        encoder::encode(wav_path, &out_path, &opts.format, &tags, opts.debug)?;
    }

    // Clean up temp WAV files.
    let _ = std::fs::remove_dir_all(&wav_dir);

    // Metadata dir
    let meta_dir = format!("{}/metadata", opts.output_dir);
    std::fs::create_dir_all(&meta_dir)?;

    write_disc_json(info, opts)?;
    write_cdtext_json(info, opts)?;

    if matches!(opts.layout, RipLayout::Archive) {
        if opts.progress_json { emit_step("Generating checksums..."); emit_progress(96.0); }
        let manifest = metadata::generate_checksums(&opts.output_dir)?;
        metadata::write_checksums(&manifest, &meta_dir)?;
    }

    if opts.progress_json { emit_progress(100.0); }
    Ok(())
}

// ── Data CD ───────────────────────────────────────────────────────────────────

fn rip_datacd(
    _info: &analyzer::DiscInfo,
    opts: &RipOptions,
) -> Result<(), Error> {
    let data_dir = if matches!(opts.layout, RipLayout::Exploded | RipLayout::Archive) {
        format!("{}/data", opts.output_dir)
    } else {
        opts.output_dir.clone()
    };

    if opts.progress_json { emit_step("Extracting data session..."); emit_progress(5.0); }

    data::extract_data_session(&opts.device, &data_dir, false, opts.debug)?;

    if opts.progress_json { emit_progress(90.0); }

    let meta_dir = format!("{}/metadata", opts.output_dir);
    std::fs::create_dir_all(&meta_dir)?;
    write_disc_json(_info, opts)?;

    if matches!(opts.layout, RipLayout::Archive) {
        if opts.progress_json { emit_step("Generating checksums..."); emit_progress(95.0); }
        let manifest = metadata::generate_checksums(&opts.output_dir)?;
        metadata::write_checksums(&manifest, &meta_dir)?;
    }

    if opts.progress_json { emit_progress(100.0); }
    Ok(())
}

// ── Blue Book ─────────────────────────────────────────────────────────────────

fn rip_bluebook(
    info: &analyzer::DiscInfo,
    opts: &RipOptions,
) -> Result<(), Error> {
    let audio_dir = format!("{}/audio", opts.output_dir);
    let data_dir  = format!("{}/data",  opts.output_dir);
    let meta_dir  = format!("{}/metadata", opts.output_dir);
    std::fs::create_dir_all(&audio_dir)?;
    std::fs::create_dir_all(&data_dir)?;
    std::fs::create_dir_all(&meta_dir)?;

    // Rip audio session first (Session 1).
    let audio_session = info.sessions.iter()
        .find(|s| matches!(s.kind, SessionKind::Audio));

    if let Some(session) = audio_session {
        let track_count = session.tracks.iter()
            .filter(|t| t.kind == TrackKind::Audio)
            .count();

        let disc_title  = session.cd_text.as_ref().and_then(|c| c.title.as_deref());
        let disc_artist = session.cd_text.as_ref().and_then(|c| c.artist.as_deref());

        let wav_dir = format!("/tmp/rustydisc_rip_{}", std::process::id());
        let wav_tracks = engine::rip_all_tracks(
            &opts.device, &wav_dir, track_count,
            opts.debug, opts.progress_json,
        )?;

        let ext = opts.format.extension();
        let total = wav_tracks.len();

        for (i, (track_num, wav_path)) in wav_tracks.iter().enumerate() {
            let track_info = session.tracks.iter().find(|t| t.number == *track_num);
            let track_title = track_info.and_then(|t| t.cd_text.as_ref())
                .and_then(|c| c.title.as_deref());
            let track_artist = track_info.and_then(|t| t.cd_text.as_ref())
                .and_then(|c| c.artist.as_deref()).or(disc_artist);

            let filename = track_filename(*track_num, total, track_title, ext);
            let out_path = format!("{}/{}", audio_dir, filename);

            if opts.progress_json {
                // Audio rip/encode = 0–60%, data extraction = 60–90%
                let pct = 5.0 + (i as f32 / total as f32) * 55.0;
                emit_step(&format!("Encoding track {} of {} — {}", i + 1, total, filename));
                emit_progress(pct);
            } else {
                eprintln!("Encoding track {} → {}", track_num, filename);
            }

            let tags = TrackTags {
                title:        track_title.map(str::to_string),
                artist:       track_artist.map(str::to_string),
                album:        disc_title.map(str::to_string),
                album_artist: disc_artist.map(str::to_string),
                track_number: Some(*track_num),
                track_total:  Some(total),
                songwriter:   track_info.and_then(|t| t.cd_text.as_ref()).and_then(|c| c.songwriter.as_deref()).map(str::to_string),
                composer:     track_info.and_then(|t| t.cd_text.as_ref()).and_then(|c| c.composer.as_deref()).map(str::to_string),
            };

            encoder::encode(wav_path, &out_path, &opts.format, &tags, opts.debug)?;
        }

        let _ = std::fs::remove_dir_all(&wav_dir);
    }

    // Extract data session (Session 2).
    if opts.progress_json { emit_step("Extracting data session..."); emit_progress(62.0); }
    data::extract_data_session(&opts.device, &data_dir, false, opts.debug)?;
    if opts.progress_json { emit_progress(90.0); }

    write_disc_json(info, opts)?;
    write_cdtext_json(info, opts)?;

    if matches!(opts.layout, RipLayout::Archive) {
        if opts.progress_json { emit_step("Generating checksums..."); emit_progress(95.0); }
        let manifest = metadata::generate_checksums(&opts.output_dir)?;
        metadata::write_checksums(&manifest, &meta_dir)?;
    }

    if opts.progress_json { emit_progress(100.0); }
    Ok(())
}

// ── Metadata writers ──────────────────────────────────────────────────────────

fn write_disc_json(info: &analyzer::DiscInfo, opts: &RipOptions) -> Result<(), Error> {
    let meta_dir = format!("{}/metadata", opts.output_dir);
    std::fs::create_dir_all(&meta_dir)?;
    let path = format!("{}/disc.json", meta_dir);
    std::fs::write(path, serde_json::to_string_pretty(info)?)?;
    Ok(())
}

fn write_cdtext_json(info: &analyzer::DiscInfo, opts: &RipOptions) -> Result<(), Error> {
    // Collect all CD-Text that was found across sessions and tracks.
    let has_cdtext = info.sessions.iter()
        .any(|s| s.cd_text.is_some() || s.tracks.iter().any(|t| t.cd_text.is_some()));

    if !has_cdtext { return Ok(()); }

    let meta_dir = format!("{}/metadata", opts.output_dir);
    let path = format!("{}/cdtext.json", meta_dir);

    #[derive(serde::Serialize)]
    struct CdTextExport<'a> {
        disc: Option<&'a analyzer::CdTextBlock>,
        tracks: Vec<TrackCdTextExport<'a>>,
    }
    #[derive(serde::Serialize)]
    struct TrackCdTextExport<'a> {
        number: usize,
        cd_text: Option<&'a analyzer::CdTextBlock>,
    }

    let audio_session = info.sessions.iter().find(|s| matches!(s.kind, SessionKind::Audio));
    if let Some(session) = audio_session {
        let export = CdTextExport {
            disc: session.cd_text.as_ref(),
            tracks: session.tracks.iter().map(|t| TrackCdTextExport {
                number: t.number,
                cd_text: t.cd_text.as_ref(),
            }).collect(),
        };
        std::fs::write(path, serde_json::to_string_pretty(&export)?)?;
    }

    Ok(())
}

/// Check that all required tools are available for ripping the given format.
pub fn check_dependencies(format: &AudioFormat) -> Vec<String> {
    let mut missing = Vec::new();

    if !Path::new("/usr/bin/cdparanoia").exists()
        && !Path::new("/usr/local/bin/cdparanoia").exists()
    {
        missing.push("cdparanoia (sudo apt install cdparanoia)".to_string());
    }

    if !Path::new("/usr/bin/ffmpeg").exists()
        && !Path::new("/usr/local/bin/ffmpeg").exists()
    {
        if *format != AudioFormat::Wav {
            missing.push("ffmpeg (sudo apt install ffmpeg)".to_string());
        }
    }

    missing
}
