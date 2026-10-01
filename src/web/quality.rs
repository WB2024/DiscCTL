//! Library: what is really in a rip's audio files: format, bit depth, bitrate, integrity,
//! loudness (with optional ReplayGain tags) and spectrograms.

use axum::{
    extract::{Path as UrlPath, Query, State},
    http::header,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{safe_join, ApiError, ApiResult, S};
use crate::{error::Error, library::audioinfo, stick::scan::is_audio_path};

fn audio_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    fn walk(d: &std::path::Path, out: &mut Vec<std::path::PathBuf>, depth: usize) {
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

fn rip_dir(st: &super::AppState, name: &str) -> ApiResult<std::path::PathBuf> {
    let dir = safe_join(&st.cfg.rips_dir, name)?;
    if !dir.is_dir() {
        return Err(ApiError::not_found(format!("No such rip: {name}")));
    }
    Ok(dir)
}

fn rel(dir: &std::path::Path, p: &std::path::Path) -> String {
    p.strip_prefix(dir).unwrap_or(p).to_string_lossy().to_string()
}

// ── Kept measurements ────────────────────────────────────────────────────────
//
// Loudness and dynamic range take a while to measure, so the results are kept in the config
// folder (not in the album, so checksums are untouched) and shown again when the album is opened.
// They are only used while the album still has the same audio files.

fn analysis_path(st: &super::AppState, name: &str) -> std::path::PathBuf {
    use sha2::{Digest, Sha256};
    let key = hex::encode(&Sha256::digest(name.as_bytes())[..12]);
    st.cfg.config_dir.join("analysis").join(format!("{key}.json"))
}

fn file_list(dir: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = audio_files(dir).iter().map(|p| rel(dir, p)).collect();
    v.sort();
    v
}

fn load_analysis(st: &super::AppState, name: &str) -> Value {
    std::fs::read(analysis_path(st, name)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_else(|| json!({}))
}

/// Keep a result. A failure to save never fails the measurement.
fn save_analysis(st: &super::AppState, name: &str, dir: &std::path::Path, kind: &str, data: &Value) {
    let mut all = load_analysis(st, name);
    let files = file_list(dir);
    // Results for a different set of files are out of date.
    if all.get("files") != Some(&json!(files)) {
        all = json!({});
    }
    all["files"] = json!(files);
    let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    all[kind] = json!({"at": at, "data": data});
    let path = analysis_path(st, name);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
        crate::perms::own_parents(&parent.join("x"));
    }
    if std::fs::write(&path, serde_json::to_vec(&all).unwrap_or_default()).is_ok() {
        crate::perms::own(&path);
    }
}

/// What was measured before for this album (loudness and dynamic range), if still current.
pub async fn analysis(State(st): S, UrlPath(name): UrlPath<String>) -> ApiResult<Json<Value>> {
    let dir = rip_dir(&st, &name)?;
    let all = load_analysis(&st, &name);
    let current = tokio::task::spawn_blocking(move || file_list(&dir)).await.map_err(|e| Error::backend(e.to_string()))?;
    if all.get("files") != Some(&json!(current)) {
        return Ok(Json(json!({"loudness": null, "dynamic_range": null})));
    }
    Ok(Json(json!({
        "loudness": all.get("loudness").cloned().unwrap_or(Value::Null),
        "dynamic_range": all.get("dynamic_range").cloned().unwrap_or(Value::Null),
    })))
}

/// Format, bit depth, sample rate and bitrate of every file, and a summary.
pub async fn facts(State(st): S, UrlPath(name): UrlPath<String>) -> ApiResult<Json<Value>> {
    let dir = rip_dir(&st, &name)?;
    let out = tokio::task::spawn_blocking(move || {
        let files = audio_files(&dir);
        let mut all = Vec::new();
        let rows: Vec<Value> = files
            .iter()
            .map(|p| match audioinfo::facts(p) {
                Ok(f) => {
                    let row = json!({"path": rel(&dir, p), "facts": f, "describe": f.describe(), "cd_quality": f.is_cd_quality()});
                    all.push(f);
                    row
                }
                Err(e) => json!({"path": rel(&dir, p), "error": e}),
            })
            .collect();
        json!({"files": rows, "summary": audioinfo::summarize(&all)})
    })
    .await
    .map_err(|e| Error::backend(e.to_string()))?;
    Ok(Json(out))
}

/// Decode every file end to end and report any that don't.
pub async fn integrity(State(st): S, UrlPath(name): UrlPath<String>) -> ApiResult<Json<Value>> {
    let dir = rip_dir(&st, &name)?;
    let out = tokio::task::spawn_blocking(move || {
        let files = audio_files(&dir);
        let results: Vec<Value> = files
            .iter()
            .map(|p| match audioinfo::integrity(p) {
                Ok(()) => json!({"path": rel(&dir, p), "ok": true}),
                Err(e) => json!({"path": rel(&dir, p), "ok": false, "error": e}),
            })
            .collect();
        let bad = results.iter().filter(|r| r["ok"] == false).count();
        json!({"results": results, "failed": bad})
    })
    .await
    .map_err(|e| Error::backend(e.to_string()))?;
    Ok(Json(out))
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct LoudnessReq {
    /// Also write ReplayGain tags into the files.
    write_replaygain: bool,
}

/// Loudness, loudness range and true peak of each track and of the album.
pub async fn loudness(State(st): S, UrlPath(name): UrlPath<String>, Json(req): Json<LoudnessReq>) -> ApiResult<Json<Value>> {
    let dir = rip_dir(&st, &name)?;
    let out = tokio::task::spawn_blocking(move || -> Result<Value, String> {
        let files = audio_files(&dir);
        if files.is_empty() {
            return Err("There's no audio in this rip".into());
        }
        let tracks: Vec<Result<audioinfo::Loudness, String>> = files.iter().map(|p| audioinfo::loudness(p)).collect();
        let refs: Vec<&std::path::Path> = files.iter().map(|p| p.as_path()).collect();
        let album = audioinfo::album_loudness(&refs)?;
        let mut written = 0usize;
        if req.write_replaygain {
            for (p, t) in files.iter().zip(&tracks) {
                if let Ok(t) = t {
                    if audioinfo::write_replaygain(p, t, &album).is_ok() {
                        written += 1;
                    }
                }
            }
        }
        if written > 0 {
            super::library_edit::refresh_manifest(&dir);
        }
        let rows: Vec<Value> = files
            .iter()
            .zip(&tracks)
            .map(|(p, t)| match t {
                Ok(l) => json!({"path": rel(&dir, p), "lufs": l.lufs, "lra": l.lra, "true_peak_db": l.true_peak_db, "gain_db": l.gain_db(), "clips": l.true_peak_db.is_some_and(|d| d > 0.0)}),
                Err(e) => json!({"path": rel(&dir, p), "error": e}),
            })
            .collect();
        Ok(json!({"tracks": rows, "album": {"lufs": album.lufs, "lra": album.lra, "true_peak_db": album.true_peak_db, "gain_db": album.gain_db()}, "written": written}))
    })
    .await
    .map_err(|e| Error::backend(e.to_string()))?
    .map_err(ApiError::bad)?;
    save_analysis(&st, &name, &rip_dir(&st, &name)?, "loudness", &out);
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct SpectroQuery {
    file: String,
}

/// A spectrogram picture of one track, drawn on first request and kept for a while.
pub async fn spectrogram(State(st): S, UrlPath(name): UrlPath<String>, Query(q): Query<SpectroQuery>) -> ApiResult<Response> {
    let dir = rip_dir(&st, &name)?;
    let file = safe_join(&dir, &q.file)?;
    if !file.is_file() || !is_audio_path(&file) {
        return Err(ApiError::not_found("That isn't an audio file in this rip"));
    }
    let key = {
        use sha2::{Digest, Sha256};
        let meta = std::fs::metadata(&file).map_err(Error::from)?;
        let mt = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
        let mut h = Sha256::new();
        h.update(format!("{}|{}|{}", file.display(), meta.len(), mt));
        hex::encode(&h.finalize()[..10])
    };
    let png = st.cfg.config_dir.join("spectrograms").join(format!("{key}.png"));
    if !png.is_file() {
        let (f, p) = (file.clone(), png.clone());
        tokio::task::spawn_blocking(move || audioinfo::spectrogram(&f, &p)).await.map_err(|e| Error::backend(e.to_string()))?.map_err(ApiError::bad)?;
    }
    let bytes = tokio::fs::read(&png).await.map_err(Error::from)?;
    Ok(([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "private, max-age=3600")], bytes).into_response())
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct DrReq {
    /// Write DR tags into the files.
    write_tags: bool,
}

/// Dynamic range (DR) of each track and the album, with a log in the usual layout.
pub async fn dynamic_range(State(st): S, UrlPath(name): UrlPath<String>, Json(req): Json<DrReq>) -> ApiResult<Json<Value>> {
    use crate::library::{dynrange, tags};
    let dir = rip_dir(&st, &name)?;
    let album_key = name.clone();
    let out = tokio::task::spawn_blocking(move || -> Result<Value, String> {
        let files = audio_files(&dir);
        if files.is_empty() {
            return Err("There's no audio in this rip".into());
        }
        let first = tags::read(&files[0]);
        let artist = first.vars.get("albumartist").or_else(|| first.vars.get("artist")).cloned().unwrap_or_default();
        let album = first.vars.get("album").cloned().unwrap_or_else(|| name.clone());
        let mut rows: Vec<(String, dynrange::TrackDr)> = Vec::new();
        let mut json_rows: Vec<Value> = Vec::new();
        let mut facts_first = None;
        for p in &files {
            let f = audioinfo::facts(p)?;
            let title = tags::read(p).vars.get("title").cloned().unwrap_or_else(|| p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default());
            match dynrange::measure(p, f.channels.unwrap_or(2) as usize, f.sample_rate.unwrap_or(44_100) as usize) {
                Ok(t) => {
                    json_rows.push(json!({"path": rel(&dir, p), "title": title, "dr": t.dr, "dr_exact": t.dr_exact, "peak_db": t.peak_db, "rms_db": t.rms_db, "seconds": t.seconds}));
                    rows.push((title, t));
                }
                Err(e) => json_rows.push(json!({"path": rel(&dir, p), "title": title, "error": e})),
            }
            facts_first.get_or_insert(f);
        }
        let tracks: Vec<dynrange::TrackDr> = rows.iter().map(|r| r.1.clone()).collect();
        let album_dr = dynrange::album_dr(&tracks).ok_or("None of the tracks could be measured")?;
        let mut written = 0usize;
        if req.write_tags {
            for (p, r) in files.iter().zip(json_rows.iter()) {
                if let Some(dr) = r.get("dr").and_then(Value::as_u64) {
                    if dynrange::write_tags(p, dr as u32, album_dr).is_ok() {
                        written += 1;
                    }
                }
            }
            if written > 0 {
                super::library_edit::refresh_manifest(&dir);
            }
        }
        Ok(json!({
            "tracks": json_rows, "album_dr": album_dr, "verdict": dynrange::verdict(album_dr), "written": written,
            "log": dynrange::log_text(&artist, &album, &rows, facts_first.as_ref()),
            "database_url": dynrange::database_url(&artist), "artist": artist, "album": album,
            "submit": {
                "artist": artist, "album": album,
                "year": first.vars.get("originalyear").or_else(|| first.vars.get("date")).map(|d| d.chars().take(4).collect::<String>()).unwrap_or_default(),
                "codec": if facts_first.as_ref().map(|f| f.lossless).unwrap_or(false) { "lossless" } else { "lossy" },
                "source": "cdda",
                "label": first.vars.get("label").cloned().unwrap_or_default(),
                "catalogNumber": first.vars.get("catalognumber").cloned().unwrap_or_default(),
                "barCode": first.vars.get("barcode").cloned().unwrap_or_default(),
                "country": first.vars.get("releasecountry").cloned().unwrap_or_default(),
                "link": first.vars.get("musicbrainz_albumid").map(|id| format!("https://musicbrainz.org/release/{id}")).unwrap_or_default(),
                "log": dynrange::log_text(&artist, &album, &rows, facts_first.as_ref()),
            },
        }))
    })
    .await
    .map_err(|e| Error::backend(e.to_string()))?
    .map_err(ApiError::bad)?;
    save_analysis(&st, &album_key, &rip_dir(&st, &album_key)?, "dynamic_range", &out);
    Ok(Json(out))
}

/// The saved rip report (and its readable log) for an album, if the rip made one.
pub async fn rip_report(State(st): S, UrlPath(name): UrlPath<String>) -> ApiResult<Json<Value>> {
    use crate::rip::report;
    let dir = rip_dir(&st, &name)?;
    let Some(json_path) = report::find(&dir) else {
        return Ok(Json(json!({ "found": false })));
    };
    let parsed: report::RipReport = serde_json::from_slice(&tokio::fs::read(&json_path).await.map_err(Error::from)?)
        .map_err(|e| ApiError::bad(format!("The rip report is unreadable: {e}")))?;
    Ok(Json(json!({ "found": true, "report": parsed, "log": report::render(&parsed) })))
}
