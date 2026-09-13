use std::path::PathBuf;
use std::sync::OnceLock;

/// The application name.
///
/// It derives the platform-specific data, config and cache directory paths.
pub const APP_NAME: &str = "Signed";

/// Lowercased form of [`APP_NAME`].
///
/// Used in XDG-style paths on Linux and FreeBSD, and the macOS `~/.config` fallback.
pub const APP_NAME_LOWERCASE: &str = "signed";

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

pub fn home_dir() -> PathBuf {
    dirs::home_dir().expect("failed to determine home directory")
}

/// Returns the current user's Desktop folder.
///
/// Falls back to the home directory or an empty path when it cannot be determined.
pub fn desktop_dir() -> PathBuf {
    dirs::desktop_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_default())
}

/// Returns the current user's Documents folder.
///
/// Falls back to the home directory or an empty path when it cannot be determined.
pub fn documents_dir() -> PathBuf {
    dirs::document_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_default())
}

pub fn config_dir() -> &'static PathBuf {
    CONFIG_DIR.get_or_init(|| {
        if cfg!(target_os = "windows") {
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

pub fn data_dir() -> &'static PathBuf {
    CURRENT_DATA_DIR.get_or_init(|| {
        if cfg!(target_os = "macos") {
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

/// Returns the path to the nostr database directory, LMDB.
pub fn nostr_dir() -> &'static PathBuf {
    static NOSTR_DIR: OnceLock<PathBuf> = OnceLock::new();
    NOSTR_DIR.get_or_init(|| data_dir().join("nostr"))
}

/// Returns the path to the local git clone cache, the grasp mirrors.
///
/// The mirrors are disposable and re-cloned from their grasp server on
/// demand, so the cache lives in the OS temp directory for the system to
/// reclaim.
pub fn repos_dir() -> &'static PathBuf {
    static REPOS_DIR: OnceLock<PathBuf> = OnceLock::new();
    REPOS_DIR.get_or_init(|| std::env::temp_dir().join(APP_NAME_LOWERCASE).join("repos"))
}

pub fn settings_file() -> &'static PathBuf {
    static SETTINGS_FILE: OnceLock<PathBuf> = OnceLock::new();
    SETTINGS_FILE.get_or_init(|| config_dir().join("settings.json"))
}
