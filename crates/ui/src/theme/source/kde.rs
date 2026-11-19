//! KDE adapter: reads the color groups Plasma writes into `~/.config/kdeglobals`
//! (plus the Plasma 6.2+ `[General] AccentColor`). The parse is pure, so
//! fixtures run on every OS; the IO lives behind the Linux cfg.

use std::collections::HashMap;

use gpui::Hsla;

use crate::theme::system::{SourceKind, SystemTheme};

/// The accent and window background stated in kdeglobals.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct KdeScheme {
    pub(crate) accent: Option<Hsla>,
    pub(crate) background: Option<Hsla>,
}

impl KdeScheme {
    /// Parses kdeglobals text.
    pub(crate) fn parse(text: &str) -> Self {
        let groups = parse_groups(text);

        let background = groups
            .get("Colors:Window")
            .and_then(|group| group_color(group, "BackgroundNormal"));

        let accent = groups
            .get("General")
            .and_then(|group| group.get("AccentColor"))
            .and_then(|value| color_list(value));

        Self { accent, background }
    }

    pub(crate) fn into_system(
        self,
        accessibility: crate::theme::system::AccessibilityPreferences,
        portal_accent: Option<Hsla>,
        portal_variant: Option<crate::theme::ThemeVariant>,
    ) -> SystemTheme {
        SystemTheme {
            // The portal states the mode on Plasma 5.21+.
            //
            // Older setups fall back to the scheme's background lightness.
            variant: portal_variant
                .or(self
                    .background
                    .map(crate::theme::ThemeVariant::by_lightness))
                .unwrap_or(crate::theme::ThemeVariant::Light),
            accent: self.accent.or(portal_accent),
            accessibility,
            window_background: None,
            source: SourceKind::Kde,
        }
    }
}

fn parse_groups(text: &str) -> HashMap<&str, HashMap<&str, &str>> {
    let mut groups: HashMap<&str, HashMap<&str, &str>> = HashMap::new();
    let mut section: Option<&str> = None;

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            section = Some(rest.split(']').next().unwrap_or(rest));
            continue;
        }
        if let (Some(section), Some((key, value))) = (section, line.split_once('=')) {
            groups
                .entry(section)
                .or_default()
                .insert(key.trim(), value.trim());
        }
    }

    groups
}

fn group_color<'a>(group: &'a HashMap<&'a str, &'a str>, key: &str) -> Option<Hsla> {
    color_list(group.get(key)?)
}

/// Parses `r,g,b[,a]` with components in 0..=255.
fn color_list(value: &str) -> Option<Hsla> {
    let mut components = value.split(',').map(str::trim);
    let red = components.next()?.parse::<f32>().ok()?;
    let green = components.next()?.parse::<f32>().ok()?;
    let blue = components.next()?.parse::<f32>().ok()?;
    let alpha = components.next().and_then(|a| a.parse::<f32>().ok());
    if !(0.0..=255.0).contains(&red)
        || !(0.0..=255.0).contains(&green)
        || !(0.0..=255.0).contains(&blue)
    {
        return None;
    }
    let scale = |component: f32| component / 255.0;

    Some(Hsla::from(gpui::Rgba {
        r: scale(red),
        g: scale(green),
        b: scale(blue),
        a: alpha.map(|a| a / 255.0).unwrap_or(1.0),
    }))
}

#[cfg(target_os = "linux")]
pub(crate) mod io {
    use std::path::PathBuf;

    use super::KdeScheme;

    fn kdeglobals_path() -> Option<PathBuf> {
        Some(
            std::env::var_os("HOME")
                .map(PathBuf::from)?
                .join(".config/kdeglobals"),
        )
    }

    pub(crate) fn read_current() -> Option<KdeScheme> {
        let text = std::fs::read_to_string(kdeglobals_path()?).ok()?;
        Some(KdeScheme::parse(&text))
    }
}
