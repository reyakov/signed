use std::path::{Path, PathBuf};

use anyhow::Result;
use gpui::{App, Context, Entity, Global};

use crate::Settings;

struct GlobalSettingsStore(Entity<SettingsStore>);

impl Global for GlobalSettingsStore {}

/// The application settings,
/// loaded from disk at startup and saved whenever they change.
///
/// Installed as a global by the app so any part of the UI can read and edit them.
pub struct SettingsStore {
    path: PathBuf,
    settings: Settings,
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

    /// Load the settings from `path`,
    /// falls back to defaults when the file is missing or unreadable.
    ///
    /// Missing keys merge with the defaults,
    /// older settings files keep working as new settings are added.
    pub fn new(path: impl AsRef<Path>, _cx: &mut Context<Self>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            settings: Self::load(path.as_ref()),
        }
    }

    /// A snapshot of the current settings.
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Mutate the settings, persist them to disk, and notify observers.
    pub fn edit(&mut self, f: impl FnOnce(&mut Settings), cx: &mut Context<Self>) {
        f(&mut self.settings);
        if let Err(err) = self.save() {
            log::error!(
                "failed to save settings to {}: {err:#}",
                self.path.display()
            );
        }
        cx.notify();
    }

    /// Read the settings file, merging any missing fields with the defaults.
    fn load(path: &Path) -> Settings {
        match std::fs::read_to_string(path) {
            Ok(contents) => match serde_json::from_str::<Settings>(&contents) {
                Ok(settings) => settings,
                Err(err) => {
                    log::error!(
                        "failed to parse settings file {}: {err}; using defaults",
                        path.display()
                    );
                    Settings::default()
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Settings::default(),
            Err(err) => {
                log::error!(
                    "failed to read settings file {}: {err}; using defaults",
                    path.display()
                );
                Settings::default()
            }
        }
    }

    /// Write the settings to disk, replacing the file atomically.
    fn save(&self) -> Result<()> {
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    static TEST_FILE_COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// A unique, temporary settings path for one test.
    fn temp_settings_path() -> PathBuf {
        let n = TEST_FILE_COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!(
            "signed-settings-test-{}-{n}.json",
            std::process::id()
        ))
    }

    fn cleanup(path: &Path) {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(path.with_extension("json.tmp"));
    }

    #[test]
    fn missing_file_loads_defaults() {
        let path = temp_settings_path();
        cleanup(&path);

        let settings = SettingsStore::load(&path);
        assert_eq!(settings, Settings::default());
        cleanup(&path);
    }

    #[test]
    fn save_and_load_roundtrip() {
        let path = temp_settings_path();
        cleanup(&path);

        let mut expected = Settings::default();
        expected.create_repository.default_folder = Some(PathBuf::from("/tmp/repos"));

        let store = SettingsStore {
            path: path.clone(),
            settings: expected.clone(),
        };
        store.save().unwrap();

        assert_eq!(SettingsStore::load(&path), expected);
        cleanup(&path);
    }
}
