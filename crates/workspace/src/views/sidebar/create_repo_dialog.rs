use dock::{DockArea, DockPlacement, panel_handle};
use gpui::prelude::*;
use gpui::{App, Entity, PathPromptOptions, SharedString, WeakEntity, Window, div, px};
use gpui_base::input::TextareaState;
use gpui_component::button::{Button, ButtonVariants, Toggle, ToggleVariants};
use gpui_component::dialog::{DialogDescription, DialogFooter, DialogHeader, DialogTitle};
use gpui_component::form::{field, v_form};
use gpui_component::input::{Input, InputState, Textarea};
use gpui_component::{ActiveTheme, Disableable, IconName, Sizable, WindowExt, h_flex, v_flex};
use nostr::prelude::*;
use signed_core::{Announcement, filters};
use signed_state::Backend;

use super::super::RepoDetailView;

/// Grasp servers offered when the user hasn't published a grasp list (kind `10317`) yet.
const DEFAULT_GRASP_SERVERS: [&str; 3] = [
    "wss://relay.ngit.dev",
    "wss://gitnostr.com",
    "wss://git.shakespeare.diy",
];

/// Shared state for the Create Repository dialog, so async results can be rendered.
#[derive(Default)]
pub struct CreateRepoState {
    pub busy: bool,
    /// The user's grasp list (kind `10317`) is being loaded.
    pub loading_servers: bool,
    pub error: Option<SharedString>,
    pub grasp_servers: Vec<RelayUrl>,
    /// Whether the grasp server section is shown; defaults to shown.
    pub servers_enabled: bool,
}

impl CreateRepoState {
    /// Defaults until the user's grasp list arrives; replaced by it when it lists any servers.
    fn new_default() -> Self {
        Self {
            loading_servers: true,
            servers_enabled: false,
            grasp_servers: DEFAULT_GRASP_SERVERS
                .iter()
                .filter_map(|url| RelayUrl::parse(url).ok())
                .collect(),
            ..Default::default()
        }
    }
}

/// Open the Create Repository dialog.
///
/// The dialog loads the user's default grasp servers (kind `10317` grasp
/// list) and falls back to [`DEFAULT_GRASP_SERVERS`] when none are set.
/// On success the dialog closes and the new repository opens in the dock.
pub fn open(dock_area: WeakEntity<DockArea>, window: &mut Window, cx: &mut App) {
    let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("Repository name"));
    let desc_input = cx.new(|cx| {
        TextareaState::new(window, cx)
            .auto_grow(3, 5)
            .placeholder("Short description")
    });
    let folder_input = cx.new(|cx| {
        InputState::new(window, cx)
            .default_value(paths::desktop_dir().to_string_lossy().to_string())
    });
    let relay_input = cx.new(|cx| {
        InputState::new(window, cx).placeholder("wss://relay.example.com or relay.example.com")
    });
    let state = cx.new(|_| CreateRepoState::new_default());

    load_user_grasp_servers(state.clone(), window, cx);

    window.open_dialog(cx, move |dialog, _window, _cx| {
        const DESC: &str = "Publish a new repository to your grasp servers.";
        const FOLDER_NOTE: &str = "Where the repository is stored, defaults to your Desktop";
        const SERVER_NOTE: &str =
            "Where the repository is hosted, the initial push goes to each server";

        let name_input = name_input.clone();
        let desc_input = desc_input.clone();
        let folder_input = folder_input.clone();
        let relay_input = relay_input.clone();
        let state = state.clone();
        let dock_area = dock_area.clone();

        dialog
            .width(px(520.))
            .margin_top(px(50.))
            .content(move |content, _window, cx| {
                let busy = state.read(cx).busy;
                let error = state.read(cx).error.clone();
                let servers = state.read(cx).grasp_servers.clone();
                let loading_servers = state.read(cx).loading_servers;
                let servers_enabled = state.read(cx).servers_enabled;

                content
                    .child(
                        DialogHeader::new()
                            .child(DialogTitle::new().child("Create repository"))
                            .child(DialogDescription::new().child(DESC)),
                    )
                    .child(
                        v_form()
                            .child(
                                field()
                                    .label("Repository name")
                                    .description("Max 100 characters")
                                    .required(true)
                                    .child(Input::new(&name_input)),
                            )
                            .child(
                                field()
                                    .label("Description")
                                    .child(Textarea::new(&desc_input)),
                            )
                            .child(
                                field().label("Folder").description(FOLDER_NOTE).child(
                                    h_flex()
                                        .gap_1()
                                        .items_center()
                                        .child(
                                            div()
                                                .flex_1()
                                                .child(Input::new(&folder_input).disabled(true)),
                                        )
                                        .child(
                                            Button::new("choose-folder")
                                                .icon(IconName::FolderOpen)
                                                .ghost()
                                                .tooltip("Choose folder")
                                                .on_click({
                                                    let folder_input = folder_input.clone();
                                                    move |_ev, window, cx| {
                                                        choose_folder(&folder_input, window, cx);
                                                    }
                                                }),
                                        ),
                                ),
                            )
                            .child(
                                field()
                                    .label_fn({
                                        let state = state.clone();
                                        move |_window, cx| {
                                            let enabled = state.read(cx).servers_enabled;
                                            h_flex()
                                                .w_full()
                                                .justify_between()
                                                .items_center()
                                                .gap_1()
                                                .child(
                                                    Toggle::new("grasp-servers-toggle")
                                                        .xsmall()
                                                        .ghost()
                                                        .icon({
                                                            if enabled {
                                                                IconName::ChevronDown
                                                            } else {
                                                                IconName::ChevronUp
                                                            }
                                                        })
                                                        .checked(enabled)
                                                        .on_click({
                                                            let state = state.clone();
                                                            move |checked, _window, cx| {
                                                                state.update(cx, |state, cx| {
                                                                    state.servers_enabled =
                                                                        *checked;
                                                                    cx.notify();
                                                                });
                                                            }
                                                        }),
                                                )
                                                .child(div().child("Grasp servers"))
                                        }
                                    })
                                    .when(servers_enabled, |this| this.description(SERVER_NOTE))
                                    .child(v_flex().gap_1().when(servers_enabled, |this| {
                                        this.children(servers.iter().enumerate().map(
                                            |(ix, relay)| {
                                                render_server_row(ix, relay, state.clone(), cx)
                                            },
                                        ))
                                        .child(
                                            h_flex()
                                                .gap_1()
                                                .items_center()
                                                .child(
                                                    div().flex_1().child(Input::new(&relay_input)),
                                                )
                                                .child(
                                                    Button::new("add-relay")
                                                        .icon(IconName::Plus)
                                                        .ghost()
                                                        .tooltip("Add grasp server")
                                                        .on_click({
                                                            let state = state.clone();
                                                            let relay_input = relay_input.clone();
                                                            move |_ev, window, cx| {
                                                                add_relay(
                                                                    &state,
                                                                    &relay_input,
                                                                    window,
                                                                    cx,
                                                                );
                                                            }
                                                        }),
                                                ),
                                        )
                                        .when(
                                            loading_servers,
                                            |this| {
                                                this.child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(cx.theme().muted_foreground)
                                                        .child("Loading your grasp servers..."),
                                                )
                                            },
                                        )
                                    })),
                            ),
                    )
                    .children(error.map(|message| {
                        div().text_sm().text_color(cx.theme().danger).child(message)
                    }))
                    .child(
                        DialogFooter::new().justify_end().child(
                            Button::new("create")
                                .primary()
                                .label("Create repository")
                                .icon(IconName::ArrowRight)
                                .tooltip("Create repository")
                                .loading(busy)
                                .disabled(busy)
                                .on_click({
                                    let name_input = name_input.clone();
                                    let desc_input = desc_input.clone();
                                    let state = state.clone();
                                    let dock_area = dock_area.clone();

                                    move |_ev, window, cx| {
                                        create_repository(
                                            name_input.clone(),
                                            desc_input.clone(),
                                            state.clone(),
                                            dock_area.clone(),
                                            window,
                                            cx,
                                        );
                                    }
                                }),
                        ),
                    )
            })
    });
}

/// A grasp server row: the host as a tag plus a remove button.
fn render_server_row(
    ix: usize,
    relay: &RelayUrl,
    state: Entity<CreateRepoState>,
    cx: &App,
) -> impl IntoElement {
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
                .child(display_server(relay)),
        )
        .child(
            Button::new(format!("remove-relay:{ix}"))
                .icon(IconName::Close)
                .ghost()
                .flex_shrink_0()
                .tooltip("Remove")
                .on_click({
                    let state = state.clone();
                    move |_ev, _window, cx| {
                        state.update(cx, |state, _| {
                            state.grasp_servers.remove(ix);
                        });
                    }
                }),
        )
}

/// The bare host of a grasp server (defaults are entered without a scheme).
fn display_server(relay: &RelayUrl) -> SharedString {
    relay
        .domain()
        .map(SharedString::from)
        .unwrap_or_else(|| SharedString::from(relay.to_string()))
}

/// Prompt the user to pick the folder the repository will be stored in, using
/// the platform's native folder picker, and show the result in the disabled
/// folder input.
fn choose_folder(folder_input: &Entity<InputState>, window: &mut Window, cx: &mut App) {
    let handle = window.window_handle();
    let folder_input = folder_input.clone();

    let prompt = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: Some("Choose folder".into()),
    });

    cx.spawn(async move |cx| {
        if let Ok(Ok(Some(mut paths))) = prompt.await
            && let Some(path) = paths.pop()
        {
            let path = path.to_string_lossy().to_string();
            cx.update_window(handle, |_, window, cx| {
                folder_input.update(cx, |input, cx| input.set_value(path, window, cx));
            })
            .ok();
        }
    })
    .detach();
}

/// Parse the relay input (accepting a bare host) and append it to the list.
fn add_relay(
    state: &Entity<CreateRepoState>,
    input: &Entity<InputState>,
    window: &mut Window,
    cx: &mut App,
) {
    let value = input.read(cx).value().trim().to_owned();
    if value.is_empty() {
        return;
    }

    let normalized = if value.contains("://") {
        value.clone()
    } else {
        format!("wss://{value}")
    };

    match RelayUrl::parse(&normalized) {
        Ok(relay) => {
            state.update(cx, |state, _| {
                state.error = None;
                if !state.grasp_servers.contains(&relay) {
                    state.grasp_servers.push(relay);
                }
            });
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
        Err(_) => {
            state.update(cx, |state, _| {
                state.error = Some(format!("Invalid grasp server URL: {value}").into());
            });
        }
    }
}

/// Run the create-repository flow; closes the dialog and opens the new repository on success.
fn create_repository(
    name_input: Entity<InputState>,
    desc_input: Entity<TextareaState>,
    state: Entity<CreateRepoState>,
    dock_area: WeakEntity<DockArea>,
    window: &mut Window,
    cx: &mut App,
) {
    let name = name_input.read(cx).value().trim().to_owned();
    let description = desc_input.read(cx).value().trim().to_owned();
    let servers = state.read(cx).grasp_servers.clone();

    if name.is_empty() {
        state.update(cx, |state, _| {
            state.error = Some("Repository name is required".into());
        });
        return;
    }
    if servers.is_empty() {
        state.update(cx, |state, _| {
            state.error = Some("Add at least one grasp server".into());
        });
        return;
    }

    state.update(cx, |state, _| {
        state.busy = true;
        state.error = None;
    });

    let backend = Backend::global(cx);
    let task = backend.update(cx, |backend, cx| {
        backend.create_repository(&name, &description, servers, cx)
    });
    let handle = window.window_handle();
    let state = state.clone();
    let dock_area = dock_area.clone();

    cx.spawn(async move |cx| match task.await {
        Ok(announcement) => {
            cx.update_window(handle, |_, window, cx| {
                window.close_dialog(cx);
                open_repo(dock_area, announcement, window, cx);
            })
            .ok();
        }
        Err(e) => {
            cx.update_window(handle, |_, _window, cx| {
                state.update(cx, |state, _| {
                    state.busy = false;
                    state.error = Some(e.to_string().into());
                });
            })
            .ok();
        }
    })
    .detach();
}

/// Open the newly created repository in the dock's center.
fn open_repo(
    dock_area: WeakEntity<DockArea>,
    announcement: Announcement,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(dock_area) = dock_area.upgrade() else {
        return;
    };

    let panel = cx.new(|cx| RepoDetailView::new(dock_area.downgrade(), announcement, window, cx));

    dock_area.update(cx, |dock_area, cx| {
        dock_area.add_panel_view(panel_handle(panel), DockPlacement::Center, None, window, cx);
    });
}

/// Load the user's grasp list (kind `10317`) from the local database and
/// replace the defaults with it when it lists any servers.
fn load_user_grasp_servers(state: Entity<CreateRepoState>, window: &mut Window, cx: &mut App) {
    let backend = Backend::global(cx);
    let Some(user) = backend.read(cx).current_user() else {
        state.update(cx, |state, _| state.loading_servers = false);
        return;
    };
    let client = backend.read(cx).client();
    let handle = window.window_handle();

    cx.spawn(async move |cx| {
        let result: anyhow::Result<Vec<RelayUrl>> = async {
            let mut events: Vec<Event> = client
                .database()
                .query(filters::grasp_list(user))
                .await?
                .into_iter()
                .collect();
            events.sort_by_key(|event| event.created_at);

            Ok(events
                .into_iter()
                .last()
                .map(|event| {
                    event
                        .tags
                        .iter()
                        .filter(|tag| tag.kind() == "g")
                        .filter_map(|tag| tag.content())
                        .filter_map(|url| RelayUrl::parse(url).ok())
                        .collect()
                })
                .unwrap_or_default())
        }
        .await;

        let _ = cx.update_window(handle, |_, _window, cx| {
            state.update(cx, |state, _| {
                state.loading_servers = false;
                if let Ok(servers) = result
                    && !servers.is_empty()
                {
                    state.grasp_servers = servers;
                }
            });
        });
    })
    .detach();
}
