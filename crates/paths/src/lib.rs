use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// The application name, which derives the platform-specific directory paths.
pub const APP_NAME: &str = "Signed";

/// Lowercased [`APP_NAME`], used in XDG paths and the macOS `~/.config` fallback.
pub const APP_NAME_LOWERCASE: &str = "signed";

/// The resolved config directory, under the platform's standard config location.
static CONFIG_DIR: LazyLock<PathBuf> = LazyLock::new(|| {
    if cfg!(target_os = "windows") {
        resolve_dir(dirs::config_dir(), "RoamingAppData").join(APP_NAME)
    } else if cfg!(any(target_os = "linux", target_os = "freebsd")) {
        if let Ok(flatpak_xdg_config) = std::env::var("FLATPAK_XDG_CONFIG_HOME") {
            flatpak_xdg_config.into()
        } else {
            resolve_dir(dirs::config_dir(), "XDG_CONFIG_HOME")
        }
        .join(APP_NAME_LOWERCASE)
    } else {
        home_dir().join(".config").join(APP_NAME_LOWERCASE)
    }
});

/// The resolved data directory, under the platform's standard data location.
static CURRENT_DATA_DIR: LazyLock<PathBuf> = LazyLock::new(|| {
    if cfg!(target_os = "macos") {
        home_dir()
            .join("Library/Application Support")
            .join(APP_NAME)
    } else if cfg!(any(target_os = "linux", target_os = "freebsd")) {
        if let Ok(flatpak_xdg_data) = std::env::var("FLATPAK_XDG_DATA_HOME") {
            flatpak_xdg_data.into()
        } else {
            resolve_dir(dirs::data_local_dir(), "XDG_DATA_HOME")
        }
        .join(APP_NAME_LOWERCASE)
    } else if cfg!(target_os = "windows") {
        resolve_dir(dirs::data_local_dir(), "LocalAppData").join(APP_NAME)
    } else {
        config_dir().to_path_buf()
    }
});

static NOSTR_DIR: LazyLock<PathBuf> = LazyLock::new(|| data_dir().join("nostr"));

static REPOS_DIR: LazyLock<PathBuf> =
    LazyLock::new(|| std::env::temp_dir().join(APP_NAME_LOWERCASE).join("repos"));

static SETTINGS_FILE: LazyLock<PathBuf> = LazyLock::new(|| config_dir().join("settings.json"));

/// Returns `dir`, or the temp directory when the platform could not resolve it.
fn resolve_dir(dir: Option<PathBuf>, name: &str) -> PathBuf {
    dir.unwrap_or_else(|| {
        log::warn!("failed to determine the {name} directory; falling back to the temp directory");
        std::env::temp_dir()
    })
}

fn home_dir() -> PathBuf {
    resolve_dir(dirs::home_dir(), "home")
}

/// Returns the user's Desktop folder, falling back to home and then to an empty path.
pub fn desktop_dir() -> PathBuf {
    dirs::desktop_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_default())
}

/// Returns the user's Documents folder, falling back to home and then to an empty path.
pub fn documents_dir() -> PathBuf {
    dirs::document_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_default())
}

fn config_dir() -> &'static Path {
    CONFIG_DIR.as_path()
}

fn data_dir() -> &'static Path {
    CURRENT_DATA_DIR.as_path()
}

/// Returns the path to the nostr database directory, LMDB.
pub fn nostr_dir() -> &'static Path {
    NOSTR_DIR.as_path()
}

/// The grasp mirror cache, disposable and re-cloned on demand, so it lives in the temp directory.
pub fn repos_dir() -> &'static Path {
    REPOS_DIR.as_path()
}

pub fn settings_file() -> &'static Path {
    SETTINGS_FILE.as_path()
}
