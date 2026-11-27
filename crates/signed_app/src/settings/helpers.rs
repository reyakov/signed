use std::path::PathBuf;

use gpui::{
    AnyElement, App, AppContext as _, Div, ElementId, Entity, IntoElement, ParentElement as _,
    PathPromptOptions, SharedString, Styled as _, WeakEntity, Window, div, px,
};
use settings::{AppearanceMode, EventFetchingStrategy, SettingsStore};
use ui::button::{Button, ButtonVariant};
use ui::common::{StyledExt as _, h_flex, v_flex};
use ui::icons::{Icon, IconRole};
use ui::input::{Input, InputState};
use ui::select::{Select, SelectOption};
use ui::theme::ActiveTheme as _;
use ui::tooltip::Tooltip;

use crate::settings::{PathRow, ServerRow, SettingsDialog};
use crate::shared::labels::ServerInputExt as _;

/// The title and description a settings row or block opens with.
fn setting_header(
    cx: &App,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
) -> Div {
    v_flex()
        .gap_1()
        .child(div().text_sm().font_semibold().child(title.into()))
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().colors.muted_foreground)
                .child(description.into()),
        )
}

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
        .child(setting_header(cx, title, description))
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
        .child(setting_header(cx, title, description))
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
        })
}

pub fn event_fetching_select(cx: &App) -> Select<EventFetchingStrategy> {
    let store = SettingsStore::global(cx);
    let current = store.read(cx).settings().event_fetching;

    Select::new("settings-event-fetching")
        .options(vec![
            SelectOption::new(EventFetchingStrategy::Curated, "Curated"),
            SelectOption::new(EventFetchingStrategy::Uncensored, "Uncensored"),
        ])
        .selected(Some(current))
        .on_change(move |value, _, cx| {
            let Some(strategy) = value else {
                return;
            };

            store.update(cx, |store, cx| {
                store.edit(
                    |settings| {
                        settings.event_fetching = strategy;
                    },
                    cx,
                );
            });
        })
}

/// One editable value: a muted row with a trailing remove button.
fn editor_value_row(
    id: ElementId,
    text: SharedString,
    on_remove: impl Fn(&mut App) + 'static,
    cx: &App,
) -> Div {
    h_flex()
        .gap_1()
        .child(
            h_flex()
                .flex_1()
                .h_8()
                .px_2()
                .bg(cx.theme().colors.muted)
                .text_color(cx.theme().colors.muted_foreground)
                .text_xs()
                .rounded(cx.theme().radius.control)
                .child(text),
        )
        .child(
            Button::new(id)
                .icon(Icon::named(IconRole::Close))
                .variant(ButtonVariant::Ghost)
                .tooltip(Tooltip::text("Remove"))
                .on_click(move |_, _, cx| {
                    on_remove(cx);
                }),
        )
}

pub fn grasp_servers_editor(
    input: &Entity<InputState>,
    servers: &[ServerRow],
    cx: &App,
) -> AnyElement {
    v_flex()
        .gap_2()
        .children(servers.iter().map(|server| {
            let url = server.url.clone();

            editor_value_row(
                server.id.clone(),
                server.host.clone(),
                move |cx| {
                    let store = SettingsStore::global(cx);
                    store.update(cx, |store, cx| {
                        store.edit(
                            |settings| {
                                settings
                                    .grasp_servers
                                    .default_servers
                                    .retain(|known| known != &url);
                            },
                            cx,
                        );
                    });
                },
                cx,
            )
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
                        .variant(ButtonVariant::Ghost)
                        .tooltip(Tooltip::text("Add grasp server"))
                        .on_click({
                            let input = input.clone();
                            move |_, window, cx| {
                                add_server(&input, window, cx);
                            }
                        }),
                ),
        )
        .into_any_element()
}

pub fn scan_paths_editor(
    this: &WeakEntity<SettingsDialog>,
    scan_paths: &[PathRow],
    cx: &App,
) -> AnyElement {
    v_flex()
        .gap_2()
        .children(scan_paths.iter().map(|row| {
            let path = row.path.clone();
            editor_value_row(
                row.id.clone(),
                row.label.clone(),
                move |cx| {
                    let store = SettingsStore::global(cx);
                    store.update(cx, |store, cx| {
                        store.edit(
                            |settings| {
                                settings
                                    .local_repos
                                    .scan_paths
                                    .retain(|known| known != &path);
                            },
                            cx,
                        );
                    });
                },
                cx,
            )
        }))
        .child(
            h_flex().justify_end().child(
                Button::new("settings-add-path")
                    .icon(Icon::named(IconRole::Plus))
                    .label("Add directory")
                    .on_click({
                        let this = this.clone();
                        move |_, _, cx| {
                            add_scan_path(&this, cx);
                        }
                    }),
            ),
        )
        .into_any_element()
}

pub fn default_folder_selector(
    default_folder: &Entity<InputState>,
    this: &WeakEntity<SettingsDialog>,
) -> AnyElement {
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
                .variant(ButtonVariant::Ghost)
                .tooltip(Tooltip::text("Choose folder"))
                .on_click({
                    let this = this.clone();
                    move |_, window, cx| choose_default_folder(&folder, &this, window, cx)
                }),
        )
        .into_any_element()
}

fn add_server(input: &Entity<InputState>, window: &mut Window, cx: &mut App) {
    let settings = SettingsStore::global(cx);
    let value = input.read(cx).value().trim().to_owned();

    if value.is_empty() {
        return;
    }

    let Some((normalized, _)) = value.normalize_server() else {
        return;
    };

    settings.update(cx, |store, cx| {
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

fn add_scan_path(this: &WeakEntity<SettingsDialog>, cx: &mut App) {
    let prompt = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: true,
        prompt: Some("Choose directories".into()),
    });

    let task = cx.spawn(async move |cx| {
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
    });

    this.update(cx, |dialog, _cx| {
        dialog.tasks.push(task);
    })
    .ok();
}

fn choose_default_folder(
    default_folder: &Entity<InputState>,
    this: &WeakEntity<SettingsDialog>,
    window: &mut Window,
    cx: &mut App,
) {
    let handle = window.window_handle();
    let default_folder = default_folder.clone();

    let prompt = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: Some("Choose default folder".into()),
    });

    let task = cx.spawn(async move |cx| {
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
    });

    this.update(cx, |dialog, _cx| {
        dialog.tasks.push(task);
    })
    .ok();
}
