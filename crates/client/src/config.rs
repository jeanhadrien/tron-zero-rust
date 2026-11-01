//! Persisted client preferences (name, key bindings, manager URL).
//!
//! A small JSON file in the platform config dir (override with
//! `TRON_CONFIG_FILE`). Reading or writing it can fail for ordinary reasons
//! (first run, read-only disk, hand-edit typos): every failure is logged and
//! the game carries on with defaults / unsaved settings.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use shared::protocol::sanitize_name;

use crate::settings::{Binding, PlayerName, TurnBindings};

const CONFIG_FILE_ENV: &str = "TRON_CONFIG_FILE";
const APP_DIR: &str = "tron-zero";
const FILE_NAME: &str = "client.json";

/// On-disk shape. Every field is optional so older or hand-trimmed files
/// still load; unknown fields are ignored.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedConfig {
    pub player_name: Option<String>,
    pub left_keys: Option<Vec<Binding>>,
    pub right_keys: Option<Vec<Binding>>,
    /// Lobby manager base URL; the `MANAGER_URL` env var still wins.
    pub manager_url: Option<String>,
}

impl SavedConfig {
    /// The name to start with (sanitized so a bad file cannot poison it).
    pub fn name(&self) -> PlayerName {
        self.player_name
            .as_deref()
            .map_or_else(PlayerName::default, |raw| PlayerName(sanitize_name(raw)))
    }

    /// Saved bindings if present and legal, else the defaults.
    pub fn bindings(&self) -> TurnBindings {
        let (Some(left), Some(right)) = (&self.left_keys, &self.right_keys) else {
            return TurnBindings::default();
        };
        let loaded = TurnBindings {
            left: left.clone(),
            right: right.clone(),
        };
        if loaded.is_valid() {
            loaded
        } else {
            warn!("Ignoring invalid saved key bindings");
            TurnBindings::default()
        }
    }
}

/// Where the config file lives, if the platform has a config dir.
pub fn config_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(CONFIG_FILE_ENV).filter(|p| !p.is_empty()) {
        return Some(path.into());
    }
    dirs::config_dir().map(|dir| dir.join(APP_DIR).join(FILE_NAME))
}

/// Read the config; a missing, unreadable or corrupt file yields defaults.
pub fn load_from(path: &Path) -> SavedConfig {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|err| {
            warn!(path = %path.display(), %err, "Config file is corrupt; using defaults");
            SavedConfig::default()
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => SavedConfig::default(),
        Err(err) => {
            warn!(path = %path.display(), %err, "Could not read config; using defaults");
            SavedConfig::default()
        }
    }
}

/// Write the config, creating its directory. Errors are returned for the
/// caller to log.
pub fn save_to(path: &Path, config: &SavedConfig) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(config).map_err(std::io::Error::other)?;
    std::fs::write(path, text)
}

/// Where and what to persist; the manager URL is carried through untouched.
#[derive(Resource, Debug, Clone)]
pub struct Preferences {
    path: Option<PathBuf>,
    manager_url: Option<String>,
}

impl Preferences {
    /// Load the config file and return it with the handle used to save later.
    pub fn load() -> (Self, SavedConfig) {
        let path = config_path();
        let saved = path.as_deref().map(load_from).unwrap_or_default();
        match &path {
            Some(path) => info!(path = %path.display(), "Client preferences"),
            None => warn!("No config directory on this platform; settings will not persist"),
        }
        let prefs = Self {
            path,
            manager_url: saved.manager_url.clone(),
        };
        (prefs, saved)
    }
}

/// Save name and bindings whenever either changes. The first run is skipped:
/// resources just inserted from the file count as "changed".
pub fn persist_preferences(
    prefs: Res<Preferences>,
    name: Res<PlayerName>,
    bindings: Res<TurnBindings>,
    mut started: Local<bool>,
) {
    if !*started {
        *started = true;
        return;
    }
    if !(name.is_changed() || bindings.is_changed()) {
        return;
    }
    let Some(path) = &prefs.path else {
        return;
    };
    let config = SavedConfig {
        player_name: Some(name.0.clone()),
        left_keys: Some(bindings.left.clone()),
        right_keys: Some(bindings.right.clone()),
        manager_url: prefs.manager_url.clone(),
    };
    if let Err(err) = save_to(path, &config) {
        warn!(path = %path.display(), %err, "Could not save preferences");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(label: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("tron-zero-config-{}-{label}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("nested").join(FILE_NAME)
    }

    #[test]
    fn round_trips_name_bindings_and_manager_url() {
        let path = temp_file("roundtrip");
        let mut bindings = TurnBindings::default();
        bindings.left.truncate(1);
        let config = SavedConfig {
            player_name: Some("Neo".into()),
            left_keys: Some(bindings.left.clone()),
            right_keys: Some(bindings.right.clone()),
            manager_url: Some("http://lobby.example:3001".into()),
        };
        save_to(&path, &config).unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded, config);
        assert_eq!(loaded.name().0, "Neo");
        assert_eq!(loaded.bindings(), bindings);
        let _ = std::fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn missing_corrupt_or_illegal_files_fall_back_to_defaults() {
        let path = temp_file("bad");
        assert_eq!(load_from(&path), SavedConfig::default());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(load_from(&path), SavedConfig::default());
        // Parses, but binds Escape: the bindings are rejected, the name kept.
        let mut bindings = TurnBindings::default();
        bindings.left[0] = Binding::from_code(KeyCode::Escape);
        let config = SavedConfig {
            player_name: Some("  Trin\u{7}ity  ".into()),
            left_keys: Some(bindings.left),
            right_keys: Some(bindings.right),
            manager_url: None,
        };
        assert_eq!(config.bindings(), TurnBindings::default());
        assert_eq!(config.name().0, "Trinity");
        let _ = std::fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn unwritable_location_reports_an_error_instead_of_panicking() {
        // A path whose parent is a regular file can never be created.
        let file = temp_file("blocked");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "x").unwrap();
        assert!(save_to(&file.join("client.json"), &SavedConfig::default()).is_err());
        let _ = std::fs::remove_dir_all(file.parent().unwrap().parent().unwrap());
    }
}
