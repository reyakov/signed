use std::path::PathBuf;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, WeakEntity, Window, div, px,
};
use gpui_native_kit::button::Button;
use gpui_native_kit::common::v_flex;
use gpui_native_kit::dialog::Dialog;
use gpui_native_kit::field::Field;
use gpui_native_kit::icons::{Icon, IconRole};
use gpui_native_kit::input::{Input, InputState};
use gpui_native_kit::select::{Select, SelectOption};
use gpui_native_kit::theme::ActiveTheme as _;
use settings::SettingsStore;
use signed_core::RepoAddr;
use signed_git::Repo;
use signed_state::Backend;

use super::grasp_servers::{GraspServersState, grasp_servers_field, load_user_grasp_servers};
use super::progress::{DialogProgress, error_row, push_steps_row};
use super::{close_global, open_global};
use crate::views::repo::RepoDetailView;

pub struct InitRepoDialog {
    local_path: PathBuf,
    name_input: Entity<InputState>,
    desc_input: Entity<InputState>,
    relay_input: Entity<InputState>,
    state: Entity<DialogProgress>,
    grasp_state: Entity<GraspServersState>,
    branches: Vec<SharedString>,
    selected_branch: Option<SharedString>,
    view: WeakEntity<RepoDetailView>,
}

impl InitRepoDialog {
    pub fn new(
        local_path: PathBuf,
        view: WeakEntity<RepoDetailView>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let default_name = local_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let grasp_settings = SettingsStore::global(cx)
            .read(cx)
            .settings()
            .grasp_servers
            .clone();

        let state = cx.new(|_| DialogProgress::default());
        let grasp_state = cx.new(|_| GraspServersState::new_default(&grasp_settings));
        let name_input = InputState::new_with_text(default_name, cx);
        name_input.update(cx, |state, cx| state.set_read_only(true, cx));
        let desc_input = InputState::new(cx);
        desc_input.update(cx, |state, cx| state.set_multiline(true, cx));
        let relay_input = InputState::new(cx);

        load_user_grasp_servers(grasp_state.clone(), window, cx);

        let dialog = cx.new(|_| Self {
            local_path,
            name_input,
            desc_input,
            relay_input,
            state,
            grasp_state,
            branches: Vec::new(),
            selected_branch: None,
            view,
        });

        load_branches(dialog.clone(), cx);
        dialog
    }

    fn init_repository(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.read(cx).busy {
            return;
        }

        let name = self.name_input.read(cx).value().trim().to_owned();
        let description = self.desc_input.read(cx).value().trim().to_owned();
        let servers = self.grasp_state.read(cx).grasp_servers.clone();
        let default_branch = self
            .selected_branch
            .as_ref()
            .map(|branch| branch.trim().to_owned())
            .filter(|branch| !branch.is_empty());

        if name.is_empty() {
            self.fail("Repository name is required", cx);
            return;
        }
        if servers.is_empty() {
            self.fail("Add at least one grasp server", cx);
            return;
        }

        let backend = Backend::global(cx);
        let Some(user) = backend.read(cx).current_user() else {
            self.fail("Sign in to publish a repository", cx);
            return;
        };

        let addr = RepoAddr::new(user, RepoAddr::identifier_from_name(&name));
        self.state.update(cx, |state, cx| {
            state.begin_push(addr, cx);
            cx.notify();
        });

        let local_path = self.local_path.clone();
        let task = backend.update(cx, |backend, cx| {
            backend.publish_local_repo(local_path, &name, &description, servers, default_branch, cx)
        });
        let state = self.state.clone();
        let view = self.view.clone();
        let handle = window.window_handle();

        cx.spawn(async move |_this, cx| match task.await {
            Ok(announcement) => {
                cx.update_window(handle, |_, _window, cx| {
                    close_global(cx);
                    if let Some(view) = view.upgrade() {
                        view.update(cx, |this, cx| this.apply_announcement(announcement, cx));
                    }
                })
                .ok();
            }
            Err(error) => {
                state.update(cx, |state, cx| {
                    state.fail(error.to_string());
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn fail(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            state.fail(message);
            cx.notify();
        });
    }
}

impl Render for InitRepoDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let name_input = self.name_input.clone();
        let desc_input = self.desc_input.clone();
        let relay_input = self.relay_input.clone();
        let state = self.state.clone();
        let grasp_state = self.grasp_state.clone();
        let action_state = state.clone();
        let this = cx.entity().downgrade();
        let folder = self.local_path.display().to_string();
        let branches = self.branches.clone();
        let selected_branch = self.selected_branch.clone();

        Dialog::new("init-repo-dialog")
            .open(true)
            .min_width(px(520.))
            .on_change(|open, _, cx| {
                if !open {
                    close_global(cx);
                }
            })
            .title("Initialize repository")
            .content(move |_, cx| {
                let progress = state.read(cx);

                v_flex()
                    .gap_3()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().colors.muted_foreground)
                            .child("Publish this local repository to Nostr."),
                    )
                    .child(
                        Field::new("init-name", "Repository name")
                            .description("Max 100 characters")
                            .child(Input::new("init-name-input", name_input.clone())),
                    )
                    .child(
                        Field::new("init-description", "Description").child(
                            Input::new("init-description-input", desc_input.clone()).rows(3),
                        ),
                    )
                    .child(
                        Field::new("init-folder", "Folder")
                            .description("The local repository being published")
                            .child(
                                div()
                                    .h_8()
                                    .w_full()
                                    .px_2()
                                    .bg(cx.theme().colors.muted)
                                    .text_sm()
                                    .text_color(cx.theme().colors.muted_foreground)
                                    .rounded(cx.theme().radius.control)
                                    .child(folder.clone()),
                            ),
                    )
                    .child(
                        Field::new("init-branch", "Default branch")
                            .description("The local branch to announce as the default")
                            .child(
                                Select::new("init-branch-select")
                                    .options(
                                        branches
                                            .iter()
                                            .cloned()
                                            .map(|branch| SelectOption::new(branch.clone(), branch))
                                            .collect::<Vec<_>>(),
                                    )
                                    .selected(selected_branch.clone())
                                    .placeholder("Select branch"),
                            ),
                    )
                    .child(grasp_servers_field(&grasp_state, &relay_input, cx))
                    .when_some(push_steps_row(&progress.push_steps, cx), |el, steps| {
                        el.child(steps)
                    })
                    .when_some(error_row(&progress.error, cx), |el, error| el.child(error))
                    .into_any_element()
            })
            .actions(move |_, cx| {
                let busy = action_state.read(cx).busy;
                let this = this.clone();

                let init: AnyElement = Button::new("initialize-repository")
                    .label("Initialize")
                    .icon(Icon::named(IconRole::Plus))
                    .loading(busy)
                    .disabled(busy)
                    .on_click(move |_, window, cx| {
                        let _ = this.update(cx, |this, cx| this.init_repository(window, cx));
                    })
                    .into_any_element();

                vec![init]
            })
    }
}

fn load_branches(dialog: Entity<InitRepoDialog>, cx: &mut App) {
    let local_path = dialog.read(cx).local_path.clone();

    cx.spawn(async move |cx| {
        let loaded = cx
            .background_spawn(async move {
                let repo = Repo::open(&local_path)?;
                let current: Option<SharedString> = repo.current_branch().map(Into::into);
                Ok::<_, anyhow::Error>((repo.branches()?, current))
            })
            .await;

        let Ok((branches, current)) = loaded else {
            log::warn!("failed to list branches for the default branch selector");
            return;
        };

        dialog.update(cx, |dialog, cx| {
            dialog.branches = branches.into_iter().map(Into::into).collect();
            dialog.selected_branch = current;
            cx.notify();
        });
    })
    .detach();
}

/// Opens the initialize form for `local_path`.
pub fn open(
    local_path: PathBuf,
    view: WeakEntity<RepoDetailView>,
    window: &mut Window,
    cx: &mut App,
) {
    open_global(InitRepoDialog::new(local_path, view, window, cx), cx);
}
