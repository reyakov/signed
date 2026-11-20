//! Sparse theme definitions: the shared schema for bundled presets and
//! user theme files. Any field may be omitted; omitted fields fall through
//! to the system theme, then to the neutral presets.

use gpui::{Hsla, Rgba};
use serde::Deserialize;

use crate::theme::ThemeVariant;

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct ThemeDefinition {
    mode: Option<ThemeVariant>,
    control_radius: Option<f32>,
    overlay_radius: Option<f32>,
    sm_radius: Option<f32>,
    accent: Option<String>,
    accent_foreground: Option<String>,
    background: Option<String>,
    foreground: Option<String>,
    surface: Option<String>,
    surface_foreground: Option<String>,
    primary: Option<String>,
    primary_foreground: Option<String>,
    secondary: Option<String>,
    secondary_foreground: Option<String>,
    muted: Option<String>,
    muted_hover: Option<String>,
    muted_foreground: Option<String>,
    destructive: Option<String>,
    destructive_foreground: Option<String>,
    success: Option<String>,
    success_foreground: Option<String>,
    warning: Option<String>,
    warning_foreground: Option<String>,
    info: Option<String>,
    info_foreground: Option<String>,
    border: Option<String>,
    input: Option<String>,
    ring: Option<String>,
    selection: Option<String>,
    overlay: Option<String>,
    scrollbar_thumb: Option<String>,
}

impl ThemeDefinition {
    pub fn parse_toml(source: &str) -> anyhow::Result<Self> {
        Ok(toml::from_str(source)?)
    }

    pub fn mode(&self) -> Option<ThemeVariant> {
        self.mode
    }

    pub fn control_radius(&self) -> Option<f32> {
        self.control_radius
    }

    pub fn overlay_radius(&self) -> Option<f32> {
        self.overlay_radius
    }

    pub fn sm_radius(&self) -> Option<f32> {
        self.sm_radius
    }

    pub fn accent(&self) -> Option<Hsla> {
        self.color(&self.accent, "accent")
    }

    pub fn accent_foreground(&self) -> Option<Hsla> {
        self.color(&self.accent_foreground, "accent_foreground")
    }

    pub fn background(&self) -> Option<Hsla> {
        self.color(&self.background, "background")
    }

    pub fn foreground(&self) -> Option<Hsla> {
        self.color(&self.foreground, "foreground")
    }

    pub fn surface(&self) -> Option<Hsla> {
        self.color(&self.surface, "surface")
    }

    pub fn surface_foreground(&self) -> Option<Hsla> {
        self.color(&self.surface_foreground, "surface_foreground")
    }

    pub fn primary(&self) -> Option<Hsla> {
        self.color(&self.primary, "primary")
    }

    pub fn primary_foreground(&self) -> Option<Hsla> {
        self.color(&self.primary_foreground, "primary_foreground")
    }

    pub fn secondary(&self) -> Option<Hsla> {
        self.color(&self.secondary, "secondary")
    }

    pub fn secondary_foreground(&self) -> Option<Hsla> {
        self.color(&self.secondary_foreground, "secondary_foreground")
    }

    pub fn muted(&self) -> Option<Hsla> {
        self.color(&self.muted, "muted")
    }

    pub fn muted_hover(&self) -> Option<Hsla> {
        self.color(&self.muted_hover, "muted_hover")
    }

    pub fn muted_foreground(&self) -> Option<Hsla> {
        self.color(&self.muted_foreground, "muted_foreground")
    }

    pub fn destructive(&self) -> Option<Hsla> {
        self.color(&self.destructive, "destructive")
    }

    pub fn destructive_foreground(&self) -> Option<Hsla> {
        self.color(&self.destructive_foreground, "destructive_foreground")
    }

    pub fn success(&self) -> Option<Hsla> {
        self.color(&self.success, "success")
    }

    pub fn success_foreground(&self) -> Option<Hsla> {
        self.color(&self.success_foreground, "success_foreground")
    }

    pub fn warning(&self) -> Option<Hsla> {
        self.color(&self.warning, "warning")
    }

    pub fn warning_foreground(&self) -> Option<Hsla> {
        self.color(&self.warning_foreground, "warning_foreground")
    }

    pub fn info(&self) -> Option<Hsla> {
        self.color(&self.info, "info")
    }

    pub fn info_foreground(&self) -> Option<Hsla> {
        self.color(&self.info_foreground, "info_foreground")
    }

    pub fn border(&self) -> Option<Hsla> {
        self.color(&self.border, "border")
    }

    pub fn input(&self) -> Option<Hsla> {
        self.color(&self.input, "input")
    }

    pub fn ring(&self) -> Option<Hsla> {
        self.color(&self.ring, "ring")
    }

    pub fn selection(&self) -> Option<Hsla> {
        self.color(&self.selection, "selection")
    }

    pub fn overlay(&self) -> Option<Hsla> {
        self.color(&self.overlay, "overlay")
    }

    pub fn scrollbar_thumb(&self) -> Option<Hsla> {
        self.color(&self.scrollbar_thumb, "scrollbar_thumb")
    }

    /// Parses a color field, warning when a stated value is malformed.
    fn color(&self, field: &Option<String>, role: &str) -> Option<Hsla> {
        let value = field.as_deref()?;
        match parse_hex(value) {
            Some(color) => Some(color),
            None => {
                log::warn!("theme definition {role} is not #rgb/#rrggbb: {value:?}");
                None
            }
        }
    }
}

/// Parses `#rgb` or `#rrggbb` into an opaque color.
pub(crate) fn parse_hex(value: &str) -> Option<Hsla> {
    let digits = value.strip_prefix('#')?;
    // Lengths are byte lengths; non-ASCII input would panic the slicing below.
    if !digits.is_ascii() {
        return None;
    }
    let expand = |channels: [f32; 3]| Some(expand_rgb([channels[0], channels[1], channels[2]]));

    match digits.len() {
        3 => {
            let mut channels = [0u8; 3];
            for (i, byte) in channels.iter_mut().enumerate() {
                *byte = u8::from_str_radix(&digits[i..i + 1], 16).ok()?;
            }
            expand([
                channels[0] as f32 / 15.0,
                channels[1] as f32 / 15.0,
                channels[2] as f32 / 15.0,
            ])
        }
        6 => {
            let mut channels = [0u8; 3];
            for (i, byte) in channels.iter_mut().enumerate() {
                *byte = u8::from_str_radix(&digits[i * 2..i * 2 + 2], 16).ok()?;
            }
            expand([
                channels[0] as f32 / 255.0,
                channels[1] as f32 / 255.0,
                channels[2] as f32 / 255.0,
            ])
        }
        _ => None,
    }
}

fn expand_rgb([r, g, b]: [f32; 3]) -> Hsla {
    Hsla::from(Rgba { r, g, b, a: 1.0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hex_rejects_non_ascii_digits() {
        assert_eq!(parse_hex("#éa"), None);
        assert_eq!(parse_hex("#éaabbcc"), None);
    }

    #[test]
    fn parse_hex_resolves_three_and_six_digits() {
        assert!(parse_hex("#abc").is_some());
        assert!(parse_hex("#aabbcc").is_some());
        assert_eq!(parse_hex("abc"), None);
        assert_eq!(parse_hex("#ab"), None);
    }
}
