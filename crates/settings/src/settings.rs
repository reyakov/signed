use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The default grasp servers, offered while the user has not published a grasp list.
pub const DEFAULT_GRASP_SERVERS: [&str; 3] = [
    "wss://relay.ngit.dev",
    "wss://gitnostr.com",
    "wss://git.shakespeare.diy",
];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppearanceMode {
    #[default]
    System,
    Light,
    Dark,
}

/// Fields mirror the gpui-component `Theme` surface customized at startup.
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
    pub path: PathBuf,
    /// Repository address as a string, `30617:<pubkey>:<id>`.
    pub addr: String,
    /// Unix seconds of the last use, for freshest-first ordering.
    pub last_used: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CheckoutsSettings {
    /// The latest use of a path and repo pair replaces the older record.
    pub records: Vec<CheckoutRecord>,
}

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
    pub appearance: AppearanceMode,
    pub theme: ThemeSettings,
    pub grasp_servers: GraspServersSettings,
    pub local_repos: LocalReposSettings,
    pub checkouts: CheckoutsSettings,
    pub create_repository: CreateRepositorySettings,
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
