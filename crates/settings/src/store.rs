use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use gpui::{App, Context, Entity, Global};

use crate::Settings;

struct GlobalSettingsStore(Entity<SettingsStore>);

impl Global for GlobalSettingsStore {}

/// The application settings, loaded from disk and saved on change; installed as a global.
pub struct SettingsStore {
    path: PathBuf,
    settings: Settings,
    /// Why the on-disk file was not loaded; such a file is never overwritten.
    unreadable: Option<String>,
    /// Why the settings are currently not persisted.
    pub error: Option<String>,
}

impl SettingsStore {
    /// Retrieve the global settings store, created at startup by the app.
    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalSettingsStore>().0.clone()
    }

    /// Retrieve the global settings store if one has been installed.
    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalSettingsStore>()
            .map(|store| store.0.clone())
    }

    pub fn set_global(entity: Entity<Self>, cx: &mut App) {
        cx.set_global(GlobalSettingsStore(entity));
    }

    /// Loads the settings from `path`, with defaults for a missing or unreadable file.
    pub fn new(path: impl AsRef<Path>) -> Self {
        let (settings, error) = Self::load(path.as_ref());
        Self {
            path: path.as_ref().to_path_buf(),
            settings,
            unreadable: error.clone(),
            error,
        }
    }

    /// A snapshot of the current settings.
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Mutate the settings, persist them to disk, and notify observers.
    pub fn edit(&mut self, f: impl FnOnce(&mut Settings), cx: &mut Context<Self>) {
        f(&mut self.settings);
        match self.save() {
            Ok(()) => self.error = None,
            Err(err) => {
                log::error!(
                    "failed to save settings to {}: {err:#}",
                    self.path.display()
                );
                self.error = Some(format!("{err:#}"));
            }
        }
        cx.notify();
    }

    /// Read the settings file, merging any missing fields with the defaults.
    fn load(path: &Path) -> (Settings, Option<String>) {
        match std::fs::read_to_string(path) {
            Ok(contents) => match serde_json::from_str::<Settings>(&contents) {
                Ok(settings) => (settings, None),
                Err(err) => {
                    let error = format!("failed to parse settings file {}: {err}", path.display());
                    log::error!("{error}; using defaults");
                    (Settings::default(), Some(error))
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => (Settings::default(), None),
            Err(err) => {
                let error = format!("failed to read settings file {}: {err}", path.display());
                log::error!("{error}; using defaults");
                (Settings::default(), Some(error))
            }
        }
    }

    /// Write the settings to disk, replacing the file atomically.
    fn save(&self) -> Result<()> {
        if let Some(reason) = &self.unreadable {
            bail!("settings file was not loaded and is left untouched: {reason}");
        }

        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let json = serde_json::to_string_pretty(&self.settings)?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;

        // `rename` cannot replace an existing file on Windows.
        if cfg!(target_os = "windows") && self.path.exists() {
            std::fs::remove_file(&self.path)?;
        }

        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}
