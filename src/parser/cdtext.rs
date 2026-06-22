use lofty::prelude::*;
use lofty::probe::Probe;
use crate::model::disc::{CdText, TrackTitle};

/// Read CD-Text metadata from the embedded tags of a list of audio track files.
///
/// Disc title  — ALBUM tag of the first track with one
/// Disc artist — ALBUMARTIST if present; common ARTIST if all tracks share one;
///               "Various Artists" for compilations
/// Per-track   — TITLE and ARTIST from each file (None if the tag is absent)
pub fn from_tags(tracks: &[String]) -> (Option<CdText>, Option<Vec<TrackTitle>>) {
    let entries: Vec<TagEntry> = tracks.iter().map(|p| read_tags(p)).collect();

    if entries.iter().all(|e| e.title.is_none() && e.artist.is_none()) {
        // No usable tags anywhere — silently skip rather than writing empty CD-Text
        return (None, None);
    }

    let disc_title = entries.iter().find_map(|e| e.album.clone());

    let disc_artist = entries
        .iter()
        .find_map(|e| e.album_artist.clone())
        .or_else(|| infer_disc_artist(&entries));

    let cd_text = if disc_title.is_some() || disc_artist.is_some() {
        Some(CdText {
            title: disc_title,
            artist: disc_artist,
        })
    } else {
        None
    };

    let track_titles: Vec<TrackTitle> = entries
        .into_iter()
        .map(|e| TrackTitle {
            title: e.title,
            artist: e.artist,
        })
        .collect();

    let has_any = track_titles.iter().any(|t| t.title.is_some() || t.artist.is_some());
    let track_titles = if has_any { Some(track_titles) } else { None };

    (cd_text, track_titles)
}

struct TagEntry {
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    album_artist: Option<String>,
}

fn read_tags(path: &str) -> TagEntry {
    let Ok(tagged) = Probe::open(path).and_then(|p| p.read()) else {
        return empty_entry();
    };

    let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) else {
        return empty_entry();
    };

    TagEntry {
        title:        tag.title().map(|s| s.to_string()),
        artist:       tag.artist().map(|s| s.to_string()),
        album:        tag.album().map(|s| s.to_string()),
        album_artist: tag.get_string(&lofty::tag::ItemKey::AlbumArtist).map(|s| s.to_string()),
    }
}

fn empty_entry() -> TagEntry {
    TagEntry { title: None, artist: None, album: None, album_artist: None }
}

fn infer_disc_artist(entries: &[TagEntry]) -> Option<String> {
    let artists: Vec<&str> = entries.iter().filter_map(|e| e.artist.as_deref()).collect();
    if artists.is_empty() {
        return None;
    }
    if artists.windows(2).all(|w| w[0] == w[1]) {
        Some(artists[0].to_string())
    } else {
        Some("Various Artists".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_when_no_tracks() {
        let (cd_text, track_titles) = from_tags(&[]);
        assert!(cd_text.is_none());
        assert!(track_titles.is_none());
    }

    #[test]
    fn empty_when_files_have_no_tags() {
        // Plain WAV files with no tags — should not error, just return None
        let (cd_text, track_titles) = from_tags(&["tests/fixtures/audio/track01.wav".to_string()]);
        assert!(cd_text.is_none());
        assert!(track_titles.is_none());
    }

    #[test]
    fn infer_disc_artist_single() {
        let entries = vec![
            TagEntry { title: None, artist: Some("Donna Lewis".into()), album: None, album_artist: None },
            TagEntry { title: None, artist: Some("Donna Lewis".into()), album: None, album_artist: None },
        ];
        assert_eq!(infer_disc_artist(&entries).as_deref(), Some("Donna Lewis"));
    }

    #[test]
    fn infer_disc_artist_compilation() {
        let entries = vec![
            TagEntry { title: None, artist: Some("Donna Lewis".into()), album: None, album_artist: None },
            TagEntry { title: None, artist: Some("Talking Heads".into()), album: None, album_artist: None },
        ];
        assert_eq!(infer_disc_artist(&entries).as_deref(), Some("Various Artists"));
    }

    #[test]
    fn infer_disc_artist_missing_some() {
        // One track lacks an artist tag — the tagged tracks all agree, so use that artist
        // (a missing tag is treated as a tagging gap, not a different artist)
        let entries = vec![
            TagEntry { title: None, artist: Some("Artist A".into()), album: None, album_artist: None },
            TagEntry { title: None, artist: None, album: None, album_artist: None },
        ];
        assert_eq!(infer_disc_artist(&entries).as_deref(), Some("Artist A"));
    }

    #[test]
    fn infer_disc_artist_truly_various() {
        // Tagged tracks disagree → compilation
        let entries = vec![
            TagEntry { title: None, artist: Some("Artist A".into()), album: None, album_artist: None },
            TagEntry { title: None, artist: None, album: None, album_artist: None },
            TagEntry { title: None, artist: Some("Artist B".into()), album: None, album_artist: None },
        ];
        assert_eq!(infer_disc_artist(&entries).as_deref(), Some("Various Artists"));
    }
}
