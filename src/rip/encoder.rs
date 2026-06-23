use std::process::Command;
use std::str::FromStr;
use crate::error::Error;

#[derive(Debug, Clone, PartialEq)]
pub enum AudioFormat {
    Wav,
    Flac,
    Alac,
    Aiff,
    OggVorbis,
    Mp3,
    Opus,
}

impl AudioFormat {
    pub fn extension(&self) -> &'static str {
        match self {
            AudioFormat::Wav      => "wav",
            AudioFormat::Flac     => "flac",
            AudioFormat::Alac     => "m4a",
            AudioFormat::Aiff     => "aiff",
            AudioFormat::OggVorbis => "ogg",
            AudioFormat::Mp3      => "mp3",
            AudioFormat::Opus     => "opus",
        }
    }

    pub fn is_lossless(&self) -> bool {
        matches!(self, AudioFormat::Wav | AudioFormat::Flac | AudioFormat::Alac | AudioFormat::Aiff)
    }
}

impl std::fmt::Display for AudioFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AudioFormat::Wav      => write!(f, "WAV"),
            AudioFormat::Flac     => write!(f, "FLAC"),
            AudioFormat::Alac     => write!(f, "ALAC"),
            AudioFormat::Aiff     => write!(f, "AIFF"),
            AudioFormat::OggVorbis => write!(f, "OGG Vorbis"),
            AudioFormat::Mp3      => write!(f, "MP3"),
            AudioFormat::Opus     => write!(f, "Opus"),
        }
    }
}

impl FromStr for AudioFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "wav"        => Ok(AudioFormat::Wav),
            "flac"       => Ok(AudioFormat::Flac),
            "alac"       => Ok(AudioFormat::Alac),
            "aiff"       => Ok(AudioFormat::Aiff),
            "ogg" | "vorbis" | "ogg-vorbis" => Ok(AudioFormat::OggVorbis),
            "mp3"        => Ok(AudioFormat::Mp3),
            "opus"       => Ok(AudioFormat::Opus),
            other => Err(format!(
                "Unknown audio format '{}'. Valid: wav, flac, alac, aiff, ogg, mp3, opus", other
            )),
        }
    }
}

/// Metadata tags to embed in the encoded file.
#[derive(Debug, Default, Clone)]
pub struct TrackTags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_number: Option<usize>,
    pub track_total: Option<usize>,
    pub songwriter: Option<String>,
    pub composer: Option<String>,
    pub year: Option<String>,
    pub mb_release_id: Option<String>,
    pub mb_recording_id: Option<String>,
    pub mb_artist_id: Option<String>,
}

/// Encode a raw CDDA WAV file to the target format.
///
/// `input_wav` — the source WAV produced by cdparanoia (44.1kHz/16-bit/stereo PCM)
/// `output_path` — full path for the encoded output file
/// `format` — target codec
/// `tags` — optional metadata tags to embed
/// `cover_art` — optional path to a cover image to embed
pub fn encode(
    input_wav: &str,
    output_path: &str,
    format: &AudioFormat,
    tags: &TrackTags,
    cover_art: Option<&str>,
    debug: bool,
) -> Result<(), Error> {
    if *format == AudioFormat::Wav {
        std::fs::copy(input_wav, output_path)?;
        return Ok(());
    }

    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y")
       .arg("-i").arg(input_wav);

    // Add cover art input if available (not supported for AIFF)
    let embed_art = cover_art.is_some() && !matches!(format, AudioFormat::Aiff);
    if let Some(art) = cover_art {
        if embed_art {
            cmd.arg("-i").arg(art);
        }
    }

    // Stream mapping: audio from input 0, cover art from input 1 (if present)
    if embed_art {
        cmd.arg("-map").arg("0:a").arg("-map").arg("1:v");
    }

    // Codec flags per format
    match format {
        AudioFormat::Flac => {
            cmd.arg("-c:a").arg("flac")
               .arg("-compression_level").arg("8");
            if embed_art {
                cmd.arg("-c:v").arg("copy")
                   .arg("-metadata:s:v").arg("title=Album cover")
                   .arg("-metadata:s:v").arg("comment=Cover (front)");
            }
        }
        AudioFormat::Alac => {
            cmd.arg("-c:a").arg("alac");
            if embed_art {
                cmd.arg("-c:v").arg("copy");
            }
        }
        AudioFormat::Aiff => {
            cmd.arg("-f").arg("aiff")
               .arg("-c:a").arg("pcm_s16be");
            // AIFF cover art embedding is not reliably supported by ffmpeg
        }
        AudioFormat::OggVorbis => {
            cmd.arg("-c:a").arg("libvorbis")
               .arg("-q:a").arg("10");
            if embed_art {
                cmd.arg("-c:v").arg("copy");
            }
        }
        AudioFormat::Mp3 => {
            cmd.arg("-c:a").arg("libmp3lame")
               .arg("-q:a").arg("0");
            if embed_art {
                cmd.arg("-c:v").arg("copy")
                   .arg("-metadata:s:v").arg("title=Album cover")
                   .arg("-metadata:s:v").arg("comment=Cover (front)");
            }
        }
        AudioFormat::Opus => {
            cmd.arg("-c:a").arg("libopus")
               .arg("-b:a").arg("320k");
            // Opus cover art via ffmpeg is unreliable; skip embedding
        }
        AudioFormat::Wav => unreachable!(),
    }

    // Embed metadata tags
    if let Some(ref v) = tags.title         { cmd.arg("-metadata").arg(format!("title={}", v)); }
    if let Some(ref v) = tags.artist        { cmd.arg("-metadata").arg(format!("artist={}", v)); }
    if let Some(ref v) = tags.album         { cmd.arg("-metadata").arg(format!("album={}", v)); }
    if let Some(ref v) = tags.album_artist  { cmd.arg("-metadata").arg(format!("album_artist={}", v)); }
    if let Some(ref v) = tags.songwriter    { cmd.arg("-metadata").arg(format!("composer={}", v)); }
    if let Some(ref v) = tags.composer      { cmd.arg("-metadata").arg(format!("composer={}", v)); }
    if let Some(ref v) = tags.year          { cmd.arg("-metadata").arg(format!("date={}", v)); }
    if let Some(n) = tags.track_number {
        let tag = if let Some(total) = tags.track_total {
            format!("{}/{}", n, total)
        } else {
            n.to_string()
        };
        cmd.arg("-metadata").arg(format!("track={}", tag));
    }
    // MusicBrainz identifiers — stored as custom tags; most players / taggers
    // recognise these field names (Picard convention).
    if let Some(ref v) = tags.mb_recording_id {
        cmd.arg("-metadata").arg(format!("MUSICBRAINZ_TRACKID={}", v));
    }
    if let Some(ref v) = tags.mb_release_id {
        cmd.arg("-metadata").arg(format!("MUSICBRAINZ_ALBUMID={}", v));
    }
    if let Some(ref v) = tags.mb_artist_id {
        cmd.arg("-metadata").arg(format!("MUSICBRAINZ_ARTISTID={}", v));
    }

    cmd.arg(output_path);

    if debug { eprintln!("Running: {:?}", cmd); }

    let output = cmd.output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::backend(format!(
            "ffmpeg encode to {} failed: {}",
            format,
            stderr.lines().last().unwrap_or("unknown error")
        )));
    }

    Ok(())
}

/// Build a safe filename for a track.
/// Format: `01. Artist - Album - Title.ext`
pub fn track_filename(
    number: usize,
    total: usize,
    artist: Option<&str>,
    album: Option<&str>,
    title: Option<&str>,
    ext: &str,
) -> String {
    let width = if total >= 100 { 3 } else { 2 };
    let prefix = format!("{:0width$}.", number, width = width);

    let sanitise = |s: &str| -> String {
        s.chars()
            .map(|c| if c.is_ascii_alphanumeric() || " .-_'&()!,".contains(c) { c } else { '_' })
            .collect::<String>()
            .trim()
            .to_string()
    };

    match (artist, album, title) {
        (Some(ar), Some(al), Some(ti)) => {
            format!("{} {} - {} - {}.{}", prefix, sanitise(ar), sanitise(al), sanitise(ti), ext)
        }
        (Some(ar), None, Some(ti)) => {
            format!("{} {} - {}.{}", prefix, sanitise(ar), sanitise(ti), ext)
        }
        (None, Some(al), Some(ti)) => {
            format!("{} {} - {}.{}", prefix, sanitise(al), sanitise(ti), ext)
        }
        (_, _, Some(ti)) => {
            format!("{} {}.{}", prefix, sanitise(ti), ext)
        }
        _ => format!("{} Track {}.{}", prefix, number, ext),
    }
}
