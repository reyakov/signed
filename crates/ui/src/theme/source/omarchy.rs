//! Omarchy adapter: reads the current theme's `colors.toml` from the state
//! directory (`~/.local/state/omarchy/current/theme/`). The parse is pure,
//! so fixtures run on every OS; the IO lives behind the Linux cfg.

use gpui::Hsla;
use toml::Table;

use crate::theme::system::{SourceKind, SystemTheme};
use crate::theme::{AccessibilityPreferences, ThemeVariant};

/// The mode and accent Omarchy deploys with the current theme.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct OmarchyTheme {
    pub(crate) variant: ThemeVariant,
    pub(crate) accent: Option<Hsla>,
}

impl OmarchyTheme {
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let table = text.parse::<Table>().ok()?;

        let background = hex_color(&table, "background")?;

        let variant = table
            .get("mode")
            .and_then(toml::Value::as_str)
            .and_then(mode_variant)
            .unwrap_or_else(|| ThemeVariant::by_lightness(background));

        let accent = hex_color(&table, "accent").or_else(|| hex_color(&table, "color4"));

        Some(Self { variant, accent })
    }

    pub(crate) fn into_system(self, accessibility: AccessibilityPreferences) -> SystemTheme {
        SystemTheme {
            variant: self.variant,
            accent: self.accent,
            accessibility,
            window_background: None,
            source: SourceKind::Omarchy,
        }
    }
}

fn mode_variant(mode: &str) -> Option<ThemeVariant> {
    match mode {
        "dark" => Some(ThemeVariant::Dark),
        "light" => Some(ThemeVariant::Light),
        _ => None,
    }
}

fn hex_color(table: &Table, key: &str) -> Option<Hsla> {
    let value = table.get(key)?.as_str()?;
    let digits = value.trim().strip_prefix('#')?;
    if digits.len() != 6 {
        return None;
    }
    let hex = u32::from_str_radix(digits, 16).ok()?;
    Some(Hsla::from(gpui::rgb(hex)))
}

#[cfg(target_os = "linux")]
pub(crate) mod io {
    use std::path::PathBuf;

    use super::OmarchyTheme;

    fn home_dir() -> Option<PathBuf> {
        std::env::var_os("HOME").map(PathBuf::from)
    }

    /// The directory theme-set swaps on every theme change.
    pub(crate) fn current_dir() -> Option<PathBuf> {
        Some(home_dir()?.join(".local/state/omarchy/current"))
    }

    pub(crate) fn read_current() -> Option<OmarchyTheme> {
        let text = std::fs::read_to_string(
            home_dir()?.join(".local/state/omarchy/current/theme/colors.toml"),
        )
        .ok()?;
        OmarchyTheme::parse(&text)
    }
}
