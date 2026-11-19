//! The adapter for unknown systems:
//! no colors of its own, resolution degrades to the neutral presets.

use super::ThemeSource;
use crate::theme::ThemeVariant;
use crate::theme::system::SystemTheme;

// Constructed only by `platform_source` on platforms without their own adapter.
#[cfg_attr(any(target_os = "macos", target_os = "linux"), allow(dead_code))]
pub(crate) struct FallbackSource;

impl ThemeSource for FallbackSource {
    fn snapshot(&self, variant: ThemeVariant) -> SystemTheme {
        SystemTheme::fallback(variant)
    }

    fn label(&self) -> &'static str {
        crate::theme::system::SourceKind::Fallback.label()
    }
}
