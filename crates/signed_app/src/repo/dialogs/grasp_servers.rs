use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Entity, IntoElement, ParentElement as _, SharedString,
    Styled as _, Window, div,
};
use gpui_native_kit::button::{Button, ButtonVariant};
use gpui_native_kit::common::{h_flex, v_flex};
use gpui_native_kit::icons::{Icon, IconRole};
use gpui_native_kit::input::{Input, InputState};
use gpui_native_kit::switch::Switch;
use gpui_native_kit::theme::{ActiveTheme as _, TextRole, Typeset as _};
use gpui_native_kit::tooltip::Tooltip;
use nostr::prelude::RelayUrl;
use settings::{DEFAULT_GRASP_SERVERS, GraspServersSettings};
use signed_state::Backend;

/// State of the grasp-server section of a publish dialog, so async results can be rendered.
#[derive(Default)]
pub struct GraspServersState {
    /// Set while the user's kind `10317` grasp list loads.
    pub loading_servers: bool,
    pub grasp_servers: Vec<RelayUrl>,
    pub servers_enabled: bool,
    /// Error from the last grasp-server edit, such as an invalid relay URL.
    pub error: Option<SharedString>,
}

impl GraspServersState {
    /// Defaults used until the user's grasp list loads and replaces them.
    pub fn new_default(settings: &GraspServersSettings) -> Self {
        let urls: Vec<String> = if settings.default_servers.is_empty() {
            DEFAULT_GRASP_SERVERS
                .iter()
                .map(|url| (*url).to_owned())
                .collect()
        } else {
            settings.default_servers.clone()
        };
        Self {
            loading_servers: true,
            servers_enabled: false,
            grasp_servers: urls
                .iter()
                .filter_map(|url| RelayUrl::parse(url).ok())
                .collect(),
            ..Default::default()
        }
    }
}

/// The Grasp servers section shared by the publish dialogs.
pub fn grasp_servers_field(
    state: &Entity<GraspServersState>,
    relay_input: &Entity<InputState>,
    cx: &App,
) -> AnyElement {
    let state = state.clone();
    let relay_input = relay_input.clone();

    let servers = state.read(cx).grasp_servers.clone();
    let loading_servers = state.read(cx).loading_servers;
    let servers_enabled = state.read(cx).servers_enabled;
    let error = state.read(cx).error.clone();

    let add_state = state.clone();
    let add_input = relay_input.clone();
    let toggle_state = state.clone();

    v_flex()
        .gap_2()
        .child(
            h_flex()
                .justify_between()
                .items_center()
                .child(div().typeset(TextRole::Body, cx).child("Grasp servers"))
                .child(
                    Switch::new("grasp-servers-toggle")
                        .checked(servers_enabled)
                        .on_change(move |checked, _, cx| {
                            toggle_state.update(cx, |state, cx| {
                                state.servers_enabled = checked;
                                cx.notify();
                            });
                        }),
                ),
        )
        .when(servers_enabled, |section| {
            section
                .child(
                    div()
                        .typeset(TextRole::Caption, cx)
                        .text_color(cx.theme().colors.muted_foreground)
                        .child("Where the repository is hosted"),
                )
                .child(
                    v_flex().gap_1().children(
                        servers
                            .iter()
                            .enumerate()
                            .map(|(ix, relay)| render_server_row(ix, relay, state.clone(), cx)),
                    ),
                )
                .child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(
                            div()
                                .flex_1()
                                .child(Input::new("grasp-relay-input", relay_input.clone())),
                        )
                        .child(
                            Button::new("add-relay")
                                .icon(Icon::named(IconRole::Plus))
                                .variant(ButtonVariant::Ghost)
                                .tooltip(Tooltip::text("Add grasp server"))
                                .on_click(move |_, window, cx| {
                                    add_relay(&add_state, &add_input, window, cx);
                                }),
                        ),
                )
                .when(loading_servers, |section| {
                    section.child(
                        div()
                            .typeset(TextRole::Caption, cx)
                            .text_color(cx.theme().colors.muted_foreground)
                            .child("Loading your grasp servers..."),
                    )
                })
                .when_some(error, |section, error| {
                    section.child(
                        div()
                            .typeset(TextRole::Caption, cx)
                            .text_color(cx.theme().ramps.destructive.base)
                            .child(error),
                    )
                })
        })
        .into_any_element()
}

fn render_server_row(
    ix: usize,
    relay: &RelayUrl,
    state: Entity<GraspServersState>,
    cx: &App,
) -> AnyElement {
    h_flex()
        .w_full()
        .gap_1()
        .items_center()
        .child(
            div()
                .flex_1()
                .h_8()
                .px_2()
                .bg(cx.theme().colors.muted)
                .text_color(cx.theme().colors.muted_foreground)
                .typeset(TextRole::Body, cx)
                .rounded(cx.theme().radius.control)
                .child(server_host(relay)),
        )
        .child(
            Button::new(SharedString::from(format!("remove-relay:{ix}")))
                .icon(Icon::named(IconRole::Close))
                .variant(ButtonVariant::Ghost)
                .tooltip(Tooltip::text("Remove"))
                .on_click(move |_, _, cx| {
                    state.update(cx, |state, cx| {
                        if ix < state.grasp_servers.len() {
                            state.grasp_servers.remove(ix);
                            cx.notify();
                        }
                    });
                }),
        )
        .into_any_element()
}

/// Accepts a bare host as well as a full URL.
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

    match normalize_server(&value) {
        Some((_, relay)) => {
            state.update(cx, |state, cx| {
                state.error = None;
                if !state.grasp_servers.contains(&relay) {
                    state.grasp_servers.push(relay);
                }
                cx.notify();
            });
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
        None => {
            state.update(cx, |state, cx| {
                state.error = Some(format!("Invalid grasp server URL: {value}").into());
                cx.notify();
            });
        }
    }
}

/// Loads the user's kind `10317` grasp list from the local database.
pub fn load_user_grasp_servers(
    state: Entity<GraspServersState>,
    window: &mut Window,
    cx: &mut App,
) {
    let backend = Backend::global(cx);
    let (user, client) = {
        let backend = backend.read(cx);
        (backend.current_user(), backend.client())
    };

    let Some(user) = user else {
        state.update(cx, |state, _| state.loading_servers = false);
        return;
    };

    let task =
        cx.background_spawn(
            async move { signed_state::user_grasp_list_servers(&client, user).await },
        );
    let handle = window.window_handle();

    cx.spawn(async move |cx| {
        let result = task.await;

        if let Err(error) = cx.update_window(handle, |_, _window, cx| {
            state.update(cx, |state, cx| {
                state.loading_servers = false;
                if let Ok(servers) = result
                    && !servers.is_empty()
                {
                    state.grasp_servers = servers;
                }
                cx.notify();
            });
        }) {
            log::warn!("failed to apply grasp servers: {error}");
        }
    })
    .detach();
}

/// Normalize a user-typed grasp server, adding a `wss://` scheme when none is given.
pub fn normalize_server(input: &str) -> Option<(String, RelayUrl)> {
    let text = if input.contains("://") {
        input.to_owned()
    } else {
        format!("wss://{input}")
    };
    RelayUrl::parse(&text).ok().map(|relay| (text, relay))
}

/// The host of a relay URL, which is what the server lists show; the scheme is implied.
pub fn server_host(relay: &RelayUrl) -> SharedString {
    relay
        .domain()
        .map(SharedString::from)
        .unwrap_or_else(|| SharedString::from(relay.to_string()))
}
