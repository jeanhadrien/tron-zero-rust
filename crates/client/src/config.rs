//! Persisted client preferences (name, key bindings, rider look, manager URL).
//!
//! A small JSON file in the platform config dir (override with
//! `TRON_CONFIG_FILE`). Reading or writing it can fail for ordinary reasons
//! (first run, read-only disk, hand-edit typos): every failure is logged and
//! the game carries on with defaults / unsaved settings.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use shared::RiderShape;
use shared::protocol::sanitize_name;

use crate::account::{Accounts, SavedAccount};
use crate::look::{Look, random_look};
use crate::render::FxSettings;
use crate::settings::{Binding, PlayerName, TurnBindings};
use crate::theme::{Theme, ThemeId};

#[cfg(not(target_family = "wasm"))]
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
    /// Selected visual theme; `None` (or an unknown name) means the default.
    #[serde(deserialize_with = "lenient")]
    pub theme: Option<ThemeId>,
    /// Particle effects on or off; `None` means on.
    pub effects: Option<bool>,
    /// Rider color (packed RGB); `None` means "roll one on first launch".
    #[serde(deserialize_with = "lenient")]
    pub rider_color: Option<u32>,
    /// Rider shape; `None` (or an unknown name) means "roll one".
    #[serde(deserialize_with = "lenient")]
    pub rider_shape: Option<RiderShape>,
    /// Anonymous ranked accounts, by manager base URL.
    pub accounts: BTreeMap<String, SavedAccount>,
}

/// A value this build cannot read (an unknown theme or shape saved by another
/// version, a typo) reads as "not set" instead of failing the whole file, which
/// would throw away the name and key bindings too.
fn lenient<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let raw = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(raw.and_then(|value| serde_json::from_value(value).ok()))
}

impl SavedConfig {
    /// Effects setting to start with.
    pub fn fx(&self) -> FxSettings {
        FxSettings {
            enabled: self.effects.unwrap_or(true),
        }
    }

    /// The theme to start with.
    pub fn theme(&self) -> Theme {
        Theme::from_id(self.theme.unwrap_or_default())
    }

    /// The look to start with. Missing parts are rolled randomly; the flag says
    /// whether anything was rolled, so the caller can save it (a look that is
    /// not saved would change on every launch).
    pub fn look(&self) -> (Look, bool) {
        let rolled = (self.rider_color.is_none() || self.rider_shape.is_none()).then(random_look);
        let look = Look {
            color: self
                .rider_color
                .map(|color| color & 0xFF_FFFF)
                .or(rolled.map(|r| r.color))
                .unwrap_or_default(),
            shape: self
                .rider_shape
                .or(rolled.map(|r| r.shape))
                .unwrap_or_default(),
        };
        (look, rolled.is_some())
    }

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

/// Where the config lives, if the platform has a place for it. In the browser
/// the "path" is only the `localStorage` key.
#[cfg(target_family = "wasm")]
pub fn config_path() -> Option<PathBuf> {
    Some(PathBuf::from(APP_DIR).join(FILE_NAME))
}

/// Where the config file lives, if the platform has a config dir.
#[cfg(not(target_family = "wasm"))]
pub fn config_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(CONFIG_FILE_ENV).filter(|p| !p.is_empty()) {
        return Some(path.into());
    }
    dirs::config_dir().map(|dir| dir.join(APP_DIR).join(FILE_NAME))
}

/// Read the config from `localStorage`; missing or corrupt yields defaults.
#[cfg(target_family = "wasm")]
pub fn load_from(path: &Path) -> SavedConfig {
    let Some(text) = crate::platform::local_storage::read(&path.to_string_lossy()) else {
        return SavedConfig::default();
    };
    serde_json::from_str(&text).unwrap_or_else(|err| {
        warn!(%err, "Saved config is corrupt; using defaults");
        SavedConfig::default()
    })
}

/// Write the config to `localStorage`.
#[cfg(target_family = "wasm")]
pub fn save_to(path: &Path, config: &SavedConfig) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(config).map_err(std::io::Error::other)?;
    crate::platform::local_storage::write(&path.to_string_lossy(), &text)
        .map_err(std::io::Error::other)
}

/// Read the config; a missing, unreadable or corrupt file yields defaults.
#[cfg(not(target_family = "wasm"))]
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

/// Write the config, creating its directory. The file is written next to its
/// destination and renamed over it, so a crash or a full disk mid-write leaves
/// the previous config intact instead of a truncated one. Errors are returned
/// for the caller to log.
#[cfg(not(target_family = "wasm"))]
pub fn save_to(path: &Path, config: &SavedConfig) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(config).map_err(std::io::Error::other)?;
    let mut temp = path.as_os_str().to_owned();
    temp.push(".tmp");
    let temp = PathBuf::from(temp);
    std::fs::write(&temp, text)?;
    std::fs::rename(&temp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temp);
    })
}

/// How long the preferences must stay unchanged before they are written, so
/// typing a name does not hit the disk on every keystroke.
const SAVE_DEBOUNCE_SECS: f64 = 0.5;

/// Where and what to persist; the manager URL is carried through untouched.
#[derive(Resource, Debug, Clone)]
pub struct Preferences {
    path: Option<PathBuf>,
    manager_url: Option<String>,
    /// When the newest unsaved change happened (`Time<Real>` seconds).
    dirty_since: Option<f64>,
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
            dirty_since: None,
        };
        (prefs, saved)
    }

    /// Schedule a save for something the preference systems cannot see change
    /// (a look rolled at startup: the first run of the persist system is skipped).
    pub fn mark_dirty(&mut self) {
        self.dirty_since.get_or_insert(0.0);
    }
}

/// Save name and bindings once they have stopped changing for
/// [`SAVE_DEBOUNCE_SECS`], and immediately on app exit. The first run is
/// skipped: resources just inserted from the file count as "changed".
// Bevy injects each resource as its own parameter; the count is structural.
#[allow(clippy::too_many_arguments)]
pub fn persist_preferences(
    mut prefs: ResMut<Preferences>,
    name: Res<PlayerName>,
    bindings: Res<TurnBindings>,
    theme: Res<Theme>,
    fx: Res<FxSettings>,
    look: Res<Look>,
    accounts: Res<Accounts>,
    time: Res<Time<Real>>,
    mut exits: MessageReader<AppExit>,
    mut started: Local<bool>,
) {
    let now = time.elapsed_secs_f64();
    let exiting = exits.read().count() > 0;
    if !*started {
        *started = true;
    } else if name.is_changed()
        || bindings.is_changed()
        || theme.is_changed()
        || fx.is_changed()
        || look.is_changed()
        || accounts.is_changed()
    {
        prefs.dirty_since = Some(now);
    }
    let Some(since) = prefs.dirty_since else {
        return;
    };
    if !exiting && now - since < SAVE_DEBOUNCE_SECS {
        return;
    }
    prefs.dirty_since = None;
    let Some(path) = &prefs.path else {
        return;
    };
    let config = SavedConfig {
        player_name: Some(name.0.clone()),
        left_keys: Some(bindings.left.clone()),
        right_keys: Some(bindings.right.clone()),
        manager_url: prefs.manager_url.clone(),
        theme: Some(theme.id),
        effects: Some(fx.enabled),
        rider_color: Some(look.color),
        rider_shape: Some(look.shape),
        accounts: accounts.0.clone(),
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
            theme: Some(ThemeId::Grid82),
            effects: Some(false),
            rider_color: Some(0x12_34AB),
            rider_shape: Some(RiderShape::Hexagon),
            accounts: BTreeMap::from([(
                "http://lobby.example:3001".into(),
                SavedAccount {
                    id: "u1".into(),
                    credential: "u1.secret".into(),
                },
            )]),
        };
        save_to(&path, &config).unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded, config);
        assert_eq!(loaded.name().0, "Neo");
        assert_eq!(loaded.bindings(), bindings);
        let (look, rolled) = loaded.look();
        assert!(!rolled, "a complete saved look is never re-rolled");
        assert_eq!(look.color, 0x12_34AB);
        assert_eq!(look.shape, RiderShape::Hexagon);
        let _ = std::fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn an_unknown_theme_keeps_the_rest_of_the_file() {
        let text = r#"{ "player_name": "Neo", "theme": "FromTheFuture", "effects": false }"#;
        let config: SavedConfig = serde_json::from_str(text).unwrap();
        assert_eq!(config.theme, None);
        assert_eq!(config.theme().id, ThemeId::default());
        assert_eq!(config.name().0, "Neo");
        assert_eq!(config.effects, Some(false));
    }

    #[test]
    fn an_unknown_shape_or_bad_color_is_rolled_without_losing_the_file() {
        let text = r#"{ "player_name": "Neo", "rider_shape": "Blob", "rider_color": -4 }"#;
        let config: SavedConfig = serde_json::from_str(text).unwrap();
        assert_eq!((config.rider_shape, config.rider_color), (None, None));
        assert_eq!(config.name().0, "Neo");
        let (_, rolled) = config.look();
        assert!(rolled);
    }

    #[test]
    fn only_the_missing_part_of_the_look_is_rolled() {
        let color_only = SavedConfig {
            rider_color: Some(0xABCDEF),
            ..Default::default()
        };
        let (look, rolled) = color_only.look();
        assert!(rolled);
        assert_eq!(look.color, 0xABCDEF);
        let shape_only = SavedConfig {
            rider_shape: Some(RiderShape::Chevron),
            ..Default::default()
        };
        let (look, rolled) = shape_only.look();
        assert!(rolled);
        assert_eq!(look.shape, RiderShape::Chevron);
        // The wire only carries 24 bits; a hand-edited wider value is masked.
        let wide = SavedConfig {
            rider_color: Some(0xFF12_3456),
            rider_shape: Some(RiderShape::Circle),
            ..Default::default()
        };
        assert_eq!(wide.look().0.color, 0x12_3456);
    }

    #[test]
    fn a_rolled_look_is_saved_even_though_the_first_run_is_skipped() {
        let path = temp_file("rolled");
        let mut world = persist_world(path.clone());
        world.resource_mut::<Preferences>().mark_dirty();
        let mut schedule = Schedule::default();
        schedule.add_systems(persist_preferences);
        frame(&mut world, &mut schedule, 1);
        assert!(!path.exists(), "still debounced");
        frame(&mut world, &mut schedule, 600);
        let saved = load_from(&path);
        assert_eq!(saved.rider_color, Some(Look::default().color));
        assert_eq!(saved.rider_shape, Some(Look::default().shape));
        let _ = std::fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn changing_the_look_saves_it() {
        let path = temp_file("look-change");
        let mut world = persist_world(path.clone());
        let mut schedule = Schedule::default();
        schedule.add_systems(persist_preferences);
        frame(&mut world, &mut schedule, 1);
        world.resource_mut::<Look>().shape = RiderShape::Diamond;
        frame(&mut world, &mut schedule, 100);
        frame(&mut world, &mut schedule, 600);
        assert_eq!(load_from(&path).rider_shape, Some(RiderShape::Diamond));
        let _ = std::fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn saving_replaces_the_file_atomically_and_leaves_no_temp_file() {
        let path = temp_file("atomic");
        let first = SavedConfig {
            player_name: Some("One".into()),
            ..Default::default()
        };
        let second = SavedConfig {
            player_name: Some("Two".into()),
            ..Default::default()
        };
        save_to(&path, &first).unwrap();
        save_to(&path, &second).unwrap();
        assert_eq!(load_from(&path), second);
        let entries: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(entries, vec![std::ffi::OsString::from(FILE_NAME)]);
        let _ = std::fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    }

    fn persist_world(path: PathBuf) -> World {
        let mut world = World::new();
        world.init_resource::<Time<Real>>();
        world.init_resource::<Messages<AppExit>>();
        world.init_resource::<TurnBindings>();
        world.insert_resource(PlayerName("Neo".into()));
        world.init_resource::<Theme>();
        world.init_resource::<FxSettings>();
        world.init_resource::<Look>();
        world.init_resource::<Accounts>();
        world.insert_resource(Preferences {
            path: Some(path),
            manager_url: None,
            dirty_since: None,
        });
        world
    }

    /// Run the system once with a persistent `Local` (the real one runs every
    /// frame); a schedule keeps its state between runs.
    fn frame(world: &mut World, schedule: &mut Schedule, advance_ms: u64) {
        world
            .resource_mut::<Time<Real>>()
            .advance_by(std::time::Duration::from_millis(advance_ms));
        schedule.run(world);
        world.clear_trackers();
    }

    #[test]
    fn typing_is_saved_once_after_the_debounce_and_on_exit() {
        let path = temp_file("debounce");
        let mut world = persist_world(path.clone());
        let mut schedule = Schedule::default();
        schedule.add_systems(persist_preferences);
        frame(&mut world, &mut schedule, 1); // first run is skipped
        assert!(!path.exists());
        for letter in ["N", "Ne", "Neo2"] {
            world.resource_mut::<PlayerName>().0 = letter.into();
            frame(&mut world, &mut schedule, 100);
            assert!(!path.exists(), "no write while typing");
        }
        frame(&mut world, &mut schedule, 600);
        assert_eq!(load_from(&path).player_name.as_deref(), Some("Neo2"));
        std::fs::remove_file(&path).unwrap();
        frame(&mut world, &mut schedule, 600);
        assert!(!path.exists(), "nothing new to save");
        // A change right before exit is flushed without waiting.
        world.resource_mut::<PlayerName>().0 = "Last".into();
        world.write_message(AppExit::Success);
        frame(&mut world, &mut schedule, 1);
        assert_eq!(load_from(&path).player_name.as_deref(), Some("Last"));
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
            theme: None,
            effects: None,
            rider_color: None,
            rider_shape: None,
            accounts: BTreeMap::new(),
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
