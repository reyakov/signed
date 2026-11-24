//! Windows 11 adapter: WinRT `UISettings` accent and background with a
//! `Personalize` registry fallback, watched through `ColorValuesChanged`,
//! `AdvancedEffectsEnabledChanged`, and `HighContrastChanged`.

use windows::Foundation::TypedEventHandler;
use windows::UI::ViewManagement::{AccessibilitySettings, UIColorType, UISettings};
use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::core::IInspectable;

use super::ThemeSource;
use super::signal::ReapplySignal;
use crate::theme::ThemeVariant;
use crate::theme::system::{AccessibilityPreferences, SourceKind, SystemTheme};

/// The `Personalize` registry values and the OS build number.
#[derive(Clone, Copy, Debug, Default)]
struct RegistrySettings {
    apps_use_light_theme: Option<bool>,
    enable_transparency: Option<bool>,
    current_build: Option<u32>,
}

impl RegistrySettings {
    fn read() -> Self {
        let personalize = || {
            windows_registry::CURRENT_USER
                .open(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
        };
        let current_version = || {
            windows_registry::LOCAL_MACHINE.open(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion")
        };
        Self {
            apps_use_light_theme: personalize()
                .and_then(|key| key.get_u32("AppsUseLightTheme"))
                .ok()
                .map(|value| value != 0),
            enable_transparency: personalize()
                .and_then(|key| key.get_u32("EnableTransparency"))
                .ok()
                .map(|value| value != 0),
            current_build: current_version()
                .and_then(|key| key.get_string("CurrentBuildNumber"))
                .ok()
                .and_then(|build| build.parse::<u32>().ok()),
        }
    }
}

pub(crate) struct WindowsSource {
    ui_settings: UISettings,
    accessibility_settings: AccessibilitySettings,
    registry: RegistrySettings,
    /// Keeps the main-thread channel loop alive.
    _signal: Option<ReapplySignal>,
}

impl WindowsSource {
    fn new() -> Option<Self> {
        let ui_settings = match UISettings::new() {
            Ok(ui_settings) => ui_settings,
            Err(error) => {
                log::warn!("theme: UISettings unavailable: {error}");
                return None;
            }
        };

        let accessibility_settings = match AccessibilitySettings::new() {
            Ok(accessibility_settings) => accessibility_settings,
            Err(error) => {
                log::warn!("theme: AccessibilitySettings unavailable: {error}");
                return None;
            }
        };

        Some(Self {
            ui_settings,
            accessibility_settings,
            registry: RegistrySettings::read(),
            _signal: None,
        })
    }

    fn color(&self, kind: UIColorType) -> Option<gpui::Hsla> {
        let color = self.ui_settings.GetColorValue(kind).ok()?;
        Some(gpui::Hsla::from(gpui::Rgba {
            r: color.R as f32 / 255.0,
            g: color.G as f32 / 255.0,
            b: color.B as f32 / 255.0,
            a: color.A as f32 / 255.0,
        }))
    }

    fn accessibility(&self) -> AccessibilityPreferences {
        AccessibilityPreferences {
            high_contrast: self.accessibility_settings.HighContrast().unwrap_or(false),
            // Windows states animations separately in Win32; not read yet.
            reduce_motion: false,
            reduce_transparency: self
                .registry
                .enable_transparency
                .map(|enabled| !enabled)
                .unwrap_or(false),
        }
    }

    /// The native backdrop: Mica on Windows 11, acrylic on Windows 10 1809+.
    fn window_material(&self) -> Option<gpui::WindowBackgroundAppearance> {
        match self.registry.current_build {
            // 22621: Windows 11 22H2, where the DWM backdrop types exist.
            Some(build) if build >= 22621 => Some(gpui::WindowBackgroundAppearance::MicaBackdrop),
            // 17763: Windows 10 1809, where acrylic blur-behind exists.
            Some(build) if build >= 17763 => Some(gpui::WindowBackgroundAppearance::Blurred),
            _ => None,
        }
    }
}

impl ThemeSource for WindowsSource {
    fn snapshot(&self, variant: ThemeVariant) -> SystemTheme {
        let accent = self.color(UIColorType::Accent);
        let background = self.color(UIColorType::Background);

        let variant = background
            .map(ThemeVariant::by_lightness)
            .or(self.registry.apps_use_light_theme.map(|light| {
                if light {
                    ThemeVariant::Light
                } else {
                    ThemeVariant::Dark
                }
            }))
            .unwrap_or(variant);

        SystemTheme {
            variant,
            accent,
            accessibility: self.accessibility(),
            window_background: self.window_material(),
            source: SourceKind::Windows,
        }
    }

    fn install_watchers(&mut self, async_cx: gpui::AsyncApp) {
        let signal = ReapplySignal::install(&async_cx);
        let color_signal = signal.sender();
        let color_handler = TypedEventHandler::<UISettings, IInspectable>::new(move |_, _| {
            ReapplySignal::poke(&color_signal);
            Ok(())
        });

        if let Err(error) = self.ui_settings.ColorValuesChanged(&color_handler) {
            log::warn!("theme: color change watch failed: {error}");
        }

        let effects_signal = signal.sender();
        let effects_handler = TypedEventHandler::<UISettings, IInspectable>::new(move |_, _| {
            ReapplySignal::poke(&effects_signal);
            Ok(())
        });

        if let Err(error) = self
            .ui_settings
            .AdvancedEffectsEnabledChanged(&effects_handler)
        {
            log::warn!("theme: advanced effects watch failed: {error}");
        }

        let contrast_signal = signal.sender();
        let contrast_handler =
            TypedEventHandler::<AccessibilitySettings, IInspectable>::new(move |_, _| {
                ReapplySignal::poke(&contrast_signal);
                Ok(())
            });

        if let Err(error) = self
            .accessibility_settings
            .HighContrastChanged(&contrast_handler)
        {
            log::warn!("theme: high contrast watch failed: {error}");
        }

        self._signal = Some(signal);
    }

    fn label(&self) -> &'static str {
        SourceKind::Windows.label()
    }
}

pub(crate) fn platform_source() -> Box<dyn ThemeSource> {
    // SAFETY: WinRT activation requires COM on this thread,
    // the STA matches the UI thread's message pump,
    // and RPC_E_CHANGED_MODE means COM is already initialized in another apartment.
    let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };

    if initialized.is_err() && initialized != RPC_E_CHANGED_MODE {
        log::error!("theme: COM initialization failed: {initialized:?}");
    }

    match WindowsSource::new() {
        Some(source) => Box::new(source),
        None => {
            log::warn!("theme: Windows system theme unavailable; using fallback");
            Box::new(super::fallback::FallbackSource)
        }
    }
}
