use std::sync::Arc;

use gpui::*;
use gpui_platform::application;
use settings::SettingsStore;
use ui::theme::{ActiveTheme as _, Theme};

actions!(signed, [Quit]);

/// Embeds app artwork and serves it plus the ui crate's icon set.
#[derive(rust_embed::RustEmbed)]
#[folder = "assets"]
#[exclude = "*.DS_Store"]
struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> gpui::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        if let Some(name) = path.strip_prefix("icons/") {
            return Ok(ui::Assets::get(name).map(|file| file.data));
        }
        Ok(Self::get(path).map(|file| file.data))
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        let mut files: Vec<SharedString> = ui::Assets::iter()
            .map(|name| SharedString::from(format!("icons/{name}")))
            .filter(|file| file.starts_with(path))
            .collect();

        files.extend(
            Self::iter()
                .filter(|file| file.starts_with(path))
                .map(SharedString::from),
        );

        Ok(files)
    }
}

fn main() {
    tracing_subscriber::fmt::init();

    application()
        .with_assets(AppAssets)
        .with_http_client(Arc::new(reqwest_client::ReqwestClient::new()))
        .run(move |cx| {
            ui::init(cx);

            cx.bind_keys([
                KeyBinding::new("cmd-q", Quit, None),
                KeyBinding::new("ctrl-q", Quit, None),
            ]);

            cx.on_action(|_: &Quit, cx| cx.quit());
            cx.set_app_identity("build.signed.signed", "Signed");

            let store = cx.new(|_| SettingsStore::new(paths::settings_file()));
            SettingsStore::set_global(store.clone(), cx);

            let settings = store.read(cx).settings().clone();
            signed_app::AppRoot::apply_appearance_mode(settings.appearance, cx);

            if let Err(error) = std::fs::create_dir_all(paths::nostr_dir()) {
                tracing::warn!("failed to create nostr dir: {error}");
            }

            if let Err(error) = std::fs::create_dir_all(paths::repos_dir()) {
                tracing::warn!("failed to create repos dir: {error}");
            }

            if let Err(error) = signed_state::init(
                paths::nostr_dir(),
                paths::repos_dir().to_path_buf(),
                settings.local_repos.scan_paths.clone(),
                cx,
            ) {
                tracing::error!("failed to initialize application state: {error:#}");
                cx.quit();
                return;
            }

            metadata::init(cx);

            let theme = cx.theme();
            let bounds = Bounds::centered(None, size(px(1120.0), px(750.0)), cx);

            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(960.0), px(640.0))),
                titlebar: Some(TitlebarOptions {
                    title: Some("Signed".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(
                        px(12.0),
                        (theme.titlebar_height - px(16.0)) / 2.0,
                    )),
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

                signed_app::AppRoot::mount(window, cx)
            }) {
                tracing::error!("failed to open window: {error}");
                cx.quit();
                return;
            }

            cx.activate(true);
        });
}
