use std::path::PathBuf;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, PathPromptOptions,
    Render, SharedString, Styled as _, Task, WeakEntity, Window, div, px,
};
use settings::SettingsStore;
use signed_state::{Backend, CheckoutsStore};
use ui::button::{Button, ButtonVariant};
use ui::common::{ResultLogErr as _, h_flex, v_flex};
use ui::dialog::{Dialog, DialogHandle};
use ui::field::Field;
use ui::icons::{Icon, IconRole};
use ui::input::{Input, InputState};
use ui::theme::ActiveTheme as _;
use ui::tooltip::Tooltip;

use super::grasp_servers::{GraspServersState, grasp_servers_field, load_user_grasp_servers};
use crate::app::AppRoot;
use crate::shared::log_dropped;

/// Progress of the create-repository flow, so async results can be rendered.
pub struct CreateRepoDialog {
    workspace: WeakEntity<AppRoot>,
    handle: DialogHandle,
    name_input: Entity<InputState>,
    desc_input: Entity<InputState>,
    folder_input: Entity<InputState>,
    relay_input: Entity<InputState>,
    busy: bool,
    error: Option<SharedString>,
    grasp_state: Entity<GraspServersState>,
    /// Held so the grasp-server load is cancelled when the dialog drops.
    task: Option<Task<()>>,
    /// Holds the folder prompt so it is cancelled when the dialog drops.
    folder_task: Option<Task<()>>,
}

impl CreateRepoDialog {
    pub fn new(workspace: WeakEntity<AppRoot>, window: &mut Window, cx: &mut App) -> Entity<Self> {
        let settings = SettingsStore::global(cx);
        let grasp_settings = settings.read(cx).settings().grasp_servers.clone();

        let name_input = InputState::new(cx);
        let desc_input = InputState::builder().multiline().build(cx);

        let default_folder = settings
            .read(cx)
            .settings()
            .create_repository
            .default_folder
            .clone()
            .unwrap_or_else(paths::desktop_dir);

        let folder_input = InputState::builder()
            .text(default_folder.to_string_lossy())
            .read_only()
            .build(cx);

        let relay_input = InputState::new(cx);
        let grasp_state = cx.new(|_| GraspServersState::new_default(&grasp_settings));

        cx.new(|cx| {
            cx.defer_in(window, move |this: &mut Self, window, cx| {
                let task = load_user_grasp_servers(this.grasp_state.clone(), window, cx);
                this.task = Some(task);
            });

            Self {
                workspace,
                handle: DialogHandle::new(true),
                name_input,
                desc_input,
                folder_input,
                relay_input,
                busy: false,
                error: None,
                grasp_state,
                task: None,
                folder_task: None,
            }
        })
    }

    fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }

        let backend = Backend::global(cx);
        let name = self.name_input.read(cx).value().trim().to_owned();
        let description = self.desc_input.read(cx).value().trim().to_owned();
        let folder = PathBuf::from(self.folder_input.read(cx).value().trim());
        let servers = self.grasp_state.read(cx).grasp_servers.clone();

        let Some(_user) = backend.read(cx).current_user() else {
            self.error = Some("Sign in to create a repository".into());
            cx.notify();
            return;
        };

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

        self.busy = true;
        self.error = None;
        cx.notify();

        let task = backend.update(cx, |backend, cx| {
            backend.create_repository(&name, &description, folder, servers, cx)
        });

        let workspace = self.workspace.clone();
        let close = self.handle.clone();
        let handle = window.window_handle();

        self.task = Some(cx.spawn(async move |this, cx| match task.await {
            Ok((announcement, local_path)) => {
                cx.update_window(handle, |_, window, cx| {
                    let checkouts = CheckoutsStore::global(cx);
                    checkouts.update(cx, |store, cx| {
                        store.record(local_path.clone(), announcement.addr(), cx);
                    });

                    cx.open_with_system(&local_path);
                    let result = workspace.update(cx, |root, cx| {
                        root.open_repo_panel(&announcement.addr(), Some(&announcement), window, cx);
                    });

                    log_dropped(result);
                    close.close(cx);
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
}

impl Render for CreateRepoDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let name_input = self.name_input.clone();
        let desc_input = self.desc_input.clone();
        let folder_input = self.folder_input.clone();
        let relay_input = self.relay_input.clone();

        let busy = self.busy;
        let error = self.error.clone();

        let grasp_state = self.grasp_state.clone();
        let this = cx.weak_entity();

        Dialog::new("create-repo-dialog")
            .handle(self.handle.clone())
            .close_on_escape(!busy)
            .close_on_backdrop(!busy)
            .min_width(px(520.))
            .title("Create Repository")
            .content({
                let this = this.clone();
                move |_window, cx| {
                    v_flex()
                        .gap_3()
                        .child(
                            Field::new("repo-name", "Repository Name")
                                .description("Max 100 characters")
                                .child(Input::new("repo-name-input", name_input.clone())),
                        )
                        .child(Field::new("repo-description", "Description").child(
                            Input::new("repo-description-input", desc_input.clone()).rows(3),
                        ))
                        .child(
                            Field::new("repo-folder", "Folder")
                                .description("Where the repository's working copy is created.")
                                .child(
                                    h_flex()
                                        .gap_1()
                                        .w_full()
                                        .child(div().flex_1().min_w_0().child(Input::new(
                                            "repo-folder-input",
                                            folder_input.clone(),
                                        )))
                                        .child(
                                            Button::new("choose-folder")
                                                .icon(Icon::named(IconRole::Folder))
                                                .variant(ButtonVariant::Ghost)
                                                .tooltip(Tooltip::text("Choose folder"))
                                                .on_click({
                                                    let this = this.clone();
                                                    move |_, window, cx| {
                                                        log_dropped(this.update(cx, |this, cx| {
                                                            this.choose_folder(window, cx)
                                                        }));
                                                    }
                                                }),
                                        ),
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
                }
            })
            .actions(move |_, _cx| {
                let this = this.clone();

                vec![
                    Button::new("create-repository")
                        .label("Create repository")
                        .icon(Icon::named(IconRole::Plus))
                        .loading(busy)
                        .disabled(busy)
                        .on_click(move |_, window, cx| {
                            log_dropped(this.update(cx, |this, cx| {
                                this.create(window, cx);
                            }));
                        })
                        .into_any_element(),
                ]
            })
    }
}

impl CreateRepoDialog {
    /// Prompts for the repository's folder and remembers it as the default.
    fn choose_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let store = SettingsStore::global(cx);

        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose folder".into()),
        });

        let handle = window.window_handle();
        let folder_input = self.folder_input.clone();

        self.folder_task = Some(cx.spawn(async move |_, cx| {
            if let Ok(Ok(Some(mut paths))) = prompt.await
                && let Some(path) = paths.pop()
                && let Err(error) = cx.update_window(handle, |_, window, cx| {
                    let text = path.to_string_lossy().to_string();
                    store.update(cx, |store, cx| {
                        store.edit(
                            |settings| {
                                settings.create_repository.default_folder = Some(path.clone());
                            },
                            cx,
                        );
                    });
                    folder_input.update(cx, |input, cx| {
                        input.set_value(text, window, cx);
                    });
                })
            {
                log::warn!("failed to apply selected folder: {error}");
            }
        }));
    }
}
