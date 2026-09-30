//! Web UI + JSON API for RustyDisc.
//!
//! Read-only queries (disc info, MusicBrainz, library) run in-process; anything that
//! writes or takes a long time (rip, burn, recover, verify) is a job that runs the
//! `rustydisc` CLI as a child process with `--progress-json` and is followed by the
//! browser over Server-Sent Events.

mod jobs;
mod mock;

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
use futures_util::{Stream, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use tower_http::services::ServeDir;

use crate::{
    analyzer,
    error::{DiscError, Error},
    rip::{self, encoder::AudioFormat, musicbrainz},
};
use jobs::{Event, Jobs};

const INDEX_HTML: &str = include_str!("index.html");

pub struct Config {
    pub exe: PathBuf,
    pub device: String,
    /// Where rips are written and the library is read from.
    pub rips_dir: PathBuf,
    /// Where burn sources (audio files, data folders, playlists) are picked from.
    pub media_dir: PathBuf,
    pub mock: bool,
}

struct AppState {
    cfg: Config,
    jobs: Jobs,
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
    let exe = st.cfg.exe.clone();
    tokio::spawn(async move {
        jobs::run_process(job, exe, args).await;
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
    let state = Arc::new(AppState { cfg, jobs: Jobs::default() });

    let app = Router::new()
        .route("/", get(|| async { Html(INDEX_HTML) }))
        .route("/api/status", get(status))
        .route("/api/info", get(info))
        .route("/api/musicbrainz", get(musicbrainz_lookup))
        .route("/api/cover", get(cover))
        .route("/api/eject", post(eject))
        .route("/api/browse", get(browse))
        .route("/api/library", get(library))
        .route("/api/library/{name}", get(library_entry))
        .route("/api/plan", post(plan))
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
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|e| Error::backend(format!("Cannot bind {bind}: {e}")))?;
    eprintln!("RustyDisc web UI listening on http://{bind}");
    axum::serve(listener, app)
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

    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "mock": st.cfg.mock,
        "default_device": st.cfg.device,
        "devices": devices,
        "rips_dir": path_str(&st.cfg.rips_dir),
        "media_dir": path_str(&st.cfg.media_dir),
        "formats": ["flac", "wav", "alac", "aiff", "ogg", "mp3", "opus"],
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
    discid: String,
}

async fn musicbrainz_lookup(State(st): S, Query(q): Query<MbQuery>) -> ApiResult<Json<Value>> {
    if st.cfg.mock {
        return Ok(Json(serde_json::to_value(mock::release()).map_err(Error::from)?));
    }
    let release = tokio::task::spawn_blocking(move || musicbrainz::lookup(&q.discid, false))
        .await
        .map_err(|e| Error::backend(e.to_string()))??;
    Ok(Json(serde_json::to_value(release).map_err(Error::from)?))
}

#[derive(Deserialize)]
struct CoverQuery {
    mbid: String,
}

async fn cover(State(st): S, Query(q): Query<CoverQuery>) -> Response {
    if st.cfg.mock {
        return ([(header::CONTENT_TYPE, "image/svg+xml")], mock::cover_svg()).into_response();
    }
    if !q.mbid.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return StatusCode::NOT_FOUND.into_response();
    }
    let art = tokio::task::spawn_blocking(move || musicbrainz::fetch_cover_art(&q.mbid, false))
        .await
        .ok()
        .flatten();
    match art {
        Some((bytes, mime)) => (
            [(header::CONTENT_TYPE, mime), (header::CACHE_CONTROL, "public, max-age=86400")],
            bytes,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
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
    label: Option<String>,
    cd_text: bool,
    transcode: Option<String>,
    dry_run: bool,
    debug: bool,
    device: Option<String>,
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
        if !["redbook", "datacd", "bluebook"].contains(&format) {
            return Err(ApiError::bad(format!("Unknown format '{format}' (use redbook, datacd or bluebook)")));
        }
        a.extend(["--format".into(), format.into()]);
        if !req.audio.is_empty() {
            a.push("--audio".into());
            for f in &req.audio {
                a.push(path_str(&safe_join(&cfg.media_dir, f)?));
            }
        }
        if let Some(pl) = &req.playlist {
            a.extend(["--playlist".into(), path_str(&safe_join(&cfg.media_dir, pl)?)]);
        }
        if let Some(d) = &req.data {
            a.extend(["--data".into(), path_str(&safe_join(&cfg.media_dir, d)?)]);
        }
        if let Some(l) = req.label.as_deref().filter(|l| !l.trim().is_empty()) {
            a.extend(["--label".into(), l.into()]);
        }
        if req.cd_text {
            a.push("--cd-text".into());
        }
        if is_burn {
            if let Some(t) = req.transcode.as_deref().filter(|t| !t.is_empty()) {
                a.extend(["--transcode".into(), t.into()]);
            }
        }
    }

    if is_burn {
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

async fn plan(State(st): S, Json(req): Json<BurnReq>) -> ApiResult<Json<Value>> {
    let (args, tmp) = burn_args("plan", &req, &st.cfg)?;
    let res = run_cli_capture(&st, args).await;
    if let Some(p) = tmp {
        let _ = std::fs::remove_file(p);
    }
    let stdout = res?;
    Ok(Json(serde_json::from_str(&stdout).map_err(Error::from)?))
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

// ── Jobs ─────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct RipReq {
    device: Option<String>,
    format: Option<String>,
    archive: bool,
    no_musicbrainz: bool,
    debug: bool,
    /// Explicit folder name inside the rips directory. Empty = auto-name from metadata.
    folder: Option<String>,
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
    for (flag, on) in [("--archive", req.archive), ("--no-musicbrainz", req.no_musicbrainz), ("--debug", req.debug)] {
        if on {
            args.push(flag.into());
        }
    }

    let job = start_job(&st, "rip", &format!("Rip {device} → {}", format.to_uppercase()), true)?;
    if st.cfg.mock {
        tokio::spawn(mock::rip(job.clone(), st.cfg.rips_dir.clone(), folder, req.archive, format, req.no_musicbrainz));
    } else {
        spawn_cli(&st, job.clone(), args, None);
    }
    Ok(Json(json!(job.summary())))
}

async fn start_burn(State(st): S, Json(req): Json<BurnReq>) -> ApiResult<Json<Value>> {
    let (args, tmp) = burn_args("burn", &req, &st.cfg)?;
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
) -> ApiResult<Sse<impl Stream<Item = Result<sse::Event, Infallible>>>> {
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

    Ok(Sse::new(stream).keep_alive(sse::KeepAlive::default()))
}
