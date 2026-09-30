//! Reading a file's tags under the names Picard's naming scripts use.

use std::{collections::HashMap, path::Path};

use lofty::{
    file::{AudioFile, TaggedFileExt},
    prelude::*,
    probe::Probe,
    tag::{ItemKey, Tag, TagType},
};

/// Everything a naming script can see about one file, plus the facts import needs.
#[derive(Debug, Clone, Default)]
pub struct FileTags {
    pub vars: HashMap<String, String>,
    pub duration_secs: Option<f64>,
    /// The file had no readable tags at all.
    pub untagged: bool,
}

fn joined(tag: &Tag, key: &ItemKey) -> Option<String> {
    let parts: Vec<String> = tag
        .get_strings(key)
        .flat_map(|s| s.split('\0'))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    (!parts.is_empty()).then(|| parts.join("; "))
}

fn first(tag: &Tag, key: &ItemKey) -> Option<String> {
    tag.get_strings(key).flat_map(|s| s.split('\0')).map(|s| s.trim().to_string()).find(|s| !s.is_empty())
}

/// A field with no lofty name: its Vorbis name, ID3 description and MP4 freeform name.
fn custom(tag: &Tag, vorbis: &str, id3: &str) -> Option<String> {
    let native = match tag.tag_type() {
        TagType::VorbisComments => vorbis.to_string(),
        TagType::Id3v2 => id3.to_string(),
        TagType::Mp4Ilst => format!("----:com.apple.iTunes:{id3}"),
        _ => return None,
    };
    first(tag, &ItemKey::from_key(tag.tag_type(), &native))
}

pub fn read(path: &Path) -> FileTags {
    let mut out = FileTags::default();
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    out.vars.insert("_extension".into(), ext);
    let Ok(tagged) = Probe::open(path).and_then(|p| p.read()) else {
        out.untagged = true;
        return out;
    };
    let d = tagged.properties().duration().as_secs_f64();
    if d > 0.0 {
        out.duration_secs = Some(d);
        let secs = d.round() as u64;
        out.vars.insert("_length".into(), format!("{}:{:02}", secs / 60, secs % 60));
    }
    let props = tagged.properties();
    if let Some(v) = props.audio_bitrate() { out.vars.insert("_bitrate".into(), format!("{v}.0")); }
    if let Some(v) = props.sample_rate() { out.vars.insert("_sample_rate".into(), v.to_string()); }
    if let Some(v) = props.bit_depth() { out.vars.insert("_bits_per_sample".into(), v.to_string()); }
    if let Some(v) = props.channels() { out.vars.insert("_channels".into(), v.to_string()); }
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) { out.vars.insert("_filename".into(), stem.to_string()); }
    let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) else {
        out.untagged = true;
        return out;
    };
    let mut put = |name: &str, v: Option<String>| {
        if let Some(v) = v.filter(|v| !v.is_empty()) {
            out.vars.insert(name.to_string(), v);
        }
    };

    put("title", first(tag, &ItemKey::TrackTitle));
    put("artist", first(tag, &ItemKey::TrackArtist));
    put("artists", joined(tag, &ItemKey::TrackArtists));
    put("albumartist", first(tag, &ItemKey::AlbumArtist));
    put("album", first(tag, &ItemKey::AlbumTitle));
    put("artistsort", first(tag, &ItemKey::TrackArtistSortOrder));
    put("albumartistsort", first(tag, &ItemKey::AlbumArtistSortOrder));
    put("tracknumber", tag.track().map(|n| n.to_string()));
    put("totaltracks", tag.track_total().map(|n| n.to_string()));
    put("discnumber", tag.disk().map(|n| n.to_string()));
    put("totaldiscs", tag.disk_total().map(|n| n.to_string()));
    put("discsubtitle", first(tag, &ItemKey::SetSubtitle));
    let date = first(tag, &ItemKey::RecordingDate).or_else(|| first(tag, &ItemKey::Year)).or_else(|| tag.year().map(|y| y.to_string()));
    put("date", date);
    let original = first(tag, &ItemKey::OriginalReleaseDate);
    put("originalyear", original.as_deref().map(|d| d.chars().take(4).collect()));
    put("originaldate", original);
    put("genre", joined(tag, &ItemKey::Genre));
    put("label", first(tag, &ItemKey::Label));
    put("catalognumber", first(tag, &ItemKey::CatalogNumber));
    put("barcode", first(tag, &ItemKey::Barcode));
    put("isrc", joined(tag, &ItemKey::Isrc));
    put("composer", joined(tag, &ItemKey::Composer));
    put("lyricist", joined(tag, &ItemKey::Lyricist));
    put("producer", joined(tag, &ItemKey::Producer).or_else(|| custom(tag, "PRODUCER", "PRODUCER")));
    put("work", joined(tag, &ItemKey::Work));
    put("language", first(tag, &ItemKey::Language));
    put("script", first(tag, &ItemKey::Script));
    put("compilation", first(tag, &ItemKey::FlagCompilation));
    put("media", first(tag, &ItemKey::OriginalMediaType).or_else(|| custom(tag, "MEDIA", "MEDIA")));
    put("musicbrainz_albumid", first(tag, &ItemKey::MusicBrainzReleaseId));
    put("musicbrainz_releasegroupid", first(tag, &ItemKey::MusicBrainzReleaseGroupId));
    put("musicbrainz_albumartistid", first(tag, &ItemKey::MusicBrainzReleaseArtistId));
    put("musicbrainz_artistid", first(tag, &ItemKey::MusicBrainzArtistId));
    put("musicbrainz_trackid", first(tag, &ItemKey::MusicBrainzRecordingId));
    put("musicbrainz_releasetrackid", first(tag, &ItemKey::MusicBrainzTrackId));
    put("releasestatus", custom(tag, "MUSICBRAINZ_ALBUMSTATUS", "MusicBrainz Album Status"));
    put("releasetype", custom(tag, "MUSICBRAINZ_ALBUMTYPE", "MusicBrainz Album Type"));
    put("releasecountry", custom(tag, "RELEASECOUNTRY", "MusicBrainz Album Release Country"));
    put("asin", custom(tag, "ASIN", "ASIN"));
    put("_releasecomment", custom(tag, "MUSICBRAINZ_ALBUMCOMMENT", "MusicBrainz Album Comment"));

    // Picard's own derived variables, and stand-ins for the "Additional Artists Variables" plugin
    // (the credit as a whole stands in for its first artist and for all of them).
    let rt = out.vars.get("releasetype").cloned().unwrap_or_default();
    let mut types = rt.split(';').map(str::trim).filter(|t| !t.is_empty());
    if let Some(primary) = types.next() { out.vars.insert("_primaryreleasetype".into(), primary.to_string()); }
    let secondary: Vec<&str> = types.collect();
    if !secondary.is_empty() { out.vars.insert("_secondaryreleasetype".into(), secondary.join("; ")); }
    let alias = |v: &mut HashMap<String, String>, from: &str, to: &[&str]| {
        if let Some(x) = v.get(from).cloned() { for t in to { v.entry(t.to_string()).or_insert_with(|| x.clone()); } }
    };
    alias(&mut out.vars, "albumartist", &["_artists_album_primary_std", "_artists_album_primary_cred", "_artists_album_all_std", "_artists_album_all_cred"]);
    alias(&mut out.vars, "albumartistsort", &["_artists_album_primary_sort", "_artists_album_all_sort", "_artists_album_all_sort_primary"]);
    alias(&mut out.vars, "artist", &["_artists_track_primary_std", "_artists_track_primary_cred", "_artists_track_all_std", "_artists_track_all_cred"]);
    alias(&mut out.vars, "artistsort", &["_artists_track_primary_sort", "_artists_track_all_sort"]);
    out
}
