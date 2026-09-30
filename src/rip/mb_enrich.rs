//! The fuller MusicBrainz lookup: everything Picard would tag a file with.
//!
//! The DiscID lookup that finds a release is deliberately light. Once we know the release, one
//! more request (with labels, ISRCs, genres and the artist/work relationships) fills in the
//! rest: sort names, release comment, label and catalogue number, original date, disc number,
//! composers, producers and so on, plus the IDs Lidarr, Navidrome and Picard use to recognise a
//! file again.

use serde_json::Value;

use super::musicbrainz::{mb_call, MbTrackInfo, ReleaseInfo, MB_API, USER_AGENT};

const INC: &str = "artist-credits+labels+recordings+release-groups+media+discids+isrcs+genres+artist-rels+recording-level-rels+work-rels+work-level-rels";

/// Fetch the full release and fill in `release`. Failures leave it as it was: the rip goes on
/// with the basic tags.
pub fn enrich(release: &mut ReleaseInfo, discid: Option<&str>, debug: bool) {
    let url = format!("{}/release/{}?inc={}&fmt=json", MB_API, release.mb_release_id, INC);
    if debug {
        eprintln!("MusicBrainz full lookup: {url}");
    }
    match mb_call(|| ureq::get(&url).set("User-Agent", USER_AGENT)).map(|r| r.into_json::<Value>()) {
        Ok(Ok(full)) => apply(release, &full, discid),
        Ok(Err(e)) => eprintln!("MusicBrainz details couldn't be read (continuing with basic tags): {e}"),
        Err(e) => eprintln!("MusicBrainz details unavailable (continuing with basic tags): {e}"),
    }
}

fn s(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::trim).filter(|x| !x.is_empty()).map(str::to_string)
}

fn arr<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key).and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
}

/// Names, sort names and IDs of an artist credit, plus the joined display strings.
#[derive(Default)]
struct Credit {
    names: Vec<String>,
    ids: Vec<String>,
    sort_joined: String,
}

fn credit(v: &Value) -> Credit {
    let mut c = Credit::default();
    for part in arr(v, "artist-credit") {
        let artist = part.get("artist").unwrap_or(&Value::Null);
        let name = s(part, "name").or_else(|| s(artist, "name")).unwrap_or_default();
        let sort = s(artist, "sort-name").unwrap_or_else(|| name.clone());
        let join = part.get("joinphrase").and_then(Value::as_str).unwrap_or("");
        if !name.is_empty() {
            c.names.push(name);
        }
        if let Some(id) = s(artist, "id") {
            c.ids.push(id);
        }
        c.sort_joined.push_str(&sort);
        c.sort_joined.push_str(join);
    }
    c.sort_joined = c.sort_joined.trim().to_string();
    c
}

fn capitalise(g: &str) -> String {
    g.split(' ').map(|w| { let mut c = w.chars(); c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default() }).collect::<Vec<_>>().join(" ")
}

fn genres(v: &Value) -> Vec<String> {
    let mut g: Vec<(u64, String)> = arr(v, "genres").iter().filter_map(|x| Some((x.get("count").and_then(Value::as_u64).unwrap_or(0), capitalise(&s(x, "name")?)))).collect();
    g.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    g.into_iter().map(|x| x.1).collect()
}

fn push_unique(list: &mut Vec<String>, v: String) {
    if !v.is_empty() && !list.contains(&v) {
        list.push(v);
    }
}

/// Artist relationships of a recording or work, sorted into roles.
fn roles(rels: &[Value], t: &mut MbTrackInfo, on_work: bool) {
    for r in rels {
        if r.get("target-type").and_then(Value::as_str) != Some("artist") {
            continue;
        }
        let Some(name) = r.get("artist").and_then(|a| s(a, "name")) else { continue };
        let kind = s(r, "type").unwrap_or_default();
        let attrs: Vec<String> = arr(r, "attributes").iter().filter_map(|a| a.as_str().map(str::to_string)).collect();
        match (kind.as_str(), on_work) {
            ("composer", _) => push_unique(&mut t.composers, name),
            ("lyricist", _) => push_unique(&mut t.lyricists, name),
            ("writer", _) => push_unique(&mut t.writers, name),
            ("arranger" | "orchestrator", _) => push_unique(&mut t.arrangers, name),
            ("conductor", false) => push_unique(&mut t.conductors, name),
            ("producer", false) => push_unique(&mut t.producers, name),
            ("mix", false) => push_unique(&mut t.mixers, name),
            ("engineer" | "recording" | "sound", false) => push_unique(&mut t.engineers, name),
            ("remixer", false) => push_unique(&mut t.remixers, name),
            ("vocal" | "instrument" | "performer", false) => {
                let what = if kind == "vocal" && attrs.is_empty() { "vocals".to_string() } else { attrs.join(", ") };
                push_unique(&mut t.performers, if what.is_empty() { name } else { format!("{name} ({what})") });
            }
            _ => {}
        }
    }
}

/// Pure part of [`enrich`]: fill `release` from the full release JSON. `discid` picks the right
/// disc of a multi-disc release when it is known.
pub fn apply(release: &mut ReleaseInfo, full: &Value, discid: Option<&str>) {
    let ac = credit(full);
    if !ac.names.is_empty() {
        release.album_artists = ac.names;
    }
    if !ac.ids.is_empty() {
        release.album_artist_ids = ac.ids;
    }
    if !ac.sort_joined.is_empty() {
        release.album_artist_sort = Some(ac.sort_joined);
    }
    release.release_comment = s(full, "disambiguation");
    release.status = s(full, "status");
    release.country = s(full, "country");
    release.barcode = s(full, "barcode");
    release.asin = s(full, "asin");
    let text = full.get("text-representation").unwrap_or(&Value::Null);
    release.script = s(text, "script");
    release.language = s(text, "language");
    if let Some(li) = arr(full, "label-info").first() {
        release.label = li.get("label").and_then(|l| s(l, "name"));
        release.catalog_number = s(li, "catalog-number");
    }
    let rg = full.get("release-group").unwrap_or(&Value::Null);
    release.original_date = s(rg, "first-release-date");
    if release.mb_release_group_id.is_none() {
        release.mb_release_group_id = s(rg, "id");
    }
    let mut types: Vec<String> = s(rg, "primary-type").into_iter().collect();
    types.extend(arr(rg, "secondary-types").iter().filter_map(|t| t.as_str().map(str::to_lowercase)));
    if !types.is_empty() {
        types[0] = types[0].to_lowercase();
        release.release_type = Some(types.join("; "));
    }
    let mut g = genres(rg);
    for x in genres(full) {
        if !g.contains(&x) {
            g.push(x);
        }
    }
    release.genres = g;

    // Which disc are we? The one carrying the DiscID, else the one whose first recording matches.
    let media = arr(full, "media");
    let first_rec = release.tracks.first().and_then(|t| t.mb_recording_id.clone());
    let medium = discid
        .and_then(|d| media.iter().find(|m| arr(m, "discs").iter().any(|x| s(x, "id").as_deref() == Some(d))))
        .or_else(|| media.iter().find(|m| arr(m, "tracks").first().and_then(|t| t.get("recording")).and_then(|r| s(r, "id")) == first_rec && first_rec.is_some()))
        .or_else(|| media.iter().find(|m| arr(m, "tracks").len() == release.tracks.len()))
        .or(media.first());
    let Some(medium) = medium else { return };
    release.disc_number = medium.get("position").and_then(Value::as_u64).map(|n| n as usize);
    release.disc_total = Some(media.len());
    release.disc_title = s(medium, "title");
    release.media_format = s(medium, "format");

    let album_credit_ids = release.album_artist_ids.clone();
    for t in arr(medium, "tracks") {
        let pos = t.get("position").and_then(Value::as_u64).map(|n| n as usize);
        let Some(mine) = release.tracks.iter_mut().find(|m| Some(m.number) == pos) else { continue };
        mine.mb_release_track_id = s(t, "id");
        mine.length_ms = t.get("length").and_then(Value::as_u64);
        let rec = t.get("recording").unwrap_or(&Value::Null);
        // The track's own credit wins, then the recording's, then the album's.
        let tc = if !arr(t, "artist-credit").is_empty() { credit(t) } else { credit(rec) };
        if !tc.names.is_empty() {
            mine.artists = tc.names;
            mine.artist_ids = tc.ids;
            mine.artist_sort = Some(tc.sort_joined).filter(|x| !x.is_empty());
        } else {
            mine.artist_ids = album_credit_ids.clone();
        }
        mine.isrcs = arr(rec, "isrcs").iter().filter_map(|i| i.as_str().map(str::to_string)).collect();
        roles(arr(rec, "relations"), mine, false);
        for r in arr(rec, "relations") {
            if r.get("target-type").and_then(Value::as_str) == Some("work") {
                if let Some(w) = r.get("work") {
                    if let Some(id) = s(w, "id") {
                        push_unique(&mut mine.work_ids, id);
                    }
                    if let Some(title) = s(w, "title") {
                        push_unique(&mut mine.works, title);
                    }
                    roles(arr(w, "relations"), mine, true);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_in_the_picard_fields() {
        let full: Value = serde_json::from_str(r#"{
            "id": "rel", "title": "Album", "disambiguation": "remastered", "status": "Official", "country": "GB",
            "barcode": "123", "asin": "B00X", "text-representation": {"language": "eng", "script": "Latn"},
            "artist-credit": [
                {"name": "Simon", "joinphrase": " & ", "artist": {"id": "a1", "name": "Simon", "sort-name": "Simon, Paul"}},
                {"name": "Garfunkel", "joinphrase": "", "artist": {"id": "a2", "name": "Art Garfunkel", "sort-name": "Garfunkel, Art"}}],
            "label-info": [{"catalog-number": "CAT-1", "label": {"name": "Columbia"}}],
            "release-group": {"id": "rg", "primary-type": "Album", "secondary-types": ["Live"], "first-release-date": "1970-01-26",
                              "genres": [{"name": "folk rock", "count": 3}, {"name": "pop", "count": 1}]},
            "media": [
                {"position": 1, "format": "CD", "title": "", "discs": [{"id": "OTHER"}], "tracks": []},
                {"position": 2, "format": "CD", "title": "Bonus", "discs": [{"id": "DISC"}], "tracks": [
                    {"id": "rt1", "position": 1, "length": 200000, "title": "Song",
                     "recording": {"id": "rec1", "isrcs": ["US123"], "artist-credit": [],
                        "relations": [
                          {"target-type": "artist", "type": "producer", "artist": {"name": "Prod"}},
                          {"target-type": "artist", "type": "instrument", "attributes": ["guitar"], "artist": {"name": "Guy"}},
                          {"target-type": "artist", "type": "vocal", "attributes": [], "artist": {"name": "Voice"}},
                          {"target-type": "work", "type": "performance", "work": {"id": "w1", "title": "The Song",
                             "relations": [{"target-type": "artist", "type": "composer", "artist": {"name": "Comp"}},
                                           {"target-type": "artist", "type": "lyricist", "artist": {"name": "Lyr"}}]}}]}}]}]
        }"#).unwrap();
        let mut r = ReleaseInfo { mb_release_id: "rel".into(), tracks: vec![MbTrackInfo { number: 1, title: "Song".into(), mb_recording_id: Some("rec1".into()), ..Default::default() }], ..Default::default() };
        apply(&mut r, &full, Some("DISC"));
        assert_eq!(r.album_artist_sort.as_deref(), Some("Simon, Paul & Garfunkel, Art"));
        assert_eq!(r.album_artist_ids, ["a1", "a2"]);
        assert_eq!((r.release_comment.as_deref(), r.status.as_deref(), r.country.as_deref()), (Some("remastered"), Some("Official"), Some("GB")));
        assert_eq!((r.label.as_deref(), r.catalog_number.as_deref()), (Some("Columbia"), Some("CAT-1")));
        assert_eq!(r.original_date.as_deref(), Some("1970-01-26"));
        assert_eq!(r.release_type.as_deref(), Some("album; live"));
        assert_eq!(r.genres, ["Folk Rock", "Pop"]);
        assert_eq!((r.disc_number, r.disc_total, r.disc_title.as_deref()), (Some(2), Some(2), Some("Bonus")));
        let t = &r.tracks[0];
        assert_eq!((t.mb_release_track_id.as_deref(), t.length_ms), (Some("rt1"), Some(200000)));
        assert_eq!(t.isrcs, ["US123"]);
        assert_eq!(t.producers, ["Prod"]);
        assert_eq!(t.performers, ["Guy (guitar)", "Voice (vocals)"]);
        assert_eq!((t.works.clone(), t.work_ids.clone()), (vec!["The Song".to_string()], vec!["w1".to_string()]));
        assert_eq!((t.composers.clone(), t.lyricists.clone()), (vec!["Comp".to_string()], vec!["Lyr".to_string()]));
        // no track credit: falls back to the album artists' IDs
        assert_eq!(t.artist_ids, ["a1", "a2"]);
    }

    /// Talks to the real MusicBrainz: `cargo test live_enrich -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_enrich() {
        let (mut r, _) = crate::rip::musicbrainz::lookup_release("bc8d517f-6ce0-4e45-b6d8-af0f29cdd1ea", None, None, false).unwrap();
        enrich(&mut r, None, true);
        eprintln!("{}", serde_json::to_string_pretty(&r).unwrap());
    }
}
