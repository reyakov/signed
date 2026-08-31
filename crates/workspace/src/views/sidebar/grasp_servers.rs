use gpui::prelude::*;
use gpui::{App, Entity, SharedString, Window, div};
use gpui_component::button::{Button, ButtonVariants, Toggle, ToggleVariants};
use gpui_component::form::{Field, field};
use gpui_component::input::{Input, InputState};
use gpui_component::{ActiveTheme, IconName, Sizable, h_flex, v_flex};
use nostr::prelude::*;
use signed_core::filters;
use signed_state::Backend;

/// Grasp servers offered when the user hasn't published a grasp list (kind `10317`) yet.
const DEFAULT_GRASP_SERVERS: [&str; 3] = [
    "wss://relay.ngit.dev",
    "wss://gitnostr.com",
    "wss://git.shakespeare.diy",
];

/// State of the grasp-server section of a publish dialog, so async
/// results can be rendered.
#[derive(Default)]
pub struct GraspServersState {
    /// The user's grasp list (kind `10317`) is being loaded.
    pub loading_servers: bool,
    pub grasp_servers: Vec<RelayUrl>,
    /// Whether the grasp server section is shown; defaults to shown.
    pub servers_enabled: bool,
    /// Error of the last grasp-server edit (e.g. an invalid relay URL).
    pub error: Option<SharedString>,
}

impl GraspServersState {
    /// Defaults until the user's grasp list arrives; replaced by it when it lists any servers.
    pub fn new_default() -> Self {
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

/// The "Grasp servers" form field shared by the publish dialogs: an
/// expandable toggle, the configured servers (each removable) and an
/// add-relay input, with a loading hint while the user's grasp list
/// (kind `10317`) is being fetched.
pub fn grasp_servers_field(
    state: &Entity<GraspServersState>,
    relay_input: &Entity<InputState>,
    cx: &App,
) -> Field {
    const SERVER_NOTE: &str =
        "Where the repository is hosted, the initial push goes to each server";

    let state = state.clone();
    let relay_input = relay_input.clone();

    let servers = state.read(cx).grasp_servers.clone();
    let loading_servers = state.read(cx).loading_servers;
    let servers_enabled = state.read(cx).servers_enabled;
    let error = state.read(cx).error.clone();

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
                                        state.servers_enabled = *checked;
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
            this.children(
                servers
                    .iter()
                    .enumerate()
                    .map(|(ix, relay)| render_server_row(ix, relay, state.clone(), cx)),
            )
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(div().flex_1().child(Input::new(&relay_input)))
                    .child(
                        Button::new("add-relay")
                            .icon(IconName::Plus)
                            .ghost()
                            .tooltip("Add grasp server")
                            .on_click({
                                let state = state.clone();
                                let relay_input = relay_input.clone();
                                move |_ev, window, cx| {
                                    add_relay(&state, &relay_input, window, cx);
                                }
                            }),
                    ),
            )
            .when(loading_servers, |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("Loading your grasp servers..."),
                )
            })
            .when_some(error, |this, error| {
                this.child(div().text_xs().text_color(cx.theme().danger).child(error))
            })
        }))
}

/// A grasp server row: the host as a tag plus a remove button.
fn render_server_row(
    ix: usize,
    relay: &RelayUrl,
    state: Entity<GraspServersState>,
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

/// Parse the relay input (accepting a bare host) and append it to the list.
fn add_relay(
    state: &Entity<GraspServersState>,
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

/// Load the user's grasp list (kind `10317`) from the local database and
/// replace the defaults with it when it lists any servers.
pub fn load_user_grasp_servers(
    state: Entity<GraspServersState>,
    window: &mut Window,
    cx: &mut App,
) {
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
