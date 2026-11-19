//! Linux adapter selection: Omarchy → KDE → portal (GNOME/generic) →
//! fallback, per the plan's detection cascade. Omarchy and KDE state the mode
//! and accent; the portal supplements both with the accessibility flags;
//! `gsettings` covers sessions without a portal.

use std::path::{Path, PathBuf};

use gpui::Hsla;
use notify::{RecursiveMode, Watcher};

use super::signal::ReapplySignal;
use super::{ThemeSource, kde, omarchy, portal};
use crate::theme::ThemeVariant;
use crate::theme::system::{SourceKind, SystemTheme};

pub(crate) struct LinuxSource {
    backend: Backend,
    portal: Option<portal::io::PortalClient>,
    gsettings: Option<GSettingsSnapshot>,
    file_watcher: Option<notify::RecommendedWatcher>,
    /// Keeps the main-thread channel loop alive.
    _signal: Option<ReapplySignal>,
}

/// Which payload the source reads.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Backend {
    Omarchy,
    Kde,
    Portal,
    Fallback,
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Whether the session runs on Wayland, where compositors state blur protocols.
fn wayland_session() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var("XDG_SESSION_TYPE").is_ok_and(|session| session == "wayland")
}

fn detect(home: &Path, portal_reachable: bool, gsettings_reachable: bool) -> Backend {
    let omarchy_marker = home
        .join(".local/state/omarchy/current/theme.name")
        .exists()
        || home.join(".local/state/omarchy/current/theme").is_dir()
        || home.join(".config/omarchy").is_dir();
    if omarchy_marker {
        Backend::Omarchy
    } else if home.join(".config/kdeglobals").exists() {
        Backend::Kde
    } else if portal_reachable || gsettings_reachable {
        Backend::Portal
    } else {
        Backend::Fallback
    }
}

/// What `gsettings` states for GNOME sessions.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct GSettingsSnapshot {
    variant: Option<ThemeVariant>,
    accent: Option<Hsla>,
}

fn gsettings_value(args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("gsettings")
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

/// The named accent colors GNOME ships (libadwaita's background variants).
fn accent_rgb(name: &str) -> Option<Hsla> {
    let hex = match name {
        "blue" => 0x3584e4,
        "teal" => 0x2190a4,
        "green" => 0x3a944a,
        "yellow" => 0xc88800,
        "orange" => 0xed5b00,
        "red" => 0xe62d42,
        "pink" => 0xd56199,
        "purple" => 0x9141ac,
        "slate" => 0x6f8396,
        _ => return None,
    };
    Some(Hsla::from(gpui::rgb(hex)))
}

fn read_gsettings() -> Option<GSettingsSnapshot> {
    let variant = gsettings_value(&["get", "org.gnome.desktop.interface", "color-scheme"])
        .and_then(|value| match value.trim().trim_matches('\'') {
            "prefer-dark" => Some(ThemeVariant::Dark),
            "prefer-light" => Some(ThemeVariant::Light),
            _ => None,
        });

    let accent = gsettings_value(&["get", "org.gnome.desktop.interface", "accent-color"])
        .and_then(|value| accent_rgb(value.trim().trim_matches('\'')));

    if variant.is_none() && accent.is_none() {
        return None;
    }
    Some(GSettingsSnapshot { variant, accent })
}

pub(crate) fn platform_source() -> Box<dyn ThemeSource> {
    let portal = portal::io::PortalClient::connect();
    let gsettings = read_gsettings();

    let backend = home_dir()
        .map(|home| detect(&home, portal.is_some(), gsettings.is_some()))
        .unwrap_or(Backend::Fallback);

    Box::new(LinuxSource {
        backend,
        portal,
        gsettings,
        file_watcher: None,
        _signal: None,
    })
}

impl LinuxSource {
    fn gnome_snapshot(
        &self,
        variant: ThemeVariant,
        accent: Option<Hsla>,
        accessibility: crate::theme::system::AccessibilityPreferences,
    ) -> SystemTheme {
        SystemTheme {
            variant,
            accent,
            accessibility,
            window_background: None,
            source: SourceKind::Gnome,
        }
    }

    fn portal_snapshot(
        &self,
        variant: ThemeVariant,
        portal_appearance: Option<portal::Appearance>,
        accessibility: crate::theme::system::AccessibilityPreferences,
    ) -> SystemTheme {
        if let Some(appearance) = portal_appearance {
            return self.gnome_snapshot(
                appearance.variant(variant),
                appearance.accent,
                accessibility,
            );
        }

        if let Some(gsettings) = self.gsettings {
            return self.gnome_snapshot(
                gsettings.variant.unwrap_or(variant),
                gsettings.accent,
                accessibility,
            );
        }

        SystemTheme::fallback(variant)
    }
}

impl ThemeSource for LinuxSource {
    fn snapshot(&self, variant: ThemeVariant) -> SystemTheme {
        let portal_appearance = self.portal.as_ref().map(|portal| portal.appearance());
        let accessibility = portal_appearance
            .map(portal::Appearance::accessibility)
            .unwrap_or_default();

        match self.backend {
            Backend::Omarchy => omarchy::io::read_current()
                .map(|theme| {
                    let mut system = theme.into_system(accessibility);
                    // Hyprland blurs transparent surfaces on its own.
                    system.window_background = Some(gpui::WindowBackgroundAppearance::Blurred);
                    system
                })
                .unwrap_or_else(|| SystemTheme::fallback(variant)),
            Backend::Kde => kde::io::read_current()
                .map(|scheme| {
                    let mut system = scheme.into_system(
                        accessibility,
                        portal_appearance.and_then(|appearance| appearance.accent),
                        portal_appearance.map(|appearance| appearance.variant(variant)),
                    );
                    // KWin states the blur protocols; X11 compositors do not.
                    system.window_background =
                        wayland_session().then_some(gpui::WindowBackgroundAppearance::Blurred);
                    system
                })
                .unwrap_or_else(|| SystemTheme::fallback(variant)),
            Backend::Portal => self.portal_snapshot(variant, portal_appearance, accessibility),
            Backend::Fallback => SystemTheme::fallback(variant),
        }
    }

    fn install_watchers(&mut self, async_cx: gpui::AsyncApp) {
        let signal = ReapplySignal::install(&async_cx);
        let file_signal = signal.sender();

        if let Some(portal) = &self.portal {
            portal.spawn_watch(signal.sender());
        }

        // Stored before any fallible step: an early return below
        // must not drop and cancel the reapply loop.
        self._signal = Some(signal);

        match self.backend {
            Backend::Omarchy => {
                let Some(current) = omarchy::io::current_dir() else {
                    return;
                };

                let watcher = notify::recommended_watcher(
                    move |event: std::result::Result<notify::Event, notify::Error>| match event {
                        Ok(_) => ReapplySignal::poke(&file_signal),
                        Err(error) => log::warn!("theme: omarchy watch error: {error}"),
                    },
                );

                match watcher {
                    Ok(mut watcher) => {
                        if let Err(error) = watcher.watch(&current, RecursiveMode::NonRecursive) {
                            log::warn!("theme: omarchy watch failed: {error}");
                            return;
                        }
                        self.file_watcher = Some(watcher);
                    }
                    Err(error) => log::warn!("theme: omarchy watcher unavailable: {error}"),
                }
            }
            Backend::Kde => {
                let Some(config) = home_dir().map(|home| home.join(".config")) else {
                    return;
                };

                let watcher = notify::recommended_watcher(
                    move |event: std::result::Result<notify::Event, notify::Error>| match event {
                        Ok(event) => {
                            if event.paths.iter().any(|path| {
                                path.file_name().is_some_and(|name| name == "kdeglobals")
                            }) {
                                ReapplySignal::poke(&file_signal);
                            }
                        }
                        Err(error) => log::warn!("theme: kde watch error: {error}"),
                    },
                );
                match watcher {
                    Ok(mut watcher) => {
                        if let Err(error) = watcher.watch(&config, RecursiveMode::NonRecursive) {
                            log::warn!("theme: kde watch failed: {error}");
                            return;
                        }
                        self.file_watcher = Some(watcher);
                    }
                    Err(error) => log::warn!("theme: kde watcher unavailable: {error}"),
                }
            }
            Backend::Portal | Backend::Fallback => {}
        }
    }

    fn label(&self) -> &'static str {
        match self.backend {
            Backend::Omarchy => SourceKind::Omarchy.label(),
            Backend::Kde => SourceKind::Kde.label(),
            Backend::Portal => SourceKind::Gnome.label(),
            Backend::Fallback => SourceKind::Fallback.label(),
        }
    }
}
