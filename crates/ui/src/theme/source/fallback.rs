//! The adapter for unknown systems:
//! no colors of its own, resolution degrades to the neutral presets.

use super::ThemeSource;
use crate::theme::ThemeVariant;
use crate::theme::system::{SourceKind, SystemTheme};

/// The source for platforms without their own adapter, and for tests.
pub(crate) struct FallbackSource;

impl ThemeSource for FallbackSource {
    fn snapshot(&self, variant: ThemeVariant) -> SystemTheme {
        SystemTheme::fallback(variant)
    }

    fn label(&self) -> &'static str {
        SourceKind::Fallback.label()
    }
}
