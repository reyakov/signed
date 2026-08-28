use std::sync::Arc;

use assets::Assets;
use dock::TAB_BAR_HEIGHT;
use gpui::*;
use gpui_component::{Theme, ThemeRegistry, theme};
use gpui_platform::application;

fn main() {
    // Initialize logging
    tracing_subscriber::fmt::init();

    application()
        .with_assets(Assets)
        .with_http_client(Arc::new(reqwest_client::ReqwestClient::new()))
        .run(move |cx| {
            // Set app identity
            cx.set_app_identity("su.reya.signed", "Signed");

            // Initialize components
            gpui_component::init(cx);

            // Initialize theme
            theme::init(cx);

            // Register the built-in "Signed" theme (light + dark variants)
            // and make it the active theme, following the system appearance.
            let registry = ThemeRegistry::global_mut(cx);
            for (name, content) in Assets.themes() {
                if let Err(err) = registry.load_themes_from_str(&content) {
                    tracing::error!("Failed to load theme {name}: {err}");
                }
            }
            let light_theme = registry.themes().get("Signed Light").cloned();
            let dark_theme = registry.themes().get("Signed Dark").cloned();

            let theme = Theme::global_mut(cx);
            theme.radius = px(2.);
            theme.radius_lg = px(6.);
            theme.focus_ring = false;
            theme.shadow = false;

            if let Some(light) = light_theme {
                theme.light_theme = light;
            } else {
                tracing::warn!("Signed Light theme is missing from the registry");
            }

            if let Some(dark) = dark_theme {
                theme.dark_theme = dark;
            } else {
                tracing::warn!("Signed Dark theme is missing from the registry");
            }

            // Sync the theme with the system appearance
            Theme::sync_system_appearance(None, cx);

            // Initialize backend and stores (connects relays, restores session)
            std::fs::create_dir_all(paths::nostr_dir()).ok();
            signed_state::init(paths::nostr_dir(), cx);

            // Local git clone cache for browsing repository contents.
            std::fs::create_dir_all(paths::repos_dir()).ok();
            signed_state::GitStore::set_global(paths::repos_dir().clone(), cx);

            // Set up the window options
            let bounds = Bounds::centered(None, size(px(1120.0), px(750.0)), cx);

            // The dock's tab bar acts as the window title bar: the app owns
            // title-bar dragging (via `start_window_move` on the tab bar), so
            // AppKit must not treat the top strip as a native drag region.
            let opts = WindowOptions {
                window_background: WindowBackgroundAppearance::Opaque,
                window_decorations: Some(WindowDecorations::Client),
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(960.0), px(640.0))),
                kind: WindowKind::Normal,
                app_id: Some("Signed".to_owned()),
                titlebar: Some(TitlebarOptions {
                    title: Some(SharedString::new_static("Signed")),
                    // AppKit's traffic-light buttons are 14 pt tall; offset them so their vertical center matches the tab bar.
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

            // Bring the app to the foreground
            cx.activate(true);
        });
}
