//! User settings, stored as JSON in the config directory so they survive restarts
//! (mount a volume there when running in Docker).

use std::{
    path::{Path, PathBuf},
    sync::RwLock,
};

use serde::{Deserialize, Serialize};

use crate::rip::cover::{self, CoverOptions, CoverSource};

pub const FORMATS: &[&str] = &["flac", "wav", "alac", "aiff", "ogg", "mp3", "opus", "aac"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    // Rip defaults (pre-fill the Rip page)
    pub rip_format: String,
    pub rip_archive: bool,
    pub rip_skip_musicbrainz: bool,
    pub rip_skip_accuraterip: bool,
    /// Quality choice for the default format (empty = the best).
    pub rip_quality: String,
    /// Add ReplayGain tags after ripping.
    pub rip_replaygain: bool,
    /// Measure dynamic range (DR) after ripping.
    pub rip_dynamic_range: bool,
    /// Drive read offset: "off", "auto", or a number of samples.
    pub rip_offset: String,

    // Cover art
    /// Enabled sources, best first: "fanart" and/or "caa".
    pub cover_sources: Vec<String>,
    /// Save cover.jpg / cover.png next to the tracks.
    pub cover_save_file: bool,
    /// Embed the cover in every audio file.
    pub cover_embed: bool,
    /// fanart.tv API key. Never sent back to the browser.
    #[serde(default)]
    pub fanart_api_key: String,

    // Rusty Stick
    /// Folders (absolute paths) that may be written to besides detected USB sticks.
    pub stick_extra_folders: Vec<String>,
    /// Default folder layout: a preset id.
    pub stick_preset: String,

    // Converted files (from "convert to fit" in Rusty Stick and Data discs)
    /// "delete" (right after use), "days" (keep for `convert_cache_days`) or "forever".
    pub convert_cache_mode: String,
    pub convert_cache_days: u32,
    /// Keep the cache under this many GB, removing the least recently used first (0 = no limit).
    pub convert_cache_max_gb: u32,
    /// Where they are kept; empty = the `cache` folder next to the settings.
    pub convert_cache_dir: String,

    // Music library (importing rips)
    /// The library folder rips are imported into. Empty = not set up yet.
    pub library_path: String,
    /// A Picard naming script. Empty = the built-in one.
    pub library_script: String,
    /// "move", "copy" or "hardlink"
    pub import_mode: String,
    pub import_cover: bool,
    pub import_other: bool,
    pub import_delete_leftovers: bool,
    /// skip | replace | higher_quality | lower_quality | newer | keep_both
    pub import_conflict: String,

    // Lidarr
    pub lidarr_url: String,
    /// Never sent back to the browser.
    #[serde(default)]
    pub lidarr_api_key: String,
    /// Where new artists go; empty = Lidarr's first root folder.
    pub lidarr_root_folder: String,
    /// 0 = the root folder's default.
    pub lidarr_quality_profile: u64,
    pub lidarr_metadata_profile: u64,
    /// A RustyDisc path prefix and what Lidarr calls the same place.
    pub lidarr_path_from: String,
    pub lidarr_path_to: String,
    /// "move" or "copy"
    pub lidarr_mode: String,
    /// "rustydisc" (the naming script) or "lidarr": what Import uses by default.
    pub import_target: String,

    // Optional login
    pub auth_user: String,
    /// Argon2 hash. Never sent to the browser.
    pub auth_password_hash: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            rip_format: "flac".into(),
            rip_archive: false,
            rip_skip_musicbrainz: false,
            rip_skip_accuraterip: false,
            rip_quality: String::new(),
            rip_replaygain: false,
            rip_dynamic_range: false,
            rip_offset: "off".into(),
            cover_sources: vec!["caa".into()],
            cover_save_file: true,
            cover_embed: true,
            fanart_api_key: String::new(),
            stick_extra_folders: Vec::new(),
            stick_preset: "artist-album".into(),
            convert_cache_mode: "delete".into(),
            convert_cache_days: 7,
            convert_cache_max_gb: 20,
            convert_cache_dir: String::new(),
            library_path: String::new(),
            library_script: String::new(),
            import_mode: "move".into(),
            import_cover: true,
            import_other: false,
            import_delete_leftovers: false,
            import_conflict: "skip".into(),
            lidarr_url: String::new(),
            lidarr_api_key: String::new(),
            lidarr_root_folder: String::new(),
            lidarr_quality_profile: 0,
            lidarr_metadata_profile: 0,
            lidarr_path_from: String::new(),
            lidarr_path_to: String::new(),
            lidarr_mode: "move".into(),
            import_target: "rustydisc".into(),
            auth_user: String::new(),
            auth_password_hash: String::new(),
        }
    }
}

impl Settings {
    /// Check values and tidy them (dedupe sources, trim the key).
    pub fn validate(mut self) -> Result<Settings, String> {
        if !FORMATS.contains(&self.rip_format.as_str()) {
            return Err(format!("Unknown audio format '{}'", self.rip_format));
        }
        self.rip_quality = self.rip_quality.trim().to_string();
        if !self.rip_quality.is_empty() {
            let fmt: crate::rip::encoder::AudioFormat = self.rip_format.parse().map_err(|e: String| e)?;
            if !crate::rip::encoder::quality_choices(&fmt).iter().any(|c| c.id == self.rip_quality) {
                self.rip_quality.clear();
            }
        }
        let mut seen: Vec<CoverSource> = Vec::new();
        for s in &self.cover_sources {
            let parsed: CoverSource = s.parse()?;
            if !seen.contains(&parsed) {
                seen.push(parsed);
            }
        }
        self.cover_sources = seen.iter().map(|s| s.id().to_string()).collect();
        self.stick_extra_folders = self
            .stick_extra_folders
            .iter()
            .map(|f| f.trim().trim_end_matches('/').to_string())
            .filter(|f| !f.is_empty())
            .collect();
        for f in &self.stick_extra_folders {
            if !f.starts_with('/') || f.split('/').any(|c| c == "..") || f == "/" {
                return Err(format!("'{f}' isn't a usable folder: give an absolute path such as /mnt/usb"));
            }
        }
        if !crate::stick::layout::PRESETS.iter().any(|p| p.id == self.stick_preset) {
            self.stick_preset = "artist-album".into();
        }
        if !["delete", "days", "forever"].contains(&self.convert_cache_mode.as_str()) {
            self.convert_cache_mode = "delete".into();
        }
        self.convert_cache_days = self.convert_cache_days.clamp(1, 3650);
        self.convert_cache_max_gb = self.convert_cache_max_gb.min(100_000);
        self.convert_cache_dir = self.convert_cache_dir.trim().trim_end_matches('/').to_string();
        if !self.convert_cache_dir.is_empty() && (!self.convert_cache_dir.starts_with('/') || self.convert_cache_dir.split('/').any(|c| c == "..")) {
            return Err(format!("'{}' isn't a usable folder: give an absolute path such as /cache", self.convert_cache_dir));
        }
        self.library_path = self.library_path.trim().trim_end_matches('/').to_string();
        if !self.library_path.is_empty() && (!self.library_path.starts_with('/') || self.library_path.split('/').any(|c| c == "..") || self.library_path == "/") {
            return Err(format!("'{}' isn't a usable library folder: give an absolute path such as /library", self.library_path));
        }
        if self.import_mode.parse::<crate::library::import::Mode>().is_err() {
            self.import_mode = "move".into();
        }
        if self.import_conflict.parse::<crate::stick::existing::Conflict>().is_err() {
            self.import_conflict = "skip".into();
        }
        // The built-in script is stored as "empty", so improvements to it reach everyone who hasn't customised it.
        let norm = |t: &str| t.replace("\r\n", "\n").trim().to_string();
        if self.library_script.trim().is_empty() || norm(&self.library_script) == norm(crate::library::script::DEFAULT_SCRIPT) {
            self.library_script.clear();
        } else {
            if self.library_script.len() > 100_000 {
                return Err("That naming script is too long".into());
            }
            crate::library::script::check(&self.library_script).map_err(|e| format!("The naming script has a problem: {e}"))?;
        }
        self.lidarr_url = self.lidarr_url.trim().trim_end_matches('/').to_string();
        if !self.lidarr_url.is_empty() && !(self.lidarr_url.starts_with("http://") || self.lidarr_url.starts_with("https://")) {
            return Err("The Lidarr address must start with http:// or https://".into());
        }
        self.lidarr_api_key = self.lidarr_api_key.trim().to_string();
        if self.lidarr_api_key.len() > 200 || self.lidarr_api_key.chars().any(char::is_whitespace) {
            return Err("That doesn't look like a Lidarr API key".into());
        }
        self.lidarr_root_folder = self.lidarr_root_folder.trim().to_string();
        self.lidarr_path_from = self.lidarr_path_from.trim().trim_end_matches('/').to_string();
        self.lidarr_path_to = self.lidarr_path_to.trim().trim_end_matches('/').to_string();
        if self.lidarr_mode != "copy" {
            self.lidarr_mode = "move".into();
        }
        if self.import_target != "lidarr" {
            self.import_target = "rustydisc".into();
        }
        self.fanart_api_key = self.fanart_api_key.trim().to_string();
        if self.fanart_api_key.len() > 200 || self.fanart_api_key.chars().any(char::is_whitespace) {
            return Err("That doesn't look like a fanart.tv API key".into());
        }
        Ok(self)
    }

    pub fn cover_options(&self) -> CoverOptions {
        CoverOptions {
            sources: cover::parse_sources(&self.cover_sources.join(",")).unwrap_or_default(),
            fanart_key: Some(self.fanart_api_key.clone()).filter(|k| !k.is_empty()),
            save_file: self.cover_save_file,
            embed: self.cover_embed,
        }
    }

    /// The settings as shown to the browser: everything except the key itself.
    pub fn public(&self) -> serde_json::Value {
        let mut v = serde_json::to_value(self).unwrap_or_default();
        if let Some(o) = v.as_object_mut() {
            o.remove("fanart_api_key");
            o.remove("auth_password_hash");
            o.remove("auth_user");
            o.remove("lidarr_api_key");
            let lk = &self.lidarr_api_key;
            o.insert("lidarr_api_key_set".into(), serde_json::json!(!lk.is_empty()));
            o.insert("lidarr_api_key_hint".into(), serde_json::json!(if lk.len() > 4 { format!("••••{}", &lk[lk.len() - 4..]) } else if lk.is_empty() { String::new() } else { "••••".into() }));
            let key = &self.fanart_api_key;
            o.insert("fanart_api_key_set".into(), serde_json::json!(!key.is_empty()));
            let hint = if key.len() > 4 { format!("••••{}", &key[key.len() - 4..]) } else if key.is_empty() { String::new() } else { "••••".into() };
            o.insert("fanart_api_key_hint".into(), serde_json::json!(hint));
        }
        v
    }
}

pub struct Store {
    path: PathBuf,
    current: RwLock<Settings>,
}

impl Store {
    /// Load settings from `dir/settings.json`; missing or unreadable files give the defaults.
    pub fn load(dir: &Path) -> Store {
        let path = dir.join("settings.json");
        let current = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Settings>(&b).ok())
            .and_then(|s| s.validate().ok())
            .unwrap_or_default();
        Store { path, current: RwLock::new(current) }
    }

    pub fn get(&self) -> Settings {
        self.current.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn set(&self, new: Settings) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // Write to a temp file and rename so a crash can't leave half a file.
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&new)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?; // holds an API key
        }
        std::fs::rename(&tmp, &self.path)?;
        *self.current.write().unwrap_or_else(|e| e.into_inner()) = new;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rustydisc_settings_{}_{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn defaults_when_no_file() {
        let d = tmpdir("none");
        assert_eq!(Store::load(&d).get(), Settings::default());
    }

    #[test]
    fn saves_and_reloads_and_hides_the_key() {
        let d = tmpdir("roundtrip");
        let store = Store::load(&d);
        let s = Settings {
            rip_format: "mp3".into(),
            cover_sources: vec!["fanart".into(), "caa".into()],
            cover_embed: false,
            fanart_api_key: "abcdef1234567890".into(),
            ..Default::default()
        };
        store.set(s.clone().validate().unwrap()).unwrap();
        assert_eq!(Store::load(&d).get(), s);

        let public = s.public();
        assert!(public.get("fanart_api_key").is_none());
        assert_eq!(public["fanart_api_key_set"], true);
        assert_eq!(public["fanart_api_key_hint"], "••••7890");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn validation_rejects_bad_values_and_cleans_up() {
        assert!(Settings { rip_format: "wma".into(), ..Default::default() }.validate().is_err());
        assert!(Settings { cover_sources: vec!["bing".into()], ..Default::default() }.validate().is_err());
        assert!(Settings { fanart_api_key: "has space".into(), ..Default::default() }.validate().is_err());
        let s = Settings { cover_sources: vec!["CAA".into(), "fanart.tv".into(), "caa".into()], fanart_api_key: "  key ".into(), ..Default::default() }
            .validate()
            .unwrap();
        assert_eq!(s.cover_sources, vec!["caa", "fanart"]);
        assert_eq!(s.fanart_api_key, "key");
    }

    #[test]
    fn corrupt_file_falls_back_to_defaults() {
        let d = tmpdir("corrupt");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("settings.json"), b"{ not json").unwrap();
        assert_eq!(Store::load(&d).get(), Settings::default());
        let _ = std::fs::remove_dir_all(&d);
    }
}
