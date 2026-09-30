//! Writing the full set of MusicBrainz tags into a ripped file.
//!
//! ffmpeg encodes the audio and embeds the cover; this then writes everything Picard would:
//! IDs (recording, release track, release, release group, artists, work), sort names, disc
//! number and subtitle, dates, label, catalogue number, barcode, ISRCs, genres and credits.
//! Going through lofty means each format gets its own native frame or field (UFID for the
//! recording ID in MP3, `MUSICBRAINZ_TRACKID` in FLAC, freeform atoms in M4A).

use lofty::{
    config::WriteOptions,
    file::{AudioFile, TaggedFileExt},
    prelude::*,
    probe::Probe,
    tag::{ItemKey, ItemValue, Tag, TagItem, TagType},
};

use super::{
    encoder::TrackTags,
    musicbrainz::{MbTrackInfo, ReleaseInfo},
};

const VARIOUS_ARTISTS_ID: &str = "89ad4ac3-39f7-470e-963a-56509c546377";

pub(crate) fn set(tag: &mut Tag, key: ItemKey, value: &str) {
    if !value.trim().is_empty() {
        tag.remove_key(&key);
        // "unchecked": a few keys (the recording ID in an MP3, say) have no plain name to check
        // against; lofty still writes them with the right frame.
        tag.push_unchecked(TagItem::new(key, ItemValue::Text(value.trim().to_string())));
    }
}

/// Several values for one field: separate fields in Vorbis and MP4, one frame with the values
/// split by a NUL byte in ID3v2.4 (a second frame of the same kind would be ignored).
pub(crate) fn set_many(tag: &mut Tag, key: ItemKey, values: &[String]) {
    let values: Vec<String> = values.iter().map(|v| v.trim().to_string()).filter(|v| !v.is_empty()).collect();
    if values.is_empty() {
        return;
    }
    let key = match (tag.tag_type(), &key) {
        // ID3 keeps these in its "involved people" frames, which lofty's generic tag doesn't reach;
        // a text field with the same name is what most players and taggers fall back to.
        (TagType::Id3v2, ItemKey::Producer) => ItemKey::from_key(TagType::Id3v2, "PRODUCER"),
        (TagType::Id3v2, ItemKey::Engineer) => ItemKey::from_key(TagType::Id3v2, "ENGINEER"),
        (TagType::Id3v2, ItemKey::MixEngineer) => ItemKey::from_key(TagType::Id3v2, "MIXER"),
        (TagType::Id3v2, ItemKey::Performer) => ItemKey::from_key(TagType::Id3v2, "PERFORMER"),
        _ => key,
    };
    tag.remove_key(&key);
    if tag.tag_type() == TagType::Id3v2 {
        tag.push_unchecked(TagItem::new(key, ItemValue::Text(values.join("\0"))));
    } else {
        for v in values {
            tag.push_unchecked(TagItem::new(key.clone(), ItemValue::Text(v)));
        }
    }
}

/// A field lofty has no name for: (Vorbis name, ID3 description, MP4 freeform name).
pub(crate) fn custom(tag: &mut Tag, names: (&str, &str), value: &Option<String>) {
    let Some(v) = value.as_deref().filter(|v| !v.trim().is_empty()) else { return };
    let native = match tag.tag_type() {
        TagType::VorbisComments => names.0.to_string(),
        // A four-letter name would be taken for a frame ID (and refused), so ASIN is skipped here.
        TagType::Id3v2 if names.1.len() != 4 => names.1.to_string(),
        TagType::Id3v2 => return,
        TagType::Mp4Ilst => format!("----:com.apple.iTunes:{}", names.1),
        _ => return,
    };
    tag.push_unchecked(TagItem::new(ItemKey::from_key(tag.tag_type(), &native), ItemValue::Text(v.trim().to_string())));
}

/// Tag one file. Failures are reported but never stop the rip: the audio is already safe.
pub fn apply(path: &str, basic: &TrackTags, release: Option<&ReleaseInfo>, track: Option<&MbTrackInfo>, debug: bool) {
    if let Err(e) = try_apply(path, basic, release, track) {
        eprintln!("Warning: couldn't write all tags to {path}: {e}");
    } else if debug {
        eprintln!("Tagged {path}");
    }
}

fn try_apply(path: &str, basic: &TrackTags, release: Option<&ReleaseInfo>, track: Option<&MbTrackInfo>) -> Result<(), String> {
    let mut file = Probe::open(path).map_err(|e| e.to_string())?.read().map_err(|e| e.to_string())?;
    if file.primary_tag().is_none() {
        let tt = file.primary_tag_type();
        file.insert_tag(Tag::new(tt));
    }
    let tag = file.primary_tag_mut().ok_or("this format can't hold tags")?;

    // The basics, so files that ffmpeg couldn't tag (WAV) have them too.
    if let Some(v) = &basic.title { set(tag, ItemKey::TrackTitle, v); }
    if let Some(v) = &basic.artist { set(tag, ItemKey::TrackArtist, v); }
    if let Some(v) = &basic.album { set(tag, ItemKey::AlbumTitle, v); }
    if let Some(v) = &basic.album_artist { set(tag, ItemKey::AlbumArtist, v); }
    if let Some(n) = basic.track_number { tag.set_track(n as u32); }
    if let Some(n) = basic.track_total { tag.set_track_total(n as u32); }

    if let Some(r) = release {
        set(tag, ItemKey::MusicBrainzReleaseId, &r.mb_release_id);
        if let Some(v) = &r.mb_release_group_id { set(tag, ItemKey::MusicBrainzReleaseGroupId, v); }
        set_many(tag, ItemKey::MusicBrainzReleaseArtistId, &r.album_artist_ids);
        if let Some(v) = &r.album_artist_sort { set(tag, ItemKey::AlbumArtistSortOrder, v); }
        if let Some(v) = &r.date { set(tag, ItemKey::RecordingDate, v); }
        if let Some(v) = &r.original_date { set(tag, ItemKey::OriginalReleaseDate, v); }
        if let Some(v) = &r.label { set(tag, ItemKey::Label, v); }
        if let Some(v) = &r.catalog_number { set(tag, ItemKey::CatalogNumber, v); }
        if let Some(v) = &r.barcode { set(tag, ItemKey::Barcode, v); }
        if let Some(v) = &r.language { set(tag, ItemKey::Language, v); }
        if let Some(v) = &r.script { set(tag, ItemKey::Script, v); }
        if let Some(v) = &r.disc_title { set(tag, ItemKey::SetSubtitle, v); }
        set_many(tag, ItemKey::Genre, &r.genres);
        if let Some(n) = r.disc_number { tag.set_disk(n as u32); }
        if let Some(n) = r.disc_total { tag.set_disk_total(n as u32); }
        if r.album_artist_ids.iter().any(|id| id == VARIOUS_ARTISTS_ID) {
            set(tag, ItemKey::FlagCompilation, "1");
        }
        custom(tag, ("MUSICBRAINZ_ALBUMSTATUS", "MusicBrainz Album Status"), &r.status);
        custom(tag, ("MUSICBRAINZ_ALBUMTYPE", "MusicBrainz Album Type"), &r.release_type);
        custom(tag, ("RELEASECOUNTRY", "MusicBrainz Album Release Country"), &r.country);
        custom(tag, ("MEDIA", "MEDIA"), &r.media_format);
        custom(tag, ("ASIN", "ASIN"), &r.asin);
        custom(tag, ("MUSICBRAINZ_ALBUMCOMMENT", "MusicBrainz Album Comment"), &r.release_comment);
    }

    if let Some(t) = track {
        if let Some(v) = &t.mb_recording_id { set(tag, ItemKey::MusicBrainzRecordingId, v); }
        if let Some(v) = &t.mb_release_track_id { set(tag, ItemKey::MusicBrainzTrackId, v); }
        set_many(tag, ItemKey::MusicBrainzArtistId, &t.artist_ids);
        set_many(tag, ItemKey::TrackArtists, &t.artists);
        if let Some(v) = &t.artist_sort { set(tag, ItemKey::TrackArtistSortOrder, v); }
        set_many(tag, ItemKey::Isrc, &t.isrcs);
        set_many(tag, ItemKey::MusicBrainzWorkId, &t.work_ids);
        set_many(tag, ItemKey::Work, &t.works);
        set_many(tag, ItemKey::Composer, &t.composers);
        set_many(tag, ItemKey::Lyricist, &t.lyricists);
        set_many(tag, ItemKey::Writer, &t.writers);
        set_many(tag, ItemKey::Arranger, &t.arrangers);
        set_many(tag, ItemKey::Conductor, &t.conductors);
        set_many(tag, ItemKey::Producer, &t.producers);
        set_many(tag, ItemKey::MixEngineer, &t.mixers);
        set_many(tag, ItemKey::Engineer, &t.engineers);
        set_many(tag, ItemKey::Remixer, &t.remixers);
        set_many(tag, ItemKey::Performer, &t.performers);
    }

    let id3 = tag.tag_type() == TagType::Id3v2;
    file.save_to_path(path, WriteOptions::default()).map_err(|e| e.to_string())?;
    if id3 {
        if let Some(id) = track.and_then(|t| t.mb_recording_id.as_deref()) {
            add_ufid(path, id)?;
        }
    }
    Ok(())
}

/// The recording ID in an MP3 or AIFF lives in a UFID frame owned by musicbrainz.org (that is
/// where Picard, Lidarr and Navidrome look). lofty's generic tag can't write it, so add it to
/// the ID3v2 tag directly.
fn add_ufid(path: &str, recording_id: &str) -> Result<(), String> {
    use lofty::{
        config::ParseOptions,
        id3::v2::{Frame, UniqueFileIdentifierFrame},
        iff::aiff::AiffFile,
        mpeg::MpegFile,
    };
    let frame = || Frame::UniqueFileIdentifier(UniqueFileIdentifierFrame::new("http://musicbrainz.org".to_string(), recording_id.as_bytes().to_vec()));
    let mut f = std::fs::OpenOptions::new().read(true).write(true).open(path).map_err(|e| e.to_string())?;
    let kind = Probe::new(std::io::BufReader::new(&f)).guess_file_type().map_err(|e| e.to_string())?.file_type();
    use std::io::Seek;
    f.rewind().map_err(|e| e.to_string())?;
    match kind {
        Some(lofty::file::FileType::Mpeg) => {
            let mut m = MpegFile::read_from(&mut f, ParseOptions::new()).map_err(|e| e.to_string())?;
            m.id3v2_mut().ok_or("no ID3v2 tag")?.insert(frame());
            m.save_to_path(path, WriteOptions::default()).map_err(|e| e.to_string())
        }
        Some(lofty::file::FileType::Aiff) => {
            let mut a = AiffFile::read_from(&mut f, ParseOptions::new()).map_err(|e| e.to_string())?;
            a.id3v2_mut().ok_or("no ID3v2 tag")?.insert(frame());
            a.save_to_path(path, WriteOptions::default()).map_err(|e| e.to_string())
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn have_ffmpeg() -> bool {
        Command::new("ffmpeg").arg("-version").output().is_ok()
    }

    fn release() -> (ReleaseInfo, MbTrackInfo) {
        let t = MbTrackInfo {
            number: 1, title: "Song".into(),
            mb_recording_id: Some("11111111-1111-4111-8111-111111111111".into()),
            mb_release_track_id: Some("22222222-2222-4222-8222-222222222222".into()),
            artist_ids: vec!["33333333-3333-4333-8333-333333333333".into()],
            artists: vec!["Band".into()], artist_sort: Some("Band, The".into()),
            isrcs: vec!["GBAAA0000001".into()], composers: vec!["Comp".into()], producers: vec!["Prod".into()],
            performers: vec!["Guy (guitar)".into()], work_ids: vec!["44444444-4444-4444-8444-444444444444".into()],
            ..Default::default()
        };
        let r = ReleaseInfo {
            mb_release_id: "55555555-5555-4555-8555-555555555555".into(), album: "Rec".into(), album_artist: "Band".into(),
            mb_release_group_id: Some("66666666-6666-4666-8666-666666666666".into()),
            album_artist_ids: vec!["33333333-3333-4333-8333-333333333333".into()], album_artist_sort: Some("Band, The".into()),
            date: Some("1999-04-12".into()), original_date: Some("1998-01-01".into()), label: Some("Label".into()),
            catalog_number: Some("CAT1".into()), barcode: Some("123456".into()), status: Some("Official".into()),
            release_type: Some("album".into()), country: Some("GB".into()), media_format: Some("CD".into()), asin: Some("B000".into()),
            disc_number: Some(2), disc_total: Some(3), disc_title: Some("Bonus".into()), genres: vec!["Rock".into(), "Indie".into()],
            tracks: vec![t.clone()], ..Default::default()
        };
        (r, t)
    }

    /// Encode a real (tiny) file in each format, tag it, and read every tag back.
    #[test]
    fn writes_musicbrainz_tags_in_every_format() {
        if !have_ffmpeg() {
            eprintln!("skipping: ffmpeg not installed");
            return;
        }
        let dir = std::env::temp_dir().join(format!("rd_tagging_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let wav = dir.join("in.wav");
        assert!(Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=2", "-ar", "44100", "-ac", "2"]).arg(&wav).status().unwrap().success());
        let (r, t) = release();
        let basic = TrackTags { title: Some("Song".into()), artist: Some("Band".into()), album: Some("Rec".into()), album_artist: Some("Band".into()), track_number: Some(1), track_total: Some(9), ..Default::default() };

        for fmt in ["flac", "mp3", "m4a", "ogg", "opus"] {
            let out = dir.join(format!("out.{fmt}"));
            let codec: &[&str] = match fmt { "flac" => &["-c:a", "flac"], "mp3" => &["-c:a", "libmp3lame"], "m4a" => &["-c:a", "aac"], "ogg" => &["-c:a", "libvorbis"], _ => &["-c:a", "libopus"] };
            assert!(Command::new("ffmpeg").args(["-v", "error", "-y", "-i"]).arg(&wav).args(codec).arg(&out).status().unwrap().success(), "{fmt}");
            apply(out.to_str().unwrap(), &basic, Some(&r), Some(&t), false);

            let tagged = Probe::open(&out).unwrap().read().unwrap();
            let tag = tagged.primary_tag().unwrap_or_else(|| panic!("{fmt}: no tag"));
            if std::env::var("TAG_DEBUG").is_ok() { eprintln!("{fmt}: {:?}", tag.items().map(|i| (i.key().clone(), i.value().clone())).collect::<Vec<_>>()); }
            let get = |k: ItemKey| tag.get_string(&k).map(str::to_string);
            assert_eq!(get(ItemKey::MusicBrainzRecordingId).as_deref(), t.mb_recording_id.as_deref(), "{fmt} recording id");
            assert_eq!(get(ItemKey::MusicBrainzTrackId).as_deref(), t.mb_release_track_id.as_deref(), "{fmt} release track id");
            assert_eq!(get(ItemKey::MusicBrainzReleaseId).as_deref(), Some(r.mb_release_id.as_str()), "{fmt} release id");
            assert_eq!(get(ItemKey::MusicBrainzReleaseGroupId).as_deref(), r.mb_release_group_id.as_deref(), "{fmt} rg id");
            assert_eq!(get(ItemKey::MusicBrainzArtistId).as_deref(), Some(t.artist_ids[0].as_str()), "{fmt} artist id");
            assert_eq!(get(ItemKey::AlbumArtistSortOrder).as_deref(), Some("Band, The"), "{fmt} sort");
            assert_eq!(get(ItemKey::Label).as_deref(), Some("Label"), "{fmt} label");
            assert_eq!(get(ItemKey::Isrc).as_deref(), Some("GBAAA0000001"), "{fmt} isrc");
            assert_eq!(get(ItemKey::Composer).as_deref(), Some("Comp"), "{fmt} composer");
            assert_eq!(tag.disk(), Some(2), "{fmt} disc");
            assert_eq!(tag.disk_total(), Some(3), "{fmt} discs");
            assert_eq!(tag.track(), Some(1), "{fmt} track");
            let genres: Vec<String> = tag.get_strings(&ItemKey::Genre).flat_map(|g| g.split('\0')).map(str::to_string).collect();
            assert_eq!(genres, ["Rock", "Indie"], "{fmt} genres");
            assert_eq!(get(ItemKey::Work), None);
            if fmt == "flac" {
                assert_eq!(get(ItemKey::from_key(TagType::VorbisComments, "MUSICBRAINZ_ALBUMSTATUS")).as_deref(), Some("Official"));
                assert_eq!(get(ItemKey::from_key(TagType::VorbisComments, "RELEASECOUNTRY")).as_deref(), Some("GB"));
                assert_eq!(get(ItemKey::from_key(TagType::VorbisComments, "ASIN")).as_deref(), Some("B000"));
            }
            if fmt == "mp3" {
                assert_eq!(get(ItemKey::from_key(TagType::Id3v2, "MusicBrainz Album Status")).as_deref(), Some("Official"));
            }
            if fmt != "mp3" { assert_eq!(get(ItemKey::Producer).as_deref(), Some("Prod"), "{fmt} producer"); }
            else { assert_eq!(get(ItemKey::from_key(TagType::Id3v2, "PRODUCER")).as_deref(), Some("Prod")); }
        }
        if std::env::var("TAG_KEEP").is_err() { std::fs::remove_dir_all(&dir).ok(); }
    }
}
