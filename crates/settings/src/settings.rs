use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The default grasp servers,
/// offered while the user has not published a grasp list.
pub const DEFAULT_GRASP_SERVERS: [&str; 3] = [
    "wss://relay.ngit.dev",
    "wss://gitnostr.com",
    "wss://git.shakespeare.diy",
];

/// How the application picks its appearance.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppearanceMode {
    /// Follow the system appearance, light or dark, at runtime.
    #[default]
    System,
    /// Always use the light theme.
    Light,
    /// Always use the dark theme.
    Dark,
}

/// Theme configuration,
/// fields mirror the gpui-component `Theme` surface customized at startup.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeSettings {
    /// Name of the light theme in the theme registry.
    pub light_theme: String,
    /// Name of the dark theme in the theme registry.
    pub dark_theme: String,
    /// The base font size in pixels.
    pub font_size: f32,
    /// The monospace font size in pixels.
    pub mono_font_size: f32,
    /// Corner radius for general elements in pixels.
    pub radius: f32,
    /// Corner radius for large elements, dialogs and notifications, in pixels.
    pub radius_lg: f32,
    /// Whether focused controls draw a ring outside their border.
    pub focus_ring: bool,
    /// Whether to render shadows.
    pub shadow: bool,
}

impl Default for ThemeSettings {
    fn default() -> Self {
        Self {
            light_theme: "Signed Light".into(),
            dark_theme: "Signed Dark".into(),
            font_size: 16.0,
            mono_font_size: 13.0,
            radius: 2.0,
            radius_lg: 6.0,
            focus_ring: false,
            shadow: false,
        }
    }
}

/// Default grasp server settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GraspServersSettings {
    /// Servers offered while the user has not published a grasp list, kind `10317`.
    pub default_servers: Vec<String>,
}

impl Default for GraspServersSettings {
    fn default() -> Self {
        Self {
            default_servers: DEFAULT_GRASP_SERVERS
                .iter()
                .map(|server| (*server).to_owned())
                .collect(),
        }
    }
}

/// Local repository scanning.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalReposSettings {
    /// The directories scanned for local git repositories,
    /// defaults to the user's Desktop and Documents folders.
    pub scan_paths: Vec<PathBuf>,
}

fn default_scan_paths() -> Vec<PathBuf> {
    vec![paths::desktop_dir(), paths::documents_dir()]
}

impl Default for LocalReposSettings {
    fn default() -> Self {
        Self {
            scan_paths: default_scan_paths(),
        }
    }
}

/// A remembered association between a local checkout folder and an announced repository,
/// recorded when the user clones a repository or picks a folder in the New PR panel.
///
/// The panel can then prefill the folder later without asking again.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CheckoutRecord {
    /// Local folder of the checkout.
    pub path: PathBuf,
    /// Repository address as a string, `30617:<pubkey>:<id>`.
    pub addr: String,
    /// Unix seconds of the last use, for freshest-first ordering.
    pub last_used: u64,
}

/// Remembered local checkouts, see [`CheckoutRecord`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CheckoutsSettings {
    /// The remembered records.
    /// The latest use of a path and repo pair replaces the older record.
    pub records: Vec<CheckoutRecord>,
}

/// The create-repository dialog.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CreateRepositorySettings {
    /// The folder the create-repository dialog defaults to, the user's Desktop when unset.
    pub default_folder: Option<PathBuf>,
}

/// The complete set of persisted application settings.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// How the application picks its appearance.
    pub appearance: AppearanceMode,
    /// Theme configuration.
    pub theme: ThemeSettings,
    /// Default grasp servers.
    pub grasp_servers: GraspServersSettings,
    /// Local repository scanning.
    pub local_repos: LocalReposSettings,
    /// Remembered local checkouts.
    pub checkouts: CheckoutsSettings,
    /// The create-repository dialog.
    pub create_repository: CreateRepositorySettings,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_app_conventions() {
        let settings = Settings::default();
        assert_eq!(settings.appearance, AppearanceMode::System);
        assert_eq!(settings.theme.light_theme, "Signed Light");
        assert_eq!(settings.theme.dark_theme, "Signed Dark");
        assert_eq!(settings.theme.font_size, 16.0);
        assert_eq!(settings.theme.mono_font_size, 13.0);
        assert_eq!(settings.theme.radius, 2.0);
        assert_eq!(settings.theme.radius_lg, 6.0);
        assert!(!settings.theme.focus_ring);
        assert!(!settings.theme.shadow);
        assert_eq!(
            settings.grasp_servers.default_servers,
            DEFAULT_GRASP_SERVERS.map(String::from).to_vec()
        );
        assert_eq!(settings.local_repos.scan_paths.len(), 2);
        assert_eq!(
            settings.local_repos.scan_paths,
            vec![paths::desktop_dir(), paths::documents_dir()]
        );
        assert_eq!(settings.create_repository.default_folder, None);
    }

    #[test]
    fn json_roundtrip_preserves_everything() {
        let settings = Settings {
            appearance: AppearanceMode::Dark,
            theme: ThemeSettings {
                radius: 8.0,
                ..Default::default()
            },
            create_repository: CreateRepositorySettings {
                default_folder: Some(PathBuf::from("/tmp/repos")),
            },
            ..Default::default()
        };

        let json = serde_json::to_string(&settings).unwrap();
        let parsed: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, settings);
    }

    #[test]
    fn missing_keys_fall_back_to_defaults() {
        let settings: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn partial_json_merges_with_defaults() {
        let settings: Settings =
            serde_json::from_str(r#"{"appearance": "dark", "theme": {"radius": 4.0}}"#).unwrap();
        assert_eq!(settings.appearance, AppearanceMode::Dark);
        assert_eq!(settings.theme.radius, 4.0);
        // The rest of the theme and the other groups keep their defaults.
        assert_eq!(settings.theme.light_theme, "Signed Light");
        assert_eq!(settings.grasp_servers, GraspServersSettings::default());
        assert_eq!(settings.create_repository.default_folder, None);
    }

    #[test]
    fn appearance_serializes_to_snake_case_names() {
        assert_eq!(
            serde_json::to_string(&AppearanceMode::System).unwrap(),
            "\"system\""
        );
        assert_eq!(
            serde_json::to_string(&AppearanceMode::Light).unwrap(),
            "\"light\""
        );
        assert_eq!(
            serde_json::to_string(&AppearanceMode::Dark).unwrap(),
            "\"dark\""
        );
    }
}
