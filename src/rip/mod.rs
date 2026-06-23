pub mod data;
pub mod encoder;
pub mod engine;
pub mod metadata;
pub mod musicbrainz;

use std::path::Path;
use crate::{analyzer::{self, DiscFormat, SessionKind, TrackKind}, error::Error};
use encoder::{AudioFormat, TrackTags, track_filename};
use engine::{emit_progress, emit_step};
use musicbrainz::ReleaseInfo;

pub struct RipOptions {
    pub device: String,
    /// Explicit output directory — files land here. Mutually exclusive with `base_dir`.
    pub output_dir: Option<String>,
    /// Base directory — a subfolder named "Artist - Album (Year)" is auto-created here.
    pub base_dir: Option<String>,
    pub format: AudioFormat,
    /// Archive mode: audio/ + metadata/ subdirectories, plus checksums.json.
    pub archive: bool,
    pub debug: bool,
    pub progress_json: bool,
    pub no_musicbrainz: bool,
}

// ── Entry point ───────────────────────────────────────────────────────────────

pub fn rip(opts: &RipOptions) -> Result<(), Error> {
    if opts.output_dir.is_none() && opts.base_dir.is_none() {
        return Err(Error::validation("Specify --output <path> or --dir <base-dir>"));
    }

    // Step 1: analyse disc
    if opts.progress_json { emit_step("Analysing disc..."); emit_progress(0.0); }
    let info = analyzer::analyze(&opts.device)?;

    if opts.progress_json {
        emit_step(&format!("Detected: {}", info.format));
    } else {
        eprintln!("Detected: {}", info.format);
        eprintln!("Sessions: {}", info.sessions.len());
        if let Some(ref id) = info.discid {
            eprintln!("DiscID:   {}", id);
        }
    }

    // Step 2: MusicBrainz lookup (once — used for both folder naming and tags)
    let mb: Option<ReleaseInfo> = if !opts.no_musicbrainz {
        if let Some(ref discid) = info.discid {
            if opts.progress_json { emit_step("Looking up metadata on MusicBrainz..."); }
            else { eprintln!("Looking up DiscID on MusicBrainz..."); }

            match musicbrainz::lookup(discid, opts.debug) {
                Ok(Some(release)) => {
                    if opts.progress_json {
                        emit_step(&format!("Found: {} — {}", release.album, release.album_artist));
                    } else {
                        eprintln!("Found: \"{}\" by \"{}\"{}",
                            release.album, release.album_artist,
                            release.year.as_deref().map(|y| format!(" ({})", y)).unwrap_or_default());
                        if release.total_releases > 1 {
                            eprintln!("  ({} releases share this DiscID — using first match)",
                                release.total_releases);
                        }
                    }
                    Some(release)
                }
                Ok(None) => {
                    if !opts.progress_json { eprintln!("Not found in MusicBrainz — using CD-Text/defaults"); }
                    None
                }
                Err(ref e) => {
                    if !opts.progress_json { eprintln!("MusicBrainz error (non-fatal): {}", e); }
                    None
                }
            }
        } else {
            if !opts.progress_json && !matches!(info.format, DiscFormat::DataCD) {
                eprintln!("No DiscID available — skipping MusicBrainz lookup");
            }
            None
        }
    } else {
        None
    };

    // Step 3: resolve final output directory
    let output_dir = resolve_output_dir(opts, &mb, &info)?;
    std::fs::create_dir_all(&output_dir)?;

    if !opts.progress_json {
        eprintln!("Output:   {}", output_dir);
    }

    // Step 4: fetch cover art (before ripping so it's ready for embedding)
    let cover_art_path: Option<String> = if let Some(ref release) = mb {
        if opts.progress_json { emit_step("Fetching cover art..."); }
        else { eprintln!("Fetching cover art..."); }

        match musicbrainz::fetch_cover_art(&release.mb_release_id, opts.debug) {
            Some((bytes, ext)) => {
                let path = format!("{}/cover.{}", output_dir, ext);
                match std::fs::write(&path, &bytes) {
                    Ok(()) => {
                        if !opts.progress_json {
                            eprintln!("Cover art saved → cover.{} ({} KB)", ext, bytes.len() / 1024);
                        }
                        Some(path)
                    }
                    Err(e) => {
                        if !opts.progress_json { eprintln!("Cover art save failed: {}", e); }
                        None
                    }
                }
            }
            None => {
                if !opts.progress_json { eprintln!("No cover art found in Cover Art Archive"); }
                None
            }
        }
    } else {
        None
    };

    match info.format {
        DiscFormat::RedBook  => rip_redbook(&info, &mb, opts, &output_dir, cover_art_path.as_deref()),
        DiscFormat::DataCD   => rip_datacd(&info, opts, &output_dir),
        DiscFormat::BlueBook => rip_bluebook(&info, &mb, opts, &output_dir, cover_art_path.as_deref()),
        DiscFormat::Unknown  => Err(Error::validation(
            "Could not determine disc format. Insert a disc and try again.",
        )),
    }
}

/// Resolve the final output directory from opts + MB metadata.
fn resolve_output_dir(
    opts: &RipOptions,
    mb: &Option<ReleaseInfo>,
    info: &analyzer::DiscInfo,
) -> Result<String, Error> {
    if let Some(ref explicit) = opts.output_dir {
        return Ok(explicit.clone());
    }

    let base = opts.base_dir.as_deref().unwrap();

    // Build "Artist - Album (Year)" from MB data or CD-Text fallbacks.
    let audio_session = info.sessions.iter().find(|s| matches!(s.kind, SessionKind::Audio));
    let cd_artist = audio_session.and_then(|s| s.cd_text.as_ref()).and_then(|c| c.artist.as_deref());
    let cd_album  = audio_session.and_then(|s| s.cd_text.as_ref()).and_then(|c| c.title.as_deref());

    let artist = mb.as_ref().map(|r| r.album_artist.as_str()).or(cd_artist).unwrap_or("Unknown Artist");
    let album  = mb.as_ref().map(|r| r.album.as_str()).or(cd_album).unwrap_or("Unknown Album");
    let year   = mb.as_ref().and_then(|r| r.year.as_deref());

    let folder = if let Some(y) = year {
        format!("{} - {} ({})", safe_path(artist), safe_path(album), y)
    } else {
        format!("{} - {}", safe_path(artist), safe_path(album))
    };

    Ok(format!("{}/{}", base.trim_end_matches('/'), folder))
}

/// Strip characters that are problematic in directory names.
fn safe_path(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c => c,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

// ── Red Book ──────────────────────────────────────────────────────────────────

fn rip_redbook(
    info: &analyzer::DiscInfo,
    mb: &Option<ReleaseInfo>,
    opts: &RipOptions,
    output_dir: &str,
    cover_art: Option<&str>,
) -> Result<(), Error> {
    let Some(session) = info.sessions.first() else {
        return Err(Error::validation("No sessions found on disc"));
    };

    // Flat layout: files go directly in output_dir.
    // Archive layout: files go in output_dir/audio/, metadata in output_dir/metadata/.
    let audio_dir = if opts.archive {
        format!("{}/audio", output_dir)
    } else {
        output_dir.to_string()
    };
    std::fs::create_dir_all(&audio_dir)?;

    let track_count = session.tracks.len();
    let cd_title   = session.cd_text.as_ref().and_then(|c| c.title.as_deref());
    let cd_artist  = session.cd_text.as_ref().and_then(|c| c.artist.as_deref());

    let album            = mb.as_ref().map(|r| r.album.as_str()).or(cd_title);
    let album_artist     = mb.as_ref().map(|r| r.album_artist.as_str()).or(cd_artist);
    let year             = mb.as_ref().and_then(|r| r.year.as_deref());
    let mb_release_id    = mb.as_ref().map(|r| r.mb_release_id.clone());
    let mb_artist_id_alb = mb.as_ref().and_then(|r| r.mb_artist_id.clone());

    let wav_dir = format!("/tmp/rustydisc_rip_{}", std::process::id());
    let wav_tracks = engine::rip_all_tracks(
        &opts.device, &wav_dir, track_count, opts.debug, opts.progress_json,
    )?;

    let ext   = opts.format.extension();
    let total = wav_tracks.len();

    for (i, (track_num, wav_path)) in wav_tracks.iter().enumerate() {
        let track_info = session.tracks.iter().find(|t| t.number == *track_num);
        let mb_track   = mb.as_ref().and_then(|r| r.tracks.iter().find(|t| t.number == *track_num));

        let title = mb_track.map(|t| t.title.as_str())
            .or_else(|| track_info.and_then(|t| t.cd_text.as_ref()).and_then(|c| c.title.as_deref()));

        let artist = mb_track.and_then(|t| t.artist.as_deref())
            .or_else(|| track_info.and_then(|t| t.cd_text.as_ref()).and_then(|c| c.artist.as_deref()))
            .or(album_artist);

        let filename = track_filename(*track_num, total, artist, album, title, ext);
        let out_path = format!("{}/{}", audio_dir, filename);

        if opts.progress_json {
            emit_step(&format!("Encoding track {} of {} — {}", i + 1, total, filename));
            emit_progress(5.0 + (i as f32 / total as f32) * 90.0);
        } else {
            eprintln!("  Encoding track {:2} → {}", track_num, filename);
        }

        encoder::encode(wav_path, &out_path, &opts.format, &TrackTags {
            title:           title.map(str::to_string),
            artist:          artist.map(str::to_string),
            album:           album.map(str::to_string),
            album_artist:    album_artist.map(str::to_string),
            track_number:    Some(*track_num),
            track_total:     Some(total),
            year:            year.map(str::to_string),
            songwriter:      track_info.and_then(|t| t.cd_text.as_ref()).and_then(|c| c.songwriter.as_deref()).map(str::to_string),
            composer:        track_info.and_then(|t| t.cd_text.as_ref()).and_then(|c| c.composer.as_deref()).map(str::to_string),
            mb_release_id:   mb_release_id.clone(),
            mb_recording_id: mb_track.and_then(|t| t.mb_recording_id.clone()),
            mb_artist_id:    mb_track.and_then(|t| t.mb_artist_id.clone()).or_else(|| mb_artist_id_alb.clone()),
        }, cover_art, opts.debug)?;
    }

    let _ = std::fs::remove_dir_all(&wav_dir);

    // Metadata — only written in archive mode.
    if opts.archive {
        let meta_dir = format!("{}/metadata", output_dir);
        std::fs::create_dir_all(&meta_dir)?;
        write_disc_json(info, output_dir)?;
        write_cdtext_json(info, output_dir)?;
        if let Some(release) = mb {
            write_mb_json(release, output_dir)?;
        }
        if opts.progress_json { emit_step("Generating checksums..."); emit_progress(96.0); }
        let manifest = metadata::generate_checksums(output_dir)?;
        metadata::write_checksums(&manifest, &meta_dir)?;
    }

    if opts.progress_json { emit_progress(100.0); }
    Ok(())
}

// ── Data CD ───────────────────────────────────────────────────────────────────

fn rip_datacd(
    info: &analyzer::DiscInfo,
    opts: &RipOptions,
    output_dir: &str,
) -> Result<(), Error> {
    let data_dir = if opts.archive {
        format!("{}/data", output_dir)
    } else {
        output_dir.to_string()
    };

    if opts.progress_json { emit_step("Extracting data session..."); emit_progress(5.0); }
    data::extract_data_session(&opts.device, &data_dir, false, opts.debug)?;
    if opts.progress_json { emit_progress(90.0); }

    if opts.archive {
        let meta_dir = format!("{}/metadata", output_dir);
        std::fs::create_dir_all(&meta_dir)?;
        write_disc_json(info, output_dir)?;
        let manifest = metadata::generate_checksums(output_dir)?;
        metadata::write_checksums(&manifest, &meta_dir)?;
    }

    if opts.progress_json { emit_progress(100.0); }
    Ok(())
}

// ── Blue Book ─────────────────────────────────────────────────────────────────

fn rip_bluebook(
    info: &analyzer::DiscInfo,
    mb: &Option<ReleaseInfo>,
    opts: &RipOptions,
    output_dir: &str,
    cover_art: Option<&str>,
) -> Result<(), Error> {
    let audio_dir = format!("{}/audio", output_dir);
    let data_dir  = format!("{}/data",  output_dir);
    let meta_dir  = format!("{}/metadata", output_dir);
    std::fs::create_dir_all(&audio_dir)?;
    std::fs::create_dir_all(&data_dir)?;

    let audio_session = info.sessions.iter().find(|s| matches!(s.kind, SessionKind::Audio));

    if let Some(session) = audio_session {
        let track_count = session.tracks.iter().filter(|t| t.kind == TrackKind::Audio).count();

        let cd_title  = session.cd_text.as_ref().and_then(|c| c.title.as_deref());
        let cd_artist = session.cd_text.as_ref().and_then(|c| c.artist.as_deref());
        let album            = mb.as_ref().map(|r| r.album.as_str()).or(cd_title);
        let album_artist     = mb.as_ref().map(|r| r.album_artist.as_str()).or(cd_artist);
        let year             = mb.as_ref().and_then(|r| r.year.as_deref());
        let mb_release_id    = mb.as_ref().map(|r| r.mb_release_id.clone());
        let mb_artist_id_alb = mb.as_ref().and_then(|r| r.mb_artist_id.clone());

        let wav_dir = format!("/tmp/rustydisc_rip_{}", std::process::id());
        let wav_tracks = engine::rip_all_tracks(
            &opts.device, &wav_dir, track_count, opts.debug, opts.progress_json,
        )?;

        let ext   = opts.format.extension();
        let total = wav_tracks.len();

        for (i, (track_num, wav_path)) in wav_tracks.iter().enumerate() {
            let track_info = session.tracks.iter().find(|t| t.number == *track_num);
            let mb_track   = mb.as_ref().and_then(|r| r.tracks.iter().find(|t| t.number == *track_num));

            let title = mb_track.map(|t| t.title.as_str())
                .or_else(|| track_info.and_then(|t| t.cd_text.as_ref()).and_then(|c| c.title.as_deref()));
            let artist = mb_track.and_then(|t| t.artist.as_deref())
                .or_else(|| track_info.and_then(|t| t.cd_text.as_ref()).and_then(|c| c.artist.as_deref()))
                .or(album_artist);

            let filename = track_filename(*track_num, total, artist, album, title, ext);
            let out_path = format!("{}/{}", audio_dir, filename);

            if opts.progress_json {
                emit_step(&format!("Encoding track {} of {} — {}", i + 1, total, filename));
                emit_progress(5.0 + (i as f32 / total as f32) * 55.0);
            } else {
                eprintln!("  Encoding track {:2} → {}", track_num, filename);
            }

            encoder::encode(wav_path, &out_path, &opts.format, &TrackTags {
                title:           title.map(str::to_string),
                artist:          artist.map(str::to_string),
                album:           album.map(str::to_string),
                album_artist:    album_artist.map(str::to_string),
                track_number:    Some(*track_num),
                track_total:     Some(total),
                year:            year.map(str::to_string),
                songwriter:      track_info.and_then(|t| t.cd_text.as_ref()).and_then(|c| c.songwriter.as_deref()).map(str::to_string),
                composer:        track_info.and_then(|t| t.cd_text.as_ref()).and_then(|c| c.composer.as_deref()).map(str::to_string),
                mb_release_id:   mb_release_id.clone(),
                mb_recording_id: mb_track.and_then(|t| t.mb_recording_id.clone()),
                mb_artist_id:    mb_track.and_then(|t| t.mb_artist_id.clone()).or_else(|| mb_artist_id_alb.clone()),
            }, cover_art, opts.debug)?;
        }

        let _ = std::fs::remove_dir_all(&wav_dir);
    }

    if opts.progress_json { emit_step("Extracting data session..."); emit_progress(62.0); }
    data::extract_data_session(&opts.device, &data_dir, false, opts.debug)?;
    if opts.progress_json { emit_progress(90.0); }

    std::fs::create_dir_all(&meta_dir)?;
    write_disc_json(info, output_dir)?;
    write_cdtext_json(info, output_dir)?;
    if let Some(release) = mb {
        write_mb_json(release, output_dir)?;
    }

    if opts.archive {
        if opts.progress_json { emit_step("Generating checksums..."); emit_progress(95.0); }
        let manifest = metadata::generate_checksums(output_dir)?;
        metadata::write_checksums(&manifest, &meta_dir)?;
    }

    if opts.progress_json { emit_progress(100.0); }
    Ok(())
}

// ── Metadata writers ──────────────────────────────────────────────────────────

fn write_disc_json(info: &analyzer::DiscInfo, output_dir: &str) -> Result<(), Error> {
    let meta_dir = format!("{}/metadata", output_dir);
    std::fs::create_dir_all(&meta_dir)?;
    std::fs::write(format!("{}/disc.json", meta_dir), serde_json::to_string_pretty(info)?)?;
    Ok(())
}

fn write_mb_json(release: &ReleaseInfo, output_dir: &str) -> Result<(), Error> {
    let meta_dir = format!("{}/metadata", output_dir);
    std::fs::create_dir_all(&meta_dir)?;
    std::fs::write(format!("{}/musicbrainz.json", meta_dir), serde_json::to_string_pretty(release)?)?;
    Ok(())
}

fn write_cdtext_json(info: &analyzer::DiscInfo, output_dir: &str) -> Result<(), Error> {
    let has_cdtext = info.sessions.iter()
        .any(|s| s.cd_text.is_some() || s.tracks.iter().any(|t| t.cd_text.is_some()));
    if !has_cdtext { return Ok(()); }

    let meta_dir = format!("{}/metadata", output_dir);

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
        std::fs::write(format!("{}/cdtext.json", meta_dir), serde_json::to_string_pretty(&export)?)?;
    }
    Ok(())
}

// ── Dependency check ──────────────────────────────────────────────────────────

pub fn check_dependencies(format: &AudioFormat) -> Vec<String> {
    let mut missing = Vec::new();
    if !Path::new("/usr/bin/cdparanoia").exists() && !Path::new("/usr/local/bin/cdparanoia").exists() {
        missing.push("cdparanoia (sudo apt install cdparanoia)".to_string());
    }
    if *format != AudioFormat::Wav
        && !Path::new("/usr/bin/ffmpeg").exists()
        && !Path::new("/usr/local/bin/ffmpeg").exists()
    {
        missing.push("ffmpeg (sudo apt install ffmpeg)".to_string());
    }
    missing
}
