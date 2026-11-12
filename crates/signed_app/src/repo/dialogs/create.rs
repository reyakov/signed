use std::path::PathBuf;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _,
    PathPromptOptions, Render, Styled as _, WeakEntity, Window, div, px,
};
use gpui_native_kit::button::{Button, ButtonVariant};
use gpui_native_kit::common::{h_flex, v_flex};
use gpui_native_kit::dialog::Dialog;
use gpui_native_kit::field::Field;
use gpui_native_kit::icons::{Icon, IconRole};
use gpui_native_kit::input::{Input, InputState};
use gpui_native_kit::layout::dock::DockState;
use gpui_native_kit::theme::ActiveTheme as _;
use gpui_native_kit::tooltip::Tooltip;
use settings::SettingsStore;
use signed_state::{Backend, CheckoutsStore};

use super::grasp_servers::{GraspServersState, grasp_servers_field, load_user_grasp_servers};
use crate::repo::actions::open_repo_panel;
use crate::shared::dialogs::{close_global, open_global};
use crate::shared::progress::{DialogProgress, error_row, push_steps_row};

/// Progress of the create-repository flow, so async results can be rendered.
pub struct CreateRepoDialog {
    name_input: Entity<InputState>,
    desc_input: Entity<InputState>,
    folder_input: Entity<InputState>,
    relay_input: Entity<InputState>,
    state: Entity<DialogProgress>,
    grasp_state: Entity<GraspServersState>,
    dock: WeakEntity<DockState>,
}

impl CreateRepoDialog {
    pub fn new(dock: WeakEntity<DockState>, window: &mut Window, cx: &mut App) -> Entity<Self> {
        let settings = SettingsStore::global(cx);
        let default_folder = settings
            .read(cx)
            .settings()
            .create_repository
            .default_folder
            .clone()
            .unwrap_or_else(paths::desktop_dir);
        let grasp_settings = settings.read(cx).settings().grasp_servers.clone();

        let name_input = InputState::new(cx);
        let desc_input = InputState::builder().multiline().build(cx);
        let folder_input = InputState::builder()
            .text(default_folder.to_string_lossy())
            .build(cx);
        folder_input.update(cx, |state, cx| state.set_read_only(true, cx));

        let relay_input = InputState::new(cx);
        let state = cx.new(|_| DialogProgress::default());
        let grasp_state = cx.new(|_| GraspServersState::new_default(&grasp_settings));

        load_user_grasp_servers(grasp_state.clone(), window, cx);

        cx.new(|_| Self {
            name_input,
            desc_input,
            folder_input,
            relay_input,
            state,
            grasp_state,
            dock,
        })
    }

    fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.read(cx).busy {
            return;
        }

        let name = self.name_input.read(cx).value().trim().to_owned();
        let description = self.desc_input.read(cx).value().trim().to_owned();
        let folder = PathBuf::from(self.folder_input.read(cx).value().trim());
        let servers = self.grasp_state.read(cx).grasp_servers.clone();

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
            self.fail("Sign in to create a repository", cx);
            return;
        };

        let addr =
            signed_core::RepoAddr::new(user, signed_core::RepoAddr::identifier_from_name(&name));
        self.state.update(cx, |state, cx| {
            state.begin_push(addr, cx);
            cx.notify();
        });

        let task = backend.update(cx, |backend, cx| {
            backend.create_repository(&name, &description, folder, servers, cx)
        });
        let state = self.state.clone();
        let dock = self.dock.clone();
        let handle = window.window_handle();

        cx.spawn(async move |_this, cx| match task.await {
            Ok((announcement, local_path)) => {
                cx.update_window(handle, |_, window, cx| {
                    let checkouts = CheckoutsStore::global(cx);
                    checkouts.update(cx, |store, cx| {
                        store.record(local_path.clone(), announcement.addr(), cx);
                    });
                    cx.open_with_system(&local_path);
                    open_repo_panel(&dock, &announcement.addr(), Some(&announcement), window, cx);
                    close_global(cx);
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

    fn fail(&mut self, message: impl Into<gpui::SharedString>, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            state.fail(message);
            cx.notify();
        });
    }
}

impl Render for CreateRepoDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let name_input = self.name_input.clone();
        let desc_input = self.desc_input.clone();
        let folder_input = self.folder_input.clone();
        let relay_input = self.relay_input.clone();
        let state = self.state.clone();
        let grasp_state = self.grasp_state.clone();
        let action_state = state.clone();
        let this = cx.entity().downgrade();

        Dialog::new("create-repo-dialog")
            .open(true)
            .min_width(px(520.))
            .on_change(|open, _, cx| {
                if !open {
                    close_global(cx);
                }
            })
            .title("Create repository")
            .content(move |_window, cx| {
                let progress = state.read(cx);

                v_flex()
                    .gap_3()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().colors.muted_foreground)
                            .child("Publish a new repository to your grasp servers."),
                    )
                    .child(
                        Field::new("repo-name", "Repository name")
                            .description("Max 100 characters")
                            .child(Input::new("repo-name-input", name_input.clone())),
                    )
                    .child(
                        Field::new("repo-description", "Description").child(
                            Input::new("repo-description-input", desc_input.clone()).rows(3),
                        ),
                    )
                    .child(
                        Field::new("repo-folder", "Folder")
                            .description("Where the repository's working copy is created.")
                            .child(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .child(div().flex_1().child(Input::new(
                                        "repo-folder-input",
                                        folder_input.clone(),
                                    )))
                                    .child(
                                        Button::new("choose-folder")
                                            .icon(Icon::named(IconRole::Folder))
                                            .variant(ButtonVariant::Ghost)
                                            .tooltip(Tooltip::text("Choose folder"))
                                            .on_click({
                                                let folder_input = folder_input.clone();
                                                move |_, window, cx| {
                                                    choose_folder(&folder_input, window, cx);
                                                }
                                            }),
                                    ),
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

                let create: AnyElement = Button::new("create-repository")
                    .label("Create repository")
                    .icon(Icon::named(IconRole::Plus))
                    .loading(busy)
                    .disabled(busy)
                    .on_click(move |_, window, cx| {
                        let _ = this.update(cx, |this, cx| this.create(window, cx));
                    })
                    .into_any_element();

                vec![create]
            })
    }
}

fn choose_folder(folder_input: &Entity<InputState>, window: &mut Window, cx: &mut App) {
    let store = SettingsStore::global(cx);
    let prompt = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: Some("Choose folder".into()),
    });
    let handle = window.window_handle();
    let folder_input = folder_input.clone();

    cx.spawn(async move |cx| {
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
                folder_input.update(cx, |input, cx| input.set_value(text, window, cx));
            })
        {
            log::warn!("failed to apply selected folder: {error}");
        }
    })
    .detach();
}

/// Opens the create-repository form.
pub fn open(dock: WeakEntity<DockState>, window: &mut Window, cx: &mut App) {
    open_global(CreateRepoDialog::new(dock, window, cx), cx);
}
