//! Native vs. browser differences, kept in one place.
//!
//! Browsers have no process environment, no config directory and no way to
//! quit, so the web build reads settings from `localStorage` and, on a local
//! dev host only, from URL query parameters.

/// A runtime setting: the process environment natively, the page's URL query
/// parameter of the same name in the browser (`?TRON_AUTOCONNECT=host:port`).
///
/// Query parameters are honored only when the page itself is served from a
/// local dev host ([`is_local_dev_host`]). On the production origin a crafted
/// link must not be able to repoint the lobby or auto-join a stranger's
/// server, so there only the build-time defaults and saved preferences apply.
pub fn env_var(key: &str) -> Option<String> {
    #[cfg(not(target_family = "wasm"))]
    {
        std::env::var(key).ok()
    }
    #[cfg(target_family = "wasm")]
    {
        let location = web_sys::window()?.location();
        if !is_local_dev_host(&location.hostname().ok()?) {
            return None;
        }
        let search = location.search().ok()?;
        web_sys::UrlSearchParams::new_with_str(&search)
            .ok()?
            .get(key)
    }
}

/// Whether a page hostname is a local development host: `localhost`,
/// `*.localhost`, `127.0.0.1` or `[::1]` (as `Location::hostname` spells it).
#[cfg_attr(not(target_family = "wasm"), allow(dead_code))]
pub fn is_local_dev_host(hostname: &str) -> bool {
    let host = hostname.trim_end_matches('.').to_ascii_lowercase();
    host == "localhost"
        || host.ends_with(".localhost")
        || host == "127.0.0.1"
        || host == "[::1]"
        || host == "::1"
}

/// Whether the player can quit the app (there is no such thing in a browser tab).
pub const CAN_QUIT: bool = cfg!(not(target_family = "wasm"));

#[cfg(test)]
mod tests {
    use super::is_local_dev_host;

    #[test]
    fn only_local_hosts_may_override_settings() {
        for host in [
            "localhost",
            "LOCALHOST.",
            "app.localhost",
            "127.0.0.1",
            "[::1]",
        ] {
            assert!(is_local_dev_host(host), "{host}");
        }
        for host in [
            "play.tronzero.dev",
            "localhost.evil.com",
            "evil-localhost",
            "127.0.0.1.evil.com",
            "10.0.0.5",
            "",
        ] {
            assert!(!is_local_dev_host(host), "{host}");
        }
    }
}

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
