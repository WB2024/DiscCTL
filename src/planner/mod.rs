pub mod discs;
pub mod split;

use crate::{
    error::Error,
    model::{
        disc::{DiscFormat, DiscGraph, Session},
        plan::{BurnPlan, BurnStep},
    },
};

/// How a disc of this format is written, in plain words.
///
/// Audio CDs are written with `cdrdao`, which always writes disc-at-once: the whole session in a
/// single uninterrupted pass from a table of contents, with no gaps added between tracks and
/// none of the run-in/run-out blocks that track-at-once writing leaves. A Blue Book disc keeps
/// that session open so a data session can follow in a second pass; the disc is closed after it.
pub fn write_mode(format: &DiscFormat) -> &'static str {
    match format {
        DiscFormat::RedBook => "Disc-at-once: the whole audio session is written in one pass, with no gaps added between tracks, and the disc is closed when it is done.",
        DiscFormat::BlueBook => "Audio session disc-at-once (kept open), then the data session is added as a second session and the disc is closed.",
        DiscFormat::DataCD => "The data session is written as one image and the disc is closed when it is done.",
        DiscFormat::DataDvd | DiscFormat::MusicDvd => "The disc image is written in one pass and the disc is closed when it is done.",
    }
}

pub fn plan(graph: &DiscGraph) -> Result<BurnPlan, Error> {
    validate(graph)?;
    let steps = build_steps(graph)?;
    Ok(BurnPlan {
        format: graph.format.to_string(),
        label: graph.label.clone(),
        speed: graph.speed,
        normalize: graph.normalize,
        write_mode: write_mode(&graph.format).to_string(),
        steps,
    })
}

pub fn validate(graph: &DiscGraph) -> Result<(), Error> {
    validate_structure(graph)?;
    validate_files(graph)
}

pub fn validate_structure(graph: &DiscGraph) -> Result<(), Error> {
    if let Some(x) = graph.speed {
        if !(1..=crate::backend::speed::MAX_X).contains(&x) {
            return Err(Error::validation(format!("Write speed must be between 1x and {}x (or left out for Auto), got {x}x", crate::backend::speed::MAX_X)));
        }
    }
    if let Some(n) = graph.normalize {
        if !matches!(graph.format, DiscFormat::RedBook | DiscFormat::BlueBook) {
            return Err(Error::validation("Normalizing is only available for audio CDs (redbook and bluebook)"));
        }
        if !(-30.0..=-6.0).contains(&n.target_lufs) {
            return Err(Error::validation(format!("The normalization target must be between -30 and -6 LUFS, got {}", n.target_lufs)));
        }
    }
    match graph.format {
        DiscFormat::RedBook => validate_redbook_structure(graph),
        DiscFormat::DataCD => validate_datacd_structure(graph),
        DiscFormat::BlueBook => validate_bluebook_structure(graph),
        DiscFormat::DataDvd => validate_datadvd_structure(graph),
        DiscFormat::MusicDvd => validate_musicdvd_structure(graph),
    }
}

fn validate_datadvd_structure(graph: &DiscGraph) -> Result<(), Error> {
    if graph.sessions.len() != 1 {
        return Err(Error::validation(format!(
            "A Data DVD requires exactly 1 data session, got {}",
            graph.sessions.len()
        )));
    }
    match &graph.sessions[0] {
        Session::Data(_) => Ok(()),
        Session::Audio(_) => Err(Error::validation("A Data DVD requires a data session")),
    }
}

/// A Music DVD is one audio session (the tracks) and, if wanted, one data session (extra files).
fn validate_musicdvd_structure(graph: &DiscGraph) -> Result<(), Error> {
    match graph.sessions.as_slice() {
        [Session::Audio(a)] | [Session::Audio(a), Session::Data(_)] => {
            if a.tracks.is_empty() {
                return Err(Error::validation("A Music DVD needs at least one audio track"));
            }
        }
        [Session::Data(_), ..] => {
            return Err(Error::validation("SESSION_ORDER_INVALID: a Music DVD lists its audio first, then any data"));
        }
        _ => {
            return Err(Error::validation("A Music DVD needs an audio session, optionally followed by one data session"));
        }
    }
    if let Some(opts) = &graph.dvd {
        opts.validate().map_err(Error::validation)?;
    }
    Ok(())
}

fn validate_redbook_structure(graph: &DiscGraph) -> Result<(), Error> {
    if graph.sessions.is_empty() {
        return Err(Error::validation("RedBook requires at least one audio session"));
    }
    if graph.sessions.len() > 1 {
        return Err(Error::validation("RedBook supports only a single session"));
    }
    match &graph.sessions[0] {
        Session::Audio(a) if a.tracks.is_empty() => {
            Err(Error::validation("Audio session has no tracks"))
        }
        Session::Audio(_) => Ok(()),
        Session::Data(_) => Err(Error::validation("RedBook format requires an audio session")),
    }
}

fn validate_datacd_structure(graph: &DiscGraph) -> Result<(), Error> {
    if graph.sessions.len() != 1 {
        return Err(Error::validation(format!(
            "DataCD requires exactly 1 data session, got {}",
            graph.sessions.len()
        )));
    }
    match &graph.sessions[0] {
        Session::Data(_) => Ok(()),
        Session::Audio(_) => Err(Error::validation("DataCD format requires a data session")),
    }
}

fn validate_bluebook_structure(graph: &DiscGraph) -> Result<(), Error> {
    if graph.sessions.len() != 2 {
        return Err(Error::validation(format!(
            "BlueBook (CD Extra) requires exactly 2 sessions (audio + data), got {}",
            graph.sessions.len()
        )));
    }
    match &graph.sessions[0] {
        Session::Data(_) => {
            return Err(Error::validation(
                "SESSION_ORDER_INVALID: Data session cannot precede audio session in BlueBook format",
            ));
        }
        Session::Audio(a) if a.tracks.is_empty() => {
            return Err(Error::validation("BlueBook session 1 (audio) has no tracks"));
        }
        Session::Audio(_) => {}
    }
    match &graph.sessions[1] {
        Session::Audio(_) => Err(Error::validation("BlueBook session 2 must be a data session")),
        Session::Data(_) => Ok(()),
    }
}

fn validate_files(graph: &DiscGraph) -> Result<(), Error> {
    for session in &graph.sessions {
        match session {
            Session::Audio(audio) => {
                for track_spec in &audio.tracks {
                    use crate::backend::source;
                    let src = source::parse(track_spec);
                    // A link or a chosen stream: audio CDs only, and links are checked here and
                    // fetched when the disc is burned.
                    if (source::is_url(src.location) || src.stream.is_some()) && graph.format.is_dvd() {
                        return Err(Error::validation(format!("'{track_spec}': links and audio stream choices are only available for audio CDs, not DVDs")));
                    }
                    if source::is_url(src.location) {
                        source::validate_url(src.location).map_err(Error::validation)?;
                        if let Some(n) = src.stream {
                            source::check_stream(src.location, n).map_err(Error::validation)?;
                        }
                        continue;
                    }
                    let track = src.location;
                    let path = std::path::Path::new(track);
                    if !path.exists() {
                        return Err(Error::validation(format!("Track file not found: {}", track)));
                    }
                    if let Some(n) = src.stream {
                        source::check_stream(track, n).map_err(Error::validation)?;
                    }
                    let dvd = graph.format.is_dvd();
                    match path.extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase()).as_deref() {
                        // A Music DVD re-encodes everything, so a WAV needn't be CD audio
                        Some("wav") if dvd => {}
                        Some("aiff") | Some("aif") | Some("ape") | Some("wv") if dvd => {}
                        Some("wav") => validate_wav_format(track)?,
                        // Video and other containers are fine when the user picked an audio stream from them
                        Some("mkv") | Some("mka") | Some("mp4") | Some("m4v") | Some("webm") | Some("mov") | Some("avi")
                        | Some("ts") | Some("m2ts") | Some("ogv") | Some("mpg") | Some("mpeg")
                            if src.stream.is_some() => {}
                        // All formats below are converted to CDDA WAV by the backend via ffmpeg
                        Some("flac") | Some("mp3") | Some("m4a") | Some("aac")
                        | Some("ogg") | Some("opus") | Some("wma") => {}
                        Some(ext) => {
                            return Err(Error::validation(format!(
                                "Unsupported audio format '{}' for: {}. \
                                 Supported: wav, flac, mp3, m4a, aac, ogg, opus",
                                ext, track
                            )));
                        }
                        None => {
                            return Err(Error::validation(format!(
                                "Track has no file extension: {}",
                                track
                            )));
                        }
                    }
                }
            }
            Session::Data(data) => {
                if !std::path::Path::new(&data.source_dir).exists() {
                    return Err(Error::validation(format!(
                        "Source directory not found: {}",
                        data.source_dir
                    )));
                }
                validate_iso_size(&data.source_dir)?;
            }
        }
    }
    Ok(())
}

// Standard PCM WAV fmt chunk layout (offsets from file start):
//   0-3: "RIFF", 4-7: file size, 8-11: "WAVE"
//   12-15: "fmt ", 16-19: chunk size (16 for PCM)
//   20-21: audio_format, 22-23: channels, 24-27: sample_rate
//   28-31: byte_rate, 32-33: block_align, 34-35: bits_per_sample
fn validate_wav_format(path: &str) -> Result<(), Error> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut header = [0u8; 36];
    f.read_exact(&mut header).map_err(|_| {
        Error::validation(format!("'{}' is too small to be a valid WAV file", path))
    })?;

    if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return Err(Error::validation(format!("'{}' is not a valid WAV file", path)));
    }
    if &header[12..16] != b"fmt " {
        return Err(Error::validation(format!(
            "'{}' has unexpected WAV structure (missing fmt chunk)",
            path
        )));
    }

    let audio_format = u16::from_le_bytes([header[20], header[21]]);
    let channels = u16::from_le_bytes([header[22], header[23]]);
    let sample_rate = u32::from_le_bytes([header[24], header[25], header[26], header[27]]);
    let bits_per_sample = u16::from_le_bytes([header[34], header[35]]);

    // 1 = PCM
    if audio_format != 1 {
        return Err(Error::validation(format!(
            "'{}' is not PCM audio (format code {}). Convert to 16-bit PCM WAV first.",
            path, audio_format
        )));
    }
    if channels != 2 {
        return Err(Error::validation(format!(
            "'{}' has {} channel(s); CDDA requires stereo (2 channels)",
            path, channels
        )));
    }
    if sample_rate != 44100 {
        return Err(Error::validation(format!(
            "'{}' has sample rate {}Hz; CDDA requires 44100Hz",
            path, sample_rate
        )));
    }
    if bits_per_sample != 16 {
        return Err(Error::validation(format!(
            "'{}' is {}-bit; CDDA requires 16-bit",
            path, bits_per_sample
        )));
    }

    Ok(())
}


fn validate_iso_size(source_dir: &str) -> Result<(), Error> {
    let total = dir_size(std::path::Path::new(source_dir))?;
    let limit = crate::backend::data::disc_capacity();
    if total > limit {
        return Err(Error::validation(format!(
            "Data directory '{}' is {:.1}MB, which exceeds the {:.0}MB disc limit. \
             Burning a Data CD splits big jobs across several discs automatically; \
             only a hand-written disc graph has to fit on one.",
            source_dir,
            total as f64 / 1024.0 / 1024.0,
            limit as f64 / 1024.0 / 1024.0,
        )));
    }
    Ok(())
}

fn dir_size(path: &std::path::Path) -> Result<u64, Error> {
    let mut total = 0u64;
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        // Follow symlinks: a staged folder is made of links to the real files.
        let meta = std::fs::metadata(entry.path())?;
        if meta.is_dir() {
            total += dir_size(&entry.path())?;
        } else {
            total += meta.len();
        }
    }
    Ok(total)
}

pub fn build_steps(graph: &DiscGraph) -> Result<Vec<BurnStep>, Error> {
    match graph.format {
        DiscFormat::DataDvd => {
            let filesystem = match &graph.sessions[0] {
                Session::Data(d) => d.filesystem.to_string(),
                _ => "iso9660".to_string(),
            };
            Ok(vec![
                BurnStep::AppendDataSession { session_index: 0, filesystem },
                BurnStep::FinalizeDisc,
            ])
        }
        DiscFormat::MusicDvd => Ok(vec![
            BurnStep::BurnMusicDvd {
                audio_session_index: 0,
                data_session_index: if graph.sessions.len() > 1 { Some(1) } else { None },
            },
            BurnStep::FinalizeDisc,
        ]),
        DiscFormat::RedBook => Ok(vec![BurnStep::BurnAudioSession {
            session_index: 0,
            finalize: true,
        }]),
        DiscFormat::DataCD => {
            let filesystem = match &graph.sessions[0] {
                Session::Data(d) => d.filesystem.to_string(),
                _ => "iso9660".to_string(),
            };
            Ok(vec![
                BurnStep::AppendDataSession {
                    session_index: 0,
                    filesystem,
                },
                BurnStep::FinalizeDisc,
            ])
        }
        DiscFormat::BlueBook => {
            let filesystem = match &graph.sessions[1] {
                Session::Data(d) => d.filesystem.to_string(),
                _ => "iso9660".to_string(),
            };
            Ok(vec![
                BurnStep::BurnAudioSession {
                    session_index: 0,
                    finalize: false,
                },
                BurnStep::AppendDataSession {
                    session_index: 1,
                    filesystem,
                },
                BurnStep::FinalizeDisc,
            ])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::disc::*;

    fn audio(tracks: Vec<&str>) -> Session {
        Session::Audio(AudioSession {
            tracks: tracks.iter().map(|s| s.to_string()).collect(),
            cd_text: None,
            track_titles: None,
        })
    }

    fn data(src: &str) -> Session {
        Session::Data(DataSession {
            source_dir: src.to_string(),
            filesystem: Filesystem::Iso9660,
            joliet: false,
            rock_ridge: false,
        })
    }

    fn graph(format: DiscFormat, sessions: Vec<Session>) -> DiscGraph {
        DiscGraph {
            format,
            label: "Test".to_string(),
            sessions,
            dvd: None,
            speed: None,
            normalize: None,
        }
    }

    #[test]
    fn redbook_valid_structure() {
        let g = graph(DiscFormat::RedBook, vec![audio(vec!["t.wav"])]);
        assert!(validate_structure(&g).is_ok());
    }

    #[test]
    fn redbook_rejects_empty_sessions() {
        let g = graph(DiscFormat::RedBook, vec![]);
        assert!(validate_structure(&g).is_err());
    }

    #[test]
    fn redbook_rejects_data_session() {
        let g = graph(DiscFormat::RedBook, vec![data("/tmp")]);
        assert!(validate_structure(&g).is_err());
    }

    #[test]
    fn redbook_rejects_empty_tracks() {
        let g = graph(DiscFormat::RedBook, vec![audio(vec![])]);
        assert!(validate_structure(&g).is_err());
    }

    #[test]
    fn datacd_valid_structure() {
        let g = graph(DiscFormat::DataCD, vec![data("/tmp")]);
        assert!(validate_structure(&g).is_ok());
    }

    #[test]
    fn datacd_rejects_audio() {
        let g = graph(DiscFormat::DataCD, vec![audio(vec!["t.wav"])]);
        assert!(validate_structure(&g).is_err());
    }

    #[test]
    fn bluebook_valid_structure() {
        let g = graph(
            DiscFormat::BlueBook,
            vec![audio(vec!["t.wav"]), data("/tmp")],
        );
        assert!(validate_structure(&g).is_ok());
    }

    #[test]
    fn bluebook_rejects_wrong_order() {
        let g = graph(
            DiscFormat::BlueBook,
            vec![data("/tmp"), audio(vec!["t.wav"])],
        );
        let err = validate_structure(&g).unwrap_err();
        assert!(err.to_string().contains("SESSION_ORDER_INVALID"));
    }

    #[test]
    fn bluebook_rejects_single_session() {
        let g = graph(DiscFormat::BlueBook, vec![audio(vec!["t.wav"])]);
        assert!(validate_structure(&g).is_err());
    }

    #[test]
    fn bluebook_rejects_two_audio_sessions() {
        let g = graph(
            DiscFormat::BlueBook,
            vec![audio(vec!["t.wav"]), audio(vec!["t2.wav"])],
        );
        assert!(validate_structure(&g).is_err());
    }

    #[test]
    fn redbook_generates_one_step() {
        let g = graph(DiscFormat::RedBook, vec![audio(vec!["t.wav"])]);
        let steps = build_steps(&g).unwrap();
        assert_eq!(steps.len(), 1);
        assert!(matches!(
            &steps[0],
            BurnStep::BurnAudioSession { finalize: true, .. }
        ));
    }

    #[test]
    fn datacd_generates_two_steps() {
        let g = graph(DiscFormat::DataCD, vec![data("/tmp")]);
        let steps = build_steps(&g).unwrap();
        assert_eq!(steps.len(), 2);
        assert!(matches!(&steps[0], BurnStep::AppendDataSession { .. }));
        assert!(matches!(&steps[1], BurnStep::FinalizeDisc));
    }

    #[test]
    fn bluebook_generates_three_steps() {
        let g = graph(
            DiscFormat::BlueBook,
            vec![audio(vec!["t.wav"]), data("/tmp")],
        );
        let steps = build_steps(&g).unwrap();
        assert_eq!(steps.len(), 3);
        assert!(matches!(
            &steps[0],
            BurnStep::BurnAudioSession { finalize: false, .. }
        ));
        assert!(matches!(&steps[1], BurnStep::AppendDataSession { .. }));
        assert!(matches!(&steps[2], BurnStep::FinalizeDisc));
    }

    // WAV format validation tests

    #[test]
    fn wav_rejects_missing_file() {
        let err = validate_wav_format("/tmp/does_not_exist_discctl.wav").unwrap_err();
        assert!(matches!(err, Error::Io(_)));
    }

    #[test]
    fn wav_rejects_wrong_sample_rate() {
        let path = write_test_wav("/tmp/discctl_test_48k.wav", 1, 2, 48000, 16);
        let err = validate_wav_format(&path).unwrap_err();
        assert!(err.to_string().contains("44100Hz"), "{}", err);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn wav_rejects_mono() {
        let path = write_test_wav("/tmp/discctl_test_mono.wav", 1, 1, 44100, 16);
        let err = validate_wav_format(&path).unwrap_err();
        assert!(err.to_string().contains("stereo"), "{}", err);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn wav_rejects_24bit() {
        let path = write_test_wav("/tmp/discctl_test_24bit.wav", 1, 2, 44100, 24);
        let err = validate_wav_format(&path).unwrap_err();
        assert!(err.to_string().contains("16-bit"), "{}", err);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn wav_accepts_cdda_spec() {
        let path = write_test_wav("/tmp/discctl_test_cdda.wav", 1, 2, 44100, 16);
        assert!(validate_wav_format(&path).is_ok());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn iso_size_accepts_small_dir() {
        let dir = std::env::temp_dir().join("discctl_test_iso");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("file.txt"), b"hello").unwrap();
        assert!(validate_iso_size(dir.to_str().unwrap()).is_ok());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn iso_size_rejects_missing_dir() {
        assert!(validate_iso_size("/tmp/discctl_no_such_dir_xyz").is_err());
    }

    /// Writes a minimal 36-byte WAV fmt-only header (no data chunk) for testing.
    fn write_test_wav(path: &str, audio_format: u16, channels: u16, sample_rate: u32, bits: u16) -> String {
        let byte_rate = sample_rate * channels as u32 * bits as u32 / 8;
        let block_align = channels * bits / 8;
        let mut h = Vec::with_capacity(36);
        h.extend_from_slice(b"RIFF");
        h.extend_from_slice(&28u32.to_le_bytes()); // 36 - 8
        h.extend_from_slice(b"WAVE");
        h.extend_from_slice(b"fmt ");
        h.extend_from_slice(&16u32.to_le_bytes()); // PCM fmt chunk size
        h.extend_from_slice(&audio_format.to_le_bytes());
        h.extend_from_slice(&channels.to_le_bytes());
        h.extend_from_slice(&sample_rate.to_le_bytes());
        h.extend_from_slice(&byte_rate.to_le_bytes());
        h.extend_from_slice(&block_align.to_le_bytes());
        h.extend_from_slice(&bits.to_le_bytes());
        assert_eq!(h.len(), 36);
        std::fs::write(path, &h).unwrap();
        path.to_string()
    }
}
