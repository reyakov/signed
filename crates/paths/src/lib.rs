//! Paths to locations used by Signed.
//!
//! Follows the same pattern as Zed's `paths` crate: platform-correct base
//! directories, resolved once and cached, with an optional custom data dir
//! override for portable/dev installs.

use std::path::PathBuf;
use std::sync::OnceLock;

/// The application name, used to derive platform-specific data, config and
/// cache directory paths.
pub const APP_NAME: &str = "Signed";

/// Lowercased form of [`APP_NAME`], for use in XDG-style paths on
/// Linux/FreeBSD and the macOS `~/.config` fallback.
pub const APP_NAME_LOWERCASE: &str = "signed";

/// A custom data directory override, set only by [`set_custom_data_dir`].
static CUSTOM_DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// The resolved data directory.
/// On macOS, this is `~/Library/Application Support/Signed`.
/// On Linux/FreeBSD, this is `$XDG_DATA_HOME/signed`.
/// On Windows, this is `%LOCALAPPDATA%\Signed`.
static CURRENT_DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// The resolved config directory.
/// On macOS, this is `~/.config/signed`.
/// On Linux/FreeBSD, this is `$XDG_CONFIG_HOME/signed`.
/// On Windows, this is `%APPDATA%\Signed`.
static CONFIG_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Returns the current user's home directory.
pub fn home_dir() -> PathBuf {
    dirs::home_dir().expect("failed to determine home directory")
}

/// Returns the current user's Desktop folder, falling back to the home
/// directory (or an empty path) when it can't be determined.
pub fn desktop_dir() -> PathBuf {
    dirs::desktop_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_default())
}

/// Sets a custom directory for all user data, overriding the default data
/// directory. Must be called before any other path operation. The directory
/// is created if it doesn't exist and canonicalized to an absolute path.
///
/// # Panics
///
/// Panics if called after [`data_dir`] or [`config_dir`] was initialized, or
/// if the directory cannot be created/canonicalized.
pub fn set_custom_data_dir(dir: &str) -> &'static PathBuf {
    if CURRENT_DATA_DIR.get().is_some() || CONFIG_DIR.get().is_some() {
        panic!("set_custom_data_dir called after data_dir or config_dir was initialized");
    }

    CUSTOM_DATA_DIR.get_or_init(|| {
        let path = PathBuf::from(dir);
        std::fs::create_dir_all(&path).expect("failed to create custom data directory");
        path.canonicalize()
            .expect("failed to canonicalize custom data directory")
    })
}

/// Returns the path to the configuration directory.
pub fn config_dir() -> &'static PathBuf {
    CONFIG_DIR.get_or_init(|| {
        if let Some(custom_dir) = CUSTOM_DATA_DIR.get() {
            custom_dir.join("config")
        } else if cfg!(target_os = "windows") {
            dirs::config_dir()
                .expect("failed to determine RoamingAppData directory")
                .join(APP_NAME)
        } else if cfg!(any(target_os = "linux", target_os = "freebsd")) {
            if let Ok(flatpak_xdg_config) = std::env::var("FLATPAK_XDG_CONFIG_HOME") {
                flatpak_xdg_config.into()
            } else {
                dirs::config_dir().expect("failed to determine XDG_CONFIG_HOME directory")
            }
            .join(APP_NAME_LOWERCASE)
        } else {
            home_dir().join(".config").join(APP_NAME_LOWERCASE)
        }
    })
}

/// Returns the path to the data directory.
pub fn data_dir() -> &'static PathBuf {
    CURRENT_DATA_DIR.get_or_init(|| {
        if let Some(custom_dir) = CUSTOM_DATA_DIR.get() {
            custom_dir.clone()
        } else if cfg!(target_os = "macos") {
            home_dir()
                .join("Library/Application Support")
                .join(APP_NAME)
        } else if cfg!(any(target_os = "linux", target_os = "freebsd")) {
            if let Ok(flatpak_xdg_data) = std::env::var("FLATPAK_XDG_DATA_HOME") {
                flatpak_xdg_data.into()
            } else {
                dirs::data_local_dir().expect("failed to determine XDG_DATA_HOME directory")
            }
            .join(APP_NAME_LOWERCASE)
        } else if cfg!(target_os = "windows") {
            dirs::data_local_dir()
                .expect("failed to determine LocalAppData directory")
                .join(APP_NAME)
        } else {
            config_dir().clone()
        }
    })
}

/// Returns the path to the cache directory.
pub fn cache_dir() -> &'static PathBuf {
    static CACHE_DIR: OnceLock<PathBuf> = OnceLock::new();
    CACHE_DIR.get_or_init(|| {
        if cfg!(target_os = "macos") {
            dirs::cache_dir()
                .expect("failed to determine caches directory")
                .join(APP_NAME)
        } else if cfg!(target_os = "windows") {
            dirs::cache_dir()
                .expect("failed to determine LocalAppData directory")
                .join(APP_NAME)
        } else if cfg!(any(target_os = "linux", target_os = "freebsd")) {
            if let Ok(flatpak_xdg_cache) = std::env::var("FLATPAK_XDG_CACHE_HOME") {
                flatpak_xdg_cache.into()
            } else {
                dirs::cache_dir().expect("failed to determine XDG_CACHE_HOME directory")
            }
            .join(APP_NAME_LOWERCASE)
        } else {
            home_dir().join(".cache").join(APP_NAME_LOWERCASE)
        }
    })
}

/// Returns the path to the logs directory.
pub fn logs_dir() -> &'static PathBuf {
    static LOGS_DIR: OnceLock<PathBuf> = OnceLock::new();
    LOGS_DIR.get_or_init(|| {
        if cfg!(target_os = "macos") {
            home_dir().join("Library/Logs").join(APP_NAME)
        } else {
            data_dir().join("logs")
        }
    })
}

/// Returns the path to the nostr database directory (LMDB).
pub fn nostr_dir() -> &'static PathBuf {
    static NOSTR_DIR: OnceLock<PathBuf> = OnceLock::new();
    NOSTR_DIR.get_or_init(|| data_dir().join("nostr"))
}

/// Returns the path to the local git clone cache (grasp mirrors).
pub fn repos_dir() -> &'static PathBuf {
    static REPOS_DIR: OnceLock<PathBuf> = OnceLock::new();
    REPOS_DIR.get_or_init(|| data_dir().join("repos"))
}

/// Returns the path to the `settings.json` file.
pub fn settings_file() -> &'static PathBuf {
    static SETTINGS_FILE: OnceLock<PathBuf> = OnceLock::new();
    SETTINGS_FILE.get_or_init(|| config_dir().join("settings.json"))
}

/// Returns the path to the `keymap.json` file.
pub fn keymap_file() -> &'static PathBuf {
    static KEYMAP_FILE: OnceLock<PathBuf> = OnceLock::new();
    KEYMAP_FILE.get_or_init(|| config_dir().join("keymap.json"))
}
