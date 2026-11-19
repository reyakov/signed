//! Platform adapters behind one seam: `snapshot()` reads the running
//! system, `install_watchers()` forwards real change events.

use gpui::AsyncApp;

use crate::theme::ThemeVariant;
use crate::theme::system::SystemTheme;

pub(crate) trait ThemeSource: 'static {
    fn snapshot(&self, variant: ThemeVariant) -> SystemTheme;
    fn install_watchers(&mut self, _async_cx: AsyncApp) {}
    fn label(&self) -> &'static str;
}

/// Narrows an f64 color component to f32, clamping to the 0..=1 range and
/// mapping non-finite values to 0.0 rather than truncating garbage silently.
pub(crate) fn narrow_component(value: f64) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0) as f32
    } else {
        0.0
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn platform_source() -> Box<dyn ThemeSource> {
    Box::new(macos::MacosSource::default())
}

#[cfg(target_os = "linux")]
pub(crate) fn platform_source() -> Box<dyn ThemeSource> {
    linux::platform_source()
}

#[cfg(target_os = "windows")]
pub(crate) fn platform_source() -> Box<dyn ThemeSource> {
    windows::platform_source()
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
pub(crate) fn platform_source() -> Box<dyn ThemeSource> {
    Box::new(fallback::FallbackSource)
}

#[cfg(target_os = "macos")]
pub(crate) mod macos;

#[cfg(target_os = "linux")]
pub(crate) mod linux;

#[cfg(target_os = "windows")]
pub(crate) mod windows;

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
pub(crate) mod signal;

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) mod kde;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) mod omarchy;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) mod portal;

pub(crate) mod fallback;
