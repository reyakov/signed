use std::sync::Arc;

use assets::Assets;
use gpui::*;
use gpui_native_kit::theme::{ActiveTheme as _, Theme};
use gpui_platform::application;
use settings::SettingsStore;

actions!(signed, [Quit]);

fn main() {
    tracing_subscriber::fmt::init();

    application()
        .with_assets(Assets)
        .with_http_client(Arc::new(reqwest_client::ReqwestClient::new()))
        .run(move |cx| {
            gpui_native_kit::init(cx);
            gpui_native_kit::editor::code::bind_code_view_keys(cx);
            gpui_native_kit::editor::markdown::bind_text_view_keys(cx);

            cx.bind_keys([
                KeyBinding::new("cmd-q", Quit, None),
                KeyBinding::new("ctrl-q", Quit, None),
            ]);
            cx.on_action(|_: &Quit, cx| cx.quit());

            let store = cx.new(|cx| SettingsStore::new(paths::settings_file(), cx));
            SettingsStore::set_global(store.clone(), cx);
            let settings = store.read(cx).settings().clone();
            signed_app::apply_appearance(&settings, cx);

            if let Err(error) = std::fs::create_dir_all(paths::nostr_dir()) {
                tracing::warn!("failed to create nostr dir: {error}");
            }
            if let Err(error) = std::fs::create_dir_all(paths::repos_dir()) {
                tracing::warn!("failed to create repos dir: {error}");
            }
            signed_state::init(
                paths::nostr_dir(),
                paths::repos_dir().clone(),
                settings.local_repos.scan_paths.clone(),
                cx,
            );
            metadata::init(cx);
            cx.set_app_identity("info.reya.signed", "Signed");

            let theme = cx.theme();
            let bounds = Bounds::centered(None, size(px(1120.0), px(750.0)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(960.0), px(640.0))),
                titlebar: Some(TitlebarOptions {
                    title: Some("Signed".into()),
                    appears_transparent: true,
                    ..Default::default()
                }),
                app_owns_titlebar_drag: true,
                window_background: theme.window_background,
                app_id: Some("Signed".to_owned()),
                ..Default::default()
            };

            if let Err(error) = cx.open_window(options, |window, cx| {
                Theme::sync_from_window(window, cx);
                Theme::sync_window_rem_size(window, cx);
                window
                    .observe_window_appearance(|window, cx| {
                        Theme::sync_from_window(window, cx);
                        Theme::sync_window_rem_size(window, cx);
                        cx.refresh_windows();
                    })
                    .detach();
                signed_app::root(window, cx)
            }) {
                tracing::error!("failed to open window: {error}");
            }
            cx.activate(true);
        });
}
