use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Entity, IntoElement, ParentElement as _, SharedString,
    Styled as _, Task, Window, div,
};
use nostr::prelude::RelayUrl;
use settings::{DEFAULT_GRASP_SERVERS, GraspServersSettings};
use signed_state::Backend;
use ui::button::{Button, ButtonVariant};
use ui::common::{Sizable, h_flex, v_flex};
use ui::icons::{Icon, IconRole};
use ui::input::{Input, InputState};
use ui::theme::ActiveTheme as _;
use ui::tooltip::Tooltip;

use crate::shared::labels::{RelayHostExt as _, ServerInputExt as _};

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
                .child(div().text_xs().child("Grasp servers"))
                .child(
                    Button::new("toggle-grasp-servers")
                        .icon(Icon::named(if servers_enabled {
                            IconRole::ChevronRight
                        } else {
                            IconRole::ChevronDown
                        }))
                        .small()
                        .variant(ButtonVariant::Ghost)
                        .tooltip(Tooltip::text(if servers_enabled {
                            "Hide grasp servers"
                        } else {
                            "Show grasp servers"
                        }))
                        .on_click(move |_, _, cx| {
                            toggle_state.update(cx, |state, cx| {
                                state.servers_enabled = !state.servers_enabled;
                                cx.notify();
                            });
                        }),
                ),
        )
        .when(servers_enabled, |section| {
            section
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().colors.muted_foreground)
                        .child("Where the repository is hosted"),
                )
                .child(
                    v_flex().gap_1().children(
                        servers
                            .iter()
                            .map(|relay| render_server_row(relay, state.clone(), cx)),
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
                            .text_xs()
                            .text_color(cx.theme().colors.muted_foreground)
                            .child("Loading your grasp servers..."),
                    )
                })
                .when_some(error, |section, error| {
                    section.child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().ramps.destructive.base)
                            .child(error),
                    )
                })
        })
        .into_any_element()
}

fn render_server_row(relay: &RelayUrl, state: Entity<GraspServersState>, cx: &App) -> AnyElement {
    let relay = relay.clone();
    h_flex()
        .w_full()
        .gap_1()
        .items_center()
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .h_8()
                .px_2()
                .bg(cx.theme().colors.muted)
                .text_color(cx.theme().colors.muted_foreground)
                .text_xs()
                .rounded(cx.theme().radius.control)
                .child(relay.host_label()),
        )
        .child(
            Button::new(SharedString::from(format!("remove-relay:{relay}")))
                .icon(Icon::named(IconRole::Close))
                .variant(ButtonVariant::Ghost)
                .tooltip(Tooltip::text("Remove"))
                .on_click(move |_, _, cx| {
                    state.update(cx, |state, cx| {
                        state.grasp_servers.retain(|known| known != &relay);
                        cx.notify();
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

    match value.normalize_server() {
        Some((_, relay)) => {
            state.update(cx, |state, cx| {
                state.error = None;
                if !state.grasp_servers.contains(&relay) {
                    state.grasp_servers.push(relay);
                }
                cx.notify();
            });

            input.update(cx, |input, cx| {
                input.set_value("", window, cx);
            });
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
) -> Task<()> {
    let backend = Backend::global(cx);
    let (user, client) = {
        let backend = backend.read(cx);
        (backend.current_user(), backend.client())
    };

    let Some(user) = user else {
        state.update(cx, |state, cx| {
            state.loading_servers = false;
            cx.notify();
        });
        return Task::ready(());
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
}
