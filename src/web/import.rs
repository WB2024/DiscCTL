//! Importing ripped albums into the music library: listing rips, planning, and the job.

use std::{collections::HashMap, path::PathBuf};

use axum::{extract::State, Json};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{path_str, safe_join, start_job, spawn_cli, ApiError, ApiResult, AppState, S};
use crate::{
    error::Error,
    library::{
        import::{self, ImportOptions, Mode},
        script, tags,
    },
    stick::existing::Conflict,
};

/// Where the library is: the setting, else `--library-dir`.
fn library(st: &AppState) -> Option<PathBuf> {
    let s = st.settings.get().library_path;
    if !s.is_empty() {
        return Some(PathBuf::from(s));
    }
    st.cfg.library_dir.clone()
}

fn script_text(st: &AppState) -> String {
    let s = st.settings.get().library_script;
    if s.trim().is_empty() { script::DEFAULT_SCRIPT.to_string() } else { s }
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct Overrides {
    mode: Option<String>,
    include_cover: Option<bool>,
    include_other: Option<bool>,
    delete_leftovers: Option<bool>,
    conflict: Option<String>,
}

fn options(st: &AppState, o: &Overrides) -> ApiResult<ImportOptions> {
    let lib = library(st).ok_or_else(|| ApiError::bad("Set the music library folder in Settings first"))?;
    let s = st.settings.get();
    let mut opts = ImportOptions::new(lib, script_text(st));
    opts.mode = o.mode.as_deref().unwrap_or(&s.import_mode).parse::<Mode>().map_err(ApiError::bad)?;
    opts.include_cover = o.include_cover.unwrap_or(s.import_cover);
    opts.include_other = o.include_other.unwrap_or(s.import_other);
    opts.delete_leftovers = o.delete_leftovers.unwrap_or(s.import_delete_leftovers);
    opts.conflict = o.conflict.as_deref().unwrap_or(&s.import_conflict).parse::<Conflict>().map_err(ApiError::bad)?;
    Ok(opts)
}

/// The rips that can be imported: each folder of the rips directory that holds music.
pub async fn list(State(st): S) -> ApiResult<Json<Value>> {
    let dir = st.cfg.rips_dir.clone();
    let lib = library(&st);
    let entries = tokio::task::spawn_blocking(move || {
        let mut out: Vec<Value> = Vec::new();
        let Ok(rd) = std::fs::read_dir(&dir) else { return out };
        let mut names: Vec<_> = rd.filter_map(|e| e.ok()).filter(|e| e.path().is_dir()).collect();
        names.sort_by_key(|e| e.file_name().to_string_lossy().to_lowercase());
        for e in names {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let peek = import::peek(&e.path());
            let tracks: usize = peek.get("tracks").and_then(|t| t.parse().ok()).unwrap_or(0);
            if tracks == 0 {
                continue;
            }
            let marker = import::imported_marker(&e.path());
            out.push(json!({
                "name": name, "tracks": tracks,
                "album": peek.get("album"), "artist": peek.get("artist"),
                "tagged": peek.get("tagged").map(|v| v == "true").unwrap_or(false),
                "has_mb": peek.get("has_mb").map(|v| v == "true").unwrap_or(false),
                "imported_at": marker.as_ref().and_then(|m| m.get("imported_at")).cloned(),
                "imported_to": marker.as_ref().and_then(|m| m.get("album_dir")).cloned(),
            }));
        }
        out
    })
    .await
    .map_err(|e| Error::backend(e.to_string()))?;
    let s = st.settings.get();
    Ok(Json(json!({
        "rips_dir": path_str(&st.cfg.rips_dir),
        "library": lib.map(|l| path_str(&l)),
        "entries": entries,
        "defaults": {
            "mode": s.import_mode, "include_cover": s.import_cover, "include_other": s.import_other,
            "delete_leftovers": s.import_delete_leftovers, "conflict": s.import_conflict,
        },
        "conflicts": [
            {"id": "skip", "label": "Skip files already in the library"},
            {"id": "higher_quality", "label": "Replace if the new file is higher quality"},
            {"id": "lower_quality", "label": "Replace if the new file is lower quality"},
            {"id": "newer", "label": "Replace if the new file is newer"},
            {"id": "replace", "label": "Always replace"},
            {"id": "keep_both", "label": "Keep both"},
        ],
    })))
}

#[derive(Deserialize)]
pub struct PlanReq {
    rips: Vec<String>,
    #[serde(default)]
    options: Overrides,
}

fn rip_paths(st: &AppState, names: &[String]) -> ApiResult<Vec<PathBuf>> {
    if names.is_empty() {
        return Err(ApiError::bad("Choose at least one rip"));
    }
    names.iter().map(|n| safe_join(&st.cfg.rips_dir, n)).collect()
}

pub async fn plan(State(st): S, Json(req): Json<PlanReq>) -> ApiResult<Json<Value>> {
    let opts = options(&st, &req.options)?;
    let paths = rip_paths(&st, &req.rips)?;
    let plans = tokio::task::spawn_blocking(move || -> Result<Vec<Value>, Error> {
        paths.iter().map(|p| Ok(serde_json::to_value(import::plan(p, &opts)?)?)).collect()
    })
    .await
    .map_err(|e| Error::backend(e.to_string()))??;
    Ok(Json(json!({"plans": plans})))
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct PreviewReq {
    /// The script to try; empty = the saved one.
    script: String,
    /// A rip folder to take the sample track from.
    rip: Option<String>,
}

/// Run a naming script on a sample track and show the path it makes.
pub async fn preview(State(st): S, Json(req): Json<PreviewReq>) -> ApiResult<Json<Value>> {
    let text = if req.script.trim().is_empty() { script_text(&st) } else { req.script.clone() };
    let sample_from = req.rip.as_deref().filter(|r| !r.is_empty()).map(|r| safe_join(&st.cfg.rips_dir, r)).transpose()?;
    let out = tokio::task::spawn_blocking(move || {
        if let Err(e) = script::check(&text) {
            return json!({"ok": false, "error": e});
        }
        // A real track's tags if we have them, else a made-up example.
        let mut real: Option<(String, HashMap<String, String>)> = None;
        if let Some(dir) = sample_from {
            let mut found = Vec::new();
            fn walk(d: &std::path::Path, out: &mut Vec<PathBuf>, depth: usize) {
                if out.len() >= 1 || depth > 4 {
                    return;
                }
                if let Ok(rd) = std::fs::read_dir(d) {
                    let mut e: Vec<_> = rd.filter_map(|e| e.ok()).collect();
                    e.sort_by_key(|e| e.file_name());
                    for e in e {
                        let p = e.path();
                        if p.is_dir() {
                            walk(&p, out, depth + 1);
                        } else if crate::stick::scan::is_audio_path(&p) {
                            out.push(p);
                            return;
                        }
                    }
                }
            }
            walk(&dir, &mut found, 0);
            if let Some(f) = found.first() {
                let t = tags::read(f);
                real = Some((f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), t.vars));
            }
        }
        let (label, vars) = real.unwrap_or_else(|| {
            let v: HashMap<String, String> = [
                ("albumartist", "The Troggs"), ("albumartistsort", "Troggs, The"), ("artist", "The Troggs"), ("album", "Wild Thing"),
                ("date", "1966-06-01"), ("tracknumber", "1"), ("totaltracks", "12"), ("title", "Wild Thing"), ("totaldiscs", "1"), ("_extension", "flac"),
            ]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
            ("an example track".to_string(), v)
        });
        match script::run(&text, &vars) {
            Ok(o) => {
                let mut path = script::to_components(&o).join("/");
                if let Some(ext) = vars.get("_extension").filter(|e| !e.is_empty()) {
                    path = format!("{path}.{ext}");
                }
                json!({"ok": true, "path": path, "sample": label})
            }
            Err(e) => json!({"ok": false, "error": e}),
        }
    })
    .await
    .map_err(|e| Error::backend(e.to_string()))?;
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct StartReq {
    rips: Vec<String>,
    #[serde(default)]
    options: Overrides,
    #[serde(default)]
    dry_run: bool,
}

pub async fn start(State(st): S, Json(req): Json<StartReq>) -> ApiResult<Json<Value>> {
    let opts = options(&st, &req.options)?;
    let paths = rip_paths(&st, &req.rips)?;
    if st.jobs.all().iter().any(|j| j.kind == "import" && !j.status().is_terminal()) {
        return Err(ApiError::bad("An import is already running"));
    }
    // The script goes in a file: it is long, and the command line isn't the place for it.
    let script_path = std::env::temp_dir().join(format!("rustydisc-naming-{}.txt", std::process::id()));
    std::fs::write(&script_path, &opts.script).map_err(Error::from)?;

    let mut a: Vec<String> = vec!["import".into(), "--library".into(), path_str(&opts.library), "--script-file".into(), path_str(&script_path), "--progress-json".into()];
    for p in &paths {
        a.extend(["--rip".into(), path_str(p)]);
    }
    a.extend(["--mode".into(), match opts.mode { Mode::Move => "move", Mode::Copy => "copy", Mode::Hardlink => "hardlink" }.into()]);
    if !opts.include_cover { a.push("--no-cover".into()); }
    if opts.include_other { a.push("--include-other".into()); }
    if opts.delete_leftovers { a.push("--delete-leftovers".into()); }
    a.extend(["--on-conflict".into(), opts.conflict.id().into()]);
    if req.dry_run { a.push("--dry-run".into()); }

    let title = if req.rips.len() == 1 { format!("Import {}", req.rips[0]) } else { format!("Import {} albums", req.rips.len()) };
    let job = start_job(&st, "import", &title, false)?;
    spawn_cli(&st, job.clone(), a, Some(script_path));
    Ok(Json(json!(job.summary())))
}

pub async fn default_script() -> Json<Value> {
    Json(json!({"script": script::DEFAULT_SCRIPT}))
}

// ── Lidarr ───────────────────────────────────────────────────────────────────

use crate::library::lidarr::{self, Lidarr};

fn lidarr_client(st: &AppState, url: Option<&str>, key: Option<&str>) -> ApiResult<Lidarr> {
    let s = st.settings.get();
    let url = url.filter(|u| !u.trim().is_empty()).map(|u| u.trim().trim_end_matches('/').to_string()).unwrap_or(s.lidarr_url.clone());
    let api_key = key.filter(|k| !k.trim().is_empty()).map(|k| k.trim().to_string()).unwrap_or(s.lidarr_api_key.clone());
    if url.is_empty() {
        return Err(ApiError::bad("Enter Lidarr's address first"));
    }
    if api_key.is_empty() {
        return Err(ApiError::bad("Enter Lidarr's API key first (Lidarr → Settings → General)"));
    }
    Ok(Lidarr {
        url,
        api_key,
        root_folder: s.lidarr_root_folder,
        quality_profile: Some(s.lidarr_quality_profile).filter(|p| *p > 0),
        metadata_profile: Some(s.lidarr_metadata_profile).filter(|p| *p > 0),
        path_from: s.lidarr_path_from,
        path_to: s.lidarr_path_to,
        mode: s.lidarr_mode,
    })
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct LidarrTest {
    url: String,
    api_key: String,
}

/// Check the address and key, and list what Lidarr offers (root folders, profiles).
pub async fn lidarr_test(State(st): S, Json(req): Json<LidarrTest>) -> ApiResult<Json<Value>> {
    let l = lidarr_client(&st, Some(&req.url), Some(&req.api_key))?;
    let out = tokio::task::spawn_blocking(move || l.test()).await.map_err(|e| Error::backend(e.to_string()))??;
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct LidarrPlanReq {
    rips: Vec<String>,
    /// rip folder name → a MusicBrainz ID or URL, for a wrong match
    #[serde(default)]
    matches: HashMap<String, String>,
}

pub async fn lidarr_plan(State(st): S, Json(req): Json<LidarrPlanReq>) -> ApiResult<Json<Value>> {
    let l = lidarr_client(&st, None, None)?;
    let paths = rip_paths(&st, &req.rips)?;
    let names = req.rips.clone();
    let matches = req.matches.clone();
    let plans = tokio::task::spawn_blocking(move || -> Result<Vec<Value>, Error> {
        paths.iter().zip(names.iter()).map(|(p, n)| Ok(serde_json::to_value(lidarr::plan(&l, p, matches.get(n).map(String::as_str))?)?)).collect()
    })
    .await
    .map_err(|e| Error::backend(e.to_string()))??;
    Ok(Json(json!({"plans": plans, "path_mapped": !st.settings.get().lidarr_path_from.is_empty()})))
}

#[derive(Deserialize)]
pub struct LidarrStartReq {
    rips: Vec<String>,
    #[serde(default)]
    matches: HashMap<String, String>,
    #[serde(default)]
    delete_leftovers: Option<bool>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

pub async fn lidarr_start(State(st): S, Json(req): Json<LidarrStartReq>) -> ApiResult<Json<Value>> {
    let l = lidarr_client(&st, None, None)?;
    let paths = rip_paths(&st, &req.rips)?;
    if st.jobs.all().iter().any(|j| j.kind == "import" && !j.status().is_terminal()) {
        return Err(ApiError::bad("An import is already running"));
    }
    let s = st.settings.get();
    let mode = req.mode.clone().unwrap_or(s.lidarr_mode.clone());
    let mut a: Vec<String> = vec!["import-lidarr".into(), "--url".into(), l.url.clone(), "--progress-json".into(), "--mode".into(), mode];
    for p in &paths {
        a.extend(["--rip".into(), path_str(p)]);
    }
    if !l.root_folder.is_empty() { a.extend(["--root-folder".into(), l.root_folder.clone()]); }
    if let Some(q) = l.quality_profile { a.extend(["--quality-profile".into(), q.to_string()]); }
    if let Some(m) = l.metadata_profile { a.extend(["--metadata-profile".into(), m.to_string()]); }
    if !l.path_from.is_empty() { a.extend(["--path-from".into(), l.path_from.clone(), "--path-to".into(), l.path_to.clone()]); }
    for (name, id) in &req.matches {
        if req.rips.contains(name) && !id.trim().is_empty() {
            a.extend(["--match".into(), format!("{name}={}", id.trim())]);
        }
    }
    if req.delete_leftovers.unwrap_or(s.import_delete_leftovers) { a.push("--delete-leftovers".into()); }
    if req.dry_run { a.push("--dry-run".into()); }

    let title = if req.rips.len() == 1 { format!("Import {} via Lidarr", req.rips[0]) } else { format!("Import {} albums via Lidarr", req.rips.len()) };
    let job = start_job(&st, "import", &title, false)?;
    // The API key travels in the environment, not on the command line.
    super::spawn_cli_env(&st, job.clone(), a, None, vec![("RUSTYDISC_LIDARR_KEY".into(), l.api_key.clone())]);
    Ok(Json(json!(job.summary())))
}
