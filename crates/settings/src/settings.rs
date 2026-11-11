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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventFetchingStrategy {
    /// Only the relays declared in the repository announcement.
    Curated,
    /// Repository relays plus every maintainer's NIP-65 relays.
    #[default]
    Uncensored,
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
    /// Defaults:
    ///
    /// - `~/Desktop`
    /// - `~/Documents`
    pub scan_paths: Vec<PathBuf>,
}

impl Default for LocalReposSettings {
    fn default() -> Self {
        Self {
            scan_paths: vec![paths::desktop_dir(), paths::documents_dir()],
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
    pub event_fetching: EventFetchingStrategy,
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
            event_fetching: EventFetchingStrategy::Curated,
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
        let settings: Settings = serde_json::from_str(
            r#"{"appearance": "dark", "grasp_servers": {"default_servers": []}}"#,
        )
        .unwrap();
        assert_eq!(settings.appearance, AppearanceMode::Dark);
        assert_eq!(settings.grasp_servers.default_servers, Vec::<String>::new());
        // Unset fields fall back to their defaults, including the fetching strategy.
        assert_eq!(settings.event_fetching, EventFetchingStrategy::Uncensored);
        assert_eq!(settings.local_repos, LocalReposSettings::default());
        assert_eq!(settings.create_repository.default_folder, None);
    }

    #[test]
    fn legacy_theme_keys_are_ignored() {
        let settings: Settings = serde_json::from_str(
            r#"{"appearance": "light", "theme": {"radius": 8.0}, "tab_bar": {"hide_navigation_buttons": false}}"#,
        )
        .unwrap();
        assert_eq!(settings.appearance, AppearanceMode::Light);
        assert_eq!(
            settings,
            Settings {
                appearance: AppearanceMode::Light,
                ..Default::default()
            }
        );
    }

    #[test]
    fn event_fetching_uses_snake_case() {
        let json = serde_json::to_string(&EventFetchingStrategy::Curated).unwrap();
        assert_eq!(json, r#""curated""#);
        let parsed: EventFetchingStrategy = serde_json::from_str(r#""uncensored""#).unwrap();
        assert_eq!(parsed, EventFetchingStrategy::Uncensored);
    }
}
