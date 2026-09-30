//! Keeping converted files so the next job can reuse them instead of converting again.
//!
//! A converted file is stored under a name made from its source (path, size and modification
//! time), the conversion and whether the cover picture was kept, so a changed source or a
//! different setting never picks up a stale file. How long files stay is set in the web UI
//! (Settings → Converted files) or with `--cache-days` / `--cache-max-gb`.

use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use clap::Args;
use sha2::{Digest, Sha256};

/// A file used within this long is never removed by a clean-up (a job may be using it).
const IN_USE_GRACE: Duration = Duration::from_secs(15 * 60);

#[derive(Args, Debug, Clone, Default)]
pub struct CacheArgs {
    /// Keep converted files in this folder and reuse them next time (default: delete them after use)
    #[arg(long, value_name = "DIR")]
    pub convert_cache: Option<String>,
    /// Remove cached files not used for this many days
    #[arg(long, value_name = "DAYS", requires = "convert_cache")]
    pub cache_days: Option<u32>,
    /// Keep the cache under this size, removing the least recently used files first
    #[arg(long, value_name = "GB", requires = "convert_cache")]
    pub cache_max_gb: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct Cache {
    dir: PathBuf,
    days: Option<u32>,
    max_bytes: Option<u64>,
}

impl Cache {
    pub fn from_args(a: &CacheArgs) -> Option<Cache> {
        let dir = PathBuf::from(a.convert_cache.as_ref()?);
        std::fs::create_dir_all(&dir).ok()?;
        Some(Cache { dir, days: a.cache_days, max_bytes: a.cache_max_gb.filter(|g| *g > 0).map(|g| g as u64 * 1_000_000_000) })
    }

    /// Identify one conversion of one source file.
    pub fn key(src: &str, conversion: &str, keep_art: bool) -> Option<String> {
        let real = std::fs::canonicalize(src).ok()?;
        let m = std::fs::metadata(&real).ok()?;
        let mtime = m.modified().ok()?.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_secs();
        let mut h = Sha256::new();
        h.update(format!("{}|{}|{}|{}|{}", real.display(), m.len(), mtime, conversion, keep_art));
        Some(hex::encode(&h.finalize()[..16]))
    }

    fn path(&self, key: &str, ext: &str) -> PathBuf {
        self.dir.join(format!("{key}.{ext}"))
    }

    /// A converted file from an earlier run, if there is one. Using it counts as a use.
    pub fn get(&self, key: &str, ext: &str) -> Option<PathBuf> {
        let p = self.path(key, ext);
        let len = std::fs::metadata(&p).ok()?.len();
        if len == 0 {
            return None;
        }
        if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&p) {
            let _ = f.set_modified(SystemTime::now());
        }
        Some(p)
    }

    /// Where to write a conversion; call [`Cache::commit`] when it is complete.
    pub fn begin(&self, key: &str, ext: &str) -> PathBuf {
        // The real extension goes last: ffmpeg picks the output format from it.
        self.dir.join(format!("{key}.part.{ext}"))
    }

    pub fn commit(&self, part: &Path, key: &str, ext: &str) -> std::io::Result<PathBuf> {
        let done = self.path(key, ext);
        std::fs::rename(part, &done)?;
        Ok(done)
    }

    /// Apply the retention settings.
    pub fn prune(&self) -> Pruned {
        prune(&self.dir, self.days, self.max_bytes)
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Pruned {
    pub removed: usize,
    pub freed_bytes: u64,
}

struct Entry {
    path: PathBuf,
    size: u64,
    used: SystemTime,
}

fn is_partial(p: &Path) -> bool {
    p.file_name().is_some_and(|n| n.to_string_lossy().contains(".part."))
}

fn entries(dir: &Path) -> Vec<Entry> {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| {
                    let m = e.metadata().ok()?;
                    m.is_file().then(|| Entry { path: e.path(), size: m.len(), used: m.modified().unwrap_or(SystemTime::UNIX_EPOCH) })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Remove files older than `days` since they were last used (`Some(0)` removes everything that
/// isn't in use), then, if the cache is still over `max_bytes`, the least recently used ones.
pub fn prune(dir: &Path, days: Option<u32>, max_bytes: Option<u64>) -> Pruned {
    let now = SystemTime::now();
    let mut out = Pruned::default();
    let mut all = entries(dir);
    all.sort_by_key(|e| e.used);
    let mut remove = |e: &Entry, out: &mut Pruned| {
        if std::fs::remove_file(&e.path).is_ok() {
            out.removed += 1;
            out.freed_bytes += e.size;
            true
        } else {
            false
        }
    };
    let idle = |e: &Entry| now.duration_since(e.used).unwrap_or_default() > IN_USE_GRACE;
    let mut kept: Vec<Entry> = Vec::new();
    for e in all {
        let expired = days.is_some_and(|d| now.duration_since(e.used).unwrap_or_default() > Duration::from_secs(d as u64 * 86_400));
        let partial = is_partial(&e.path);
        if (expired || (partial && idle(&e))) && idle(&e) && remove(&e, &mut out) {
            continue;
        }
        kept.push(e);
    }
    if let Some(max) = max_bytes {
        let mut total: u64 = kept.iter().map(|e| e.size).sum();
        for e in &kept {
            if total <= max {
                break;
            }
            if idle(e) && remove(e, &mut out) {
                total -= e.size;
            }
        }
    }
    out
}

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct Stats {
    pub files: usize,
    pub bytes: u64,
    /// Seconds since the least recently used file was used.
    pub oldest_secs: Option<u64>,
}

pub fn stats(dir: &Path) -> Stats {
    let now = SystemTime::now();
    let all: Vec<Entry> = entries(dir).into_iter().filter(|e| !is_partial(&e.path)).collect();
    Stats {
        files: all.len(),
        bytes: all.iter().map(|e| e.size).sum(),
        oldest_secs: all.iter().map(|e| now.duration_since(e.used).unwrap_or_default().as_secs()).max(),
    }
}

/// Delete everything in the cache, including files a job might be using (callers check first).
pub fn clear(dir: &Path) -> Pruned {
    let mut out = Pruned::default();
    for e in entries(dir) {
        if std::fs::remove_file(&e.path).is_ok() {
            out.removed += 1;
            out.freed_bytes += e.size;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rd_cache_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn file(d: &Path, name: &str, size: usize, age_days: u64) {
        let p = d.join(name);
        std::fs::write(&p, vec![0u8; size]).unwrap();
        File::options().write(true).open(&p).unwrap().set_modified(SystemTime::now() - Duration::from_secs(age_days * 86_400)).unwrap();
    }

    #[test]
    fn keys_change_with_the_source_and_the_settings() {
        let d = dir("keys");
        let src = d.join("a.flac");
        std::fs::write(&src, b"one").unwrap();
        let s = src.to_str().unwrap();
        let k1 = Cache::key(s, "mp3:256", true).unwrap();
        assert_eq!(k1, Cache::key(s, "mp3:256", true).unwrap());
        assert_ne!(k1, Cache::key(s, "mp3:192", true).unwrap());
        assert_ne!(k1, Cache::key(s, "mp3:256", false).unwrap());
        std::fs::write(&src, b"changed source").unwrap();
        assert_ne!(k1, Cache::key(s, "mp3:256", true).unwrap());
        assert!(Cache::key("/no/such/file", "x", false).is_none());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn stores_finds_and_touches() {
        let d = dir("store");
        let c = Cache { dir: d.clone(), days: None, max_bytes: None };
        assert!(c.get("k", "mp3").is_none());
        let part = c.begin("k", "mp3");
        std::fs::write(&part, b"converted").unwrap();
        let done = c.commit(&part, "k", "mp3").unwrap();
        assert_eq!(c.get("k", "mp3").unwrap(), done);
        assert!(!part.exists());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn old_files_expire_but_recent_ones_stay() {
        let d = dir("expire");
        file(&d, "old.mp3", 100, 10);
        file(&d, "new.mp3", 100, 1);
        let r = prune(&d, Some(7), None);
        assert_eq!(r, Pruned { removed: 1, freed_bytes: 100 });
        assert!(!d.join("old.mp3").exists() && d.join("new.mp3").exists());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn the_size_limit_removes_the_least_recently_used_first() {
        let d = dir("limit");
        file(&d, "a.mp3", 400, 3);
        file(&d, "b.mp3", 400, 2);
        file(&d, "c.mp3", 400, 1);
        prune(&d, None, Some(800));
        assert!(!d.join("a.mp3").exists() && d.join("b.mp3").exists() && d.join("c.mp3").exists());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn files_in_use_are_left_alone_and_clear_takes_everything() {
        let d = dir("inuse");
        file(&d, "busy.mp3", 100, 0);
        // "delete after use" (0 days) still spares a file used a moment ago
        assert_eq!(prune(&d, Some(0), None).removed, 0);
        assert_eq!(stats(&d).files, 1);
        assert_eq!(clear(&d).removed, 1);
        assert_eq!(stats(&d).files, 0);
        std::fs::remove_dir_all(&d).ok();
    }
}
