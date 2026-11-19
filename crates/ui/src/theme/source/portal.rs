//! The xdg-desktop-portal Settings reader (`org.freedesktop.appearance`).
//! GTK and GNOME toolkits take these values from the same place, so this is
//! the canonical source on generic Linux. The value mapping is pure and
//! fixture-tested on every OS; the DBus client lives behind the Linux cfg.

use gpui::Hsla;

use super::narrow_component;
use crate::theme::ThemeVariant;
use crate::theme::system::AccessibilityPreferences;

/// The raw portal values, already converted off the DBus wire types.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct AppearanceValues {
    /// 0 = no preference, 1 = prefer dark, 2 = prefer light.
    pub(crate) color_scheme: Option<u32>,
    /// The `accent-color` sRGB tuple, each component in 0..1.
    pub(crate) accent_rgb: Option<(f64, f64, f64)>,
    /// 0 = normal, 1 = higher.
    pub(crate) contrast: Option<u32>,
    /// 0 = no preference, 1 = reduced.
    pub(crate) reduced_motion: Option<u32>,
}

impl AppearanceValues {
    pub(crate) fn decode(self) -> Appearance {
        Appearance {
            color_scheme: self.color_scheme,
            accent: self.accent_rgb.map(|(r, g, b)| {
                Hsla::from(gpui::Rgba {
                    r: narrow_component(r),
                    g: narrow_component(g),
                    b: narrow_component(b),
                    a: 1.0,
                })
            }),
            contrast: self.contrast.unwrap_or(0),
            reduced_motion: self.reduced_motion.unwrap_or(0),
        }
    }
}

/// What the portal states under `org.freedesktop.appearance`.
///
/// Per the portal spec, unknown values read as unset.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Appearance {
    pub(crate) color_scheme: Option<u32>,
    pub(crate) accent: Option<Hsla>,
    pub(crate) contrast: u32,
    pub(crate) reduced_motion: u32,
}

impl Appearance {
    pub(crate) fn variant(&self, no_preference: ThemeVariant) -> ThemeVariant {
        match self.color_scheme {
            Some(1) => ThemeVariant::Dark,
            Some(2) => ThemeVariant::Light,
            _ => no_preference,
        }
    }

    pub(crate) fn accessibility(self) -> AccessibilityPreferences {
        AccessibilityPreferences {
            high_contrast: self.contrast >= 1,
            reduce_motion: self.reduced_motion >= 1,
            reduce_transparency: false,
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) mod io {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use zbus::blocking::{Connection, Proxy};
    use zbus::zvariant::OwnedValue;

    use super::{Appearance, AppearanceValues};

    type AppearanceCache = Arc<Mutex<Appearance>>;

    const SERVICE: &str = "org.freedesktop.portal.Desktop";
    const PATH: &str = "/org/freedesktop/portal/desktop";
    const INTERFACE: &str = "org.freedesktop.portal.Settings";
    const NAMESPACE: &str = "org.freedesktop.appearance";

    pub(crate) struct PortalClient {
        appearance: AppearanceCache,
    }

    impl PortalClient {
        /// Reads the appearance once.
        pub(crate) fn connect() -> Option<Self> {
            let proxy = settings_proxy(Connection::session().ok()?)?;
            let appearance = Arc::new(Mutex::new(read_appearance(&proxy)?));
            Some(Self { appearance })
        }

        /// The most recently read appearance.
        pub(crate) fn appearance(&self) -> Appearance {
            // A watch-thread panic must not crash the UI thread.
            let cache = self.appearance.lock();
            *cache.unwrap_or_else(std::sync::PoisonError::into_inner)
        }

        /// Forwards portal `SettingChanged` events to the reapply signal from a dedicated thread.
        pub(crate) fn spawn_watch(&self, reapply_signal: async_channel::Sender<()>) {
            let appearance = self.appearance.clone();

            std::thread::spawn(move || {
                let Some(connection) = Connection::session().ok() else {
                    log::warn!("theme: portal watch skipped, no session bus");
                    return;
                };

                let Some(proxy) = settings_proxy(connection) else {
                    log::warn!("theme: portal watch skipped, settings proxy unavailable");
                    return;
                };

                let Ok(signals) = proxy.receive_signal("SettingChanged") else {
                    log::warn!("theme: portal watch skipped, signal subscription failed");
                    return;
                };

                for message in signals {
                    let Ok((namespace, _key, _value)) =
                        message.body().deserialize::<(String, String, OwnedValue)>()
                    else {
                        continue;
                    };

                    if namespace != NAMESPACE {
                        continue;
                    }

                    match read_appearance(&proxy) {
                        Some(latest) => {
                            let mut cache = appearance
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            *cache = latest;
                            super::super::signal::ReapplySignal::poke(&reapply_signal);
                        }
                        None => log::warn!("theme: portal read failed on SettingChanged"),
                    }
                }
            });
        }
    }

    fn settings_proxy(connection: Connection) -> Option<Proxy<'static>> {
        Proxy::new(&connection, SERVICE, PATH, INTERFACE).ok()
    }

    fn read_appearance(proxy: &Proxy<'static>) -> Option<Appearance> {
        let values: HashMap<String, HashMap<String, OwnedValue>> =
            proxy.call("ReadAll", &([NAMESPACE],)).ok()?;

        let namespace = values.get(NAMESPACE)?;
        let field = |key: &str| namespace.get(key);

        Some(
            AppearanceValues {
                color_scheme: field("color-scheme")
                    .and_then(|value| u32::try_from(value.clone()).ok()),
                accent_rgb: field("accent-color")
                    .and_then(|value| <(f64, f64, f64)>::try_from(value.clone()).ok()),
                contrast: field("contrast").and_then(|value| u32::try_from(value.clone()).ok()),
                reduced_motion: field("reduced-motion")
                    .and_then(|value| u32::try_from(value.clone()).ok()),
            }
            .decode(),
        )
    }
}
