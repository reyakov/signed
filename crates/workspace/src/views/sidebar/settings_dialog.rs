use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use gpui::prelude::*;
use gpui::{
    AnyWindowHandle, App, Entity, PathPromptOptions, SharedString, Subscription, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{
    Input, InputEvent, InputState, NumberInput, NumberInputEvent, StepAction,
};
use gpui_component::select::{Select, SelectEvent, SelectState};
use gpui_component::separator::Separator;
use gpui_component::setting::NumberFieldOptions;
use gpui_component::switch::Switch;
use gpui_component::{
    ActiveTheme, IconName, IndexPath, Sizable, Theme, ThemeMode, ThemeRegistry, WindowExt, h_flex,
    v_flex,
};
use settings::{AppearanceMode, Settings, SettingsStore};
use signed_ui::{SelectOption, setting_block, setting_row};

use super::{normalize_server, server_host};

/// Looks up the option index used to seed a [`SelectState`].
fn selected_index(options: &[SelectOption], value: &str) -> Option<IndexPath> {
    options
        .iter()
        .position(|option| option.value().as_ref() == value)
        .map(|row| IndexPath::default().row(row))
}

/// Registered themes split into light and dark options, light first.
fn theme_options(cx: &App) -> (Vec<SelectOption>, Vec<SelectOption>) {
    let registry = ThemeRegistry::global(cx);
    let mut light = Vec::new();
    let mut dark = Vec::new();
    for config in registry.themes().values() {
        let name = config.name.clone();
        if config.mode.is_dark() {
            dark.push(SelectOption::new(name.clone(), name));
        } else {
            light.push(SelectOption::new(name.clone(), name));
        }
    }
    light.sort_by(|a, b| a.label().as_ref().cmp(b.label().as_ref()));
    dark.sort_by(|a, b| a.label().as_ref().cmp(b.label().as_ref()));
    (light, dark)
}

/// Created once when the dialog opens, so control state survives re-renders.
struct SettingsControls {
    appearance: Entity<SelectState<Vec<SelectOption>>>,
    light_theme: Entity<SelectState<Vec<SelectOption>>>,
    dark_theme: Entity<SelectState<Vec<SelectOption>>>,
    font_size: Entity<InputState>,
    mono_font_size: Entity<InputState>,
    radius: Entity<InputState>,
    radius_lg: Entity<InputState>,
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

        let (light_options, dark_options) = theme_options(cx);
        let light_theme = cx.new(|cx| {
            SelectState::new(
                light_options.clone(),
                selected_index(&light_options, &settings.theme.light_theme),
                window,
                cx,
            )
        });
        let dark_theme = cx.new(|cx| {
            SelectState::new(
                dark_options.clone(),
                selected_index(&dark_options, &settings.theme.dark_theme),
                window,
                cx,
            )
        });

        let font_size = cx.new(|cx| {
            InputState::new(window, cx).default_value(settings.theme.font_size.to_string())
        });
        let mono_font_size = cx.new(|cx| {
            InputState::new(window, cx).default_value(settings.theme.mono_font_size.to_string())
        });
        let radius = cx
            .new(|cx| InputState::new(window, cx).default_value(settings.theme.radius.to_string()));
        let radius_lg = cx.new(|cx| {
            InputState::new(window, cx).default_value(settings.theme.radius_lg.to_string())
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

        subscriptions.push(cx.subscribe(&light_theme, |_, event, cx| {
            if let SelectEvent::Confirm(Some(value)) = event {
                let store = SettingsStore::global(cx);
                store.update(cx, |store, cx| {
                    store.edit(
                        |settings| settings.theme.light_theme = value.to_string(),
                        cx,
                    );
                });
                apply_theme(cx);
            }
        }));

        subscriptions.push(cx.subscribe(&dark_theme, |_, event, cx| {
            if let SelectEvent::Confirm(Some(value)) = event {
                let store = SettingsStore::global(cx);
                store.update(cx, |store, cx| {
                    store.edit(|settings| settings.theme.dark_theme = value.to_string(), cx);
                });
                apply_theme(cx);
            }
        }));

        wire_number_input(
            &font_size,
            &mut subscriptions,
            window.window_handle(),
            cx,
            NumberFieldOptions {
                min: 10.,
                max: 32.,
                ..Default::default()
            },
            settings.theme.font_size as f64,
            |value, cx| {
                let store = SettingsStore::global(cx);
                store.update(cx, |store, cx| {
                    store.edit(|settings| settings.theme.font_size = value as f32, cx);
                });
                apply_theme(cx);
            },
        );

        wire_number_input(
            &mono_font_size,
            &mut subscriptions,
            window.window_handle(),
            cx,
            NumberFieldOptions {
                min: 8.,
                max: 32.,
                ..Default::default()
            },
            settings.theme.mono_font_size as f64,
            |value, cx| {
                let store = SettingsStore::global(cx);
                store.update(cx, |store, cx| {
                    store.edit(|settings| settings.theme.mono_font_size = value as f32, cx);
                });
                apply_theme(cx);
            },
        );

        wire_number_input(
            &radius,
            &mut subscriptions,
            window.window_handle(),
            cx,
            NumberFieldOptions {
                min: 0.,
                max: 24.,
                ..Default::default()
            },
            settings.theme.radius as f64,
            |value, cx| {
                let store = SettingsStore::global(cx);
                store.update(cx, |store, cx| {
                    store.edit(|settings| settings.theme.radius = value as f32, cx);
                });
                apply_theme(cx);
            },
        );

        wire_number_input(
            &radius_lg,
            &mut subscriptions,
            window.window_handle(),
            cx,
            NumberFieldOptions {
                min: 0.,
                max: 24.,
                ..Default::default()
            },
            settings.theme.radius_lg as f64,
            |value, cx| {
                let store = SettingsStore::global(cx);
                store.update(cx, |store, cx| {
                    store.edit(|settings| settings.theme.radius_lg = value as f32, cx);
                });
                apply_theme(cx);
            },
        );

        Self {
            appearance,
            light_theme,
            dark_theme,
            font_size,
            mono_font_size,
            radius,
            radius_lg,
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
        .child(theme_section(&settings, controls, cx))
        .child(Separator::horizontal())
        .child(grasp_servers_section(&settings, controls, cx))
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

fn theme_section(settings: &Settings, controls: &SettingsControls, cx: &App) -> impl IntoElement {
    v_flex()
        .gap_3()
        .w_full()
        .child(setting_row(
            cx,
            "Light Theme",
            "The theme to use when the appearance is light.",
            Select::new(&controls.light_theme).w_full(),
        ))
        .child(setting_row(
            cx,
            "Dark Theme",
            "The theme to use when the appearance is dark.",
            Select::new(&controls.dark_theme).w_full(),
        ))
        .child(setting_row(
            cx,
            "UI Font Size",
            "Font size for the UI.",
            NumberInput::new(&controls.font_size).w_full(),
        ))
        .child(setting_row(
            cx,
            "Editor Font Size",
            "Font size for editor text.",
            NumberInput::new(&controls.mono_font_size).w_full(),
        ))
        .child(setting_row(
            cx,
            "Corner Radius",
            "The corner radius for UI elements.",
            NumberInput::new(&controls.radius).w_full(),
        ))
        .child(setting_row(
            cx,
            "Large Corner Radius",
            "The corner radius for large UI elements (dialogs, notifications).",
            NumberInput::new(&controls.radius_lg).w_full(),
        ))
        .child(setting_row(
            cx,
            "Focus Ring",
            "Draw a ring around focused controls.",
            Switch::new("focus-ring")
                .checked(settings.theme.focus_ring)
                .on_click(move |checked: &bool, _window, cx| {
                    let store = SettingsStore::global(cx);
                    store.update(cx, |store, cx| {
                        store.edit(|settings| settings.theme.focus_ring = *checked, cx);
                    });
                    apply_theme(cx);
                }),
        ))
        .child(setting_row(
            cx,
            "Shadows",
            "The shadow effect for UI elements.",
            Switch::new("shadows")
                .checked(settings.theme.shadow)
                .on_click(move |checked: &bool, _window, cx| {
                    let store = SettingsStore::global(cx);
                    store.update(cx, |store, cx| {
                        store.edit(|settings| settings.theme.shadow = *checked, cx);
                    });
                    apply_theme(cx);
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

/// Step actions clamp and persist the value; typed changes parse, clamp and persist.
fn wire_number_input(
    state: &Entity<InputState>,
    subscriptions: &mut Vec<Subscription>,
    window_handle: AnyWindowHandle,
    cx: &mut App,
    options: NumberFieldOptions,
    initial: f64,
    on_change: impl Fn(f64, &mut App) + 'static,
) {
    let on_change = Rc::new(on_change);
    let initial_value = Rc::new(Cell::new(initial));
    let (min, max, step) = (options.min, options.max, options.step);

    subscriptions.push(cx.subscribe(&state.clone(), {
        let state = state.clone();
        let initial_value = initial_value.clone();
        let on_change = on_change.clone();
        move |_, event: &NumberInputEvent, cx| {
            let NumberInputEvent::Step(action) = event;
            let value = state.read(cx).value();
            if let Ok(value) = value.parse::<f64>() {
                let new_value = match action {
                    StepAction::Increment => value + step,
                    StepAction::Decrement => value - step,
                };
                let clamped = new_value.clamp(min, max);
                window_handle
                    .update(cx, |_, window, cx| {
                        state.update(cx, |input, cx| {
                            input.set_value(SharedString::from(clamped.to_string()), window, cx);
                        });
                    })
                    .ok();
                initial_value.set(clamped);
                on_change(clamped, cx);
            }
        }
    }));

    subscriptions.push(cx.subscribe(&state.clone(), {
        let state = state.clone();
        let initial_value = initial_value.clone();
        let on_change = on_change.clone();
        move |_, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                let value = state.read(cx).value();
                if value == initial_value.get().to_string() {
                    return;
                }
                if let Ok(parsed) = value.parse::<f64>() {
                    let clamped = parsed.clamp(min, max);
                    initial_value.set(clamped);
                    on_change(clamped, cx);
                    if clamped != parsed {
                        window_handle
                            .update(cx, |_, window, cx| {
                                state.update(cx, |input, cx| {
                                    input.set_value(
                                        SharedString::from(clamped.to_string()),
                                        window,
                                        cx,
                                    );
                                });
                            })
                            .ok();
                    }
                }
            }
        }
    }));
}

fn apply_appearance(appearance: AppearanceMode, cx: &mut App) {
    match appearance {
        AppearanceMode::System => Theme::sync_system_appearance(None, cx),
        AppearanceMode::Light => Theme::change(ThemeMode::Light, None, cx),
        AppearanceMode::Dark => Theme::change(ThemeMode::Dark, None, cx),
    }
}

fn apply_theme(cx: &mut App) {
    let store = SettingsStore::global(cx);
    let settings = store.read(cx).settings().theme.clone();

    let registry = ThemeRegistry::global(cx);
    let light_config = registry
        .themes()
        .get(settings.light_theme.as_str())
        .cloned();
    let dark_config = registry.themes().get(settings.dark_theme.as_str()).cloned();
    let mode = Theme::global(cx).mode;

    let theme = Theme::global_mut(cx);
    theme.radius = px(settings.radius);
    theme.radius_lg = px(settings.radius_lg);
    theme.focus_ring = settings.focus_ring;
    theme.shadow = settings.shadow;
    theme.font_size = px(settings.font_size);
    theme.mono_font_size = px(settings.mono_font_size);
    if let Some(config) = light_config {
        theme.light_theme = config;
    }
    if let Some(config) = dark_config {
        theme.dark_theme = config;
    }

    // Re-apply the active mode so the updated configs take effect.
    Theme::change(mode, None, cx);
}
