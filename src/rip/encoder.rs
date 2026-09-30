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
    Aac,
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
            AudioFormat::Aac      => "m4a",
        }
    }

    pub fn is_lossless(&self) -> bool {
        matches!(self, AudioFormat::Wav | AudioFormat::Flac | AudioFormat::Alac | AudioFormat::Aiff)
    }

    /// Short id used on the command line and in settings.
    pub fn id(&self) -> &'static str {
        match self {
            AudioFormat::Wav => "wav",
            AudioFormat::Flac => "flac",
            AudioFormat::Alac => "alac",
            AudioFormat::Aiff => "aiff",
            AudioFormat::OggVorbis => "ogg",
            AudioFormat::Mp3 => "mp3",
            AudioFormat::Opus => "opus",
            AudioFormat::Aac => "aac",
        }
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
            AudioFormat::Aac      => write!(f, "AAC"),
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
            "aac" | "m4a" => Ok(AudioFormat::Aac),
            other => Err(format!(
                "Unknown audio format '{}'. Valid: wav, flac, alac, aiff, ogg, mp3, opus, aac", other
            )),
        }
    }
}

// ── Quality ───────────────────────────────────────────────────────────────────

/// One choice offered for a format's quality: (id, label, what it means).
pub struct QualityChoice {
    pub id: &'static str,
    pub label: &'static str,
    pub note: &'static str,
}

const fn qc(id: &'static str, label: &'static str, note: &'static str) -> QualityChoice {
    QualityChoice { id, label, note }
}

/// The quality choices for a format. The first is the best (and the default).
pub fn quality_choices(format: &AudioFormat) -> Vec<QualityChoice> {
    match format {
        AudioFormat::Flac => vec![
            qc("8", "Best compression (level 8)", "Smallest lossless files; still bit-perfect. The default."),
            qc("12", "Maximum compression (level 12)", "A little smaller again, but slower to encode."),
            qc("5", "Standard (level 5)", "Faster encoding, files a touch larger."),
            qc("0", "Fastest (level 0)", "Quickest encoding, largest files. Still lossless."),
        ],
        AudioFormat::Alac => vec![qc("", "Lossless", "Apple Lossless: bit-perfect, plays everywhere Apple does.")],
        AudioFormat::Wav => vec![qc("", "Uncompressed", "The disc's audio exactly as read (16-bit / 44.1 kHz). Big files, tags limited.")],
        AudioFormat::Aiff => vec![qc("", "Uncompressed", "Like WAV, in Apple's container (16-bit / 44.1 kHz).")],
        AudioFormat::Mp3 => vec![
            qc("v0", "V0 · best (~245 kbps VBR)", "Transparent for nearly everyone. The default."),
            qc("cbr320", "320 kbps constant", "Highest MP3 bitrate; bigger than V0 with no audible gain."),
            qc("v2", "V2 (~190 kbps VBR)", "Very good, noticeably smaller."),
            qc("cbr192", "192 kbps constant", "Good for portable use."),
            qc("cbr128", "128 kbps constant", "Small; audibly lossy."),
        ],
        AudioFormat::Aac => vec![
            qc("320", "320 kbps", "Highest AAC quality."),
            qc("256", "256 kbps", "Transparent for nearly everyone (iTunes Plus)."),
            qc("192", "192 kbps", "Very good."),
            qc("128", "128 kbps", "Small."),
        ],
        AudioFormat::Opus => vec![
            qc("320", "320 kbps", "Maximum; far beyond what Opus needs. The default."),
            qc("192", "192 kbps", "Transparent."),
            qc("128", "128 kbps", "Very good, small."),
            qc("96", "96 kbps", "Good for portable use."),
        ],
        AudioFormat::OggVorbis => vec![
            qc("10", "Quality 10 (~500 kbps)", "Highest Vorbis quality. The default."),
            qc("8", "Quality 8 (~256 kbps)", "Excellent."),
            qc("6", "Quality 6 (~192 kbps)", "Very good."),
            qc("4", "Quality 4 (~128 kbps)", "Good, small."),
        ],
    }
}

/// Encoder arguments for a quality choice (`None` = the best).
fn quality_args(format: &AudioFormat, quality: Option<&str>) -> Result<Vec<String>, String> {
    let choices = quality_choices(format);
    let q = quality.map(str::trim).filter(|q| !q.is_empty()).unwrap_or(choices[0].id);
    if !choices.iter().any(|c| c.id == q) {
        return Err(format!("'{q}' isn't a quality choice for {format}. Choose one of: {}", choices.iter().map(|c| c.id).collect::<Vec<_>>().join(", ")));
    }
    let a = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    Ok(match format {
        AudioFormat::Flac => a(&["-compression_level", q]),
        AudioFormat::Mp3 => match q.strip_prefix("cbr") {
            Some(k) => a(&["-b:a", &format!("{k}k")]),
            None => a(&["-q:a", q.trim_start_matches('v')]),
        },
        AudioFormat::Aac => a(&["-b:a", &format!("{q}k")]),
        AudioFormat::Opus => a(&["-b:a", &format!("{q}k")]),
        AudioFormat::OggVorbis => a(&["-q:a", q]),
        AudioFormat::Alac | AudioFormat::Wav | AudioFormat::Aiff => Vec::new(),
    })
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
    quality: Option<&str>,
    debug: bool,
) -> Result<(), Error> {
    let qargs = quality_args(format, quality).map_err(Error::validation)?;
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
            cmd.arg("-c:a").arg("flac").args(&qargs);
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
            cmd.arg("-c:a").arg("libvorbis").args(&qargs);
            if embed_art {
                cmd.arg("-c:v").arg("copy");
            }
        }
        AudioFormat::Mp3 => {
            cmd.arg("-c:a").arg("libmp3lame").args(&qargs);
            if embed_art {
                cmd.arg("-c:v").arg("copy")
                   .arg("-metadata:s:v").arg("title=Album cover")
                   .arg("-metadata:s:v").arg("comment=Cover (front)");
            }
        }
        AudioFormat::Opus => {
            cmd.arg("-c:a").arg("libopus").args(&qargs);
            // Opus cover art via ffmpeg is unreliable; skip embedding
        }
        AudioFormat::Aac => {
            cmd.arg("-c:a").arg("aac").args(&qargs);
            if embed_art {
                cmd.arg("-c:v").arg("copy").arg("-disposition:v").arg("attached_pic");
            }
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
    // The MusicBrainz IDs and the rest of the tags are written afterwards by `tagging::apply`,
    // which gives each format its own native frame or field.

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

#[cfg(test)]
mod quality_tests {
    use super::*;
    use crate::library::audioinfo;

    #[test]
    fn quality_choices_change_the_encoded_output() {
        if std::process::Command::new("ffmpeg").arg("-version").output().is_err() {
            return;
        }
        let d = std::env::temp_dir().join(format!("rd_quality_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let wav = d.join("in.wav");
        // noise-like content, so bitrates differ clearly between settings
        assert!(std::process::Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", "anoisesrc=d=6:c=pink:r=44100:a=0.3", "-ac", "2", "-ar", "44100", "-sample_fmt", "s16"]).arg(&wav).status().unwrap().success());
        let enc = |fmt: AudioFormat, q: Option<&str>| -> audioinfo::AudioFacts {
            let out = d.join(format!("out_{}_{}.{}", fmt.id(), q.unwrap_or("best"), fmt.extension()));
            encode(wav.to_str().unwrap(), out.to_str().unwrap(), &fmt, &TrackTags::default(), None, q, false).unwrap_or_else(|e| panic!("{fmt} {q:?}: {e}"));
            audioinfo::facts(&out).unwrap()
        };
        let (f0, f12) = (enc(AudioFormat::Flac, Some("0")), enc(AudioFormat::Flac, Some("12")));
        assert!(f12.size <= f0.size && f12.lossless && f12.bit_depth == Some(16), "{} vs {}", f12.size, f0.size);
        let (m128, m320) = (enc(AudioFormat::Mp3, Some("cbr128")), enc(AudioFormat::Mp3, Some("cbr320")));
        assert!((115.0..140.0).contains(&m128.bitrate_kbps.unwrap()) && m320.bitrate_kbps.unwrap() > 290.0, "{:?} {:?}", m128.bitrate_kbps, m320.bitrate_kbps);
        assert!(enc(AudioFormat::Mp3, Some("v0")).bitrate_kbps.unwrap() > enc(AudioFormat::Mp3, Some("v2")).bitrate_kbps.unwrap());
        let (a128, a256) = (enc(AudioFormat::Aac, Some("128")), enc(AudioFormat::Aac, Some("256")));
        assert_eq!(a128.label, "AAC");
        assert!(a256.bitrate_kbps.unwrap() > a128.bitrate_kbps.unwrap() * 1.5);
        assert!(enc(AudioFormat::Opus, Some("192")).bitrate_kbps.unwrap() > enc(AudioFormat::Opus, Some("96")).bitrate_kbps.unwrap());
        assert!(enc(AudioFormat::OggVorbis, Some("10")).bitrate_kbps.unwrap() > enc(AudioFormat::OggVorbis, Some("4")).bitrate_kbps.unwrap());
        let alac = enc(AudioFormat::Alac, None);
        assert!(alac.lossless && alac.bit_depth == Some(16));
        // the default is the best, and a nonsense choice is refused
        assert_eq!(quality_choices(&AudioFormat::Flac)[0].id, "8");
        assert!(encode(wav.to_str().unwrap(), d.join("x.flac").to_str().unwrap(), &AudioFormat::Flac, &TrackTags::default(), None, Some("99"), false).is_err());
        assert!(encode(wav.to_str().unwrap(), d.join("x.mp3").to_str().unwrap(), &AudioFormat::Mp3, &TrackTags::default(), None, Some("v99"), false).is_err());
        std::fs::remove_dir_all(&d).ok();
    }
}
