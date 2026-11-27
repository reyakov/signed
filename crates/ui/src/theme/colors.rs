//! Semantic color roles. This module is the only place in the workspace
//! allowed to hold color literals.

use gpui::{Hsla, hsla, rgb, white};

use crate::theme::{
    DEFAULT_TRANSPARENCY, MIN_MATERIAL_FILL_DARK, MIN_MATERIAL_FILL_LIGHT, ThemeVariant,
};

/// Windows paints its caption close-button hover red in every theme.
pub const WINDOWS_CLOSE_HOVER: Hsla = hsla(0.986043, 0.863406, 0.488235, 1.0);
/// The close-button hover dimmed while pressed.
pub const WINDOWS_CLOSE_PRESSED: Hsla = hsla(0.986043, 0.863406, 0.488235, 0.8);
/// The close glyph's color over the red hover.
pub const WINDOWS_CLOSE_FOREGROUND: Hsla = white();

/// The semantic color roles components paint from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorTokens {
    pub background: Hsla,
    pub foreground: Hsla,
    pub surface: Hsla,
    pub surface_foreground: Hsla,
    pub primary: Hsla,
    pub primary_foreground: Hsla,
    pub secondary: Hsla,
    pub secondary_foreground: Hsla,
    pub muted: Hsla,
    pub muted_hover: Hsla,
    pub muted_foreground: Hsla,
    pub accent: Hsla,
    pub accent_foreground: Hsla,
    pub destructive: Hsla,
    pub destructive_foreground: Hsla,
    pub success: Hsla,
    pub success_foreground: Hsla,
    pub warning: Hsla,
    pub warning_foreground: Hsla,
    pub info: Hsla,
    pub info_foreground: Hsla,
    pub border: Hsla,
    pub input: Hsla,
    pub ring: Hsla,
    pub selection: Hsla,
    /// The scrim behind modal layers, translucent in every theme.
    pub overlay: Hsla,
    /// The overlay scrollbar thumb, translucent over scrolling content.
    pub scrollbar_thumb: Hsla,
}

impl ColorTokens {
    /// Neutral light palette; the fallback when no system theme applies.
    pub fn light() -> Self {
        Self {
            background: Self::hex(0xffffff),
            foreground: Self::hex(0x18181b),
            surface: Self::hex(0xfafafa),
            surface_foreground: Self::hex(0x18181b),
            primary: Self::hex(0x2563eb),
            primary_foreground: Self::hex(0xffffff),
            secondary: Self::hex(0xf4f4f5),
            secondary_foreground: Self::hex(0x18181b),
            muted: Self::hex(0xf4f4f5),
            muted_hover: Self::hex(0xe4e4e7),
            muted_foreground: Self::hex(0x71717a),
            accent: Self::hex(0x2563eb),
            accent_foreground: Self::hex(0xffffff),
            destructive: Self::hex(0xdc2626),
            destructive_foreground: Self::hex(0xffffff),
            success: Self::hex(0x16a34a),
            success_foreground: Self::hex(0xffffff),
            warning: Self::hex(0xd97706),
            warning_foreground: Self::hex(0xffffff),
            info: Self::hex(0x0284c7),
            info_foreground: Self::hex(0xffffff),
            border: Self::hex(0xe4e4e7),
            input: Self::hex(0xffffff),
            ring: Self::hex(0x2563eb),
            selection: Self::hex(0xdbeafe),
            overlay: Self::scrim(0.25),
            // macOS light overlay thumbs are translucent black.
            scrollbar_thumb: hsla(0., 0., 0., 0.25),
        }
    }

    /// Neutral dark palette; the fallback when no system theme applies.
    pub fn dark() -> Self {
        Self {
            background: Self::hex(0x09090b),
            foreground: Self::hex(0xfafafa),
            surface: Self::hex(0x18181b),
            surface_foreground: Self::hex(0xfafafa),
            primary: Self::hex(0x3b82f6),
            primary_foreground: Self::hex(0xffffff),
            secondary: Self::hex(0x27272a),
            secondary_foreground: Self::hex(0xfafafa),
            muted: Self::hex(0x27272a),
            muted_hover: Self::hex(0x3f3f46),
            muted_foreground: Self::hex(0xa1a1aa),
            accent: Self::hex(0x3b82f6),
            accent_foreground: Self::hex(0xffffff),
            destructive: Self::hex(0xef4444),
            destructive_foreground: Self::hex(0xffffff),
            success: Self::hex(0x22c55e),
            success_foreground: Self::hex(0x052e16),
            warning: Self::hex(0xf59e0b),
            warning_foreground: Self::hex(0x451a03),
            info: Self::hex(0x38bdf8),
            info_foreground: Self::hex(0x082f49),
            border: Self::hex(0x27272a),
            input: Self::hex(0x09090b),
            ring: Self::hex(0x3b82f6),
            selection: Self::hex(0x1e3a8a),
            overlay: Self::scrim(0.45),
            // macOS dark overlay thumbs are translucent light gray.
            scrollbar_thumb: hsla(0., 0., 1., 0.3),
        }
    }

    /// An opaque `0xrrggbb` color.
    fn hex(hex: u32) -> Hsla {
        rgb(hex).into()
    }

    /// The dim scrim behind modal layers; always a translucent black.
    pub(crate) fn scrim(alpha: f32) -> Hsla {
        hsla(0., 0., 0., alpha)
    }
}

/// The translucent paints surfaces lay over a native window material.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MaterialTokens {
    /// The window-sized fill the material shows through.
    pub background: Hsla,
    /// The hover wash over material surfaces.
    pub hover: Hsla,
    /// The selected-row fill over material surfaces.
    pub selection: Hsla,
}

impl MaterialTokens {
    /// Derives the material paints from the resolved palette.
    pub(crate) fn derive(
        colors: &ColorTokens,
        selection: Hsla,
        variant: ThemeVariant,
        translucent: bool,
        transparency: Option<u8>,
    ) -> Self {
        if !translucent {
            return Self {
                background: colors.background,
                hover: colors.muted,
                selection,
            };
        }

        let level = transparency.unwrap_or(DEFAULT_TRANSPARENCY) as f32 / 100.0;

        let floor = match variant {
            ThemeVariant::Light => MIN_MATERIAL_FILL_LIGHT,
            ThemeVariant::Dark => MIN_MATERIAL_FILL_DARK,
        };

        Self {
            // The fill eases toward a floor instead of zero.
            background: colors.background.alpha(1.0 - level * (1.0 - floor)),
            // A foreground wash keeps hover visible over any backdrop hue.
            hover: colors.foreground.alpha(0.06),
            selection: selection.alpha(0.85),
        }
    }
}
