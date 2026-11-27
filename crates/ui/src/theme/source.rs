//! Platform adapters behind one seam: `snapshot()` reads the running
//! system, `install_watchers()` forwards real change events.

use gpui::AsyncApp;
#[cfg(target_os = "windows")]
use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
#[cfg(target_os = "windows")]
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};

use crate::theme::ThemeVariant;
use crate::theme::system::SystemTheme;

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

pub trait ThemeSource: 'static {
    fn snapshot(&self, variant: ThemeVariant) -> SystemTheme;
    fn install_watchers(&mut self, _async_cx: AsyncApp) {}
    fn label(&self) -> &'static str;
}

/// The adapter for the running platform.
#[cfg(target_os = "macos")]
pub fn platform_source() -> Box<dyn ThemeSource> {
    Box::new(macos::MacosSource::default())
}

/// The adapter for the running platform.
#[cfg(target_os = "linux")]
pub fn platform_source() -> Box<dyn ThemeSource> {
    linux::platform_source()
}

/// The adapter for the running platform.
#[cfg(target_os = "windows")]
pub fn platform_source() -> Box<dyn ThemeSource> {
    // SAFETY: WinRT activation requires COM on this thread,
    // the STA matches the UI thread's message pump,
    // and RPC_E_CHANGED_MODE means COM is already initialized in another apartment.
    let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };

    if initialized.is_err() && initialized != RPC_E_CHANGED_MODE {
        log::error!("theme: COM initialization failed: {initialized:?}");
    }

    match windows::WindowsSource::new() {
        Some(source) => Box::new(source),
        None => {
            log::warn!("theme: Windows system theme unavailable; using fallback");
            Box::new(fallback::FallbackSource)
        }
    }
}

/// The adapter for the running platform.
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
pub fn platform_source() -> Box<dyn ThemeSource> {
    Box::new(fallback::FallbackSource)
}

/// Narrows an f64 color component to f32, clamping to the 0..=1 range and
/// mapping non-finite values to 0.0 rather than truncating garbage silently.
pub(super) fn narrow_component(value: f64) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0) as f32
    } else {
        0.0
    }
}
