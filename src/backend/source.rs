//! Where a track's audio comes from.
//!
//! A track in a disc graph is a string. Besides a plain path it can be:
//!
//! * a **link**, `https://host/track.mp3`, fetched to a temporary file before it is burned;
//! * either of those with a **stream choice**, `movie.mkv#stream=1` (the second audio stream),
//!   for files that carry more than one (languages, commentary, stereo and surround mixes).
//!
//! The choice counts audio streams only, from 0, in the order the file lists them. Without one,
//! ffmpeg picks, as it always has.

use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use serde::Serialize;

use crate::error::Error;

/// The most a downloaded track may weigh. A CD holds 700 MB, and this also stops an endless
/// stream from filling the disk.
pub const MAX_DOWNLOAD_BYTES: u64 = 700 * 1024 * 1024;
const FETCH_TIMEOUT: Duration = Duration::from_secs(600);

/// A track string taken apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source<'a> {
    /// The path or link, without any stream choice.
    pub location: &'a str,
    /// Which audio stream to use (counting audio streams from 0), if the user chose one.
    pub stream: Option<usize>,
}

/// Split `location#stream=N` (or `#audio=N`) into its parts.
pub fn parse(s: &str) -> Source<'_> {
    if let Some((loc, frag)) = s.rsplit_once('#') {
        let n = frag.strip_prefix("stream=").or_else(|| frag.strip_prefix("audio="));
        if let Some(n) = n.and_then(|n| n.parse::<usize>().ok()) {
            return Source { location: loc, stream: Some(n) };
        }
    }
    Source { location: s, stream: None }
}

/// The track string for a location and stream choice.
pub fn compose(location: &str, stream: Option<usize>) -> String {
    match stream {
        Some(n) => format!("{location}#stream={n}"),
        None => location.to_string(),
    }
}

pub fn is_url(s: &str) -> bool {
    let l = s.trim_start().to_lowercase();
    l.starts_with("http://") || l.starts_with("https://")
}

/// Is this a link we are willing to fetch?
pub fn validate_url(u: &str) -> Result<(), String> {
    if !is_url(u) {
        return Err(format!("'{u}' is not an http or https link"));
    }
    if u.len() > 2048 || u.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!("'{u}' is not a valid link"));
    }
    let rest = u.split_once("://").map(|(_, r)| r).unwrap_or("");
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or(host);
    if host.trim_matches(|c| c == ':' || c == '.').is_empty() {
        return Err(format!("'{u}' has no host name"));
    }
    if rest.split(['?', '#']).next().unwrap_or("").to_lowercase().ends_with(".m3u8") {
        return Err("That is a streaming playlist (HLS), which has no fixed length. Use a link to a single audio file.".into());
    }
    Ok(())
}

// ── Audio streams in a file ──────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AudioStream {
    /// Position among the file's audio streams, from 0: what a stream choice refers to.
    pub ordinal: usize,
    /// ffmpeg's index of the stream in the file.
    pub index: usize,
    pub codec: String,
    pub channels: Option<u32>,
    pub sample_rate: Option<u32>,
    pub language: Option<String>,
    pub title: Option<String>,
    pub default: bool,
}

impl AudioStream {
    /// A short description for a menu.
    pub fn label(&self) -> String {
        let mut parts = vec![format!("{}", self.codec.to_uppercase())];
        if let Some(c) = self.channels {
            parts.push(match c {
                1 => "mono".into(),
                2 => "stereo".into(),
                6 => "5.1".into(),
                8 => "7.1".into(),
                n => format!("{n} ch"),
            });
        }
        if let Some(l) = &self.language {
            parts.push(l.clone());
        }
        if let Some(t) = &self.title {
            parts.push(format!("“{t}”"));
        }
        if self.default {
            parts.push("default".into());
        }
        parts.join(" · ")
    }
}

pub fn parse_streams(json: &str) -> Vec<AudioStream> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return Vec::new() };
    let Some(arr) = v["streams"].as_array() else { return Vec::new() };
    arr.iter()
        .filter(|s| s["codec_type"].as_str() == Some("audio"))
        .enumerate()
        .map(|(ordinal, s)| AudioStream {
            ordinal,
            index: s["index"].as_u64().unwrap_or(ordinal as u64) as usize,
            codec: s["codec_name"].as_str().unwrap_or("audio").to_string(),
            channels: s["channels"].as_u64().map(|n| n as u32),
            sample_rate: s["sample_rate"].as_str().and_then(|n| n.parse().ok()),
            language: s["tags"]["language"].as_str().filter(|l| *l != "und").map(String::from),
            title: s["tags"]["title"].as_str().map(String::from),
            default: s["disposition"]["default"].as_u64() == Some(1),
        })
        .collect()
}

/// The audio streams in a file or link, by ffprobe.
pub fn probe_streams(location: &str) -> Result<Vec<AudioStream>, String> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-show_streams", "-select_streams", "a", "-of", "json"])
        .arg(location)
        .output()
        .map_err(|e| format!("ffprobe isn't available ({e})"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).lines().next().unwrap_or("ffprobe could not read it").trim().to_string());
    }
    Ok(parse_streams(&String::from_utf8_lossy(&out.stdout)))
}

/// Complain if a stream choice points past the streams the file has.
pub fn check_stream(location: &str, stream: usize) -> Result<(), String> {
    match probe_streams(location) {
        // Not being able to look is not a reason to refuse; the conversion will say if it fails.
        Err(_) => Ok(()),
        Ok(s) if stream < s.len() => Ok(()),
        Ok(s) => Err(format!("'{location}' has {} audio stream{}, so stream {stream} doesn't exist (they count from 0)", s.len(), if s.len() == 1 { "" } else { "s" })),
    }
}

// ── Fetching a link ──────────────────────────────────────────────────────────

fn extension_for(url: &str, content_type: Option<&str>) -> &'static str {
    const KNOWN: [&str; 9] = ["mp3", "flac", "wav", "m4a", "aac", "ogg", "opus", "wma", "aiff"];
    let path = url.split(['?', '#']).next().unwrap_or(url);
    if let Some(e) = path.rsplit('.').next().map(|e| e.to_lowercase()) {
        if let Some(k) = KNOWN.iter().find(|k| **k == e) {
            return k;
        }
    }
    match content_type.map(|c| c.split(';').next().unwrap_or("").trim().to_lowercase()).as_deref() {
        Some("audio/mpeg") | Some("audio/mp3") => "mp3",
        Some("audio/flac") | Some("audio/x-flac") => "flac",
        Some("audio/wav") | Some("audio/x-wav") | Some("audio/wave") => "wav",
        Some("audio/mp4") | Some("audio/x-m4a") => "m4a",
        Some("audio/aac") => "aac",
        Some("audio/ogg") | Some("application/ogg") => "ogg",
        Some("audio/opus") => "opus",
        _ => "bin", // ffmpeg works out the format from the content
    }
}

/// Download a link to a temporary file, refusing anything that isn't a finite audio file of
/// reasonable size.
pub fn fetch(url: &str, debug: bool) -> Result<PathBuf, Error> {
    validate_url(url).map_err(Error::validation)?;
    if debug {
        eprintln!("Fetching {url}");
    }
    let resp = ureq::get(url)
        .timeout(FETCH_TIMEOUT)
        .set("User-Agent", concat!("RustyDisc/", env!("CARGO_PKG_VERSION")))
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(code, _) => Error::backend(format!("The link answered {code}: {url}")),
            other => Error::backend(format!("Could not fetch {url}: {other}")),
        })?;
    let ctype = resp.header("Content-Type").map(String::from);
    if ctype.as_deref().is_some_and(|c| c.to_lowercase().starts_with("text/")) {
        return Err(Error::validation(format!("{url} is a web page, not an audio file")));
    }
    if let Some(len) = resp.header("Content-Length").and_then(|l| l.parse::<u64>().ok()) {
        if len > MAX_DOWNLOAD_BYTES {
            return Err(Error::validation(format!("{url} is {} MB, more than the {} MB a track may be", len / 1_048_576, MAX_DOWNLOAD_BYTES / 1_048_576)));
        }
    }
    let ext = extension_for(url, ctype.as_deref());
    let dest = {
        use sha2::{Digest, Sha256};
        let h = hex::encode(&Sha256::digest(url.as_bytes())[..8]);
        PathBuf::from(format!("/tmp/discctl_dl_{}_{h}.{ext}", std::process::id()))
    };
    let mut file = std::fs::File::create(&dest)?;
    let mut reader = resp.into_reader().take(MAX_DOWNLOAD_BYTES + 1);
    let copied = std::io::copy(&mut reader, &mut file).map_err(|e| {
        let _ = std::fs::remove_file(&dest);
        Error::backend(format!("The download of {url} was interrupted: {e}"))
    })?;
    file.flush()?;
    if copied > MAX_DOWNLOAD_BYTES {
        let _ = std::fs::remove_file(&dest);
        return Err(Error::validation(format!("{url} is larger than {} MB; a live or endless stream can't be burned", MAX_DOWNLOAD_BYTES / 1_048_576)));
    }
    if copied == 0 {
        let _ = std::fs::remove_file(&dest);
        return Err(Error::backend(format!("{url} returned nothing")));
    }
    Ok(dest)
}

/// Does a downloaded file hold audio? (A wrong link often returns something else.)
pub fn has_audio(path: &Path) -> bool {
    probe_streams(&path.to_string_lossy()).map(|s| !s.is_empty()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write as _, net::TcpListener, thread};

    #[test]
    fn splits_the_stream_choice_off() {
        assert_eq!(parse("a.mp3"), Source { location: "a.mp3", stream: None });
        assert_eq!(parse("movie.mkv#stream=2"), Source { location: "movie.mkv", stream: Some(2) });
        assert_eq!(parse("movie.mkv#audio=0"), Source { location: "movie.mkv", stream: Some(0) });
        assert_eq!(parse("https://x.org/a.mp3#stream=1"), Source { location: "https://x.org/a.mp3", stream: Some(1) });
        assert_eq!(parse("a#b.mp3"), Source { location: "a#b.mp3", stream: None }, "other # signs are part of the name");
        assert_eq!(parse("a.mkv#stream=x").stream, None);
        assert_eq!(compose("a.mkv", Some(1)), "a.mkv#stream=1");
        assert_eq!(compose("a.mkv", None), "a.mkv");
    }

    #[test]
    fn links_are_checked() {
        for ok in ["https://example.org/a.mp3", "http://192.168.1.5:8000/x.flac?token=1"] {
            assert!(validate_url(ok).is_ok(), "{ok}");
        }
        for bad in ["ftp://example.org/a.mp3", "https://", "https:///a.mp3", "https://exa mple.org/a.mp3", "file:///etc/passwd", "https://example.org/live/playlist.m3u8"] {
            assert!(validate_url(bad).is_err(), "{bad}");
        }
        assert!(is_url("HTTPS://Example.org/a.mp3"));
        assert!(!is_url("/music/a.mp3"));
    }

    #[test]
    fn reads_the_audio_streams_ffprobe_lists() {
        let json = r#"{"streams":[
            {"index":0,"codec_name":"h264","codec_type":"video"},
            {"index":1,"codec_name":"ac3","codec_type":"audio","channels":6,"sample_rate":"48000","tags":{"language":"eng","title":"Surround"},"disposition":{"default":1}},
            {"index":2,"codec_name":"aac","codec_type":"audio","channels":2,"sample_rate":"44100","tags":{"language":"fra"},"disposition":{"default":0}}]}"#;
        let s = parse_streams(json);
        assert_eq!(s.len(), 2, "the video stream is not counted");
        assert_eq!((s[0].ordinal, s[0].index, s[1].ordinal, s[1].index), (0, 1, 1, 2));
        assert_eq!(s[0].label(), "AC3 · 5.1 · eng · “Surround” · default");
        assert_eq!(s[1].label(), "AAC · stereo · fra");
        assert!(parse_streams("not json").is_empty());
    }

    /// Serve one response and stop.
    fn serve(status: &str, headers: &str, body: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let head = format!("HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n", body.len());
        thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf);
                let _ = s.write_all(head.as_bytes());
                let _ = s.write_all(&body);
            }
        });
        format!("http://{addr}/track.mp3")
    }

    #[test]
    fn a_link_is_downloaded_to_a_temporary_file() {
        let url = serve("200 OK", "Content-Type: audio/mpeg\r\n", vec![7u8; 5000]);
        let p = fetch(&url, false).unwrap();
        assert_eq!(std::fs::metadata(&p).unwrap().len(), 5000);
        assert!(p.to_string_lossy().ends_with(".mp3"));
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn bad_links_are_refused_with_a_reason() {
        let page = serve("200 OK", "Content-Type: text/html\r\n", b"<html>".to_vec());
        assert!(fetch(&page, false).unwrap_err().to_string().contains("web page"));
        let missing = serve("404 Not Found", "", b"nope".to_vec());
        assert!(fetch(&missing, false).unwrap_err().to_string().contains("404"));
        let empty = serve("200 OK", "Content-Type: audio/mpeg\r\n", Vec::new());
        assert!(fetch(&empty, false).unwrap_err().to_string().contains("nothing"));
        assert!(fetch("ftp://example.org/a.mp3", false).is_err());
    }

    #[test]
    fn the_extension_comes_from_the_link_or_the_content_type() {
        assert_eq!(extension_for("https://x.org/a/b.FLAC?x=1", None), "flac");
        assert_eq!(extension_for("https://x.org/stream", Some("audio/mpeg; charset=x")), "mp3");
        assert_eq!(extension_for("https://x.org/stream", None), "bin");
    }

    #[test]
    fn a_real_file_with_two_audio_streams_is_probed() {
        if Command::new("ffmpeg").arg("-version").output().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("rustydisc_streams_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("two.mkv");
        let ok = Command::new("ffmpeg")
            .args(["-loglevel", "error", "-y", "-f", "lavfi", "-i", "sine=f=300:d=2", "-f", "lavfi", "-i", "sine=f=900:d=2", "-map", "0:a", "-map", "1:a", "-metadata:s:a:0", "language=eng", "-metadata:s:a:1", "language=fra"])
            .arg(&f)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let s = probe_streams(&f.to_string_lossy()).unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(s[1].language.as_deref(), Some("fra"));
        assert!(check_stream(&f.to_string_lossy(), 1).is_ok());
        assert!(check_stream(&f.to_string_lossy(), 2).unwrap_err().contains("2 audio streams"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
