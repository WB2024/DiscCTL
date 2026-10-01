pub mod accuraterip;
pub mod cover;
pub mod data;
pub mod encoder;
pub mod engine;
pub mod gaps;
pub mod mb_enrich;
pub mod metadata;
pub mod musicbrainz;
pub mod offset;
pub mod readhealth;
pub mod report;
pub mod tagging;

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
    /// Use this MusicBrainz release (ID or URL) instead of looking the disc up by DiscID
    pub mb_release: Option<String>,
    /// Where cover art comes from and what to do with it
    pub cover: cover::CoverOptions,
    /// Skip the AccurateRip database check
    pub no_accuraterip: bool,
    /// A picture the user supplied: used instead of looking one up.
    pub cover_file: Option<String>,
    /// Quality choice for the format (see `encoder::quality_choices`); None = the best.
    pub quality: Option<String>,
    /// Measure loudness after ripping and write ReplayGain tags.
    pub replaygain: bool,
    /// Measure dynamic range (DR) after ripping and write DR tags.
    pub dynamic_range: bool,
    /// What to do about the drive's read offset.
    pub offset: offset::OffsetMode,
    /// How hard cdparanoia checks what it reads.
    pub paranoia: engine::Paranoia,
    /// What to do about hidden audio before track 1.
    pub hidden: gaps::HiddenTrack,
    /// What to do about gaps between tracks.
    pub gaps: gaps::GapMode,
}

// ── Entry point ───────────────────────────────────────────────────────────────

pub fn rip(opts: &RipOptions) -> Result<(), Error> {
    if opts.output_dir.is_none() && opts.base_dir.is_none() {
        return Err(Error::validation("Specify --output <path> or --dir <base-dir>"));
    }

    let started = std::time::SystemTime::now();

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
    let mb: Option<ReleaseInfo> = if let Some(ref wanted) = opts.mb_release {
        // The user picked the release explicitly: no DiscID guesswork, and no silent fallback.
        let mbid = musicbrainz::parse_release_id(wanted)?;
        if opts.progress_json { emit_step("Fetching the chosen MusicBrainz release..."); }
        else { eprintln!("Fetching MusicBrainz release {}...", mbid); }

        let audio_tracks = info.sessions.iter().flat_map(|s| s.tracks.iter())
            .filter(|t| t.kind == TrackKind::Audio).count();
        let (release, warning) = musicbrainz::lookup_release(
            &mbid, info.discid.as_deref(), Some(audio_tracks), opts.debug,
        )?;
        if opts.progress_json {
            emit_step(&format!("Using: {} — {}", release.album, release.album_artist));
            if let Some(ref w) = warning { emit_step(&format!("Warning: {}", w)); }
        } else {
            eprintln!("Using: \"{}\" by \"{}\"{}", release.album, release.album_artist,
                release.year.as_deref().map(|y| format!(" ({})", y)).unwrap_or_default());
            if let Some(ref w) = warning { eprintln!("Warning: {}", w); }
        }
        Some(release)
    } else if !opts.no_musicbrainz {
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

    // The lookups above find the release; one more request fills in the rest of what Picard tags.
    let mb = mb.map(|mut r| {
        if !opts.no_musicbrainz || opts.mb_release.is_some() {
            if opts.progress_json { emit_step("Fetching the full MusicBrainz details..."); }
            musicbrainz_details(&mut r, info.discid.as_deref(), opts.debug);
        }
        r
    });

    // Step 3: resolve final output directory
    let output_dir = resolve_output_dir(opts, &mb, &info)?;
    std::fs::create_dir_all(&output_dir)?;

    if !opts.progress_json {
        eprintln!("Output:   {}", output_dir);
    }

    // Step 4: fetch cover art (before ripping so it's ready for embedding)
    let (cover_art_path, _temp_cover) = prepare_cover(opts, &mb, &output_dir);

    let result = match info.format {
        DiscFormat::RedBook  => rip_redbook(&info, &mb, opts, &output_dir, cover_art_path.as_deref(), started),
        DiscFormat::DataCD   => rip_datacd(&info, opts, &output_dir),
        DiscFormat::BlueBook => rip_bluebook(&info, &mb, opts, &output_dir, cover_art_path.as_deref(), started),
        DiscFormat::Unknown  => Err(Error::validation(
            "Could not determine disc format. Insert a disc and try again.",
        )),
    };
    // Whatever was written, even by a rip that stopped part-way, belongs to the configured owner.
    crate::perms::own_tree(Path::new(&output_dir));
    crate::perms::own_parents(Path::new(&output_dir));
    result
}


/// Deletes a temporary file when dropped (the cover image used only for embedding).
struct TempFile(Option<String>);

impl Drop for TempFile {
    fn drop(&mut self) {
        if let Some(p) = &self.0 {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// The picture chosen for this rip, wherever it came from.
struct UsedCover {
    bytes: Vec<u8>,
    ext: &'static str,
    id: &'static str,
    label: &'static str,
}

/// Fetch the cover art according to the user's source priorities. Returns the path to embed
/// (None when embedding is off or nothing was found) and a guard for a temporary copy that
/// exists only when the image is embedded but not kept as `cover.jpg`/`cover.png`.
fn prepare_cover(opts: &RipOptions, mb: &Option<ReleaseInfo>, output_dir: &str) -> (Option<String>, TempFile) {
    let none = (None, TempFile(None));
    if !opts.cover.wanted() {
        if !opts.progress_json { eprintln!("Cover art: disabled in settings"); }
        return none;
    }

    // A picture the user uploaded beats anything we could look up.
    let uploaded = opts.cover_file.as_deref().and_then(|p| {
        let bytes = std::fs::read(p).ok()?;
        let ext = cover::sniff_ext(&bytes)?;
        Some((bytes, ext))
    });
    let found = if let Some((bytes, ext)) = uploaded {
        UsedCover { bytes, ext, id: "upload", label: "your upload" }
    } else {
        let Some(release) = mb else { return none };
        if opts.progress_json { emit_step("Fetching cover art..."); }
        else { eprintln!("Fetching cover art..."); }
        let Some(f) = cover::fetch(&release.mb_release_id, release.mb_release_group_id.as_deref(), &opts.cover, opts.debug) else {
            let tried: Vec<&str> = opts.cover.sources.iter().map(|s| s.label()).collect();
            let msg = format!("No cover art found ({})", tried.join(", "));
            if opts.progress_json { emit_step(&msg); } else { eprintln!("{}", msg); }
            return none;
        };
        UsedCover { bytes: f.bytes, ext: f.ext, id: f.source.id(), label: f.source.label() }
    };

    let kb = found.bytes.len() / 1024;
    let mut saved: Option<String> = None;
    let mut temp = TempFile(None);
    let mut embed_path: Option<String> = None;

    if opts.cover.save_file {
        let path = format!("{}/cover.{}", output_dir, found.ext);
        match std::fs::write(&path, &found.bytes) {
            Ok(()) => { saved = Some(path.clone()); embed_path = Some(path); }
            Err(e) => eprintln!("Cover art save failed: {}", e),
        }
    }
    if opts.cover.embed && embed_path.is_none() {
        // Embedding without keeping a file: ffmpeg still needs one to read from.
        let path = format!("/tmp/rustydisc_cover_{}.{}", std::process::id(), found.ext);
        if std::fs::write(&path, &found.bytes).is_ok() {
            temp = TempFile(Some(path.clone()));
            embed_path = Some(path);
        }
    }

    let what = match (saved.is_some(), opts.cover.embed) {
        (true, true) => "saved as cover file and embedded",
        (true, false) => "saved as cover file",
        (false, true) => "embedded only",
        (false, false) => "not stored",
    };
    let msg = format!("Cover art from {} ({} KB) — {}", found.label, kb, what);
    if opts.progress_json {
        emit_step(&msg);
        let event = serde_json::json!({
            "type": "cover", "source": found.id, "label": found.label,
            "file": saved, "embedded": opts.cover.embed,
        });
        println!("{}", event);
    } else {
        eprintln!("{}", msg);
    }

    (if opts.cover.embed { embed_path } else { None }, temp)
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
    started: std::time::SystemTime,
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
    let hidden_len = gaps::hidden_sectors(info);
    let read = engine::rip_all_tracks(
        &opts.device, &wav_dir, track_count, opts.paranoia,
        hidden_len.filter(|_| opts.hidden == gaps::HiddenTrack::Auto), opts.debug, opts.progress_json,
    )?;
    let hidden_outcome = hidden_outcome(hidden_len, opts, &read);
    let health = read_health(info, opts, &read);
    let wav_tracks = read.tracks;

    let (ar, offset_applied, mut offset_notes) = verify_with_offset(info, opts, &wav_tracks);
    // Gaps are handled after AccurateRip has checked the tracks cut at their official starts.
    let found_gaps = handle_gaps(info, opts, &wav_tracks, &mut offset_notes);

    let ext   = opts.format.extension();
    // A hidden track (number 0) is an extra file, not one of the disc's numbered tracks.
    let total = wav_tracks.iter().filter(|(n, _)| *n > 0).count();

    let mut outputs: Vec<String> = Vec::new();
    let mut ripped: Vec<RippedEntry> = Vec::new();
    for (i, (track_num, wav_path)) in wav_tracks.iter().enumerate() {
        let track_info = session.tracks.iter().find(|t| t.number == *track_num);
        let mb_track   = mb.as_ref().and_then(|r| r.tracks.iter().find(|t| t.number == *track_num));

        let title = mb_track.map(|t| t.title.as_str())
            .or_else(|| track_info.and_then(|t| t.cd_text.as_ref()).and_then(|c| c.title.as_deref()));
        let title = if *track_num == 0 { Some("Hidden track") } else { title };

        let artist = mb_track.and_then(|t| t.artist.as_deref())
            .or_else(|| track_info.and_then(|t| t.cd_text.as_ref()).and_then(|c| c.artist.as_deref()))
            .or(album_artist);

        let filename = track_filename(*track_num, total, artist, album, title, ext);
        let out_path = format!("{}/{}", audio_dir, filename);

        if opts.progress_json {
            emit_step(&format!("Encoding track {} of {} — {}", i + 1, wav_tracks.len(), filename));
            emit_progress(85.0 + (i as f32 / wav_tracks.len() as f32) * 10.0);
        } else {
            eprintln!("  Encoding track {:2} → {}", track_num, filename);
        }

        let tags = TrackTags {
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
        };

        encoder::encode(wav_path, &out_path, &opts.format, &tags, cover_art, opts.quality.as_deref(), opts.debug)?;

        tagging::apply(&out_path, &tags, mb.as_ref(), mb_track, opts.debug);
        if *track_num > 0 {
            outputs.push(out_path.clone());
        }
        ripped.push(RippedEntry { number: *track_num, title: title.map(str::to_string), file: filename.clone(), raw_path: wav_path.clone() });
    }

    let report_dir = if opts.archive { format!("{}/metadata", output_dir) } else { output_dir.to_string() };
    write_rip_report(info, mb, opts, ar.as_ref(), started, &ripped, &report_dir, offset_applied, &offset_notes, Some(&health), &hidden_outcome, hidden_len, &found_gaps);
    let _ = std::fs::remove_dir_all(&wav_dir);
    post_rip(opts, &outputs);

    // Metadata — only written in archive mode.
    if opts.archive {
        let meta_dir = format!("{}/metadata", output_dir);
        std::fs::create_dir_all(&meta_dir)?;
        write_disc_json(info, output_dir)?;
        write_cdtext_json(info, output_dir)?;
        if let Some(release) = mb {
            write_mb_json(release, output_dir)?;
        }
        if let Some(report) = &ar {
            write_accuraterip_json(report, output_dir)?;
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
    started: std::time::SystemTime,
) -> Result<(), Error> {
    let audio_dir = format!("{}/audio", output_dir);
    let data_dir  = format!("{}/data",  output_dir);
    let meta_dir  = format!("{}/metadata", output_dir);
    std::fs::create_dir_all(&audio_dir)?;
    std::fs::create_dir_all(&data_dir)?;

    let audio_session = info.sessions.iter().find(|s| matches!(s.kind, SessionKind::Audio));
    let mut ar: Option<accuraterip::Report> = None;
    let mut offset_applied = 0i32;
    let mut offset_notes: Vec<String> = Vec::new();
    let mut health: Option<readhealth::ReadHealth> = None;
    let mut hidden_outcome_bb = gaps::HiddenOutcome::None;
    let mut found_gaps_bb: Vec<gaps::TrackGap> = Vec::new();

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
        let hidden_len = gaps::hidden_sectors(info);
        let read = engine::rip_all_tracks(
            &opts.device, &wav_dir, track_count, opts.paranoia,
            hidden_len.filter(|_| opts.hidden == gaps::HiddenTrack::Auto), opts.debug, opts.progress_json,
        )?;
        hidden_outcome_bb = hidden_outcome(hidden_len, opts, &read);
        health = Some(read_health(info, opts, &read));
        let wav_tracks = read.tracks;

        let (checked, applied, notes) = verify_with_offset(info, opts, &wav_tracks);
        ar = checked;
        offset_applied = applied;
        offset_notes = notes;
        found_gaps_bb = handle_gaps(info, opts, &wav_tracks, &mut offset_notes);

        let ext   = opts.format.extension();
        let total = wav_tracks.iter().filter(|(n, _)| *n > 0).count();

        let mut outputs: Vec<String> = Vec::new();
        let mut ripped: Vec<RippedEntry> = Vec::new();
        for (i, (track_num, wav_path)) in wav_tracks.iter().enumerate() {
            let track_info = session.tracks.iter().find(|t| t.number == *track_num);
            let mb_track   = mb.as_ref().and_then(|r| r.tracks.iter().find(|t| t.number == *track_num));

            let title = mb_track.map(|t| t.title.as_str())
                .or_else(|| track_info.and_then(|t| t.cd_text.as_ref()).and_then(|c| c.title.as_deref()));
            let title = if *track_num == 0 { Some("Hidden track") } else { title };
            let artist = mb_track.and_then(|t| t.artist.as_deref())
                .or_else(|| track_info.and_then(|t| t.cd_text.as_ref()).and_then(|c| c.artist.as_deref()))
                .or(album_artist);

            let filename = track_filename(*track_num, total, artist, album, title, ext);
            let out_path = format!("{}/{}", audio_dir, filename);

            if opts.progress_json {
                emit_step(&format!("Encoding track {} of {} — {}", i + 1, wav_tracks.len(), filename));
                emit_progress(85.0 + (i as f32 / wav_tracks.len() as f32) * 5.0);
            } else {
                eprintln!("  Encoding track {:2} → {}", track_num, filename);
            }

            let tags = TrackTags {
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
            };

            encoder::encode(wav_path, &out_path, &opts.format, &tags, cover_art, opts.quality.as_deref(), opts.debug)?;

            tagging::apply(&out_path, &tags, mb.as_ref(), mb_track, opts.debug);
            if *track_num > 0 {
                outputs.push(out_path.clone());
            }
            ripped.push(RippedEntry { number: *track_num, title: title.map(str::to_string), file: filename.clone(), raw_path: wav_path.clone() });
        }

        write_rip_report(info, mb, opts, ar.as_ref(), started, &ripped, &meta_dir, offset_applied, &offset_notes, health.as_ref(), &hidden_outcome_bb, gaps::hidden_sectors(info), &found_gaps_bb);
        let _ = std::fs::remove_dir_all(&wav_dir);
        post_rip(opts, &outputs);
    }

    if opts.progress_json { emit_step("Extracting data session..."); emit_progress(91.0); }
    data::extract_data_session(&opts.device, &data_dir, false, opts.debug)?;
    if opts.progress_json { emit_progress(95.0); }

    std::fs::create_dir_all(&meta_dir)?;
    write_disc_json(info, output_dir)?;
    write_cdtext_json(info, output_dir)?;
    if let Some(release) = mb {
        write_mb_json(release, output_dir)?;
    }
    if let Some(report) = &ar {
        write_accuraterip_json(report, output_dir)?;
    }

    if opts.archive {
        if opts.progress_json { emit_step("Generating checksums..."); emit_progress(95.0); }
        let manifest = metadata::generate_checksums(output_dir)?;
        metadata::write_checksums(&manifest, &meta_dir)?;
    }

    if opts.progress_json { emit_progress(100.0); }
    Ok(())
}

/// Look for gaps between tracks (if asked), say so, and move them if asked.
fn handle_gaps(info: &analyzer::DiscInfo, opts: &RipOptions, wavs: &[(usize, String)], notes: &mut Vec<String>) -> Vec<gaps::TrackGap> {
    use gaps::GapMode;
    if opts.gaps == GapMode::Off {
        return Vec::new();
    }
    let say = |m: &str| if opts.progress_json { emit_step(m) } else { eprintln!("{m}") };
    say("Scanning the disc for gaps between tracks (this takes about five minutes)...");
    let found: Vec<gaps::TrackGap> = match gaps::scan(&opts.device, opts.debug) {
        Ok(g) => g.into_iter().filter(|g| g.track > 1).collect(),
        Err(e) => {
            notes.push(format!("The gap scan failed ({e}), so gaps between tracks were left as they are."));
            return Vec::new();
        }
    };
    if found.is_empty() {
        notes.push("No gaps between tracks were found.".into());
        say("No gaps between tracks");
        return found;
    }
    let list = |g: &gaps::TrackGap| format!("track {:02} ({:.1} s)", g.track, gaps::seconds(g.sectors));
    let names: Vec<String> = found.iter().map(list).collect();
    say(&format!("Gaps found before {}", names.join(", ")));
    match opts.gaps {
        GapMode::OwnTrack => match gaps::move_gaps_to_own_track(wavs, &contiguity(info, wavs), &found) {
            Ok(n) => notes.push(format!("Gaps before {} were moved to the start of the track they lead into ({n} moved).", names.join(", "))),
            Err(e) => notes.push(format!("Gaps before {} were found but could not be moved ({e}); they stay at the end of the previous track.", names.join(", "))),
        },
        _ => notes.push(format!("Gaps before {} were found. Their audio is at the end of the previous track.", names.join(", "))),
    }
    found
}

/// What became of the audio before track 1, taking the user's choice into account.
fn hidden_outcome(len: Option<u32>, opts: &RipOptions, read: &engine::RipRead) -> gaps::HiddenOutcome {
    match len {
        None => gaps::HiddenOutcome::None,
        Some(l) if opts.hidden == gaps::HiddenTrack::Skip => gaps::HiddenOutcome::Skipped { seconds: gaps::seconds(l) },
        Some(_) => read.hidden.clone(),
    }
}

/// Judge how cleanly each track was read, and tell the user.
fn read_health(info: &analyzer::DiscInfo, opts: &RipOptions, read: &engine::RipRead) -> readhealth::ReadHealth {
    let tracks: Vec<&analyzer::TrackInfo> = info.sessions.iter().flat_map(|s| s.tracks.iter()).collect();
    let ranges: Vec<(usize, u32, u32)> = read
        .tracks
        .iter()
        .filter_map(|(n, _)| {
            if *n == 0 {
                // The hidden track fills the space in front of track 1.
                return gaps::hidden_sectors(info).map(|l| (0, 0, l));
            }
            tracks.iter().find(|t| t.number == *n).map(|t| (*n, t.lba_start, t.lba_end))
        })
        .collect();
    let health = read.events.summarize(&ranges);
    let (clean, repaired, suspect) = (health.count(readhealth::Status::Clean), health.count(readhealth::Status::Repaired), health.count(readhealth::Status::Suspect));
    let line = format!("Read quality: {clean} clean, {repaired} repaired, {suspect} suspect{}", if opts.paranoia == engine::Paranoia::Full { "" } else { " (reduced checking)" });
    if opts.progress_json {
        emit_step(&line);
        println!("{}", serde_json::json!({"type": "read_health", "clean": clean, "repaired": repaired, "suspect": suspect, "tracks": health.tracks, "cache_errors": health.cache_errors}));
    } else {
        eprintln!("{line}");
        for t in health.tracks.iter().filter(|t| t.status != readhealth::Status::Clean) {
            eprintln!("  Track {:02}: {}", t.number, t.describe());
        }
    }
    health
}

/// Which tracks follow each other directly on the disc, so audio can be borrowed across the boundary.
fn contiguity(info: &analyzer::DiscInfo, wavs: &[(usize, String)]) -> Vec<bool> {
    let tracks: Vec<&analyzer::TrackInfo> = info.sessions.iter().flat_map(|s| s.tracks.iter()).collect();
    wavs.windows(2)
        .map(|w| {
            // The hidden track ends exactly where track 1 begins.
            if w[0].0 == 0 {
                return w[1].0 == 1;
            }
            let a = tracks.iter().find(|t| t.number == w[0].0);
            let b = tracks.iter().find(|t| t.number == w[1].0);
            matches!((a, b), (Some(a), Some(b)) if b.number == a.number + 1 && a.lba_end == b.lba_start)
        })
        .collect()
}

/// Check the rip against AccurateRip, correcting the drive's read offset as the options ask.
/// Returns the final report, the offset applied (samples) and notes for the rip log.
fn verify_with_offset(
    info: &analyzer::DiscInfo,
    opts: &RipOptions,
    wavs: &[(usize, String)],
) -> (Option<accuraterip::Report>, i32, Vec<String>) {
    use offset::OffsetMode;
    let say = |m: &str| if opts.progress_json { emit_step(m) } else { eprintln!("{m}") };
    let check = || if opts.no_accuraterip { None } else { accuraterip::check(info, wavs, opts.debug, opts.progress_json) };
    let mut notes = Vec::new();
    let padding_note = |o: &offset::Outcome| if o.padded_samples > 0 {
        Some(format!("{} samples at the very {} of the disc had nothing to borrow from and are silence.", o.padded_samples, if o.offset > 0 { "end" } else { "start" }))
    } else { None };

    match opts.offset {
        OffsetMode::Off => (check(), 0, notes),
        OffsetMode::Fixed(n) => {
            say(&format!("Correcting the drive's read offset ({n:+} samples)..."));
            match offset::apply(wavs, &contiguity(info, wavs), n) {
                Ok(out) => {
                    offset::discard_originals(wavs);
                    notes.push(format!("Read offset corrected by {n:+} samples (set by you)."));
                    notes.extend(padding_note(&out));
                    (check(), n, notes)
                }
                Err(e) => {
                    offset::restore(wavs);
                    notes.push(format!("The read offset could not be corrected ({e}); the audio is as the drive returned it."));
                    (check(), 0, notes)
                }
            }
        }
        OffsetMode::Auto => {
            if opts.no_accuraterip {
                notes.push("Automatic read offset correction needs AccurateRip, which was turned off.".into());
                return (None, 0, notes);
            }
            let first = check();
            let Some(shift) = first.as_ref().and_then(|r| r.detected_shift_samples).filter(|s| *s != 0) else {
                return (first, 0, notes);
            };
            say(&format!("AccurateRip proves the drive reads {shift:+} samples off; correcting it..."));
            let out = match offset::apply(wavs, &contiguity(info, wavs), shift) {
                Ok(o) => o,
                Err(e) => {
                    offset::restore(wavs);
                    notes.push(format!("The read offset could not be corrected ({e}); the audio is as the drive returned it."));
                    return (first, 0, notes);
                }
            };
            let second = check();
            let before = first.as_ref().map(|r| r.verified).unwrap_or(0);
            let better = second.as_ref().is_some_and(|r| r.verified >= before && r.detected_shift_samples.is_none());
            if better {
                offset::discard_originals(wavs);
                say(&format!("Read offset corrected: all {} verified tracks now match exactly", second.as_ref().map(|r| r.verified).unwrap_or(0)));
                notes.push(format!("Read offset of {shift:+} samples corrected automatically (found by AccurateRip), and the result re-checked."));
                notes.extend(padding_note(&out));
                (second, shift, notes)
            } else {
                offset::restore(wavs);
                say("The correction did not improve the AccurateRip result, so the audio was left as the drive returned it");
                notes.push(format!("A {shift:+} sample correction was tried but did not improve the AccurateRip result, so it was undone."));
                (first, 0, notes)
            }
        }
    }
}

/// One track as it was ripped, kept until the rip report is written.
struct RippedEntry {
    number: usize,
    title: Option<String>,
    file: String,
    raw_path: String,
}

/// Save `rip-report.json` and `rip.log`. A report that can't be written never fails the rip.
fn write_rip_report(
    info: &analyzer::DiscInfo,
    mb: &Option<ReleaseInfo>,
    opts: &RipOptions,
    ar: Option<&accuraterip::Report>,
    started: std::time::SystemTime,
    ripped: &[RippedEntry],
    dir: &str,
    offset_applied: i32,
    offset_notes: &[String],
    health: Option<&readhealth::ReadHealth>,
    hidden: &gaps::HiddenOutcome,
    hidden_len: Option<u32>,
    found_gaps: &[gaps::TrackGap],
) {
    if opts.progress_json { emit_step("Writing the rip log..."); }
    let report = report::build(report::Inputs {
        info,
        mb: mb.as_ref(),
        settings: report::Settings {
            reader: "cdparanoia".into(),
            read_mode: opts.paranoia.describe().into(),
            offset_applied_samples: offset_applied,
            format: opts.format.extension().to_uppercase(),
            quality: encoder::quality_choices(&opts.format)
                .into_iter()
                .find(|c| match opts.quality.as_deref() { Some(q) => c.id == q, None => true })
                .map(|c| c.label.to_string())
                .filter(|l| !matches!(l.as_str(), "Lossless" | "Uncompressed")),
            archive: opts.archive,
        },
        started,
        tracks: ripped.iter().map(|t| report::RippedTrack { number: t.number, title: t.title.as_deref(), file: &t.file, raw_path: &t.raw_path }).collect(),
        accuraterip: ar,
        no_accuraterip: opts.no_accuraterip,
        notes: offset_notes.to_vec(),
        health,
        paranoia_reduced: opts.paranoia != engine::Paranoia::Full,
        hidden: hidden.clone(),
        hidden_sectors: hidden_len,
        gaps: found_gaps.to_vec(),
    });
    if let Err(e) = report::write(&report, dir) {
        eprintln!("Could not write the rip log: {e}");
    }
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

fn write_accuraterip_json(report: &accuraterip::Report, output_dir: &str) -> Result<(), Error> {
    let meta_dir = format!("{}/metadata", output_dir);
    std::fs::create_dir_all(&meta_dir)?;
    std::fs::write(format!("{}/accuraterip.json", meta_dir), serde_json::to_string_pretty(report)?)?;
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

fn musicbrainz_details(r: &mut ReleaseInfo, discid: Option<&str>, debug: bool) {
    mb_enrich::enrich(r, discid, debug);
}

/// Look at what was written: the format, bit depth and bitrate (so a rip can be checked at a
/// glance), and optionally measure loudness and add ReplayGain tags.
fn post_rip(opts: &RipOptions, outputs: &[String]) {
    use crate::library::audioinfo;
    let say = |m: &str| if opts.progress_json { emit_step(m) } else { eprintln!("{m}") };
    let facts: Vec<audioinfo::AudioFacts> = outputs.iter().filter_map(|p| audioinfo::facts(Path::new(p)).ok()).collect();
    if facts.is_empty() {
        return;
    }
    let s = audioinfo::summarize(&facts);
    let rate = s.avg_bitrate_kbps.map(|b| format!(" — average {:.0} kbps", b)).unwrap_or_default();
    let kind = if s.cd_quality { " (lossless, CD quality)" } else if s.lossless { " (lossless)" } else { "" };
    say(&format!("Quality: {}{}{}", s.description, rate, kind));
    if opts.progress_json {
        println!("{}", serde_json::json!({"type": "quality", "summary": s}));
    }

    if opts.dynamic_range {
        say("Measuring dynamic range (DR)...");
        let mut rows: Vec<audioinfo::TrackDrRow> = Vec::new();
        let mut measured: Vec<crate::library::dynrange::TrackDr> = Vec::new();
        for (p, f) in outputs.iter().zip(facts.iter()) {
            if let Ok(t) = crate::library::dynrange::measure(Path::new(p), f.channels.unwrap_or(2) as usize, f.sample_rate.unwrap_or(44_100) as usize) {
                rows.push(audioinfo::TrackDrRow { path: p.clone(), dr: t.dr });
                measured.push(t);
            }
        }
        if let Some(album) = crate::library::dynrange::album_dr(&measured) {
            for r in &rows {
                let _ = crate::library::dynrange::write_tags(Path::new(&r.path), r.dr, album);
            }
            say(&format!("Dynamic range: DR{album} — {}", crate::library::dynrange::verdict(album)));
            if opts.progress_json {
                println!("{}", serde_json::json!({"type": "dynamic_range", "album_dr": album, "verdict": crate::library::dynrange::verdict(album), "tracks": measured.iter().map(|t| t.dr).collect::<Vec<_>>()}));
            }
        }
    }

    if opts.replaygain {
        say("Measuring loudness for ReplayGain...");
        let paths: Vec<&Path> = outputs.iter().map(|p| Path::new(p.as_str())).collect();
        let tracks: Vec<Option<audioinfo::Loudness>> = paths.iter().map(|p| audioinfo::loudness(p).ok()).collect();
        match audioinfo::album_loudness(&paths) {
            Ok(album) => {
                let mut ok = 0;
                for (p, t) in paths.iter().zip(&tracks) {
                    if let Some(t) = t {
                        if audioinfo::write_replaygain(p, t, &album).is_ok() {
                            ok += 1;
                        }
                    }
                }
                say(&format!("ReplayGain written to {ok} file(s): album gain {:+.2} dB, album peak {:.3}", album.gain_db().unwrap_or(0.0), album.peak_linear().unwrap_or(0.0)));
                if opts.progress_json {
                    println!("{}", serde_json::json!({"type": "replaygain", "files": ok, "album_gain_db": album.gain_db(), "album_lufs": album.lufs, "album_peak": album.peak_linear()}));
                }
            }
            Err(e) => say(&format!("ReplayGain skipped: {e}")),
        }
    }
}
