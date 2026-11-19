//! The normalized snapshot a platform adapter produces.

use gpui::{Hsla, WindowBackgroundAppearance};

use crate::theme::ThemeVariant;

/// Accessibility preferences read from the OS, forwarded into resolution.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AccessibilityPreferences {
    pub high_contrast: bool,
    pub reduce_motion: bool,
    pub reduce_transparency: bool,
}

/// Which adapter produced a [`SystemTheme`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(not(any(target_os = "linux", target_os = "windows")), allow(dead_code))]
pub enum SourceKind {
    #[default]
    Fallback,
    Macos,
    Omarchy,
    Kde,
    Gnome,
    Windows,
}

impl SourceKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Fallback => "neutral fallback",
            Self::Macos => "macOS (NSColor)",
            Self::Omarchy => "Omarchy (colors.toml)",
            Self::Kde => "KDE (kdeglobals)",
            Self::Gnome => "GNOME (portal)",
            Self::Windows => "Windows 11 (UISettings)",
        }
    }

    /// Overlay scrollbars hide while idle on macOS, GNOME, and Omarchy.
    ///
    /// Windows and KDE keep theirs drawn.
    pub fn scrollbar_autohide(self) -> bool {
        matches!(self, Self::Macos | Self::Omarchy | Self::Gnome)
    }
}

/// A normalized snapshot of the running system's theme.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SystemTheme {
    pub variant: ThemeVariant,
    pub accent: Option<Hsla>,
    pub accessibility: AccessibilityPreferences,
    /// The window material the platform natively applies, if any.
    pub window_background: Option<WindowBackgroundAppearance>,
    pub source: SourceKind,
}

impl SystemTheme {
    pub fn fallback(variant: ThemeVariant) -> Self {
        Self {
            variant,
            accent: None,
            accessibility: AccessibilityPreferences::default(),
            window_background: None,
            source: SourceKind::Fallback,
        }
    }
}
