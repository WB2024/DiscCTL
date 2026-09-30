//! Looking at and changing the tags of a ripped file, and putting a picture in it.

use std::{collections::BTreeMap, collections::HashMap, path::Path};

use lofty::{
    config::WriteOptions,
    file::{AudioFile, TaggedFileExt},
    picture::{MimeType, Picture, PictureType},
    prelude::*,
    probe::Probe,
    tag::{ItemKey, ItemValue, Tag, TagItem, TagType},
};
use serde::Serialize;

use super::tags;
use crate::rip::tagging::{set, set_many};

/// The fields the editor offers, in the order they are shown: (name, label, group).
pub const FIELDS: &[(&str, &str, &str)] = &[
    ("title", "Title", "Track"),
    ("artist", "Artist", "Track"),
    ("tracknumber", "Track number", "Track"),
    ("totaltracks", "Total tracks", "Track"),
    ("discnumber", "Disc number", "Track"),
    ("totaldiscs", "Total discs", "Track"),
    ("discsubtitle", "Disc title", "Track"),
    ("album", "Album", "Album"),
    ("albumartist", "Album artist", "Album"),
    ("date", "Date", "Album"),
    ("originaldate", "Original date", "Album"),
    ("genre", "Genre", "Album"),
    ("label", "Label", "Album"),
    ("catalognumber", "Catalogue number", "Album"),
    ("barcode", "Barcode", "Album"),
    ("artistsort", "Artist sort name", "Sorting"),
    ("albumartistsort", "Album artist sort name", "Sorting"),
    ("composer", "Composer", "Credits"),
    ("lyricist", "Lyricist", "Credits"),
    ("isrc", "ISRC", "Credits"),
    ("musicbrainz_trackid", "MusicBrainz recording ID", "MusicBrainz"),
    ("musicbrainz_releasetrackid", "MusicBrainz track ID", "MusicBrainz"),
    ("musicbrainz_albumid", "MusicBrainz release ID", "MusicBrainz"),
    ("musicbrainz_releasegroupid", "MusicBrainz release group ID", "MusicBrainz"),
    ("musicbrainz_artistid", "MusicBrainz artist ID", "MusicBrainz"),
    ("musicbrainz_albumartistid", "MusicBrainz album artist ID", "MusicBrainz"),
];

/// Fields that can hold several values (shown and typed as "a; b").
const MULTI: &[&str] = &["genre", "composer", "lyricist", "isrc", "musicbrainz_artistid", "musicbrainz_albumartistid"];

fn key_of(name: &str) -> Option<ItemKey> {
    Some(match name {
        "title" => ItemKey::TrackTitle,
        "artist" => ItemKey::TrackArtist,
        "discsubtitle" => ItemKey::SetSubtitle,
        "album" => ItemKey::AlbumTitle,
        "albumartist" => ItemKey::AlbumArtist,
        "date" => ItemKey::RecordingDate,
        "originaldate" => ItemKey::OriginalReleaseDate,
        "genre" => ItemKey::Genre,
        "label" => ItemKey::Label,
        "catalognumber" => ItemKey::CatalogNumber,
        "barcode" => ItemKey::Barcode,
        "artistsort" => ItemKey::TrackArtistSortOrder,
        "albumartistsort" => ItemKey::AlbumArtistSortOrder,
        "composer" => ItemKey::Composer,
        "lyricist" => ItemKey::Lyricist,
        "isrc" => ItemKey::Isrc,
        "musicbrainz_trackid" => ItemKey::MusicBrainzRecordingId,
        "musicbrainz_releasetrackid" => ItemKey::MusicBrainzTrackId,
        "musicbrainz_albumid" => ItemKey::MusicBrainzReleaseId,
        "musicbrainz_releasegroupid" => ItemKey::MusicBrainzReleaseGroupId,
        "musicbrainz_artistid" => ItemKey::MusicBrainzArtistId,
        "musicbrainz_albumartistid" => ItemKey::MusicBrainzReleaseArtistId,
        _ => return None,
    })
}

#[derive(Debug, Serialize)]
pub struct RawItem {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Serialize)]
pub struct PictureInfo {
    pub kind: String,
    pub mime: String,
    pub bytes: usize,
}

#[derive(Debug, Serialize, Default)]
pub struct TagView {
    /// e.g. "Vorbis comments", "ID3v2"
    pub format: String,
    pub fields: BTreeMap<String, String>,
    /// Every tag in the file under its own name, for anything the fields don't cover.
    pub raw: Vec<RawItem>,
    pub pictures: Vec<PictureInfo>,
    pub duration_secs: Option<f64>,
    pub error: Option<String>,
}

fn tag_type_name(t: TagType) -> &'static str {
    match t {
        TagType::VorbisComments => "Vorbis comments",
        TagType::Id3v2 => "ID3v2",
        TagType::Mp4Ilst => "MP4 (iTunes)",
        TagType::Id3v1 => "ID3v1",
        TagType::Ape => "APE",
        TagType::RiffInfo => "RIFF INFO",
        TagType::AiffText => "AIFF text",
        _ => "tags",
    }
}

pub fn view(path: &Path) -> TagView {
    let mut v = TagView::default();
    let t = tags::read(path);
    v.duration_secs = t.duration_secs;
    for (name, _, _) in FIELDS {
        if let Some(val) = t.vars.get(*name) {
            v.fields.insert(name.to_string(), val.clone());
        }
    }
    let Ok(file) = Probe::open(path).and_then(|p| p.read()) else {
        v.error = Some("This file's tags can't be read".into());
        return v;
    };
    let Some(tag) = file.primary_tag().or_else(|| file.first_tag()) else {
        v.format = "none".into();
        return v;
    };
    let tt = tag.tag_type();
    v.format = tag_type_name(tt).into();
    for item in tag.items() {
        let ItemValue::Text(text) = item.value() else { continue };
        let key = item.key().map_key(tt, true).map(str::to_string).unwrap_or_else(|| format!("{:?}", item.key()));
        v.raw.push(RawItem { key, value: text.replace('\0', "; ") });
    }
    v.pictures = tag.pictures().iter().map(|p| PictureInfo { kind: format!("{:?}", p.pic_type()), mime: p.mime_type().map(|m| m.to_string()).unwrap_or_default(), bytes: p.data().len() }).collect();
    v
}

#[derive(Debug, Default)]
pub struct Edit {
    /// Editor fields by name; an empty value removes the field.
    pub set: HashMap<String, String>,
    /// Anything else, by the tag's own name; `None` removes it.
    pub raw: HashMap<String, Option<String>>,
}

fn split_values(v: &str) -> Vec<String> {
    v.split(';').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect()
}

pub fn apply(path: &Path, edit: &Edit) -> Result<(), String> {
    let mut file = Probe::open(path).map_err(|e| e.to_string())?.read().map_err(|e| e.to_string())?;
    if file.primary_tag().is_none() {
        let tt = file.primary_tag_type();
        file.insert_tag(Tag::new(tt));
    }
    let tag = file.primary_tag_mut().ok_or("this format can't hold tags")?;
    let tt = tag.tag_type();

    for (name, value) in &edit.set {
        let value = value.trim();
        let num = || value.parse::<u32>().map_err(|_| format!("'{value}' isn't a number ({name})"));
        match name.as_str() {
            "tracknumber" => if value.is_empty() { tag.remove_track() } else { tag.set_track(num()?) },
            "totaltracks" => if value.is_empty() { tag.remove_track_total() } else { tag.set_track_total(num()?) },
            "discnumber" => if value.is_empty() { tag.remove_disk() } else { tag.set_disk(num()?) },
            "totaldiscs" => if value.is_empty() { tag.remove_disk_total() } else { tag.set_disk_total(num()?) },
            other => {
                let key = key_of(other).ok_or_else(|| format!("'{other}' isn't a field that can be edited"))?;
                if value.is_empty() {
                    tag.remove_key(&key);
                } else if MULTI.contains(&other) {
                    set_many(tag, key, &split_values(value));
                } else {
                    set(tag, key, value);
                }
            }
        }
    }
    for (key, value) in &edit.raw {
        let item_key = ItemKey::from_key(tt, key);
        tag.remove_key(&item_key);
        if let Some(v) = value.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
            tag.push_unchecked(TagItem::new(item_key, ItemValue::Text(v.to_string())));
        }
    }
    file.save_to_path(path, WriteOptions::default()).map_err(|e| e.to_string())
}

/// Put `image` (JPEG or PNG) in the file as its front cover, replacing any cover already there.
pub fn embed_cover(path: &Path, image: &[u8], ext: &str) -> Result<(), String> {
    let mut file = Probe::open(path).map_err(|e| e.to_string())?.read().map_err(|e| e.to_string())?;
    if file.primary_tag().is_none() {
        let tt = file.primary_tag_type();
        file.insert_tag(Tag::new(tt));
    }
    let tag = file.primary_tag_mut().ok_or("this format can't hold tags")?;
    tag.remove_picture_type(PictureType::CoverFront);
    let mime = if ext == "png" { MimeType::Png } else { MimeType::Jpeg };
    tag.push_picture(Picture::new_unchecked(PictureType::CoverFront, Some(mime), None, image.to_vec()));
    file.save_to_path(path, WriteOptions::default()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn flac(dir: &Path, name: &str) -> std::path::PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let p = dir.join(name);
        assert!(Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=duration=1", "-c:a", "flac", "-metadata", "title=Old", "-metadata", "artist=A", "-metadata", "comment=keep me"]).arg(&p).status().unwrap().success());
        p
    }

    fn mp3(dir: &Path, name: &str) -> std::path::PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let p = dir.join(name);
        assert!(Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=duration=1", "-c:a", "libmp3lame", "-metadata", "title=Old"]).arg(&p).status().unwrap().success());
        p
    }

    fn png() -> Vec<u8> {
        // 1x1 PNG
        vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0x0D, 0x49, 0x48, 0x44, 0x52, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1F, 0x15, 0xC4, 0x89, 0, 0, 0, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8, 0xFF, 0xFF, 0x3F, 0, 5, 0xFE, 2, 0xFE, 0xA7, 0x35, 0x81, 0x84, 0, 0, 0, 0, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82]
    }

    #[test]
    fn edits_change_only_what_was_asked_and_keep_everything_else() {
        if Command::new("ffmpeg").arg("-version").output().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("rd_tagedit_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for p in [flac(&dir, "a.flac"), mp3(&dir, "a.mp3")] {
            let mut e = Edit::default();
            e.set.extend([("title".to_string(), "New Title".to_string()), ("tracknumber".into(), "7".into()), ("totaltracks".into(), "12".into()),
                          ("genre".into(), "Rock; Indie".into()), ("musicbrainz_albumid".into(), "55555555-5555-4555-8555-555555555555".into())]);
            // In ID3 a four-letter name would be taken for a frame ID, so a custom field needs a longer one.
            let mood = if p.extension().unwrap() == "flac" { "MOOD" } else { "MY MOOD" };
            e.raw.insert(mood.into(), Some("calm".into()));
            apply(&p, &e).unwrap();
            let v = view(&p);
            assert_eq!(v.fields.get("title").map(String::as_str), Some("New Title"), "{p:?}");
            assert_eq!((v.fields.get("tracknumber").map(String::as_str), v.fields.get("totaltracks").map(String::as_str)), (Some("7"), Some("12")));
            assert_eq!(v.fields.get("genre").map(String::as_str), Some("Rock; Indie"));
            assert_eq!(v.fields.get("musicbrainz_albumid").map(String::as_str), Some("55555555-5555-4555-8555-555555555555"));
            assert!(v.raw.iter().any(|r| r.key.eq_ignore_ascii_case(mood) && r.value == "calm"), "{:?}", v.raw);
            if p.extension().unwrap() == "flac" {
                assert_eq!(v.fields.get("artist").map(String::as_str), Some("A"), "untouched fields stay");
                assert!(v.raw.iter().any(|r| r.value == "keep me"), "{:?}", v.raw);
            }
            // clearing a field removes it
            let mut clear = Edit::default();
            clear.set.insert("genre".into(), String::new());
            clear.raw.insert(mood.into(), None);
            apply(&p, &clear).unwrap();
            let v = view(&p);
            assert!(!v.fields.contains_key("genre") && !v.raw.iter().any(|r| r.key.eq_ignore_ascii_case(mood)));
            assert_eq!(v.fields.get("title").map(String::as_str), Some("New Title"));
        }
        let mut bad = Edit::default();
        bad.set.insert("tracknumber".into(), "x".into());
        assert!(apply(&dir.join("a.flac"), &bad).unwrap_err().contains("isn't a number"));
        let mut unknown = Edit::default();
        unknown.set.insert("nonsense".into(), "x".into());
        assert!(apply(&dir.join("a.flac"), &unknown).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_cover_is_embedded_replacing_the_old_one() {
        if Command::new("ffmpeg").arg("-version").output().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("rd_tagcover_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for p in [flac(&dir, "a.flac"), mp3(&dir, "a.mp3")] {
            embed_cover(&p, &png(), "png").unwrap();
            embed_cover(&p, &png(), "png").unwrap();
            let v = view(&p);
            assert_eq!(v.pictures.len(), 1, "{p:?}: replaced, not added");
            assert_eq!(v.pictures[0].mime, "image/png");
            assert_eq!(v.fields.get("title").map(String::as_str), Some("Old"), "tags survive");
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
