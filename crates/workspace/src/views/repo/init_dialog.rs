use std::path::PathBuf;

use assets::CustomIconName;
use gpui::prelude::*;
use gpui::{App, Entity, SharedString, WeakEntity, Window, px};
use gpui_base::h_flex;
use gpui_base::input::TextareaState;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::dialog::{DialogDescription, DialogFooter, DialogHeader, DialogTitle};
use gpui_component::form::{field, v_form};
use gpui_component::input::{Input, InputState, Textarea};
use gpui_component::select::{Select, SelectState};
use gpui_component::{ActiveTheme, Disableable, WindowExt};
use settings::SettingsStore;
use signed_git::Repo;
use signed_state::Backend;
use signed_ui::SelectOption;

use super::RepoDetailView;
use crate::views::dialog_state::{DialogProgress, error_row};
use crate::views::sidebar::grasp_servers::{
    GraspServersState, grasp_servers_field, load_user_grasp_servers,
};

pub type InitRepoState = DialogProgress;

pub fn open(
    local_path: PathBuf,
    view: WeakEntity<RepoDetailView>,
    window: &mut Window,
    cx: &mut App,
) {
    let default_name = local_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();

    let grasp_settings = SettingsStore::global(cx)
        .read(cx)
        .settings()
        .grasp_servers
        .clone();

    let state = cx.new(|_| InitRepoState::default());
    let grasp_state = cx.new(|_| GraspServersState::new_default(&grasp_settings));

    let relay_input = cx.new(|cx| InputState::new(window, cx).placeholder("relay.example.com"));
    let name_input = cx.new(|cx| InputState::new(window, cx).default_value(default_name));
    let desc_input = cx.new(|cx| {
        TextareaState::new(window, cx)
            .auto_grow(3, 5)
            .placeholder("Short description")
    });
    let branch_select: Entity<SelectState<Vec<SelectOption>>> =
        cx.new(|cx| SelectState::new(Vec::new(), None, window, cx));

    load_user_grasp_servers(grasp_state.clone(), window, cx);
    load_branches(local_path.clone(), branch_select.clone(), window, cx);

    window.open_dialog(cx, move |dialog, _window, _cx| {
        const DESC: &str = "Publish this local repository to Nostr.";

        let name_input = name_input.clone();
        let desc_input = desc_input.clone();
        let relay_input = relay_input.clone();
        let branch_select = branch_select.clone();
        let state = state.clone();
        let grasp_state = grasp_state.clone();
        let local_path = local_path.clone();
        let view = view.clone();

        dialog
            .width(px(520.))
            .margin_top(px(50.))
            .content(move |content, _window, cx| {
                let busy = state.read(cx).busy;
                let error = state.read(cx).error.clone();

                content
                    .child(
                        DialogHeader::new()
                            .child(DialogTitle::new().child("Initialize repository"))
                            .child(DialogDescription::new().child(DESC)),
                    )
                    .child(
                        v_form()
                            .child(
                                field()
                                    .label("Repository name")
                                    .description("Max 100 characters")
                                    .required(true)
                                    .child(Input::new(&name_input).readonly(true)),
                            )
                            .child(
                                field()
                                    .label("Description")
                                    .child(Textarea::new(&desc_input)),
                            )
                            .child(
                                field()
                                    .label("Folder")
                                    .description("The local repository being published")
                                    .child(
                                        h_flex()
                                            .h_8()
                                            .w_full()
                                            .px_2()
                                            .bg(cx.theme().muted)
                                            .text_sm()
                                            .text_color(cx.theme().muted_foreground)
                                            .rounded(cx.theme().radius)
                                            .child(local_path.display().to_string()),
                                    ),
                            )
                            .child(
                                field()
                                    .label("Default branch")
                                    .description("The local branch to announce as the default")
                                    .child(Select::new(&branch_select).w_full()),
                            )
                            .child(grasp_servers_field(&grasp_state, &relay_input, cx)),
                    )
                    .children(error_row(&error, cx))
                    .child(
                        DialogFooter::new().justify_end().child(
                            Button::new("init")
                                .primary()
                                .label("Initialize")
                                .icon(CustomIconName::Init)
                                .tooltip("Publish to Nostr")
                                .loading(busy)
                                .disabled(busy)
                                .on_click({
                                    let name_input = name_input.clone();
                                    let desc_input = desc_input.clone();
                                    let branch_select = branch_select.clone();
                                    let state = state.clone();
                                    let grasp_state = grasp_state.clone();
                                    let local_path = local_path.clone();
                                    let view = view.clone();

                                    move |_ev, window, cx| {
                                        init_repository(
                                            local_path.clone(),
                                            (name_input.clone(), desc_input.clone()),
                                            branch_select.clone(),
                                            state.clone(),
                                            grasp_state.clone(),
                                            view.clone(),
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

#[allow(clippy::too_many_arguments)]
fn init_repository(
    local_path: PathBuf,
    inputs: (Entity<InputState>, Entity<TextareaState>),
    branch_select: Entity<SelectState<Vec<SelectOption>>>,
    state: Entity<InitRepoState>,
    grasp_state: Entity<GraspServersState>,
    view: WeakEntity<RepoDetailView>,
    window: &mut Window,
    cx: &mut App,
) {
    let (name_input, desc_input) = inputs;
    let name = name_input.read(cx).value().trim().to_owned();
    let description = desc_input.read(cx).value().trim().to_owned();
    let servers = grasp_state.read(cx).grasp_servers.clone();
    let default_branch = branch_select
        .read(cx)
        .selected_value()
        .map(|value| value.trim().to_owned())
        .filter(|branch| !branch.is_empty());

    if name.is_empty() {
        state.update(cx, |state, _| state.fail("Repository name is required"));
        return;
    }

    if servers.is_empty() {
        state.update(cx, |state, _| state.fail("Add at least one grasp server"));
        return;
    }

    state.update(cx, |state, _| state.begin());

    let backend = Backend::global(cx);
    let task = backend.update(cx, |backend, cx| {
        backend.publish_local_repo(
            local_path.clone(),
            &name,
            &description,
            servers,
            default_branch,
            cx,
        )
    });

    let handle = window.window_handle();
    let state = state.clone();
    let view = view.clone();

    cx.spawn(async move |cx| match task.await {
        Ok(announcement) => {
            cx.update_window(handle, |_, window, cx| {
                window.close_dialog(cx);
                if let Some(view) = view.upgrade() {
                    view.update(cx, |this, cx| {
                        this.apply_announcement(announcement, cx);
                    });
                }
            })
            .ok();
        }
        Err(e) => {
            cx.update_window(handle, |_, _window, cx| {
                state.update(cx, |state, _| state.fail(e.to_string()));
            })
            .ok();
        }
    })
    .detach();
}

/// Fills the default-branch selector with the repository's local branches,
/// preselecting the checked-out one.
fn load_branches(
    local_path: PathBuf,
    select: Entity<SelectState<Vec<SelectOption>>>,
    window: &mut Window,
    cx: &mut App,
) {
    let handle = window.window_handle();

    cx.spawn(async move |cx| {
        let loaded = cx
            .background_spawn(async move {
                let repo = Repo::open(&local_path)?;
                let current: Option<SharedString> = repo.current_branch().map(Into::into);
                Ok::<_, anyhow::Error>((repo.branches()?, current))
            })
            .await;

        let _ = cx.update_window(handle, |_, window, cx| {
            let Ok((branches, current)) = loaded.map_err(|error| {
                log::warn!("failed to list branches for the default branch selector: {error:#}")
            }) else {
                return;
            };

            let options: Vec<SelectOption> = branches
                .into_iter()
                .map(|branch| SelectOption::new(branch.clone(), branch))
                .collect();

            select.update(cx, |state, cx| {
                state.set_items(options, window, cx);
                if let Some(current) = current.as_ref() {
                    state.set_selected_value(current, window, cx);
                }
            });
        });
    })
    .detach();
}
