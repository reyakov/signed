use std::sync::Arc;

use assets::Assets;
use dock::TAB_BAR_HEIGHT;
use gpui::*;
use gpui_component::{Theme, ThemeMode, ThemeRegistry, theme};
use gpui_platform::application;
use native_theme::watch::ThemeSubscription;
use native_theme_gpui::{SystemTheme, apply_system_theme};
use settings::{AppearanceMode, Settings, SettingsStore};

actions!(signed, [Quit]);

fn main() {
    tracing_subscriber::fmt::init();

    application()
        .with_assets(Assets)
        .with_http_client(Arc::new(reqwest_client::ReqwestClient::new()))
        .run(move |cx| {
            gpui_component::init(cx);
            theme::init(cx);

            cx.bind_keys([KeyBinding::new("secondary-q", Quit, None)]);
            cx.on_action(|_: &Quit, cx| cx.quit());

            // The persisted settings must load before the theme is applied.
            let store = cx.new(|cx| SettingsStore::new(paths::settings_file(), cx));
            SettingsStore::set_global(store.clone(), cx);

            let settings = store.read(cx).settings().clone();
            {
                let registry = ThemeRegistry::global_mut(cx);
                for (name, content) in Assets.themes() {
                    if let Err(err) = registry.load_themes_from_str(&content) {
                        tracing::error!("Failed to load theme {name}: {err}");
                    }
                }
            }

            match apply_native_theme(&settings, cx) {
                Ok(()) => start_theme_watcher(cx),
                Err(error) => {
                    tracing::warn!("native theme unavailable ({error})");

                    let registry = ThemeRegistry::global_mut(cx);
                    let light_theme = registry
                        .themes()
                        .get(settings.theme.light_theme.as_str())
                        .cloned();

                    let dark_theme = registry
                        .themes()
                        .get(settings.theme.dark_theme.as_str())
                        .cloned();

                    let theme = Theme::global_mut(cx);
                    theme.radius = px(settings.theme.radius);
                    theme.radius_lg = px(settings.theme.radius_lg);
                    theme.focus_ring = settings.theme.focus_ring;
                    theme.shadow = settings.theme.shadow;
                    theme.font_size = px(settings.theme.font_size);
                    theme.mono_font_size = px(settings.theme.mono_font_size);

                    if let Some(light) = light_theme {
                        theme.light_theme = light;
                    } else {
                        tracing::warn!("{} theme is missing", settings.theme.light_theme);
                    }

                    if let Some(dark) = dark_theme {
                        theme.dark_theme = dark;
                    } else {
                        tracing::warn!("{} theme is missing", settings.theme.dark_theme);
                    }

                    match settings.appearance {
                        AppearanceMode::System => Theme::sync_system_appearance(None, cx),
                        AppearanceMode::Light => Theme::change(ThemeMode::Light, None, cx),
                        AppearanceMode::Dark => Theme::change(ThemeMode::Dark, None, cx),
                    }
                }
            }

            std::fs::create_dir_all(paths::nostr_dir()).ok();
            std::fs::create_dir_all(paths::repos_dir()).ok();

            signed_state::init(
                paths::nostr_dir(),
                paths::repos_dir().clone(),
                settings.local_repos.scan_paths,
                cx,
            );
            metadata::init(cx);

            cx.set_app_identity("info.reya.signed", "Signed");

            let bounds = Bounds::centered(None, size(px(1120.0), px(750.0)), cx);
            let opts = WindowOptions {
                window_background: WindowBackgroundAppearance::Opaque,
                window_decorations: Some(WindowDecorations::Client),
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(960.0), px(640.0))),
                kind: WindowKind::Normal,
                app_id: Some("Signed".to_owned()),
                titlebar: Some(TitlebarOptions {
                    title: Some(SharedString::new_static("Signed")),
                    // Center the 14pt traffic-light buttons on the tab bar.
                    traffic_light_position: Some(point(
                        px(9.0),
                        px(TAB_BAR_HEIGHT / px(2.) - 14. / 2.),
                    )),
                    appears_transparent: true,
                }),
                app_owns_titlebar_drag: true,
                ..Default::default()
            };

            cx.spawn(async move |cx| {
                cx.open_window(opts, workspace::root).ok();
            })
            .detach();

            cx.activate(true);
        });
}

/// Apply the OS theme to the app, scaling the font size to match the system font size.
fn apply_native_theme(settings: &Settings, cx: &mut App) -> native_theme::Result<()> {
    let mut system = SystemTheme::from_system()?;
    let native = system.light.defaults.font.size;
    if native > 0.0 {
        let scaling = settings.theme.font_size / native;
        system.accessibility.text_scaling_factor *= scaling;
    }

    apply_system_theme(&system, cx);

    match settings.appearance {
        AppearanceMode::System => Theme::sync_system_appearance(None, cx),
        AppearanceMode::Light => Theme::change(ThemeMode::Light, None, cx),
        AppearanceMode::Dark => Theme::change(ThemeMode::Dark, None, cx),
    }

    Ok(())
}

/// Holds the watcher subscription so it lives as long as the app.
struct ThemeWatcher {
    _subscription: ThemeSubscription,
}

impl Global for ThemeWatcher {}

/// Re-apply the OS theme whenever it changes at runtime.
fn start_theme_watcher(cx: &mut App) {
    let (sender, receiver) = flume::unbounded();
    match native_theme::watch::on_theme_change(move |event| {
        if let Err(error) = sender.send(event) {
            tracing::warn!("theme change dropped ({error})");
        }
    }) {
        Ok(subscription) => {
            cx.set_global(ThemeWatcher {
                _subscription: subscription,
            });
            cx.spawn(async move |cx| {
                while receiver.recv_async().await.is_ok() {
                    tracing::info!("OS theme changed; re-applying");
                    cx.update(|cx| {
                        let store = SettingsStore::global(cx);
                        let settings = store.read(cx).settings().clone();
                        if let Err(error) = apply_native_theme(&settings, cx) {
                            tracing::warn!("failed to apply updated OS theme ({error})");
                        }
                    });
                }
            })
            .detach();
        }
        Err(error) => {
            tracing::warn!("OS theme watching unavailable ({error})")
        }
    }
}
