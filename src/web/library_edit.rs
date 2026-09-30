//! Library: uploading a cover, and looking at and editing tags.

use std::{collections::HashMap, path::{Path, PathBuf}};

use axum::{
    body::Bytes,
    extract::{Path as UrlPath, Query, State},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{path_str, safe_join, ApiError, ApiResult, AppState, S};
use crate::{
    error::Error,
    library::tagedit::{self, Edit},
    rip::cover::sniff_ext,
    stick::scan::is_audio_path,
};

pub const MAX_IMAGE: usize = 25 * 1024 * 1024;

fn manifest_path(dir: &Path) -> Option<PathBuf> {
    ["metadata/checksums.json", "checksums.json"].iter().map(|f| dir.join(f)).find(|p| p.is_file())
}

/// After the audio files are changed (tags, cover, ReplayGain) a rip's saved checksums have to be
/// brought up to date, or "Verify checksums" would report the edit as damage.
pub fn refresh_manifest(dir: &Path) {
    use crate::rip::metadata;
    let Some(path) = manifest_path(dir) else { return };
    if let (Ok(m), Some(parent)) = (metadata::generate_checksums(&dir.to_string_lossy()), path.parent()) {
        let _ = metadata::write_checksums(&m, &parent.to_string_lossy());
    }
}

/// Create (or refresh) the SHA-256 checksums of a rip that was made without archive mode, so it can be verified later.
pub async fn create_checksums(State(st): S, UrlPath(name): UrlPath<String>) -> ApiResult<Json<Value>> {
    let dir = rip_dir(&st, &name)?;
    let n = tokio::task::spawn_blocking(move || -> Result<usize, Error> {
        use crate::rip::metadata;
        let target = manifest_path(&dir).and_then(|p| p.parent().map(|d| d.to_path_buf())).unwrap_or_else(|| dir.clone());
        let m = metadata::generate_checksums(&dir.to_string_lossy())?;
        metadata::write_checksums(&m, &target.to_string_lossy())?;
        Ok(m.files.len())
    })
    .await
    .map_err(|e| Error::backend(e.to_string()))??;
    Ok(Json(json!({"files": n})))
}

fn image_of(body: &Bytes) -> ApiResult<&'static str> {
    if body.is_empty() {
        return Err(ApiError::bad("No image was sent"));
    }
    sniff_ext(body).ok_or_else(|| ApiError::bad("That isn't a JPEG or PNG picture"))
}

// ── Cover uploaded on the Rip page ───────────────────────────────────────────

fn uploads_dir(st: &AppState) -> PathBuf {
    st.cfg.config_dir.join("uploads")
}

/// A stored upload by its token, if it exists.
pub fn upload_path(st: &AppState, token: &str) -> Option<PathBuf> {
    if token.is_empty() || !token.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    ["jpg", "png"].iter().map(|e| uploads_dir(st).join(format!("{token}.{e}"))).find(|p| p.is_file())
}

pub async fn upload_cover(State(st): S, body: Bytes) -> ApiResult<Json<Value>> {
    let ext = image_of(&body)?;
    let dir = uploads_dir(&st);
    let len = body.len();
    let token = tokio::task::spawn_blocking(move || -> Result<String, Error> {
        std::fs::create_dir_all(&dir)?;
        // Old uploads (a day or more) are cleared out as new ones arrive.
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.filter_map(|e| e.ok()) {
                let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|a| a.as_secs() > 86_400);
                if old {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        use sha2::{Digest, Sha256};
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let mut h = Sha256::new();
        h.update(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0).to_le_bytes());
        h.update(N.fetch_add(1, std::sync::atomic::Ordering::Relaxed).to_le_bytes());
        h.update(&body[..body.len().min(4096)]);
        let token = hex::encode(&h.finalize()[..12]);
        std::fs::write(dir.join(format!("{token}.{ext}")), &body)?;
        Ok(token)
    })
    .await
    .map_err(|e| Error::backend(e.to_string()))??;
    Ok(Json(json!({"token": token, "ext": ext, "bytes": len})))
}

// ── Library: tags ────────────────────────────────────────────────────────────

fn rip_dir(st: &AppState, name: &str) -> ApiResult<PathBuf> {
    let dir = safe_join(&st.cfg.rips_dir, name)?;
    if !dir.is_dir() {
        return Err(ApiError::not_found(format!("No such rip: {name}")));
    }
    Ok(dir)
}

fn audio_files(dir: &Path) -> Vec<PathBuf> {
    fn walk(d: &Path, out: &mut Vec<PathBuf>, depth: usize) {
        if depth > 6 || out.len() >= 1000 {
            return;
        }
        let Ok(rd) = std::fs::read_dir(d) else { return };
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
    let mut v = Vec::new();
    walk(dir, &mut v, 0);
    v
}

pub async fn tags(State(st): S, UrlPath(name): UrlPath<String>) -> ApiResult<Json<Value>> {
    let dir = rip_dir(&st, &name)?;
    let out = tokio::task::spawn_blocking(move || {
        audio_files(&dir)
            .iter()
            .map(|p| {
                let rel = p.strip_prefix(&dir).unwrap_or(p).to_string_lossy().to_string();
                let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
                json!({"path": rel, "size": size, "tags": tagedit::view(p)})
            })
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|e| Error::backend(e.to_string()))?;
    Ok(Json(json!({
        "fields": tagedit::FIELDS.iter().map(|(n, l, g)| json!({"name": n, "label": l, "group": g})).collect::<Vec<_>>(),
        "files": out,
    })))
}

#[derive(Deserialize)]
pub struct FileEdit {
    file: String,
    #[serde(default)]
    set: HashMap<String, String>,
    /// tag name → new value, or null to remove it
    #[serde(default)]
    raw: HashMap<String, Option<String>>,
}

#[derive(Deserialize)]
pub struct TagsReq {
    edits: Vec<FileEdit>,
}

pub async fn save_tags(State(st): S, UrlPath(name): UrlPath<String>, Json(req): Json<TagsReq>) -> ApiResult<Json<Value>> {
    let dir = rip_dir(&st, &name)?;
    if req.edits.is_empty() {
        return Err(ApiError::bad("Nothing to change"));
    }
    let mut jobs: Vec<(String, PathBuf, Edit)> = Vec::new();
    for e in req.edits {
        let p = safe_join(&dir, &e.file)?;
        if !p.is_file() || !is_audio_path(&p) {
            return Err(ApiError::bad(format!("'{}' isn't an audio file in this rip", e.file)));
        }
        jobs.push((e.file, p, Edit { set: e.set, raw: e.raw }));
    }
    let results = tokio::task::spawn_blocking(move || {
        let dir2 = dir.clone();
        let out: Vec<Value> = jobs.into_iter()
            .map(|(file, p, edit)| match tagedit::apply(&p, &edit) {
                Ok(()) => json!({"file": file, "ok": true}),
                Err(e) => json!({"file": file, "ok": false, "error": e}),
            })
            .collect();
        refresh_manifest(&dir2);
        out
    })
    .await
    .map_err(|e| Error::backend(e.to_string()))?;
    let failed = results.iter().filter(|r| r["ok"] == false).count();
    Ok(Json(json!({"results": results, "failed": failed})))
}

// ── Library: cover ───────────────────────────────────────────────────────────

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct CoverQuery {
    /// Put it in every audio file ("1"/"true").
    embed: Option<String>,
    /// Keep it as cover.jpg/png in the album folder.
    save: Option<String>,
}

fn on(v: &Option<String>, default: bool) -> bool {
    match v.as_deref() {
        Some("1" | "true" | "on") => true,
        Some("0" | "false" | "off") => false,
        _ => default,
    }
}

pub async fn set_cover(State(st): S, UrlPath(name): UrlPath<String>, Query(q): Query<CoverQuery>, body: Bytes) -> ApiResult<Json<Value>> {
    let dir = rip_dir(&st, &name)?;
    let ext = image_of(&body)?;
    let (embed, save) = (on(&q.embed, true), on(&q.save, true));
    if !embed && !save {
        return Err(ApiError::bad("Choose to embed the picture, save it, or both"));
    }
    let out = tokio::task::spawn_blocking(move || {
        let mut saved: Option<String> = None;
        if save {
            // One cover file: the new one replaces any cover.jpg / cover.png already there.
            for old in ["cover.jpg", "cover.jpeg", "cover.png"] {
                let _ = std::fs::remove_file(dir.join(old));
            }
            let target = dir.join(format!("cover.{ext}"));
            if std::fs::write(&target, &body).is_ok() {
                saved = Some(format!("cover.{ext}"));
            }
        }
        let (mut embedded, mut failed) = (0usize, Vec::<Value>::new());
        if embed {
            for p in audio_files(&dir) {
                match tagedit::embed_cover(&p, &body, ext) {
                    Ok(()) => embedded += 1,
                    Err(e) => failed.push(json!({"file": p.file_name().map(|n| n.to_string_lossy().to_string()), "error": e})),
                }
            }
        }
        refresh_manifest(&dir);
        json!({"saved": saved, "embedded": embedded, "failed": failed})
    })
    .await
    .map_err(|e| Error::backend(e.to_string()))?;
    let _ = path_str;
    Ok(Json(out))
}
