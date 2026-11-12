use std::path::PathBuf;

use gpui::{
    AnyElement, App, AppContext as _, Entity, IntoElement, ParentElement as _, PathPromptOptions,
    SharedString, Styled as _, Window, div, px,
};
use gpui_native_kit::button::Button;
use gpui_native_kit::common::{h_flex, v_flex};
use gpui_native_kit::icons::{Icon, IconRole};
use gpui_native_kit::input::{Input, InputState};
use gpui_native_kit::select::{Select, SelectOption};
use gpui_native_kit::theme::ActiveTheme as _;
use gpui_native_kit::tooltip::Tooltip;
use settings::{AppearanceMode, EventFetchingStrategy, SettingsStore};

use crate::repo::dialogs::grasp_servers::{normalize_server, server_host};

/// A settings row with the label and description on the left and the control on the right.
pub fn setting_row(
    cx: &App,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    control: impl IntoElement,
) -> AnyElement {
    h_flex()
        .w_full()
        .justify_between()
        .gap_4()
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(title.into()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().colors.muted_foreground)
                        .child(description.into()),
                ),
        )
        .child(div().w(px(200.)).flex_shrink_0().child(control))
        .into_any_element()
}

/// A full-width settings block with title and subtitle in one header.
pub fn setting_block(
    cx: &App,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    control: impl IntoElement,
) -> AnyElement {
    v_flex()
        .w_full()
        .gap_3()
        .child(
            v_flex()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(title.into()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().colors.muted_foreground)
                        .child(description.into()),
                ),
        )
        .child(control)
        .into_any_element()
}

pub fn appearance_select(cx: &App) -> Select<AppearanceMode> {
    let current = SettingsStore::global(cx).read(cx).settings().appearance;
    Select::new("settings-appearance")
        .options(vec![
            SelectOption::new(AppearanceMode::System, "System"),
            SelectOption::new(AppearanceMode::Light, "Light"),
            SelectOption::new(AppearanceMode::Dark, "Dark"),
        ])
        .selected(Some(current))
        .on_change(|value, _, cx| {
            let Some(mode) = value else {
                return;
            };
            SettingsStore::global(cx).update(cx, |store, cx| {
                store.edit(|settings| settings.appearance = mode, cx);
            });
            crate::apply_appearance_mode(mode, cx);
        })
}

pub fn event_fetching_select(cx: &App) -> Select<EventFetchingStrategy> {
    let current = SettingsStore::global(cx).read(cx).settings().event_fetching;
    Select::new("settings-event-fetching")
        .options(vec![
            SelectOption::new(EventFetchingStrategy::Curated, "Curated"),
            SelectOption::new(EventFetchingStrategy::Uncensored, "Uncensored"),
        ])
        .selected(Some(current))
        .on_change(|value, _, cx| {
            let Some(strategy) = value else {
                return;
            };
            SettingsStore::global(cx).update(cx, |store, cx| {
                store.edit(|settings| settings.event_fetching = strategy, cx);
            });
        })
}

pub fn grasp_servers_editor(input: &Entity<InputState>, cx: &App) -> AnyElement {
    let servers = SettingsStore::global(cx)
        .read(cx)
        .settings()
        .grasp_servers
        .default_servers
        .clone();

    v_flex()
        .gap_2()
        .children(servers.iter().enumerate().map(|(ix, server)| {
            let store = SettingsStore::global(cx);
            h_flex()
                .gap_1()
                .child(
                    div()
                        .flex_1()
                        .h_8()
                        .px_2()
                        .bg(cx.theme().colors.muted)
                        .text_color(cx.theme().colors.muted_foreground)
                        .text_sm()
                        .rounded(cx.theme().radius.control)
                        .child(display_server(server)),
                )
                .child(
                    Button::new(format!("settings-remove-server:{ix}"))
                        .icon(Icon::named(IconRole::Close))
                        .variant(gpui_native_kit::button::ButtonVariant::Ghost)
                        .tooltip(Tooltip::text("Remove"))
                        .on_click({
                            let store = store.clone();
                            move |_, _, cx| {
                                store.update(cx, |store, cx| {
                                    store.edit(
                                        |settings| {
                                            settings.grasp_servers.default_servers.remove(ix);
                                        },
                                        cx,
                                    );
                                });
                            }
                        }),
                )
                .into_any_element()
        }))
        .child(
            h_flex()
                .gap_1()
                .child(
                    div()
                        .flex_1()
                        .child(Input::new("settings-grasp-input", input.clone())),
                )
                .child(
                    Button::new("settings-add-server")
                        .icon(Icon::named(IconRole::Plus))
                        .variant(gpui_native_kit::button::ButtonVariant::Ghost)
                        .tooltip(Tooltip::text("Add grasp server"))
                        .on_click({
                            let input = input.clone();
                            move |_, window, cx| add_server(&input, window, cx)
                        }),
                ),
        )
        .into_any_element()
}

fn display_server(server: &str) -> SharedString {
    match normalize_server(server) {
        Some((_, relay)) => server_host(&relay),
        None => SharedString::from(server.to_owned()),
    }
}

fn add_server(input: &Entity<InputState>, window: &mut Window, cx: &mut App) {
    let value = input.read(cx).value().trim().to_owned();
    if value.is_empty() {
        return;
    }
    let Some((normalized, _)) = normalize_server(&value) else {
        return;
    };
    SettingsStore::global(cx).update(cx, |store, cx| {
        store.edit(
            |settings| {
                if !settings.grasp_servers.default_servers.contains(&normalized) {
                    settings
                        .grasp_servers
                        .default_servers
                        .push(normalized.clone());
                }
            },
            cx,
        );
    });
    input.update(cx, |input, cx| input.set_value("", window, cx));
}

pub fn scan_paths_editor(cx: &App) -> AnyElement {
    let scan_paths = SettingsStore::global(cx)
        .read(cx)
        .settings()
        .local_repos
        .scan_paths
        .clone();
    let store = SettingsStore::global(cx);

    v_flex()
        .gap_2()
        .children(scan_paths.iter().enumerate().map(|(ix, path)| {
            let store = store.clone();
            h_flex()
                .gap_1()
                .child(
                    div()
                        .flex_1()
                        .h_8()
                        .px_2()
                        .bg(cx.theme().colors.muted)
                        .text_color(cx.theme().colors.muted_foreground)
                        .text_sm()
                        .rounded(cx.theme().radius.control)
                        .child(path.display().to_string()),
                )
                .child(
                    Button::new(format!("settings-remove-path:{ix}"))
                        .icon(Icon::named(IconRole::Close))
                        .variant(gpui_native_kit::button::ButtonVariant::Ghost)
                        .tooltip(Tooltip::text("Remove"))
                        .on_click(move |_, _, cx| {
                            store.update(cx, |store, cx| {
                                store.edit(
                                    |settings| {
                                        settings.local_repos.scan_paths.remove(ix);
                                    },
                                    cx,
                                );
                            });
                        }),
                )
                .into_any_element()
        }))
        .child(
            h_flex().justify_end().child(
                Button::new("settings-add-path")
                    .icon(Icon::named(IconRole::Plus))
                    .label("Add directory")
                    .on_click(|_, _, cx| add_scan_path(cx)),
            ),
        )
        .into_any_element()
}

fn add_scan_path(cx: &mut App) {
    let prompt = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: true,
        prompt: Some("Choose directories".into()),
    });
    cx.spawn(async move |cx| {
        if let Ok(Ok(Some(paths))) = prompt.await {
            cx.update(|cx| {
                let store = SettingsStore::global(cx);
                store.update(cx, |store, cx| {
                    store.edit(
                        |settings| {
                            for path in paths {
                                let path = path.to_string_lossy().to_string();
                                if !settings
                                    .local_repos
                                    .scan_paths
                                    .iter()
                                    .any(|existing| existing.to_string_lossy() == path)
                                {
                                    settings.local_repos.scan_paths.push(PathBuf::from(path));
                                }
                            }
                        },
                        cx,
                    );
                });
            });
        }
    })
    .detach();
}

pub fn default_folder_selector(default_folder: &Entity<InputState>) -> AnyElement {
    let folder = default_folder.clone();
    h_flex()
        .gap_1()
        .child(
            div()
                .flex_1()
                .child(Input::new("settings-default-folder", folder.clone())),
        )
        .child(
            Button::new("settings-choose-folder")
                .icon(Icon::named(IconRole::Folder))
                .variant(gpui_native_kit::button::ButtonVariant::Ghost)
                .tooltip(Tooltip::text("Choose folder"))
                .on_click(move |_, window, cx| {
                    choose_default_folder(&folder, window, cx);
                }),
        )
        .into_any_element()
}

fn choose_default_folder(default_folder: &Entity<InputState>, window: &mut Window, cx: &mut App) {
    let handle = window.window_handle();
    let default_folder = default_folder.clone();
    let prompt = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: Some("Choose default folder".into()),
    });
    cx.spawn(async move |cx| {
        if let Ok(Ok(Some(mut paths))) = prompt.await
            && let Some(path) = paths.pop()
        {
            let text = path.to_string_lossy().to_string();
            if let Err(error) = cx.update_window(handle, |_, window, cx| {
                let store = SettingsStore::global(cx);
                store.update(cx, |store, cx| {
                    store.edit(
                        |settings| settings.create_repository.default_folder = Some(path.clone()),
                        cx,
                    );
                });
                default_folder.update(cx, |input, cx| input.set_value(text, window, cx));
            }) {
                log::warn!("failed to apply default folder: {error}");
            }
        }
    })
    .detach();
}
