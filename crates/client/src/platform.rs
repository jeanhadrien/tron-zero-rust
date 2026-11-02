//! Native vs. browser differences, kept in one place.
//!
//! Browsers have no process environment, no config directory and no way to
//! quit, so the web build reads settings from URL query parameters and
//! `localStorage` instead.

/// A runtime setting: the process environment natively, the page's URL query
/// parameter of the same name in the browser (`?TRON_AUTOCONNECT=host:port`).
pub fn env_var(key: &str) -> Option<String> {
    #[cfg(not(target_family = "wasm"))]
    {
        std::env::var(key).ok()
    }
    #[cfg(target_family = "wasm")]
    {
        let search = web_sys::window()?.location().search().ok()?;
        web_sys::UrlSearchParams::new_with_str(&search)
            .ok()?
            .get(key)
    }
}

/// Whether the player can quit the app (there is no such thing in a browser tab).
pub const CAN_QUIT: bool = cfg!(not(target_family = "wasm"));

/// `localStorage` access for the web build's preferences.
#[cfg(target_family = "wasm")]
pub mod local_storage {
    fn storage() -> Option<web_sys::Storage> {
        web_sys::window()?.local_storage().ok()?
    }

    pub fn read(key: &str) -> Option<String> {
        storage()?.get_item(key).ok()?
    }

    pub fn write(key: &str, value: &str) -> Result<(), String> {
        storage()
            .ok_or("localStorage is unavailable")?
            .set_item(key, value)
            .map_err(|err| format!("{err:?}"))
    }
}
