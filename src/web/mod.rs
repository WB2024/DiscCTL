//! Web UI + JSON API for RustyDisc.
//!
//! Read-only queries (disc info, MusicBrainz, library) run in-process; anything that
//! writes or takes a long time (rip, burn, recover, verify) is a job that runs the
//! `rustydisc` CLI as a child process with `--progress-json` and is followed by the
//! browser over Server-Sent Events.

mod auth;
mod cache;
mod import;
mod jobs;
mod library_edit;
mod quality;
mod mock;
mod settings;
mod stick;

use std::{
    convert::Infallible,
    net::SocketAddr,
    path::{Component, Path, PathBuf},
    sync::{atomic::{AtomicU64, Ordering}, Arc},
};

use axum::{
    Json, Router,
    extract::{Path as UrlPath, Query, State},
    http::{StatusCode, header},
    response::{Html, IntoResponse, Response, Sse, sse},
    routing::{get, post},
};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{Value, json};
use tower_http::services::ServeDir;

use crate::{
    analyzer,
    error::{DiscError, Error},
    rip::{self, cover::{self, Cover}, encoder::AudioFormat, musicbrainz},
};
use jobs::{Event, Jobs};

const INDEX_HTML: &str = include_str!("index.html");
const DR_USERSCRIPT: &str = include_str!("../../userscripts/dynamic-range-db.user.js");

pub struct Config {
    pub exe: PathBuf,
    pub device: String,
    /// Where rips are written and the library is read from.
    pub rips_dir: PathBuf,
    /// Where burn sources (audio files, data folders, playlists) are picked from.
    pub media_dir: PathBuf,
    /// Where settings.json lives (mount a volume here in Docker).
    pub config_dir: PathBuf,
    /// Extra folders Rusty Stick may write to (besides detected USB sticks).
    pub stick_dirs: Vec<PathBuf>,
    /// Default place for converted files (Settings can override it).
    pub cache_dir: Option<PathBuf>,
    /// Default music library folder (Settings can override it).
    pub library_dir: Option<PathBuf>,
    pub mock: bool,
    /// Login from the command line / environment: (user, password).
    pub auth: Option<(String, String)>,
}

struct AppState {
    cfg: Config,
    jobs: Jobs,
    settings: settings::Store,
    auth: auth::Auth,
    cover_cache: std::sync::Mutex<std::collections::HashMap<String, Option<Arc<Cover>>>>,
    /// The last scan of a stick job's sources (tags take a while to read).
    stick_scan: std::sync::Mutex<Option<(String, std::time::Instant, Arc<crate::stick::scan::Scan>)>>,
    /// What was found on a stick, kept until its contents change.
    stick_existing: std::sync::Mutex<std::collections::HashMap<String, ((u64, u64, i64), Arc<Vec<crate::stick::existing::ExistingTrack>>)>>,
}

type S = State<Arc<AppState>>;
type ApiResult<T> = Result<T, ApiError>;

// ── Errors ───────────────────────────────────────────────────────────────────

pub struct ApiError(StatusCode, DiscError);

impl ApiError {
    fn new(status: StatusCode, code: &str, msg: impl Into<String>, recoverable: bool) -> Self {
        ApiError(status, DiscError { error: code.into(), message: msg.into(), recoverable })
    }
    fn bad(msg: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "VALIDATION_ERROR", msg, false)
    }
    fn not_found(msg: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "NOT_FOUND", msg, false)
    }
    fn busy(job: &jobs::Job) -> Self {
        Self::new(
            StatusCode::CONFLICT,
            "DRIVE_BUSY",
            format!("The drive is in use by job #{} ({})", job.id, job.title),
            true,
        )
    }
}

impl From<Error> for ApiError {
    fn from(e: Error) -> Self {
        let status = match e {
            Error::Validation(_) | Error::Json(_) | Error::Glob(_) => StatusCode::BAD_REQUEST,
            Error::Device(_) | Error::Backend(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Error::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        ApiError(status, e.to_disc_error())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(self.1)).into_response()
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Join a user-supplied relative path onto `root`, refusing `..` and absolute paths.
fn safe_join(root: &Path, rel: &str) -> ApiResult<PathBuf> {
    let rel = rel.trim_start_matches('/');
    let p = Path::new(rel);
    for c in p.components() {
        match c {
            Component::Normal(_) | Component::CurDir => {}
            _ => return Err(ApiError::bad(format!("Invalid path: {rel}"))),
        }
    }
    Ok(root.join(p))
}

fn check_device(dev: &str) -> ApiResult<()> {
    if dev.starts_with("/dev/") && !dev.contains("..") && !dev.chars().any(char::is_whitespace) {
        Ok(())
    } else {
        Err(ApiError::bad(format!("Invalid device path: {dev}")))
    }
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().to_string()
}

fn find_in_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|d| d.join(name).is_file()))
        .unwrap_or(false)
}

fn require_drive_free(st: &AppState) -> ApiResult<()> {
    match st.jobs.busy() {
        Some(j) => Err(ApiError::busy(&j)),
        None => Ok(()),
    }
}

fn start_job(st: &Arc<AppState>, kind: &str, title: &str, exclusive: bool) -> ApiResult<Arc<jobs::Job>> {
    st.jobs.create(kind, title, exclusive).map_err(|busy| ApiError::busy(&busy))
}

fn spawn_cli(st: &Arc<AppState>, job: Arc<jobs::Job>, args: Vec<String>, cleanup: Option<PathBuf>) {
    spawn_cli_env(st, job, args, cleanup, Vec::new());
}

/// Like `spawn_cli`, with extra environment variables for the child (used for secrets, so
/// they don't appear on the command line).
fn spawn_cli_env(st: &Arc<AppState>, job: Arc<jobs::Job>, args: Vec<String>, cleanup: Option<PathBuf>, envs: Vec<(String, String)>) {
    let exe = st.cfg.exe.clone();
    tokio::spawn(async move {
        jobs::run_process(job, exe, args, envs).await;
        if let Some(p) = cleanup {
            let _ = std::fs::remove_file(p);
        }
    });
}

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn write_temp_graph(graph: &Value) -> ApiResult<PathBuf> {
    let p = std::env::temp_dir().join(format!(
        "rustydisc-graph-{}-{}.json",
        std::process::id(),
        TMP_COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::write(&p, serde_json::to_vec_pretty(graph).map_err(Error::from)?).map_err(Error::from)?;
    Ok(p)
}

// ── Router ───────────────────────────────────────────────────────────────────

pub async fn serve(cfg: Config, bind: SocketAddr) -> Result<(), Error> {
    std::fs::create_dir_all(&cfg.rips_dir)?;
    let rips_dir = cfg.rips_dir.clone();
    let media_dir = cfg.media_dir.clone();
    let settings = settings::Store::load(&cfg.config_dir);
    let (env_user, env_pass) = cfg.auth.clone().unzip();
    let auth = auth::Auth::new(env_user, env_pass).map_err(Error::validation)?;
    let state = Arc::new(AppState { cfg, jobs: Jobs::default(), settings, auth, cover_cache: Default::default(), stick_scan: Default::default(), stick_existing: Default::default() });

    cache::spawn_tidier(state.clone());

    let app = Router::new()
        .route("/", get(|| async { Html(INDEX_HTML) }))
        .route("/api/import/list", get(import::list))
        .route("/api/import/plan", post(import::plan))
        .route("/api/import/preview", post(import::preview))
        .route("/api/import/default-script", get(import::default_script))
        .route("/api/import/script/builder", get(import::script_builder))
        .route("/api/import/script/generate", post(import::script_generate))
        .route("/api/lidarr/test", post(import::lidarr_test))
        .route("/api/import/lidarr/plan", post(import::lidarr_plan))
        .route("/api/jobs/import-lidarr", post(import::lidarr_start))
        .route("/api/jobs/import", post(import::start))
        .route("/api/cache", get(cache::info))
        .route("/api/cache/clear", post(cache::clear))
        .route("/api/status", get(status))
        .route("/api/login", post(auth::login))
        .route("/api/logout", post(auth::logout))
        .route("/api/auth/status", get(auth::status))
        .route("/api/auth/config", post(auth::configure))
        .route("/userscripts/dynamic-range-db.user.js", get(|| async { ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8")], DR_USERSCRIPT) }))
        .route("/api/info", get(info))
        .route("/api/musicbrainz", get(musicbrainz_lookup))
        .route("/api/musicbrainz/search", get(musicbrainz_search))
        .route("/api/cover", get(cover))
        .route("/api/cover/info", get(cover_info))
        .route("/api/settings", get(get_settings).put(put_settings))
        .route("/api/stick/targets", get(stick::targets))
        .route("/api/stick/plan", post(stick::plan))
        .route("/api/stick/mount", post(stick::mount))
        .route("/api/stick/identify", post(stick::identify))
        .route("/api/stick/browse", post(stick::browse))
        .route("/api/stick/format", post(stick::format))
        .route("/api/stick/eject", post(stick::eject))
        .route("/api/jobs/stick", post(stick::start))
        .route("/api/settings/test-fanart", post(test_fanart))
        .route("/api/eject", post(eject))
        .route("/api/browse", get(browse))
        .route("/api/library", get(library))
        .route("/api/library/{name}", get(library_entry))
        .route("/api/library/{name}/checksums", post(library_edit::create_checksums))
        .route("/api/library/{name}/quality", get(quality::facts))
        .route("/api/library/{name}/quality/integrity", post(quality::integrity))
        .route("/api/library/{name}/quality/loudness", post(quality::loudness))
        .route("/api/library/{name}/quality/dynamic-range", post(quality::dynamic_range))
        .route("/api/library/{name}/report", get(quality::rip_report))
        .route("/api/library/{name}/spectrogram", get(quality::spectrogram))
        .route("/api/library/{name}/tags", get(library_edit::tags).put(library_edit::save_tags))
        .route("/api/library/{name}/cover", post(library_edit::set_cover).layer(axum::extract::DefaultBodyLimit::max(library_edit::MAX_IMAGE)))
        .route("/api/cover/upload", post(library_edit::upload_cover).layer(axum::extract::DefaultBodyLimit::max(library_edit::MAX_IMAGE)))
        .route("/api/plan", post(plan))
        .route("/api/playlist", post(playlist_preview))
        .route("/api/audio-files", get(audio_files))
        .route("/api/validate", post(validate))
        .route("/api/jobs", get(list_jobs))
        .route("/api/jobs/rip", post(start_rip))
        .route("/api/jobs/burn", post(start_burn))
        .route("/api/jobs/recover", post(start_recover))
        .route("/api/jobs/verify", post(start_verify))
        .route("/api/jobs/{id}", get(get_job))
        .route("/api/jobs/{id}/events", get(job_events))
        .route("/api/jobs/{id}/cancel", post(cancel_job))
        .route("/api/jobs/{id}/continue", post(continue_job))
        .nest_service("/files/rips", ServeDir::new(rips_dir))
        .nest_service("/files/media", ServeDir::new(media_dir))
        .layer(axum::middleware::from_fn_with_state(state.clone(), auth::guard))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|e| Error::backend(format!("Cannot bind {bind}: {e}")))?;
    eprintln!("RustyDisc web UI listening on http://{bind}");
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .map_err(|e| Error::backend(e.to_string()))
}

// ── Status / disc info ───────────────────────────────────────────────────────

async fn status(State(st): S) -> Json<Value> {
    let mut devices: Vec<String> = std::fs::read_dir("/dev")
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| n.starts_with("sr") || n.starts_with("scd"))
                .map(|n| format!("/dev/{n}"))
                .collect()
        })
        .unwrap_or_default();
    devices.sort();
    if !devices.contains(&st.cfg.device) {
        devices.insert(0, st.cfg.device.clone());
    }

    let deps: Vec<Value> = [
        ("cdparanoia", "Secure audio ripping"),
        ("cdrdao", "Audio burning, CD-Text"),
        ("xorriso", "Data burning and extraction"),
        ("cdrecord", "TOC reading, disc state"),
        ("isoinfo", "Data session info"),
        ("ffmpeg", "Encoding, transcoding, cover art"),
        ("eject", "Ejecting the tray"),
    ]
    .iter()
    .map(|(n, why)| json!({"name": n, "purpose": why, "found": find_in_path(n)}))
    .collect();

    let quality_map: std::collections::BTreeMap<String, Vec<Value>> = ["flac", "wav", "alac", "aiff", "ogg", "mp3", "opus", "aac"]
        .iter()
        .map(|f| {
            let fmt: AudioFormat = f.parse().expect("known format");
            (f.to_string(), rip::encoder::quality_choices(&fmt).iter().map(|c| json!({"id": c.id, "label": c.label, "note": c.note})).collect())
        })
        .collect();
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "mock": st.cfg.mock,
        "default_device": st.cfg.device,
        "devices": devices,
        "rips_dir": path_str(&st.cfg.rips_dir),
        "media_dir": path_str(&st.cfg.media_dir),
        "formats": ["flac", "wav", "alac", "aiff", "ogg", "mp3", "opus", "aac"],
        "quality_choices": quality_map,
        "media_presets": crate::planner::discs::PRESETS.iter().map(|p| json!({"id": p.id, "label": p.label, "mb": p.mb, "media": p.media})).collect::<Vec<_>>(),
        "deps": deps,
        "busy_job": st.jobs.busy().map(|j| j.summary()),
    }))
}

#[derive(Deserialize)]
struct InfoQuery {
    device: Option<String>,
    /// Mock mode only: redbook | bluebook | data | none
    scenario: Option<String>,
}

async fn info(State(st): S, Query(q): Query<InfoQuery>) -> ApiResult<Json<Value>> {
    require_drive_free(&st)?;
    let device = q.device.unwrap_or_else(|| st.cfg.device.clone());
    check_device(&device)?;

    if st.cfg.mock {
        let scenario = q.scenario.unwrap_or_else(|| "redbook".into());
        return mock::info(&scenario, &device)
            .map(Json)
            .map_err(|(code, msg)| ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, &code, msg, false));
    }

    let info = tokio::task::spawn_blocking(move || analyzer::analyze(&device))
        .await
        .map_err(|e| Error::backend(e.to_string()))??;
    Ok(Json(serde_json::to_value(info).map_err(Error::from)?))
}

#[derive(Deserialize)]
struct MbQuery {
    /// Look the disc up by its MusicBrainz DiscID...
    discid: Option<String>,
    /// ...or fetch a specific release (ID or URL) instead.
    release: Option<String>,
    /// With `release`: the disc's audio track count, to pick the right disc of a multi-disc release.
    tracks: Option<usize>,
}

async fn musicbrainz_lookup(State(st): S, Query(q): Query<MbQuery>) -> ApiResult<Json<Value>> {
    if let Some(wanted) = q.release {
        let mbid = musicbrainz::parse_release_id(&wanted)?;
        if st.cfg.mock {
            let mut r = mock::release();
            r.mb_release_id = mbid;
            return Ok(Json(serde_json::to_value(r).map_err(Error::from)?));
        }
        let discid = q.discid;
        let (release, warning) = tokio::task::spawn_blocking(move || {
            musicbrainz::lookup_release(&mbid, discid.as_deref(), q.tracks, false)
        })
        .await
        .map_err(|e| Error::backend(e.to_string()))??;
        let mut v = serde_json::to_value(release).map_err(Error::from)?;
        v["warning"] = json!(warning);
        return Ok(Json(v));
    }

    let Some(discid) = q.discid else {
        return Err(ApiError::bad("Provide a discid or a release"));
    };
    if st.cfg.mock {
        if discid == mock::UNMATCHED_DISCID {
            return Ok(Json(Value::Null));
        }
        return Ok(Json(serde_json::to_value(mock::release()).map_err(Error::from)?));
    }
    let release = tokio::task::spawn_blocking(move || musicbrainz::lookup(&discid, false))
        .await
        .map_err(|e| Error::backend(e.to_string()))??;
    Ok(Json(serde_json::to_value(release).map_err(Error::from)?))
}

#[derive(Deserialize, Default)]
struct SearchParams {
    q: Option<String>,
    artist: Option<String>,
    tracks: Option<usize>,
    cd_only: Option<bool>,
    offset: Option<usize>,
}

async fn musicbrainz_search(State(st): S, Query(p): Query<SearchParams>) -> ApiResult<Json<Value>> {
    let query = musicbrainz::SearchQuery {
        text: p.q.unwrap_or_default(),
        artist: p.artist.unwrap_or_default(),
        tracks: p.tracks.filter(|&n| n > 0),
        cd_only: p.cd_only.unwrap_or(false),
        offset: p.offset.unwrap_or(0),
    };
    if st.cfg.mock {
        return Ok(Json(mock::search(&query)));
    }
    let results = tokio::task::spawn_blocking(move || musicbrainz::search_releases(&query, false))
        .await
        .map_err(|e| Error::backend(e.to_string()))??;
    Ok(Json(serde_json::to_value(results).map_err(Error::from)?))
}

#[derive(Deserialize)]
struct CoverQuery {
    mbid: String,
    /// MusicBrainz release group (fanart.tv indexes albums by it).
    rg: Option<String>,
    /// Thumbnail width: 250, 500 or 1200. Thumbnails always come from the Cover Art Archive.
    size: Option<u32>,
}

fn looks_like_mbid(s: &str) -> bool {
    s.len() <= 40 && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// The cover the configured sources give for a release, cached briefly so the preview
/// image and its "where did it come from" label cost one lookup.
async fn best_cover(st: &Arc<AppState>, mbid: &str, rg: Option<&str>) -> Option<Arc<Cover>> {
    let opts = st.settings.get().cover_options();
    let key = format!(
        "{mbid}|{}|{}|{}",
        rg.unwrap_or(""),
        opts.sources.iter().map(|s| s.id()).collect::<Vec<_>>().join(","),
        opts.fanart_key.is_some()
    );
    if let Some(hit) = st.cover_cache.lock().unwrap().get(&key) {
        return hit.clone();
    }
    let (m, r) = (mbid.to_string(), rg.map(str::to_string));
    let found = tokio::task::spawn_blocking(move || cover::fetch(&m, r.as_deref(), &opts, false))
        .await
        .ok()
        .flatten()
        .map(Arc::new);
    let mut cache = st.cover_cache.lock().unwrap();
    if cache.len() >= 40 {
        cache.clear();
    }
    cache.insert(key, found.clone());
    found
}

async fn cover(State(st): S, Query(q): Query<CoverQuery>) -> Response {
    if st.cfg.mock {
        return ([(header::CONTENT_TYPE, "image/svg+xml")], mock::cover_svg()).into_response();
    }
    if !looks_like_mbid(&q.mbid) || q.rg.as_deref().is_some_and(|r| !looks_like_mbid(r)) {
        return StatusCode::NOT_FOUND.into_response();
    }
    if let Some(px) = q.size.filter(|s| [250, 500, 1200].contains(s)) {
        let mbid = q.mbid.clone();
        let art = tokio::task::spawn_blocking(move || musicbrainz::fetch_cover_art_sized(&mbid, Some(px), false))
            .await
            .ok()
            .flatten();
        return match art {
            Some((bytes, mime)) => (
                [(header::CONTENT_TYPE, if mime == "png" { "image/png" } else { "image/jpeg" }), (header::CACHE_CONTROL, "public, max-age=86400")],
                bytes,
            )
                .into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        };
    }
    match best_cover(&st, &q.mbid, q.rg.as_deref()).await {
        Some(c) => (
            [
                (header::CONTENT_TYPE, c.mime()),
                (header::CACHE_CONTROL, "private, max-age=300"),
            ],
            c.bytes.clone(),
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Which source the preview image came from.
async fn cover_info(State(st): S, Query(q): Query<CoverQuery>) -> Json<Value> {
    if st.cfg.mock {
        return Json(json!({"source": "mock", "label": "placeholder"}));
    }
    if !looks_like_mbid(&q.mbid) || q.rg.as_deref().is_some_and(|r| !looks_like_mbid(r)) {
        return Json(json!({"source": null}));
    }
    match best_cover(&st, &q.mbid, q.rg.as_deref()).await {
        Some(c) => Json(json!({"source": c.source.id(), "label": c.source.label(), "bytes": c.bytes.len()})),
        None => Json(json!({"source": null})),
    }
}

// ── Settings ─────────────────────────────────────────────────────────────────

async fn get_settings(State(st): S) -> Json<Value> {
    Json(json!({
        "settings": st.settings.get().public(),
        "config_file": path_str(st.settings.path()),
        "library_default": st.cfg.library_dir.as_ref().map(|p| path_str(p)),
        "sources": [
            {"id": "fanart", "label": "fanart.tv", "needs_key": true},
            {"id": "caa", "label": "Cover Art Archive (MusicBrainz)", "needs_key": false},
        ],
    }))
}

#[derive(Deserialize)]
struct SettingsUpdate {
    rip_format: String,
    rip_archive: bool,
    rip_skip_musicbrainz: bool,
    rip_skip_accuraterip: bool,
    rip_quality: Option<String>,
    rip_replaygain: Option<bool>,
    rip_dynamic_range: Option<bool>,
    cover_sources: Vec<String>,
    cover_save_file: bool,
    cover_embed: bool,
    /// Omit to keep the stored key; send "" to remove it.
    fanart_api_key: Option<String>,
    #[serde(default)]
    stick_extra_folders: Vec<String>,
    #[serde(default)]
    stick_preset: String,
    convert_cache_mode: Option<String>,
    convert_cache_days: Option<u32>,
    convert_cache_max_gb: Option<u32>,
    convert_cache_dir: Option<String>,
    library_path: Option<String>,
    library_script: Option<String>,
    import_mode: Option<String>,
    import_cover: Option<bool>,
    import_other: Option<bool>,
    import_delete_leftovers: Option<bool>,
    import_conflict: Option<String>,
    /// Omit to keep the stored key; send "" to remove it.
    lidarr_api_key: Option<String>,
    lidarr_url: Option<String>,
    lidarr_root_folder: Option<String>,
    lidarr_quality_profile: Option<u64>,
    lidarr_metadata_profile: Option<u64>,
    lidarr_path_from: Option<String>,
    lidarr_path_to: Option<String>,
    lidarr_mode: Option<String>,
    import_target: Option<String>,
}

async fn put_settings(State(st): S, Json(u): Json<SettingsUpdate>) -> ApiResult<Json<Value>> {
    let current = st.settings.get();
    let new = settings::Settings {
        rip_format: u.rip_format,
        rip_archive: u.rip_archive,
        rip_skip_musicbrainz: u.rip_skip_musicbrainz,
        rip_skip_accuraterip: u.rip_skip_accuraterip,
        rip_quality: u.rip_quality.unwrap_or(current.rip_quality.clone()),
        rip_replaygain: u.rip_replaygain.unwrap_or(current.rip_replaygain),
        rip_dynamic_range: u.rip_dynamic_range.unwrap_or(current.rip_dynamic_range),
        cover_sources: u.cover_sources,
        cover_save_file: u.cover_save_file,
        cover_embed: u.cover_embed,
        fanart_api_key: u.fanart_api_key.unwrap_or(current.fanart_api_key),
        stick_extra_folders: u.stick_extra_folders,
        stick_preset: u.stick_preset,
        convert_cache_mode: u.convert_cache_mode.unwrap_or(current.convert_cache_mode),
        convert_cache_days: u.convert_cache_days.unwrap_or(current.convert_cache_days),
        convert_cache_max_gb: u.convert_cache_max_gb.unwrap_or(current.convert_cache_max_gb),
        convert_cache_dir: u.convert_cache_dir.unwrap_or(current.convert_cache_dir),
        library_path: u.library_path.unwrap_or(current.library_path),
        library_script: u.library_script.unwrap_or(current.library_script),
        import_mode: u.import_mode.unwrap_or(current.import_mode),
        import_cover: u.import_cover.unwrap_or(current.import_cover),
        import_other: u.import_other.unwrap_or(current.import_other),
        import_delete_leftovers: u.import_delete_leftovers.unwrap_or(current.import_delete_leftovers),
        import_conflict: u.import_conflict.unwrap_or(current.import_conflict),
        lidarr_url: u.lidarr_url.unwrap_or(current.lidarr_url),
        lidarr_api_key: u.lidarr_api_key.unwrap_or(current.lidarr_api_key),
        lidarr_root_folder: u.lidarr_root_folder.unwrap_or(current.lidarr_root_folder),
        lidarr_quality_profile: u.lidarr_quality_profile.unwrap_or(current.lidarr_quality_profile),
        lidarr_metadata_profile: u.lidarr_metadata_profile.unwrap_or(current.lidarr_metadata_profile),
        lidarr_path_from: u.lidarr_path_from.unwrap_or(current.lidarr_path_from),
        lidarr_path_to: u.lidarr_path_to.unwrap_or(current.lidarr_path_to),
        lidarr_mode: u.lidarr_mode.unwrap_or(current.lidarr_mode),
        import_target: u.import_target.unwrap_or(current.import_target),
        auth_user: current.auth_user,
        auth_password_hash: current.auth_password_hash,
    }
    .validate()
    .map_err(ApiError::bad)?;
    st.settings.set(new).map_err(|e| {
        Error::backend(format!("Could not save settings to {}: {e}", st.settings.path().display()))
    })?;
    st.cover_cache.lock().unwrap().clear();
    // A shorter retention applies straight away.
    let st2 = st.clone();
    tokio::task::spawn_blocking(move || cache::tidy(&st2));
    Ok(Json(json!({"settings": st.settings.get().public()})))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct TestFanart {
    /// Key to test; if empty, the saved key is used.
    key: String,
}

async fn test_fanart(State(st): S, Json(b): Json<TestFanart>) -> ApiResult<Json<Value>> {
    let key = if b.key.trim().is_empty() { st.settings.get().fanart_api_key } else { b.key.trim().to_string() };
    if key.is_empty() {
        return Err(ApiError::bad("Enter a fanart.tv API key first"));
    }
    if st.cfg.mock {
        return Ok(Json(json!({"ok": true, "message": "Mock mode: key not actually checked"})));
    }
    let status = tokio::task::spawn_blocking(move || cover::test_fanart_key(&key))
        .await
        .map_err(|e| Error::backend(e.to_string()))?;
    Ok(Json(match status {
        cover::FanartStatus::Ok => json!({"ok": true, "message": "fanart.tv accepted the key"}),
        cover::FanartStatus::BadKey => json!({"ok": false, "message": "fanart.tv rejected that API key"}),
        cover::FanartStatus::NoArtwork => json!({"ok": true, "message": "fanart.tv accepted the key"}),
        cover::FanartStatus::Error(e) => json!({"ok": false, "message": format!("Could not reach fanart.tv: {e}")}),
    }))
}

#[derive(Deserialize)]
struct DeviceBody {
    device: Option<String>,
}

async fn eject(State(st): S, Json(b): Json<DeviceBody>) -> ApiResult<Json<Value>> {
    require_drive_free(&st)?;
    let device = b.device.unwrap_or_else(|| st.cfg.device.clone());
    check_device(&device)?;
    if st.cfg.mock {
        return Ok(Json(json!({"ok": true})));
    }
    let out = tokio::process::Command::new("eject")
        .arg(&device)
        .output()
        .await
        .map_err(|e| Error::device(format!("Could not run eject: {e}")))?;
    if out.status.success() {
        Ok(Json(json!({"ok": true})))
    } else {
        Err(Error::device(String::from_utf8_lossy(&out.stderr).trim().to_string()).into())
    }
}

// ── File browser & library ───────────────────────────────────────────────────

#[derive(Deserialize)]
struct BrowseQuery {
    /// "media" (burn sources) or "rips"
    root: Option<String>,
    path: Option<String>,
}

async fn browse(State(st): S, Query(q): Query<BrowseQuery>) -> ApiResult<Json<Value>> {
    let root = match q.root.as_deref() {
        Some("rips") => &st.cfg.rips_dir,
        _ => &st.cfg.media_dir,
    };
    let rel = q.path.unwrap_or_default();
    let dir = safe_join(root, &rel)?;
    let rd = std::fs::read_dir(&dir)
        .map_err(|e| ApiError::not_found(format!("Cannot read {}: {e}", dir.display())))?;

    let mut entries: Vec<Value> = rd
        .filter_map(|e| e.ok())
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            Some(json!({
                "name": e.file_name().to_string_lossy(),
                "is_dir": meta.is_dir(),
                "size": if meta.is_dir() { 0 } else { meta.len() },
            }))
        })
        .collect();
    entries.sort_by(|a, b| {
        let key = |v: &Value| (!v["is_dir"].as_bool().unwrap_or(false), v["name"].as_str().unwrap_or("").to_lowercase());
        key(a).cmp(&key(b))
    });

    let rel = rel.trim_matches('/').to_string();
    let parent = if rel.is_empty() {
        Value::Null
    } else {
        json!(Path::new(&rel).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default())
    };
    Ok(Json(json!({"path": rel, "parent": parent, "entries": entries})))
}

const AUDIO_EXTS: &[&str] = &["flac", "wav", "alac", "aiff", "aif", "ogg", "mp3", "opus", "m4a"];

fn is_audio(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTS.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn walk(base: &Path, dir: &Path, depth: usize, out: &mut Vec<(String, u64)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.filter_map(|e| e.ok()) {
        let p = e.path();
        let Ok(meta) = e.metadata() else { continue };
        if meta.is_dir() {
            if depth < 5 {
                walk(base, &p, depth + 1, out);
            }
        } else if let Ok(rel) = p.strip_prefix(base) {
            out.push((rel.to_string_lossy().to_string(), meta.len()));
        }
    }
}

fn describe_rip(dir: &Path) -> Value {
    let name = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let mut files = Vec::new();
    walk(dir, dir, 0, &mut files);
    files.sort();

    let archive = dir.join("metadata/checksums.json").exists() || dir.join("checksums.json").exists();
    let cover = ["cover.jpg", "cover.png"].iter().find(|c| dir.join(c).exists()).map(|c| c.to_string());
    let audio: Vec<&(String, u64)> = files.iter().filter(|(p, _)| is_audio(p)).collect();
    let has_data = files.iter().any(|(p, _)| p.starts_with("data/") || p.ends_with(".iso"));
    let total: u64 = files.iter().map(|(_, s)| s).sum();

    let mb = read_json(&dir.join("metadata/musicbrainz.json")).or_else(|| read_json(&dir.join("musicbrainz.json")));
    let album = mb.as_ref().and_then(|m| m["album"].as_str()).map(String::from);
    let artist = mb.as_ref().and_then(|m| m["album_artist"].as_str()).map(String::from);
    let year = mb.as_ref().and_then(|m| m["year"].as_str()).map(String::from);

    let ar = read_json(&dir.join("metadata/accuraterip.json"));
    let accuraterip = ar.as_ref().map(|r| json!({
        "found": r["found"], "verified": r["verified"], "total": r["total"],
        "detected_shift_samples": r["detected_shift_samples"],
    }));

    let modified = std::fs::metadata(dir)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);

    json!({
        "name": name, "archive": archive, "cover": cover,
        "audio_files": audio.len(), "has_data": has_data, "total_bytes": total,
        "album": album, "artist": artist, "year": year, "modified": modified,
        "accuraterip": accuraterip,
        "files": files.iter().map(|(p, s)| json!({"path": p, "size": s, "audio": is_audio(p)})).collect::<Vec<_>>(),
    })
}

async fn library(State(st): S) -> ApiResult<Json<Value>> {
    let dir = st.cfg.rips_dir.clone();
    let entries = tokio::task::spawn_blocking(move || {
        let mut out: Vec<Value> = std::fs::read_dir(&dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter(|e| e.path().is_dir() && !e.file_name().to_string_lossy().starts_with('.'))
                    .map(|e| {
                        let mut v = describe_rip(&e.path());
                        v.as_object_mut().map(|o| o.remove("files"));
                        v
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.sort_by_key(|v| std::cmp::Reverse(v["modified"].as_u64().unwrap_or(0)));
        out
    })
    .await
    .map_err(|e| Error::backend(e.to_string()))?;
    Ok(Json(json!({"path": path_str(&st.cfg.rips_dir), "entries": entries})))
}

async fn library_entry(State(st): S, UrlPath(name): UrlPath<String>) -> ApiResult<Json<Value>> {
    let dir = safe_join(&st.cfg.rips_dir, &name)?;
    if !dir.is_dir() {
        return Err(ApiError::not_found(format!("No such rip: {name}")));
    }
    let mut v = tokio::task::spawn_blocking(move || {
        let mut v = describe_rip(&dir);
        let mb = read_json(&dir.join("metadata/musicbrainz.json")).or_else(|| read_json(&dir.join("musicbrainz.json")));
        v["musicbrainz"] = mb.unwrap_or(Value::Null);
        v["accuraterip_report"] = read_json(&dir.join("metadata/accuraterip.json")).unwrap_or(Value::Null);
        v
    })
    .await
    .map_err(|e| Error::backend(e.to_string()))?;
    v["name"] = json!(name);
    Ok(Json(v))
}

// ── Burn / plan / validate request building ──────────────────────────────────

#[derive(Deserialize, Default)]
#[serde(default)]
struct BurnReq {
    format: Option<String>,
    /// Audio files, relative to the media directory.
    audio: Vec<String>,
    /// M3U playlist, relative to the media directory.
    playlist: Option<String>,
    /// Data source folder, relative to the media directory.
    data: Option<String>,
    /// Individual files for a Data CD, relative to the media directory.
    files: Vec<String>,
    label: Option<String>,
    cd_text: bool,
    transcode: Option<String>,
    dry_run: bool,
    debug: bool,
    device: Option<String>,
    /// Disc capacity in MB (see the presets in /api/status).
    disc_size_mb: Option<u64>,
    /// Music DVD: Dolby Digital bitrate in kbps.
    dvd_audio_kbps: Option<u32>,
    /// Music DVD: "pal" or "ntsc".
    dvd_standard: Option<String>,
    /// Music DVD: picture to show, relative to the media directory.
    dvd_still: Option<String>,
    /// Full disc graph (overrides every source option).
    graph: Option<Value>,
}

/// Returns the CLI args and, when a disc graph was supplied, the temp file to delete afterwards.
fn burn_args(cmd: &str, req: &BurnReq, cfg: &Config) -> ApiResult<(Vec<String>, Option<PathBuf>)> {
    let mut a = vec![cmd.to_string()];
    let mut tmp = None;
    let is_burn = cmd == "burn";

    if let Some(g) = &req.graph {
        let p = write_temp_graph(g)?;
        a.extend(["--input".into(), path_str(&p)]);
        tmp = Some(p);
    } else {
        let format = req.format.as_deref().unwrap_or("redbook");
        if !["redbook", "datacd", "bluebook", "datadvd", "musicdvd"].contains(&format) {
            return Err(ApiError::bad(format!("Unknown format '{format}' (use redbook, datacd, bluebook, datadvd or musicdvd)")));
        }
        // Data CD and Data DVD take files, a folder or a playlist; the other formats take audio.
        let is_data_cd = matches!(format, "datacd" | "datadvd");
        a.extend(["--format".into(), format.into()]);

        if is_burn || !is_data_cd {
            if !is_data_cd && !req.audio.is_empty() {
                a.push("--audio".into());
                for f in &req.audio {
                    a.push(path_str(&safe_join(&cfg.media_dir, f)?));
                }
            }
            if let Some(pl) = &req.playlist {
                a.extend(["--playlist".into(), path_str(&safe_join(&cfg.media_dir, pl)?)]);
                // A playlist may only pull in files from the media folder.
                if is_burn {
                    a.extend(["--playlist-root".into(), path_str(&cfg.media_dir)]);
                }
            }
            if is_burn && is_data_cd && !req.files.is_empty() {
                a.push("--files".into());
                for f in &req.files {
                    a.push(path_str(&safe_join(&cfg.media_dir, f)?));
                }
            }
        }
        if let Some(d) = &req.data {
            if format != "redbook" {
                a.extend(["--data".into(), path_str(&safe_join(&cfg.media_dir, d)?)]);
            }
        }
        if let Some(l) = req.label.as_deref().filter(|l| !l.trim().is_empty()) {
            a.extend(["--label".into(), l.into()]);
        }
        if req.cd_text && matches!(format, "redbook" | "bluebook") {
            a.push("--cd-text".into());
        }
        if is_burn && is_data_cd {
            // Audio discs are always converted for the disc; transcoding only shrinks audio inside a Data disc.
            if let Some(t) = req.transcode.as_deref().filter(|t| !t.is_empty()) {
                a.extend(["--transcode".into(), t.into()]);
            }
        }
        if is_burn && format == "musicdvd" {
            if let Some(k) = req.dvd_audio_kbps {
                a.extend(["--dvd-audio-kbps".into(), k.to_string()]);
            }
            if let Some(st) = req.dvd_standard.as_deref().filter(|s| !s.is_empty()) {
                if !["pal", "ntsc"].contains(&st) {
                    return Err(ApiError::bad("dvd_standard must be pal or ntsc"));
                }
                a.extend(["--dvd-standard".into(), st.into()]);
            }
            if let Some(img) = req.dvd_still.as_deref().filter(|s| !s.is_empty()) {
                a.extend(["--dvd-still".into(), path_str(&safe_join(&cfg.media_dir, img)?)]);
            }
        }
    }

    if is_burn {
        if let Some(mb) = req.disc_size_mb {
            a.extend(["--disc-size".into(), mb.to_string()]);
        }
        let device = req.device.clone().unwrap_or_else(|| cfg.device.clone());
        check_device(&device)?;
        a.extend(["--device".into(), device, "--progress-json".into()]);
        if req.debug {
            a.push("--debug".into());
        }
        if req.dry_run {
            a.push("--dry-run".into());
        }
    }
    Ok((a, tmp))
}

/// Run a short CLI command and return its stdout, or its structured error.
async fn run_cli_capture(st: &AppState, args: Vec<String>) -> ApiResult<String> {
    let out = tokio::process::Command::new(&st.cfg.exe)
        .args(&args)
        .output()
        .await
        .map_err(Error::from)?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).to_string());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    match serde_json::from_str::<DiscError>(stderr.trim()) {
        Ok(e) => Err(ApiError(StatusCode::UNPROCESSABLE_ENTITY, e)),
        Err(_) => Err(ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "BACKEND_ERROR", stderr.trim().to_string(), true)),
    }
}

/// How many discs a job needs and what goes on each, counting the size after any transcoding.
/// A hand-written disc graph is a single disc, so it just gets its steps.
async fn plan(State(st): S, Json(req): Json<BurnReq>) -> ApiResult<Json<Value>> {
    if req.graph.is_some() {
        let (args, tmp) = burn_args("plan", &req, &st.cfg)?;
        let res = run_cli_capture(&st, args).await;
        if let Some(p) = tmp {
            let _ = std::fs::remove_file(p);
        }
        let stdout = res?;
        return Ok(Json(json!({"kind": "graph", "steps": serde_json::from_str::<Value>(&stdout).map_err(Error::from)?})));
    }

    let media = &st.cfg.media_dir;
    let join_all = |v: &[String]| -> ApiResult<Vec<String>> { v.iter().map(|f| safe_join(media, f).map(|p| path_str(&p))).collect() };
    let format = req.format.clone().unwrap_or_else(|| "redbook".into());
    let plan_req = rip_plan_request(&format, &req, media, &join_all)?;

    let planned = tokio::task::spawn_blocking(move || crate::planner::discs::plan_request(&plan_req))
        .await
        .map_err(|e| Error::backend(e.to_string()))??;
    Ok(Json(planned))
}

fn rip_plan_request(
    format: &str,
    req: &BurnReq,
    media: &Path,
    join_all: &dyn Fn(&[String]) -> ApiResult<Vec<String>>,
) -> ApiResult<crate::planner::discs::PlanRequest> {
    let is_data_cd = matches!(format, "datacd" | "datadvd");
    let opt_path = |p: &Option<String>| -> ApiResult<Option<String>> {
        p.as_deref().map(|p| safe_join(media, p).map(|j| path_str(&j))).transpose()
    };
    Ok(crate::planner::discs::PlanRequest {
        format: format.to_string(),
        audio: if is_data_cd { Vec::new() } else { join_all(&req.audio)? },
        playlist: opt_path(&req.playlist)?,
        files: if is_data_cd && !req.files.is_empty() { Some(join_all(&req.files)?) } else { None },
        data: if format == "redbook" { None } else { opt_path(&req.data)? },
        playlist_root: Some(media.to_path_buf()),
        transcode: if is_data_cd { req.transcode.clone() } else { None },
        disc_size_mb: req.disc_size_mb,
        dvd_audio_kbps: req.dvd_audio_kbps,
    })
}

async fn validate(State(st): S, Json(req): Json<BurnReq>) -> ApiResult<Json<Value>> {
    let Some(graph) = &req.graph else {
        return Err(ApiError::bad("Provide a disc graph to validate"));
    };
    let tmp = write_temp_graph(graph)?;
    let res = run_cli_capture(&st, vec!["validate".into(), path_str(&tmp)]).await;
    let _ = std::fs::remove_file(tmp);
    res?;
    Ok(Json(json!({"ok": true})))
}

// ── Playlists and audio folders (for the Burn page) ─────────────────────────

#[derive(Deserialize)]
struct PlaylistReq {
    /// The .m3u / .m3u8 file, relative to the media directory.
    path: String,
}

/// What a playlist would contribute, without burning anything.
async fn playlist_preview(State(st): S, Json(req): Json<PlaylistReq>) -> ApiResult<Json<Value>> {
    let file = safe_join(&st.cfg.media_dir, &req.path)?;
    let media = st.cfg.media_dir.clone();
    let parsed = tokio::task::spawn_blocking(move || {
        crate::parser::playlist::parse_detailed(&file.to_string_lossy(), Some(&media))
    })
    .await
    .map_err(|e| Error::backend(e.to_string()))??;

    let media = st.cfg.media_dir.canonicalize().unwrap_or_else(|_| st.cfg.media_dir.clone());
    let entries: Vec<Value> = parsed
        .entries
        .iter()
        .map(|e| {
            let p = Path::new(&e.path);
            json!({
                "path": p.strip_prefix(&media).map(|r| r.to_string_lossy().to_string()).unwrap_or_else(|_| e.path.clone()),
                "name": e.display.clone().unwrap_or_else(|| p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()),
                "duration_secs": e.duration_secs,
                "size": std::fs::metadata(p).map(|m| m.len()).unwrap_or(0),
            })
        })
        .collect();
    let total_bytes: u64 = entries.iter().map(|e| e["size"].as_u64().unwrap_or(0)).sum();
    let known: Vec<u64> = entries.iter().filter_map(|e| e["duration_secs"].as_u64()).collect();
    Ok(Json(json!({
        "entries": entries,
        "skipped": parsed.skipped.iter().map(|s| json!({"entry": s.entry, "reason": s.reason})).collect::<Vec<_>>(),
        "total_bytes": total_bytes,
        // Only meaningful when every entry carried an #EXTINF duration
        "total_secs": if !entries.is_empty() && known.len() == entries.len() { json!(known.iter().sum::<u64>()) } else { Value::Null },
    })))
}

/// Compare names the way people order tracks: "2 - x" before "10 - x", ignoring case.
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    fn chunks(s: &str) -> Vec<(bool, String)> {
        let mut out: Vec<(bool, String)> = Vec::new();
        for c in s.to_lowercase().chars() {
            let digit = c.is_ascii_digit();
            match out.last_mut() {
                Some((d, buf)) if *d == digit => buf.push(c),
                _ => out.push((digit, c.to_string())),
            }
        }
        out
    }
    let (ca, cb) = (chunks(a), chunks(b));
    for (x, y) in ca.iter().zip(cb.iter()) {
        let ord = if x.0 && y.0 {
            let (nx, ny) = (x.1.trim_start_matches('0'), y.1.trim_start_matches('0'));
            nx.len().cmp(&ny.len()).then_with(|| nx.cmp(ny))
        } else {
            x.1.cmp(&y.1)
        };
        if ord != std::cmp::Ordering::Equal {
            return ord;
        }
    }
    ca.len().cmp(&cb.len())
}

const MAX_LISTED_FILES: usize = 1000;

fn collect_audio(base: &Path, dir: &Path, recursive: bool, out: &mut Vec<(String, u64)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.filter_map(|e| e.ok()) {
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || out.len() > MAX_LISTED_FILES {
            continue;
        }
        let path = e.path();
        let Ok(meta) = std::fs::metadata(&path) else { continue };
        if meta.is_dir() {
            if recursive {
                collect_audio(base, &path, recursive, out);
            }
        } else if is_audio(&name) {
            if let Ok(rel) = path.strip_prefix(base) {
                out.push((rel.to_string_lossy().to_string(), meta.len()));
            }
        }
    }
}

#[derive(Deserialize)]
struct AudioFilesQuery {
    path: Option<String>,
    recursive: Option<bool>,
}

/// Audio files in a media folder (optionally including subfolders), in track order.
async fn audio_files(State(st): S, Query(q): Query<AudioFilesQuery>) -> ApiResult<Json<Value>> {
    let rel = q.path.unwrap_or_default();
    let dir = safe_join(&st.cfg.media_dir, &rel)?;
    if !dir.is_dir() {
        return Err(ApiError::not_found(format!("No such folder: {rel}")));
    }
    let base = st.cfg.media_dir.clone();
    let recursive = q.recursive.unwrap_or(false);
    let mut files = tokio::task::spawn_blocking(move || {
        let mut out = Vec::new();
        collect_audio(&base, &dir, recursive, &mut out);
        out
    })
    .await
    .map_err(|e| Error::backend(e.to_string()))?;
    files.sort_by(|a, b| natural_cmp(&a.0, &b.0));
    let truncated = files.len() > MAX_LISTED_FILES;
    files.truncate(MAX_LISTED_FILES);
    Ok(Json(json!({
        "files": files.iter().map(|(p, s)| json!({"path": p, "size": s})).collect::<Vec<_>>(),
        "truncated": truncated,
    })))
}

// ── Jobs ─────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(default)]
struct RipReq {
    device: Option<String>,
    format: Option<String>,
    archive: bool,
    no_musicbrainz: bool,
    no_accuraterip: bool,
    debug: bool,
    /// Use this MusicBrainz release (ID or URL) instead of the DiscID lookup.
    mb_release: Option<String>,
    /// Explicit folder name inside the rips directory. Empty = auto-name from metadata.
    folder: Option<String>,
    /// A picture uploaded with /api/cover/upload, to use as the cover.
    cover_upload: Option<String>,
    /// Encoder quality for the format (empty = the best).
    quality: Option<String>,
    replaygain: bool,
    dynamic_range: bool,
}

impl Default for RipReq {
    fn default() -> Self {
        RipReq { device: None, format: None, archive: false, no_musicbrainz: false, no_accuraterip: false, debug: false, mb_release: None, folder: None, cover_upload: None, quality: None, replaygain: false, dynamic_range: false }
    }
}

async fn start_rip(State(st): S, Json(req): Json<RipReq>) -> ApiResult<Json<Value>> {
    let device = req.device.clone().unwrap_or_else(|| st.cfg.device.clone());
    check_device(&device)?;
    let format = req.format.clone().unwrap_or_else(|| "flac".into());
    format.parse::<AudioFormat>().map_err(ApiError::bad)?;
    if !st.cfg.mock {
        let missing = rip::check_dependencies(&format.parse::<AudioFormat>().map_err(ApiError::bad)?);
        if !missing.is_empty() {
            return Err(ApiError::bad(format!("Missing required tools: {}", missing.join(", "))));
        }
    }
    let folder = req.folder.as_deref().map(str::trim).filter(|f| !f.is_empty()).map(String::from);

    let mut args = vec![
        "rip".to_string(), "--device".into(), device.clone(),
        "--format".into(), format.clone(), "--progress-json".into(),
    ];
    match &folder {
        Some(f) => {
            let out = safe_join(&st.cfg.rips_dir, f)?;
            args.extend(["--output".into(), path_str(&out)]);
        }
        None => args.extend(["--dir".into(), path_str(&st.cfg.rips_dir)]),
    }
    let mb_release = match req.mb_release.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
        Some(m) => Some(musicbrainz::parse_release_id(m)?),
        None => None,
    };
    if let Some(id) = &mb_release {
        args.extend(["--mb-release".into(), id.clone()]);
    }
    // An explicit release wins over "skip MusicBrainz".
    let skip_mb = req.no_musicbrainz && mb_release.is_none();
    let cfg = st.settings.get();
    let cover_opts = cfg.cover_options();
    args.extend(["--cover-sources".into(), cfg.cover_sources.join(",")]);
    if !cfg.cover_save_file { args.push("--no-cover-file".into()); }
    if !cfg.cover_embed { args.push("--no-cover-embed".into()); }
    let cover_file: Option<PathBuf> = match req.cover_upload.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => Some(library_edit::upload_path(&st, t).ok_or_else(|| ApiError::bad("The uploaded cover has expired. Choose it again."))?),
        None => None,
    };
    if let Some(p) = &cover_file {
        args.extend(["--cover-file".into(), path_str(p)]);
    }
    let quality = req.quality.as_deref().map(str::trim).filter(|q| !q.is_empty()).map(String::from);
    if let Some(q) = &quality {
        let fmt: AudioFormat = format.parse().map_err(ApiError::bad)?;
        if !rip::encoder::quality_choices(&fmt).iter().any(|c| c.id == q) {
            return Err(ApiError::bad(format!("'{q}' isn't a quality choice for {fmt}")));
        }
        args.extend(["--quality".into(), q.clone()]);
    }
    if req.replaygain { args.push("--replaygain".into()); }
    if req.dynamic_range { args.push("--dynamic-range".into()); }
    let mut envs: Vec<(String, String)> = Vec::new();
    if let Some(key) = &cover_opts.fanart_key {
        envs.push(("RUSTYDISC_FANART_KEY".into(), key.clone()));
    }
    for (flag, on) in [("--archive", req.archive), ("--no-musicbrainz", skip_mb), ("--no-accuraterip", req.no_accuraterip), ("--debug", req.debug)] {
        if on {
            args.push(flag.into());
        }
    }

    let job = start_job(&st, "rip", &format!("Rip {device} → {}", format.to_uppercase()), true)?;
    if st.cfg.mock {
        tokio::spawn(mock::rip(job.clone(), st.cfg.rips_dir.clone(), folder, req.archive, format, skip_mb, req.no_accuraterip, mb_release.is_some(), cover_opts, cover_file, quality.clone(), req.replaygain, req.dynamic_range));
    } else {
        spawn_cli_env(&st, job.clone(), args, None, envs);
    }
    Ok(Json(json!(job.summary())))
}

async fn start_burn(State(st): S, Json(req): Json<BurnReq>) -> ApiResult<Json<Value>> {
    let (mut args, tmp) = burn_args("burn", &req, &st.cfg)?;
    if req.graph.is_none() {
        args.extend(cache::job_args(&st));
    }
    let label = req.label.clone().filter(|l| !l.is_empty()).unwrap_or_else(|| "Untitled".into());
    let fmt = if req.graph.is_some() { "disc graph".to_string() } else { req.format.clone().unwrap_or_else(|| "redbook".into()) };
    let title = format!("{} \"{label}\" ({fmt})", if req.dry_run { "Dry-run burn" } else { "Burn" });

    let job = match start_job(&st, "burn", &title, true) {
        Ok(j) => j,
        Err(e) => {
            if let Some(p) = tmp {
                let _ = std::fs::remove_file(p);
            }
            return Err(e);
        }
    };
    if st.cfg.mock {
        tokio::spawn(mock::burn(job.clone(), req.dry_run));
        if let Some(p) = tmp {
            let _ = std::fs::remove_file(p);
        }
    } else {
        spawn_cli(&st, job.clone(), args, tmp);
    }
    Ok(Json(json!(job.summary())))
}

#[derive(Deserialize)]
struct RecoverReq {
    device: Option<String>,
    /// "fast" or "all" to blank a CD-RW; omit to just inspect / recover.
    blank: Option<String>,
    debug: bool,
}

async fn start_recover(State(st): S, Json(req): Json<RecoverReq>) -> ApiResult<Json<Value>> {
    let device = req.device.unwrap_or_else(|| st.cfg.device.clone());
    check_device(&device)?;
    let mut args = vec!["recover".to_string(), "--device".into(), device.clone()];
    if let Some(m) = &req.blank {
        if m != "fast" && m != "all" {
            return Err(ApiError::bad("blank must be 'fast' or 'all'"));
        }
        args.extend(["--blank".into(), m.clone()]);
    }
    if req.debug {
        args.push("--debug".into());
    }
    let title = match &req.blank {
        Some(m) => format!("Blank CD-RW {device} ({m})"),
        None => format!("Check / recover {device}"),
    };
    let job = start_job(&st, "recover", &title, true)?;
    if st.cfg.mock {
        tokio::spawn(mock::recover(job.clone(), req.blank));
    } else {
        spawn_cli(&st, job.clone(), args, None);
    }
    Ok(Json(json!(job.summary())))
}

#[derive(Deserialize)]
struct VerifyReq {
    /// Rip folder name inside the rips directory.
    name: String,
}

async fn start_verify(State(st): S, Json(req): Json<VerifyReq>) -> ApiResult<Json<Value>> {
    let dir = safe_join(&st.cfg.rips_dir, &req.name)?;
    if !dir.is_dir() {
        return Err(ApiError::not_found(format!("No such rip: {}", req.name)));
    }
    let job = start_job(&st, "verify", &format!("Verify {}", req.name), false)?;
    spawn_cli(&st, job.clone(), vec!["verify".into(), path_str(&dir)], None);
    Ok(Json(json!(job.summary())))
}

async fn list_jobs(State(st): S) -> Json<Value> {
    Json(json!(st.jobs.all().iter().map(|j| j.summary()).collect::<Vec<_>>()))
}

fn find_job(st: &AppState, id: u64) -> ApiResult<Arc<jobs::Job>> {
    st.jobs.get(id).ok_or_else(|| ApiError::not_found(format!("No such job: {id}")))
}

async fn get_job(State(st): S, UrlPath(id): UrlPath<u64>) -> ApiResult<Json<Value>> {
    let job = find_job(&st, id)?;
    let (events, _) = job.events_since(0);
    Ok(Json(json!({"job": job.summary(), "events": events})))
}

async fn cancel_job(State(st): S, UrlPath(id): UrlPath<u64>) -> ApiResult<Json<Value>> {
    let job = find_job(&st, id)?;
    if job.status().is_terminal() {
        return Err(ApiError::bad("Job already finished"));
    }
    job.request_cancel();
    Ok(Json(json!({"ok": true})))
}

async fn continue_job(State(st): S, UrlPath(id): UrlPath<u64>) -> ApiResult<Json<Value>> {
    let job = find_job(&st, id)?;
    if job.resume().await {
        Ok(Json(json!({"ok": true})))
    } else {
        Err(ApiError::bad("Job is not waiting for input"))
    }
}

#[derive(Deserialize)]
struct EventsQuery {
    from: Option<usize>,
}

/// Server-Sent Events: replays the job's event log from `from`, then follows it live.
async fn job_events(
    State(st): S,
    UrlPath(id): UrlPath<u64>,
    Query(q): Query<EventsQuery>,
) -> ApiResult<impl IntoResponse> {
    let job = find_job(&st, id)?;
    let rx = job.subscribe();

    let stream = futures_util::stream::unfold(
        (job, rx, q.from.unwrap_or(0), false),
        |(job, mut rx, mut idx, done)| async move {
            if done {
                return None;
            }
            loop {
                let (evs, terminal) = job.events_since(idx);
                if !evs.is_empty() || terminal {
                    idx += evs.len();
                    let mut out: Vec<Result<sse::Event, Infallible>> = evs
                        .iter()
                        .map(|e: &Event| Ok(sse::Event::default().data(serde_json::to_string(e).unwrap_or_default())))
                        .collect();
                    if terminal {
                        out.push(Ok(sse::Event::default().event("end").data("")));
                    }
                    return Some((out, (job, rx, idx, terminal)));
                }
                if rx.changed().await.is_err() {
                    return None;
                }
            }
        },
    )
    .flat_map(futures_util::stream::iter);

    // X-Accel-Buffering stops nginx-style reverse proxies from holding events back.
    Ok(([("x-accel-buffering", "no")], Sse::new(stream).keep_alive(sse::KeepAlive::default())))
}

#[cfg(test)]
mod tests {
    use super::natural_cmp;

    #[test]
    fn natural_order_puts_2_before_10() {
        let mut v = vec!["10 - b.flac", "2 - a.flac", "01 - z.flac", "Album/1.flac", "album/02.flac", "Album/10.flac"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["01 - z.flac", "2 - a.flac", "10 - b.flac", "Album/1.flac", "album/02.flac", "Album/10.flac"]);
    }
}
