use std::path::PathBuf;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Task, WeakEntity, Window, div, px,
};
use settings::SettingsStore;
use signed_git::Repo;
use signed_state::Backend;
use ui::button::Button;
use ui::common::{ResultLogErr as _, v_flex};
use ui::dialog::{Dialog, DialogHandle};
use ui::field::Field;
use ui::icons::{Icon, IconRole};
use ui::input::{Input, InputState};
use ui::layout::h_flex;
use ui::select::{Select, SelectOption};
use ui::theme::ActiveTheme as _;

use super::grasp_servers::{GraspServersState, grasp_servers_field, load_user_grasp_servers};
use crate::repo::RepoDetailView;
use crate::shared::log_dropped;

pub struct InitRepoDialog {
    handle: DialogHandle,
    local_path: PathBuf,
    name_input: Entity<InputState>,
    desc_input: Entity<InputState>,
    relay_input: Entity<InputState>,
    busy: bool,
    error: Option<SharedString>,
    grasp_state: Entity<GraspServersState>,
    branches: Vec<SharedString>,
    /// `Select` options for `branches`, rebuilt when they load.
    branch_options: Vec<SelectOption<SharedString>>,
    /// The folder shown in the form, formatted once.
    folder_label: SharedString,
    selected_branch: Option<SharedString>,
    view: WeakEntity<RepoDetailView>,
    /// Held so the grasp-server load is cancelled when the dialog drops.
    task: Option<Task<()>>,
    /// Holds the default-branch load so it is cancelled when the dialog drops.
    branches_task: Option<Task<()>>,
}

impl InitRepoDialog {
    pub fn new(
        local_path: PathBuf,
        view: WeakEntity<RepoDetailView>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let settings = SettingsStore::global(cx);
        let grasp_settings = settings.read(cx).settings().grasp_servers.clone();
        let grasp_state = cx.new(|_| GraspServersState::new_default(&grasp_settings));
        let local_path_label = local_path.display().to_string();

        let default_name = local_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();

        let name_input = InputState::builder()
            .text(default_name)
            .read_only()
            .build(cx);

        let desc_input = InputState::builder().multiline().build(cx);
        let relay_input = InputState::new(cx);

        cx.new(|cx| {
            cx.defer_in(window, move |this: &mut Self, window, cx| {
                let task = load_user_grasp_servers(this.grasp_state.clone(), window, cx);
                this.task = Some(task);

                this.load_branches(cx);
            });

            Self {
                handle: DialogHandle::new(true),
                local_path,
                name_input,
                desc_input,
                relay_input,
                busy: false,
                error: None,
                grasp_state,
                branches: Vec::new(),
                branch_options: Vec::new(),
                folder_label: SharedString::from(local_path_label),
                selected_branch: None,
                view,
                task: None,
                branches_task: None,
            }
        })
    }

    fn init_repository(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
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
            self.error = Some("Repository name is required".into());
            cx.notify();
            return;
        }

        if servers.is_empty() {
            self.error = Some("Add at least one grasp server".into());
            cx.notify();
            return;
        }

        let backend = Backend::global(cx);
        let Some(_user) = backend.read(cx).current_user() else {
            self.error = Some("Sign in to publish a repository".into());
            cx.notify();
            return;
        };

        self.busy = true;
        self.error = None;
        cx.notify();

        let local_path = self.local_path.clone();
        let task = backend.update(cx, |backend, cx| {
            backend.publish_local_repo(local_path, &name, &description, servers, default_branch, cx)
        });

        let view = self.view.clone();
        let close = self.handle.clone();
        let handle = window.window_handle();

        self.task = Some(cx.spawn(async move |this, cx| match task.await {
            Ok(announcement) => {
                cx.update_window(handle, |_, _window, cx| {
                    close.close(cx);
                    if let Some(view) = view.upgrade() {
                        view.update(cx, |this, cx| this.apply_announcement(announcement, cx));
                    }
                })
                .log_err();
            }
            Err(error) => {
                log_dropped(this.update(cx, |this, cx| {
                    this.busy = false;
                    this.error = Some(error.to_string().into());
                    cx.notify();
                }));
            }
        }));
    }

    /// Lists the repository's branches for the default-branch selector.
    fn load_branches(&mut self, cx: &mut Context<Self>) {
        let local_path = self.local_path.clone();

        self.branches_task = Some(cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_spawn(async move {
                    let repo = Repo::open(&local_path)?;
                    let current: Option<SharedString> = repo.current_branch().map(Into::into);
                    Ok::<_, anyhow::Error>((repo.branches()?, current))
                })
                .await;

            let (branches, current) = match loaded {
                Ok(loaded) => loaded,
                Err(error) => {
                    log::warn!("failed to list branches for the default branch selector: {error}");
                    let _ = this.update(cx, |this, cx| {
                        this.error = Some("Could not list the repository's branches".into());
                        cx.notify();
                    });
                    return;
                }
            };

            let _ = this.update(cx, |this, cx| {
                this.branches = branches.into_iter().map(Into::into).collect();
                this.branch_options = this
                    .branches
                    .iter()
                    .cloned()
                    .map(|branch| SelectOption::new(branch.clone(), branch))
                    .collect();
                this.selected_branch = current;
                cx.notify();
            });
        }));
    }
}

impl Render for InitRepoDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let name_input = self.name_input.clone();
        let desc_input = self.desc_input.clone();
        let relay_input = self.relay_input.clone();
        let error = self.error.clone();
        let grasp_state = self.grasp_state.clone();
        let folder = self.folder_label.clone();
        let branch_options = self.branch_options.clone();
        let selected_branch = self.selected_branch.clone();
        let busy = self.busy;

        let this = cx.weak_entity();

        Dialog::new("init-repo-dialog")
            .handle(self.handle.clone())
            .close_on_escape(!busy)
            .close_on_backdrop(!busy)
            .min_width(px(520.))
            .title("Initialize repository")
            .content(move |_, cx| {
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
                                h_flex()
                                    .h_8()
                                    .w_full()
                                    .px_2()
                                    .bg(cx.theme().colors.muted)
                                    .text_sm()
                                    .text_color(cx.theme().colors.muted_foreground)
                                    .rounded(cx.theme().radius.control)
                                    .child(
                                        div().flex_1().min_w_0().truncate().child(folder.clone()),
                                    ),
                            ),
                    )
                    .child(
                        Field::new("init-branch", "Default branch")
                            .description("The local branch to announce as the default")
                            .child(
                                Select::new("init-branch-select")
                                    .options(branch_options.clone())
                                    .selected(selected_branch.clone())
                                    .placeholder("Select branch"),
                            ),
                    )
                    .child(grasp_servers_field(&grasp_state, &relay_input, cx))
                    .when_some(error.clone(), |el, error| {
                        el.child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().ramps.destructive.base)
                                .child(error),
                        )
                    })
                    .into_any_element()
            })
            .actions(move |_, _cx| {
                let this = this.clone();

                vec![
                    Button::new("initialize-repository")
                        .label("Initialize")
                        .icon(Icon::named(IconRole::Plus))
                        .loading(busy)
                        .disabled(busy)
                        .on_click(move |_, window, cx| {
                            log_dropped(this.update(cx, |this, cx| {
                                this.init_repository(window, cx);
                            }));
                        })
                        .into_any_element(),
                ]
            })
    }
}
