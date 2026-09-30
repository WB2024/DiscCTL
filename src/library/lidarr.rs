//! Handing a finished rip to Lidarr.
//!
//! A rip already carries the MusicBrainz IDs of its release, so there is nothing to guess. This
//! finds the album in Lidarr (adding the artist and album, unmonitored, if they aren't there),
//! asks Lidarr to match the files to its tracks, and has it import them with its own naming and
//! rules. RustyDisc only decides *what* to import; where it lands is Lidarr's business.

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use serde::Serialize;
use serde_json::{json, Value};

use super::{import, tags};
use crate::{error::Error, rip::musicbrainz, stick::scan::is_audio_path};

#[derive(Debug, Clone)]
pub struct Lidarr {
    pub url: String,
    pub api_key: String,
    /// Where new artists go (a Lidarr root folder). Empty = Lidarr's first.
    pub root_folder: String,
    pub quality_profile: Option<u64>,
    pub metadata_profile: Option<u64>,
    /// A RustyDisc path prefix and what Lidarr calls it (they differ when the containers mount things differently).
    pub path_from: String,
    pub path_to: String,
    /// "move" or "copy"
    pub mode: String,
}

impl Lidarr {
    fn agent(&self) -> ureq::Agent {
        ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(8)).timeout(Duration::from_secs(120)).build()
    }

    fn base(&self) -> String {
        format!("{}/api/v1", self.url.trim_end_matches('/'))
    }

    fn fail(&self, what: &str, e: ureq::Error) -> Error {
        match e {
            ureq::Error::Status(401 | 403, _) => Error::backend("Lidarr refused the API key. Check it in Settings → Lidarr."),
            ureq::Error::Status(code, resp) => {
                let body = resp.into_string().unwrap_or_default();
                Error::backend(format!("Lidarr said {code} when {what}: {}", body.chars().take(300).collect::<String>()))
            }
            other => Error::backend(format!("Couldn't reach Lidarr at {} ({what}): {other}", self.url)),
        }
    }

    fn get(&self, path: &str, query: &[(&str, &str)]) -> Result<Value, Error> {
        let mut r = self.agent().get(&format!("{}/{}", self.base(), path)).set("X-Api-Key", &self.api_key);
        for (k, v) in query {
            r = r.query(k, v);
        }
        r.call().map_err(|e| self.fail(&format!("asking for {path}"), e))?.into_json().map_err(|e| Error::backend(format!("Lidarr's answer for {path} couldn't be read: {e}")))
    }

    fn post(&self, path: &str, body: &Value) -> Result<Value, Error> {
        self.agent()
            .post(&format!("{}/{}", self.base(), path))
            .set("X-Api-Key", &self.api_key)
            .send_json(body.clone())
            .map_err(|e| self.fail(&format!("sending {path}"), e))?
            .into_json()
            .map_err(|e| Error::backend(format!("Lidarr's answer for {path} couldn't be read: {e}")))
    }

    /// RustyDisc's path → the path Lidarr sees.
    pub fn map_path(&self, p: &Path) -> String {
        let s = p.to_string_lossy().to_string();
        let from = self.path_from.trim_end_matches('/');
        if !from.is_empty() && (s == from || s.starts_with(&format!("{from}/"))) {
            return format!("{}{}", self.path_to.trim_end_matches('/'), &s[from.len()..]);
        }
        s
    }

    /// Version, root folders and profiles: for the Settings "Test" button.
    pub fn test(&self) -> Result<Value, Error> {
        let status = self.get("system/status", &[])?;
        let pick = |v: Value, fields: &[&str]| -> Vec<Value> {
            v.as_array().map(|a| a.iter().map(|x| Value::Object(fields.iter().filter_map(|f| x.get(*f).map(|v| (f.to_string(), v.clone()))).collect())).collect()).unwrap_or_default()
        };
        Ok(json!({
            "version": status.get("version"),
            "app": status.get("appName"),
            "root_folders": pick(self.get("rootfolder", &[])?, &["id", "path", "defaultQualityProfileId", "defaultMetadataProfileId"]),
            "quality_profiles": pick(self.get("qualityprofile", &[])?, &["id", "name"]),
            "metadata_profiles": pick(self.get("metadataprofile", &[])?, &["id", "name"]),
        }))
    }
}

// ── Finding the album ─────────────────────────────────────────────────────────

/// What a rip says about itself.
#[derive(Debug, Clone, Default, Serialize)]
pub struct RipIdentity {
    pub album: Option<String>,
    pub artist: Option<String>,
    pub release_id: Option<String>,
    pub release_group_id: Option<String>,
    pub audio_files: usize,
}

pub fn identify(rip: &Path) -> RipIdentity {
    let mut files = Vec::new();
    fn walk(d: &Path, out: &mut Vec<PathBuf>, depth: usize) {
        if depth > 6 {
            return;
        }
        if let Ok(rd) = std::fs::read_dir(d) {
            let mut e: Vec<_> = rd.filter_map(|e| e.ok()).collect();
            e.sort_by_key(|e| e.file_name());
            for e in e {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out, depth + 1);
                } else if is_audio_path(&p) {
                    out.push(p);
                }
            }
        }
    }
    walk(rip, &mut files, 0);
    let mut id = RipIdentity { audio_files: files.len(), ..Default::default() };
    if let Some(first) = files.first() {
        let t = tags::read(first);
        id.album = t.vars.get("album").cloned();
        id.artist = t.vars.get("albumartist").or_else(|| t.vars.get("artist")).cloned();
        id.release_id = t.vars.get("musicbrainz_albumid").cloned();
        id.release_group_id = t.vars.get("musicbrainz_releasegroupid").cloned();
    }
    id
}

#[derive(Debug, Clone, Serialize)]
pub struct AlbumMatch {
    pub album: String,
    pub artist: String,
    pub foreign_album_id: String,
    pub foreign_artist_id: String,
    pub album_in_lidarr: bool,
    pub artist_in_lidarr: bool,
    /// How it was found: "musicbrainz release group", "musicbrainz release", "search"
    pub how: String,
    #[serde(skip)]
    pub lookup: Value,
}

fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect()
}

/// Lidarr keys albums by MusicBrainz release group. A rip knows its release, so ask
/// MusicBrainz which group that is.
fn group_of_release(release_id: &str) -> Option<String> {
    musicbrainz::lookup_release(release_id, None, None, false).ok().and_then(|(r, _)| r.mb_release_group_id)
}

fn to_match(album: &Value, how: &str) -> AlbumMatch {
    let artist = album.get("artist").unwrap_or(&Value::Null);
    AlbumMatch {
        album: album.get("title").and_then(Value::as_str).unwrap_or("").to_string(),
        artist: artist.get("artistName").and_then(Value::as_str).unwrap_or("").to_string(),
        foreign_album_id: album.get("foreignAlbumId").and_then(Value::as_str).unwrap_or("").to_string(),
        foreign_artist_id: artist.get("foreignArtistId").and_then(Value::as_str).unwrap_or("").to_string(),
        album_in_lidarr: album.get("id").and_then(Value::as_u64).unwrap_or(0) > 0,
        artist_in_lidarr: artist.get("id").and_then(Value::as_u64).unwrap_or(0) > 0,
        how: how.to_string(),
        lookup: album.clone(),
    }
}

/// Find the rip's album in Lidarr. `override_id` (a MusicBrainz release or release group ID or URL) wins.
pub fn find_album(l: &Lidarr, id: &RipIdentity, override_id: Option<&str>) -> Result<AlbumMatch, Error> {
    let (mut group, mut release) = (id.release_group_id.clone(), id.release_id.clone());
    if let Some(o) = override_id.map(str::trim).filter(|o| !o.is_empty()) {
        if o.contains("/release-group/") {
            group = Some(musicbrainz::parse_uuid(o)?);
            release = None;
        } else {
            release = Some(musicbrainz::parse_release_id(o)?);
            group = None;
        }
    }
    let mut how = "musicbrainz release group";
    if group.is_none() {
        if let Some(r) = &release {
            group = group_of_release(r);
            how = "musicbrainz release";
        }
    }
    if let Some(g) = &group {
        let found = l.get("album/lookup", &[("term", &format!("lidarr:{g}"))])?;
        if let Some(a) = found.as_array().and_then(|a| a.iter().find(|a| a.get("foreignAlbumId").and_then(Value::as_str) == Some(g.as_str())).or_else(|| a.first())) {
            return Ok(to_match(a, how));
        }
    }
    // No MusicBrainz IDs (or Lidarr doesn't know them): search by name, and only accept a clear match.
    let (Some(album), Some(artist)) = (&id.album, &id.artist) else {
        return Err(Error::validation("This rip has no MusicBrainz IDs or album tags, so Lidarr can't tell what it is. Paste a MusicBrainz link for it."));
    };
    let found = l.get("album/lookup", &[("term", &format!("{artist} {album}"))])?;
    let hit = found.as_array().and_then(|a| {
        a.iter().find(|x| norm(x.get("title").and_then(Value::as_str).unwrap_or("")) == norm(album) && norm(x.get("artist").and_then(|a| a.get("artistName")).and_then(Value::as_str).unwrap_or("")) == norm(artist))
    });
    match hit {
        Some(a) => Ok(to_match(a, "search")),
        None => Err(Error::validation(format!("Lidarr has no clear match for “{album}” by {artist}. Paste the album's MusicBrainz link to choose it."))),
    }
}

// ── Plan ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct MatchedFile {
    pub name: String,
    pub track: Option<String>,
    pub quality: Option<String>,
    pub rejections: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct LidarrPlan {
    pub rip: String,
    pub identity: RipIdentity,
    #[serde(rename = "match")]
    pub album_match: Option<AlbumMatch>,
    pub error: Option<String>,
    /// Lidarr will first add the artist and/or album (unmonitored).
    pub will_add_artist: bool,
    pub will_add_album: bool,
    pub files: Vec<MatchedFile>,
    pub matched: usize,
    pub unmatched: usize,
    pub warnings: Vec<String>,
}

fn audio_root(rip: &Path) -> PathBuf {
    rip.to_path_buf()
}

/// Ask Lidarr how it would match the files in the folder.
fn manual_import_items(l: &Lidarr, folder: &Path, artist_id: Option<u64>) -> Result<Vec<Value>, Error> {
    let mapped = l.map_path(&audio_root(folder));
    let mut q = vec![("folder", mapped.as_str()), ("filterExistingFiles", "false"), ("replaceExistingFiles", "true")];
    let aid = artist_id.map(|a| a.to_string());
    if let Some(a) = &aid {
        q.push(("artistId", a));
    }
    Ok(self_items(l.get("manualimport", &q)?))
}

fn self_items(v: Value) -> Vec<Value> {
    v.as_array().cloned().unwrap_or_default()
}

fn describe(items: &[Value]) -> Vec<MatchedFile> {
    items
        .iter()
        .map(|i| MatchedFile {
            name: i.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
            track: i.get("tracks").and_then(Value::as_array).and_then(|t| t.first()).and_then(|t| t.get("title")).and_then(Value::as_str).map(str::to_string),
            quality: i.get("quality").and_then(|q| q.get("quality")).and_then(|q| q.get("name")).and_then(Value::as_str).map(str::to_string),
            rejections: i.get("rejections").and_then(Value::as_array).map(|r| r.iter().filter_map(|x| x.get("reason").and_then(Value::as_str).map(str::to_string)).collect()).unwrap_or_default(),
        })
        .collect()
}

fn importable(i: &Value) -> bool {
    i.get("album").map(|a| !a.is_null()).unwrap_or(false)
        && i.get("artist").map(|a| !a.is_null()).unwrap_or(false)
        && i.get("tracks").and_then(Value::as_array).is_some_and(|t| !t.is_empty())
        && i.get("rejections").and_then(Value::as_array).is_none_or(|r| r.is_empty())
}

pub fn plan(l: &Lidarr, rip: &Path, override_id: Option<&str>) -> Result<LidarrPlan, Error> {
    if !rip.is_dir() {
        return Err(Error::validation(format!("The rip folder isn't there: {}", rip.display())));
    }
    let identity = identify(rip);
    if identity.audio_files == 0 {
        return Err(Error::validation("There's no music in that folder to import"));
    }
    let mut out = LidarrPlan {
        rip: rip.to_string_lossy().to_string(), identity: identity.clone(), album_match: None, error: None,
        will_add_artist: false, will_add_album: false, files: vec![], matched: 0, unmatched: 0, warnings: vec![],
    };
    let m = match find_album(l, &identity, override_id) {
        Ok(m) => m,
        Err(e) => {
            out.error = Some(e.to_string());
            return Ok(out);
        }
    };
    out.will_add_artist = !m.artist_in_lidarr;
    out.will_add_album = !m.album_in_lidarr;
    if l.path_from.is_empty() {
        out.warnings.push("No path mapping is set. If Lidarr sees the rips folder under a different path, set it in Settings → Lidarr.".into());
    }
    if m.artist_in_lidarr {
        let artist_id = m.lookup.get("artist").and_then(|a| a.get("id")).and_then(Value::as_u64);
        let items = manual_import_items(l, rip, artist_id)?;
        out.matched = items.iter().filter(|i| importable(i)).count();
        out.unmatched = items.len().saturating_sub(out.matched);
        if items.is_empty() {
            out.warnings.push("Lidarr can't see any files in that folder. Check the path mapping (RustyDisc path → Lidarr path) in Settings → Lidarr.".into());
        }
        out.files = describe(&items);
    } else {
        out.warnings.push("The artist isn't in Lidarr yet. It will be added (unmonitored) first, then the files matched.".into());
    }
    out.album_match = Some(m);
    Ok(out)
}

// ── Import ────────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Serialize)]
pub struct Summary {
    pub imported: usize,
    pub left: usize,
    pub added_artist: bool,
    pub added_album: bool,
    pub deleted: usize,
    pub rip_removed: bool,
}

pub struct Progress<'a> {
    pub step: &'a dyn Fn(&str),
    pub pct: &'a dyn Fn(f32),
}

fn add_to_lidarr(l: &Lidarr, m: &AlbumMatch) -> Result<Value, Error> {
    let roots = l.get("rootfolder", &[])?;
    let root = if !l.root_folder.is_empty() {
        l.root_folder.clone()
    } else {
        roots.as_array().and_then(|r| r.first()).and_then(|r| r.get("path")).and_then(Value::as_str).map(str::to_string).ok_or_else(|| Error::validation("Lidarr has no root folder to put a new artist in"))?
    };
    let root_entry = roots.as_array().and_then(|r| r.iter().find(|r| r.get("path").and_then(Value::as_str).map(|p| p.trim_end_matches('/')) == Some(root.trim_end_matches('/')))).cloned().unwrap_or(Value::Null);
    let default_id = |field: &str| root_entry.get(field).and_then(Value::as_u64);
    let quality = l.quality_profile.or_else(|| default_id("defaultQualityProfileId")).unwrap_or(1);
    let metadata = l.metadata_profile.or_else(|| default_id("defaultMetadataProfileId")).unwrap_or(1);

    let mut album = m.lookup.clone();
    let mut artist = album.get("artist").cloned().unwrap_or(Value::Null);
    if let Some(o) = artist.as_object_mut() {
        o.insert("rootFolderPath".into(), json!(root));
        o.insert("qualityProfileId".into(), json!(quality));
        o.insert("metadataProfileId".into(), json!(metadata));
        o.insert("monitored".into(), json!(false));
        o.insert("monitorNewItems".into(), json!("none"));
        o.insert("addOptions".into(), json!({"monitor": "none", "searchForMissingAlbums": false}));
    }
    if let Some(o) = album.as_object_mut() {
        o.insert("artist".into(), artist);
        o.insert("monitored".into(), json!(false));
        o.insert("addOptions".into(), json!({"searchForNewAlbum": false}));
    }
    l.post("album", &album)
}

/// After adding, Lidarr fetches the artist's albums in the background: wait until ours is there.
fn wait_for_album(l: &Lidarr, foreign_album_id: &str, patience: Duration) -> Result<Value, Error> {
    let start = Instant::now();
    loop {
        let found = l.get("album", &[("foreignAlbumId", foreign_album_id)])?;
        if let Some(a) = found.as_array().and_then(|a| a.first()) {
            return Ok(a.clone());
        }
        if start.elapsed() > patience {
            return Err(Error::backend("Lidarr is still loading the artist. Try the import again in a minute."));
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

fn wait_for_command(l: &Lidarr, id: u64, patience: Duration) -> Result<Value, Error> {
    let start = Instant::now();
    loop {
        let c = l.get(&format!("command/{id}"), &[])?;
        match c.get("status").and_then(Value::as_str) {
            Some("completed") => return Ok(c),
            Some("failed" | "aborted" | "cancelled" | "orphaned") => {
                return Err(Error::backend(format!("Lidarr's import failed: {}", c.get("message").and_then(Value::as_str).unwrap_or("no reason given"))));
            }
            _ => {}
        }
        if start.elapsed() > patience {
            return Err(Error::backend("Lidarr is still importing after a long time; check its Activity page."));
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

fn count_audio(rip: &Path) -> usize {
    identify(rip).audio_files
}

pub fn execute(l: &Lidarr, rip: &Path, override_id: Option<&str>, delete_leftovers: bool, progress: &Progress) -> Result<Summary, Error> {
    let mut summary = Summary::default();
    let identity = identify(rip);
    (progress.step)("Finding the album in Lidarr...");
    let m = find_album(l, &identity, override_id)?;
    (progress.pct)(10.0);

    let (artist_id, album) = if m.artist_in_lidarr && m.album_in_lidarr {
        (m.lookup.get("artist").and_then(|a| a.get("id")).and_then(Value::as_u64), m.lookup.clone())
    } else {
        (progress.step)(&format!("Adding {} — {} to Lidarr (unmonitored)...", m.artist, m.album));
        summary.added_artist = !m.artist_in_lidarr;
        summary.added_album = !m.album_in_lidarr;
        let added = add_to_lidarr(l, &m)?;
        let album = if added.get("id").and_then(Value::as_u64).unwrap_or(0) > 0 { added } else { wait_for_album(l, &m.foreign_album_id, Duration::from_secs(120))? };
        let aid = album.get("artistId").and_then(Value::as_u64).or_else(|| album.get("artist").and_then(|a| a.get("id")).and_then(Value::as_u64));
        (aid, album)
    };
    let _ = album;
    (progress.pct)(30.0);

    (progress.step)("Asking Lidarr to match the files...");
    let items = manual_import_items(l, rip, artist_id)?;
    let good: Vec<&Value> = items.iter().filter(|i| importable(i)).collect();
    if good.is_empty() {
        let why = describe(&items).into_iter().flat_map(|f| f.rejections).next().unwrap_or_else(|| "Lidarr didn't match any of the files to the album".into());
        return Err(Error::validation(format!("Lidarr can't import these files: {why}")));
    }
    if good.len() < items.len() {
        (progress.step)(&format!("Note: Lidarr matched {} of {} files; the rest stay in the rip folder.", good.len(), items.len()));
    }
    (progress.pct)(45.0);

    let files: Vec<Value> = good
        .iter()
        .map(|i| {
            json!({
                "path": i.get("path"),
                "artistId": i.get("artist").and_then(|a| a.get("id")),
                "albumId": i.get("album").and_then(|a| a.get("id")),
                "albumReleaseId": i.get("albumReleaseId"),
                "trackIds": i.get("tracks").and_then(Value::as_array).map(|t| t.iter().filter_map(|x| x.get("id").cloned()).collect::<Vec<_>>()).unwrap_or_default(),
                "quality": i.get("quality"),
                "indexerFlags": i.get("indexerFlags").cloned().unwrap_or(json!(0)),
                "downloadId": "",
                "disableReleaseSwitching": false,
            })
        })
        .collect();
    (progress.step)(&format!("Lidarr is importing {} file(s)...", files.len()));
    let cmd = l.post("command", &json!({"name": "ManualImport", "files": files, "importMode": if l.mode == "copy" { "copy" } else { "move" }}))?;
    let id = cmd.get("id").and_then(Value::as_u64).ok_or_else(|| Error::backend("Lidarr didn't start the import"))?;
    wait_for_command(l, id, Duration::from_secs(30 * 60))?;
    (progress.pct)(90.0);

    let left = count_audio(rip);
    summary.left = left;
    summary.imported = if l.mode == "copy" { good.len() } else { good.len().saturating_sub(left.min(good.len())) };

    // Only when every file was matched, and (for a move) nothing audible is left behind.
    if delete_leftovers && good.len() == items.len() && (l.mode == "copy" || left == 0) {
        (progress.step)("Deleting what's left in the rip folder...");
        let (n, removed) = import::remove_leftovers(rip);
        summary.deleted = n;
        summary.rip_removed = removed;
    }
    (progress.pct)(99.0);
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::{Arc, Mutex},
    };

    /// A stand-in for Lidarr: answers the few calls the import makes and records what it was sent.
    struct Fake {
        url: String,
        posts: Arc<Mutex<Vec<(String, Value)>>>,
    }

    fn fake(artist_known: bool) -> Fake {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let posts: Arc<Mutex<Vec<(String, Value)>>> = Arc::default();
        let (p2, added) = (posts.clone(), Arc::new(Mutex::new(false)));
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { break };
                let mut buf = vec![0u8; 65536];
                let mut n = 0;
                let (head, body) = loop {
                    let r = s.read(&mut buf[n..]).unwrap_or(0);
                    if r == 0 { break (String::new(), String::new()); }
                    n += r;
                    let text = String::from_utf8_lossy(&buf[..n]).to_string();
                    if let Some(i) = text.find("\r\n\r\n") {
                        let head = text[..i].to_string();
                        let want = head.lines().find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))).unwrap_or(0);
                        if text.len() >= i + 4 + want { break (head, text[i + 4..i + 4 + want].to_string()); }
                    }
                };
                let first = head.lines().next().unwrap_or("").to_string();
                let mut parts = first.split(' ');
                let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or("").to_string());
                let path = target.split('?').next().unwrap_or("").to_string();
                let key_ok = head.to_lowercase().contains("x-api-key: secret");
                let album = |id: u64, artist_id: u64| json!({"id": id, "title": "Wild Thing", "foreignAlbumId": "rg-1", "artistId": artist_id,
                    "artist": {"id": artist_id, "artistName": "The Troggs", "foreignArtistId": "artist-1"}});
                let (status, reply) = if !key_ok {
                    (401, json!({}))
                } else {
                    match (method, path.as_str()) {
                        ("GET", "/api/v1/system/status") => (200, json!({"version": "3.1.5", "appName": "Lidarr"})),
                        ("GET", "/api/v1/rootfolder") => (200, json!([{"id": 1, "path": "/music", "defaultQualityProfileId": 5, "defaultMetadataProfileId": 4}])),
                        ("GET", "/api/v1/qualityprofile") => (200, json!([{"id": 2, "name": "Lossless"}])),
                        ("GET", "/api/v1/metadataprofile") => (200, json!([{"id": 4, "name": "All Official"}])),
                        ("GET", "/api/v1/album/lookup") => (200, json!([album(if artist_known { 11 } else { 0 }, if artist_known { 3 } else { 0 })])),
                        ("POST", "/api/v1/album") => { *added.lock().unwrap() = true; (200, album(11, 3)) }
                        ("GET", "/api/v1/album") => (200, json!([album(11, 3)])),
                        ("GET", "/api/v1/manualimport") => (200, json!([
                            {"path": "/data/rips/a/01.flac", "name": "01", "artist": {"id": 3}, "album": {"id": 11}, "albumReleaseId": 40,
                             "tracks": [{"id": 101, "title": "Wild Thing"}], "quality": {"quality": {"id": 6, "name": "FLAC"}}, "rejections": [], "indexerFlags": 0},
                            {"path": "/data/rips/a/99.flac", "name": "99", "artist": null, "album": null, "tracks": [], "quality": {"quality": {"id": 6, "name": "FLAC"}},
                             "rejections": [{"reason": "Unknown Artist"}]}])),
                        ("POST", "/api/v1/command") => (200, json!({"id": 77, "status": "queued"})),
                        ("GET", "/api/v1/command/77") => (200, json!({"id": 77, "status": "completed"})),
                        _ => (404, json!({})),
                    }
                };
                if method == "POST" {
                    p2.lock().unwrap().push((path, serde_json::from_str(&body).unwrap_or(Value::Null)));
                }
                let text = reply.to_string();
                let _ = write!(s, "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", text.len());
            }
        });
        Fake { url, posts }
    }

    fn client(f: &Fake) -> Lidarr {
        Lidarr { url: f.url.clone(), api_key: "secret".into(), root_folder: String::new(), quality_profile: None, metadata_profile: None, path_from: "/rips".into(), path_to: "/data/rips".into(), mode: "move".into() }
    }

    fn rip_with_ids(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rd_lidarr_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("01.flac");
        assert!(std::process::Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=duration=1", "-c:a", "flac"]).arg(&p).status().map(|s| s.success()).unwrap_or(false));
        let r = crate::rip::musicbrainz::ReleaseInfo { mb_release_id: "55555555-5555-4555-8555-555555555555".into(), album: "Wild Thing".into(), album_artist: "The Troggs".into(), mb_release_group_id: Some("rg-1".into()), ..Default::default() };
        let basic = crate::rip::encoder::TrackTags { title: Some("Wild Thing".into()), artist: Some("The Troggs".into()), album: Some("Wild Thing".into()), album_artist: Some("The Troggs".into()), track_number: Some(1), track_total: Some(1), ..Default::default() };
        crate::rip::tagging::apply(p.to_str().unwrap(), &basic, Some(&r), None, false);
        dir
    }

    #[test]
    fn paths_are_mapped_to_what_lidarr_sees() {
        let l = Lidarr { url: String::new(), api_key: String::new(), root_folder: String::new(), quality_profile: None, metadata_profile: None, path_from: "/rips/".into(), path_to: "/data/CD Rips".into(), mode: "move".into() };
        assert_eq!(l.map_path(Path::new("/rips/Album (2001)")), "/data/CD Rips/Album (2001)");
        assert_eq!(l.map_path(Path::new("/rips")), "/data/CD Rips");
        assert_eq!(l.map_path(Path::new("/ripsy/x")), "/ripsy/x");
        assert_eq!(l.map_path(Path::new("/other/x")), "/other/x");
    }

    #[test]
    fn a_wrong_api_key_is_reported_clearly() {
        let f = fake(true);
        let mut l = client(&f);
        l.api_key = "wrong".into();
        assert!(l.test().unwrap_err().to_string().contains("API key"));
        assert_eq!(client(&f).test().unwrap()["version"], "3.1.5");
    }

    #[test]
    fn the_plan_shows_what_lidarr_would_match() {
        let dir = rip_with_ids("plan");
        let f = fake(true);
        let p = plan(&client(&f), &dir, None).unwrap();
        let m = p.album_match.as_ref().unwrap();
        assert_eq!((m.album.as_str(), m.artist.as_str(), m.how.as_str()), ("Wild Thing", "The Troggs", "musicbrainz release group"));
        assert!(!p.will_add_artist && !p.will_add_album);
        assert_eq!((p.matched, p.unmatched), (1, 1));
        assert_eq!(p.files[1].rejections, ["Unknown Artist"]);

        let f2 = fake(false);
        let p2 = plan(&client(&f2), &dir, None).unwrap();
        assert!(p2.will_add_artist && p2.will_add_album && p2.files.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn importing_adds_the_missing_artist_unmonitored_then_imports_the_matched_files() {
        let dir = rip_with_ids("exec");
        let f = fake(false);
        let mut l = client(&f);
        l.quality_profile = Some(2);
        let s = execute(&l, &dir, None, false, &Progress { step: &|_| {}, pct: &|_| {} }).unwrap();
        assert!(s.added_artist && s.added_album);
        let posts = f.posts.lock().unwrap();
        let add = &posts.iter().find(|(p, _)| p == "/api/v1/album").expect("album added").1;
        assert_eq!(add["monitored"], false);
        assert_eq!(add["artist"]["rootFolderPath"], "/music");
        assert_eq!(add["artist"]["qualityProfileId"], 2);
        assert_eq!(add["artist"]["metadataProfileId"], 4, "the root folder's default");
        assert_eq!(add["artist"]["addOptions"]["monitor"], "none");
        assert_eq!(add["addOptions"]["searchForNewAlbum"], false);
        let cmd = &posts.iter().find(|(p, _)| p == "/api/v1/command").expect("import started").1;
        assert_eq!(cmd["name"], "ManualImport");
        assert_eq!(cmd["importMode"], "move");
        let files = cmd["files"].as_array().unwrap();
        assert_eq!(files.len(), 1, "the rejected file isn't sent");
        assert_eq!((files[0]["artistId"].as_u64(), files[0]["albumId"].as_u64(), files[0]["albumReleaseId"].as_u64()), (Some(3), Some(11), Some(40)));
        assert_eq!(files[0]["trackIds"][0], 101);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_known_artist_is_not_added_again() {
        let dir = rip_with_ids("known");
        let f = fake(true);
        let mut l = client(&f);
        l.mode = "copy".into();
        let s = execute(&l, &dir, None, false, &Progress { step: &|_| {}, pct: &|_| {} }).unwrap();
        assert!(!s.added_artist && !s.added_album);
        let posts = f.posts.lock().unwrap();
        assert!(posts.iter().all(|(p, _)| p != "/api/v1/album"));
        assert_eq!(posts.iter().find(|(p, _)| p == "/api/v1/command").unwrap().1["importMode"], "copy");
        std::fs::remove_dir_all(&dir).ok();
    }
}
