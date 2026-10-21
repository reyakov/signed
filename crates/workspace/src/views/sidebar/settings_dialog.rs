use std::path::PathBuf;
use std::rc::Rc;

use gpui::prelude::*;
use gpui::{App, Entity, PathPromptOptions, SharedString, Subscription, Window, div, px};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputState};
use gpui_component::select::{Select, SelectEvent, SelectState};
use gpui_component::separator::Separator;
use gpui_component::switch::Switch;
use gpui_component::{
    ActiveTheme, IconName, IndexPath, Sizable, Theme, ThemeMode, WindowExt, h_flex, v_flex,
};
use settings::{AppearanceMode, EventFetchingStrategy, Settings, SettingsStore};
use signed_ui::{SelectOption, setting_block, setting_row};

use super::{normalize_server, server_host};

/// Looks up the option index used to seed a [`SelectState`].
fn selected_index(options: &[SelectOption], value: &str) -> Option<IndexPath> {
    options
        .iter()
        .position(|option| option.value().as_ref() == value)
        .map(|row| IndexPath::default().row(row))
}

/// Created once when the dialog opens, so control state survives re-renders.
struct SettingsControls {
    appearance: Entity<SelectState<Vec<SelectOption>>>,
    event_fetching: Entity<SelectState<Vec<SelectOption>>>,
    grasp_server_input: Entity<InputState>,
    /// The effective create-repository folder, shown in a disabled input.
    default_folder: Entity<InputState>,
    /// Keeps the control subscriptions alive while the dialog is open.
    _subscriptions: Vec<Subscription>,
}

impl SettingsControls {
    fn new(window: &mut Window, cx: &mut App) -> Self {
        let store = SettingsStore::global(cx);
        let settings = store.read(cx).settings().clone();

        let appearance_options = vec![
            SelectOption::new("system", "System"),
            SelectOption::new("light", "Light"),
            SelectOption::new("dark", "Dark"),
        ];
        let appearance_value = match settings.appearance {
            AppearanceMode::System => "system",
            AppearanceMode::Light => "light",
            AppearanceMode::Dark => "dark",
        };
        let appearance = cx.new(|cx| {
            SelectState::new(
                appearance_options.clone(),
                selected_index(&appearance_options, appearance_value),
                window,
                cx,
            )
        });

        let event_fetching_options = vec![
            SelectOption::new("curated", "Curated"),
            SelectOption::new("uncensored", "Uncensored"),
        ];
        let event_fetching_value = match settings.event_fetching {
            EventFetchingStrategy::Curated => "curated",
            EventFetchingStrategy::Uncensored => "uncensored",
        };
        let event_fetching = cx.new(|cx| {
            SelectState::new(
                event_fetching_options.clone(),
                selected_index(&event_fetching_options, event_fetching_value),
                window,
                cx,
            )
        });

        let grasp_server_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("wss://relay.example.com"));

        let default_folder = cx.new(|cx| {
            let folder = settings
                .create_repository
                .default_folder
                .clone()
                .unwrap_or_else(paths::desktop_dir);
            InputState::new(window, cx).default_value(folder.to_string_lossy().to_string())
        });

        let mut subscriptions = Vec::new();

        subscriptions.push(cx.subscribe(&appearance, |_, event, cx| {
            if let SelectEvent::Confirm(Some(value)) = event {
                let appearance = match value.as_ref() {
                    "light" => AppearanceMode::Light,
                    "dark" => AppearanceMode::Dark,
                    _ => AppearanceMode::System,
                };
                let store = SettingsStore::global(cx);
                store.update(cx, |store, cx| {
                    store.edit(|settings| settings.appearance = appearance, cx);
                });
                apply_appearance(appearance, cx);
            }
        }));

        subscriptions.push(cx.subscribe(&event_fetching, |_, event, cx| {
            if let SelectEvent::Confirm(Some(value)) = event {
                let strategy = match value.as_ref() {
                    "curated" => EventFetchingStrategy::Curated,
                    _ => EventFetchingStrategy::Uncensored,
                };
                let store = SettingsStore::global(cx);
                store.update(cx, |store, cx| {
                    store.edit(|settings| settings.event_fetching = strategy, cx);
                });
            }
        }));

        Self {
            appearance,
            event_fetching,
            grasp_server_input,
            default_folder,
            _subscriptions: subscriptions,
        }
    }
}

pub fn open(window: &mut Window, cx: &mut App) {
    let controls = Rc::new(SettingsControls::new(window, cx));

    window.open_dialog(cx, move |dialog, _window, cx| {
        let controls = controls.clone();
        dialog
            .title("Settings")
            .width(px(650.))
            .h(px(560.))
            .child(settings_view(&controls, cx))
    });
}

fn settings_view(controls: &SettingsControls, cx: &mut App) -> impl IntoElement {
    let store = SettingsStore::global(cx);
    let settings = store.read(cx).settings().clone();

    v_flex()
        .mt_2()
        .gap_4()
        .w_full()
        .child(appearance_section(controls, cx))
        .child(Separator::horizontal())
        .child(tab_bar_section(&settings, cx))
        .child(Separator::horizontal())
        .child(grasp_servers_section(&settings, controls, cx))
        .child(Separator::horizontal())
        .child(event_fetching_section(controls, cx))
        .child(Separator::horizontal())
        .child(repositories_section(&settings, controls, cx))
}

fn appearance_section(controls: &SettingsControls, cx: &App) -> impl IntoElement {
    v_flex().w_full().gap_3().child(setting_row(
        cx,
        "Appearance",
        "Choose whether the app follows the system theme or uses a light/dark theme.",
        Select::new(&controls.appearance).w_full(),
    ))
}

/// Which relays repository activity is fetched from.
fn event_fetching_section(controls: &SettingsControls, cx: &App) -> impl IntoElement {
    v_flex().w_full().gap_3().child(setting_row(
        cx,
        "Event Fetching Strategy",
        "Curated fetches events from relays in the repository's announcement. Uncensored also fetches from every maintainer's relays.",
        Select::new(&controls.event_fetching).w_full(),
    ))
}

/// Tab bar chrome, such as its next/previous tab buttons.
fn tab_bar_section(settings: &Settings, cx: &App) -> impl IntoElement {
    v_flex().w_full().gap_3().child(setting_row(
        cx,
        "Hide Tab Navigation Buttons",
        "Enable to show navigation buttons when more than 10 tabs.",
        Switch::new("hide-tab-navigation")
            .checked(settings.tab_bar.hide_navigation_buttons)
            .on_click(move |checked: &bool, _window, cx| {
                let store = SettingsStore::global(cx);
                store.update(cx, |store, cx| {
                    store.edit(
                        |settings| settings.tab_bar.hide_navigation_buttons = *checked,
                        cx,
                    );
                });
            }),
    ))
}

/// Default grasp servers, used until the user's kind `10317` grasp list loads.
fn grasp_servers_section(
    settings: &Settings,
    controls: &SettingsControls,
    cx: &App,
) -> impl IntoElement {
    let servers = settings.grasp_servers.default_servers.clone();

    v_flex().w_full().gap_3().child(setting_block(
        cx,
        "Grasp Servers",
        "Servers used to host your git repositories via the Grasp protocol",
        grasp_server_editor(&servers, controls, cx),
    ))
}

/// Styled like the grasp-server section of the publish dialogs.
fn grasp_server_editor(
    servers: &[String],
    controls: &SettingsControls,
    cx: &App,
) -> impl IntoElement {
    let server_input = controls.grasp_server_input.clone();

    v_flex()
        .w_full()
        .gap_2()
        .children(servers.iter().enumerate().map(|(ix, server)| {
            h_flex()
                .w_full()
                .gap_1()
                .items_center()
                .child(
                    h_flex()
                        .h_8()
                        .w_full()
                        .px_2()
                        .bg(cx.theme().muted)
                        .text_color(cx.theme().muted_foreground)
                        .text_sm()
                        .rounded(cx.theme().radius)
                        .child(display_server(server)),
                )
                .child(
                    Button::new(format!("settings-remove-server:{ix}"))
                        .icon(IconName::Close)
                        .ghost()
                        .flex_shrink_0()
                        .tooltip("Remove")
                        .on_click(move |_event, _window, cx| {
                            let store = SettingsStore::global(cx);
                            store.update(cx, |store, cx| {
                                store.edit(
                                    |settings| {
                                        settings.grasp_servers.default_servers.remove(ix);
                                    },
                                    cx,
                                );
                            });
                        }),
                )
        }))
        .child(
            h_flex()
                .gap_1()
                .items_center()
                .child(div().flex_1().child(Input::new(&server_input)))
                .child(
                    Button::new("settings-add-server")
                        .icon(IconName::Plus)
                        .ghost()
                        .tooltip("Add grasp server")
                        .on_click({
                            let server_input = server_input.clone();
                            move |_event, window, cx| add_server(&server_input, window, cx)
                        }),
                ),
        )
}

/// Shows only the host, since grasp servers are entered without a scheme.
fn display_server(server: &str) -> SharedString {
    match normalize_server(server) {
        Some((_, relay)) => server_host(&relay),
        None => SharedString::from(server.to_owned()),
    }
}

fn repositories_section(
    settings: &Settings,
    controls: &SettingsControls,
    cx: &App,
) -> impl IntoElement {
    let scan_paths = settings.local_repos.scan_paths.clone();

    v_flex()
        .w_full()
        .gap_3()
        .child(setting_block(
            cx,
            "Scan Directories",
            "Directories scanned for local git repositories.",
            scan_paths_editor(&scan_paths, cx),
        ))
        .child(setting_block(
            cx,
            "Default Folder",
            "The folder for newly created repositories.",
            folder_selector(controls),
        ))
}

/// Styled like the grasp-server list.
fn scan_paths_editor(scan_paths: &[PathBuf], cx: &App) -> impl IntoElement {
    v_flex()
        .w_full()
        .gap_2()
        .children(scan_paths.iter().enumerate().map(|(ix, path)| {
            h_flex()
                .w_full()
                .gap_1()
                .items_center()
                .child(
                    h_flex()
                        .h_8()
                        .w_full()
                        .px_2()
                        .bg(cx.theme().muted)
                        .text_color(cx.theme().muted_foreground)
                        .text_sm()
                        .rounded(cx.theme().radius)
                        .child(path.display().to_string()),
                )
                .child(
                    Button::new(format!("settings-remove-path:{ix}"))
                        .icon(IconName::Close)
                        .ghost()
                        .flex_shrink_0()
                        .tooltip("Remove")
                        .on_click(move |_event, _window, cx| {
                            let store = SettingsStore::global(cx);
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
        }))
        .child(
            h_flex().justify_end().items_center().child(
                Button::new("settings-add-path")
                    .icon(IconName::Plus)
                    .label("Add directory")
                    .secondary()
                    .small()
                    .on_click(move |_event, _window, cx| add_scan_path(cx)),
            ),
        )
}

/// Matches the create-repository dialog.
fn folder_selector(controls: &SettingsControls) -> impl IntoElement {
    let default_folder = controls.default_folder.clone();
    h_flex()
        .w_full()
        .gap_1()
        .items_center()
        .child(
            div()
                .flex_1()
                .child(Input::new(&default_folder).disabled(true)),
        )
        .child(
            Button::new("settings-choose-folder")
                .icon(IconName::Folder)
                .ghost()
                .tooltip("Choose folder")
                .on_click(move |_event, window, cx| {
                    choose_default_folder(&default_folder, window, cx);
                }),
        )
}

/// Accepts a bare host as well as a full URL.
fn add_server(input: &Entity<InputState>, window: &mut Window, cx: &mut App) {
    let value = input.read(cx).value().trim().to_owned();
    if value.is_empty() {
        return;
    }
    let Some((normalized, _)) = normalize_server(&value) else {
        return;
    };

    let store = SettingsStore::global(cx);
    store.update(cx, |store, cx| {
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
                                    settings.local_repos.scan_paths.push(path.into());
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

/// Persists the choice and reflects it in the disabled input.
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
            let path = path.to_string_lossy().to_string();
            cx.update_window(handle, |_, window, cx| {
                let store = SettingsStore::global(cx);
                store.update(cx, |store, cx| {
                    store.edit(
                        |settings| {
                            settings.create_repository.default_folder = Some(path.clone().into())
                        },
                        cx,
                    );
                });
                default_folder.update(cx, |input, cx| input.set_value(path, window, cx));
            })
            .ok();
        }
    })
    .detach();
}

fn apply_appearance(appearance: AppearanceMode, cx: &mut App) {
    match appearance {
        AppearanceMode::System => Theme::sync_system_appearance(None, cx),
        AppearanceMode::Light => Theme::change(ThemeMode::Light, None, cx),
        AppearanceMode::Dark => Theme::change(ThemeMode::Dark, None, cx),
    }
}
