//! The converted-files cache: where it is, what is in it, and keeping it tidy.

use std::{path::PathBuf, sync::Arc, time::Duration};

use axum::{extract::State, Json};
use serde_json::{json, Value};

use super::{ApiError, ApiResult, AppState, S};
use crate::backend::cache;

/// Where converted files are kept: the folder chosen in Settings, else `--cache-dir`, else `cache`
/// next to the settings.
pub fn dir(st: &AppState) -> PathBuf {
    let chosen = st.settings.get().convert_cache_dir;
    if !chosen.is_empty() {
        return PathBuf::from(chosen);
    }
    st.cfg.cache_dir.clone().unwrap_or_else(|| st.cfg.config_dir.join("cache"))
}

/// Command-line flags that make a job keep and reuse converted files, as the settings say.
/// Nothing is added for "delete right after use", which is how jobs behave by default.
pub fn job_args(st: &AppState) -> Vec<String> {
    let s = st.settings.get();
    if s.convert_cache_mode == "delete" {
        return Vec::new();
    }
    let mut a = vec!["--convert-cache".to_string(), dir(st).to_string_lossy().to_string()];
    if s.convert_cache_mode == "days" {
        a.extend(["--cache-days".into(), s.convert_cache_days.to_string()]);
    }
    if s.convert_cache_max_gb > 0 {
        a.extend(["--cache-max-gb".into(), s.convert_cache_max_gb.to_string()]);
    }
    a
}

/// Apply the retention setting to what is on disk now.
pub fn tidy(st: &AppState) -> cache::Pruned {
    let s = st.settings.get();
    let d = dir(st);
    if !d.is_dir() {
        return cache::Pruned::default();
    }
    let (days, max) = match s.convert_cache_mode.as_str() {
        "delete" => (Some(0), None),
        "days" => (Some(s.convert_cache_days), Some(s.convert_cache_max_gb as u64 * 1_000_000_000).filter(|m| *m > 0)),
        _ => (None, Some(s.convert_cache_max_gb as u64 * 1_000_000_000).filter(|m| *m > 0)),
    };
    cache::prune(&d, days, max)
}

/// Tidy at start-up and then every hour, so old files go even when nothing is being converted.
pub fn spawn_tidier(st: Arc<AppState>) {
    tokio::spawn(async move {
        loop {
            let s = st.clone();
            let _ = tokio::task::spawn_blocking(move || tidy(&s)).await;
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    });
}

fn converting(st: &AppState) -> bool {
    st.jobs.all().iter().any(|j| matches!(j.kind.as_str(), "stick" | "burn") && !j.status().is_terminal())
}

pub async fn info(State(st): S) -> ApiResult<Json<Value>> {
    let d = dir(&st);
    let stats = tokio::task::spawn_blocking({
        let d = d.clone();
        move || cache::stats(&d)
    })
    .await
    .map_err(|e| ApiError::bad(e.to_string()))?;
    Ok(Json(json!({"dir": d.to_string_lossy(), "stats": stats, "mode": st.settings.get().convert_cache_mode})))
}

pub async fn clear(State(st): S) -> ApiResult<Json<Value>> {
    if converting(&st) {
        return Err(ApiError::bad("A burn or stick job is running and may be using these files. Try again when it has finished."));
    }
    let d = dir(&st);
    let r = tokio::task::spawn_blocking(move || cache::clear(&d)).await.map_err(|e| ApiError::bad(e.to_string()))?;
    Ok(Json(json!({"removed": r.removed, "freed_bytes": r.freed_bytes})))
}
