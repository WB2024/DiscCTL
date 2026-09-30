//! User settings, stored as JSON in the config directory so they survive restarts
//! (mount a volume there when running in Docker).

use std::{
    path::{Path, PathBuf},
    sync::RwLock,
};

use serde::{Deserialize, Serialize};

use crate::rip::cover::{self, CoverOptions, CoverSource};

pub const FORMATS: &[&str] = &["flac", "wav", "alac", "aiff", "ogg", "mp3", "opus"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    // Rip defaults (pre-fill the Rip page)
    pub rip_format: String,
    pub rip_archive: bool,
    pub rip_skip_musicbrainz: bool,
    pub rip_skip_accuraterip: bool,

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
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            rip_format: "flac".into(),
            rip_archive: false,
            rip_skip_musicbrainz: false,
            rip_skip_accuraterip: false,
            cover_sources: vec!["caa".into()],
            cover_save_file: true,
            cover_embed: true,
            fanart_api_key: String::new(),
        }
    }
}

impl Settings {
    /// Check values and tidy them (dedupe sources, trim the key).
    pub fn validate(mut self) -> Result<Settings, String> {
        if !FORMATS.contains(&self.rip_format.as_str()) {
            return Err(format!("Unknown audio format '{}'", self.rip_format));
        }
        let mut seen: Vec<CoverSource> = Vec::new();
        for s in &self.cover_sources {
            let parsed: CoverSource = s.parse()?;
            if !seen.contains(&parsed) {
                seen.push(parsed);
            }
        }
        self.cover_sources = seen.iter().map(|s| s.id().to_string()).collect();
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
