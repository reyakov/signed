use std::path::PathBuf;

use dock::DockArea;
use gpui::prelude::*;
use gpui::{App, Entity, PathPromptOptions, WeakEntity, Window, div, px};
use gpui_base::input::TextareaState;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::dialog::{DialogDescription, DialogFooter, DialogHeader, DialogTitle};
use gpui_component::form::{field, v_form};
use gpui_component::input::{Input, InputState, Textarea};
use gpui_component::{Disableable, IconName, WindowExt, h_flex};
use settings::SettingsStore;
use signed_core::Announcement;
use signed_state::{Backend, CheckoutsStore};

use super::super::open_repo_panel;
use super::grasp_servers::{GraspServersState, grasp_servers_field, load_user_grasp_servers};
use crate::views::dialog_state::{DialogProgress, error_row};

/// Shared state for the Create Repository dialog, so async results can be rendered.
pub type CreateRepoState = DialogProgress;

/// Open the Create Repository dialog.
pub fn open(dock_area: WeakEntity<DockArea>, window: &mut Window, cx: &mut App) {
    let settings = SettingsStore::global(cx);
    let default_folder = settings
        .read(cx)
        .settings()
        .create_repository
        .default_folder
        .clone()
        .unwrap_or_else(paths::desktop_dir);

    let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("Repository name"));
    let desc_input = cx.new(|cx| {
        TextareaState::new(window, cx)
            .auto_grow(3, 5)
            .placeholder("Short description")
    });
    let folder_input = cx.new(|cx| {
        InputState::new(window, cx).default_value(default_folder.to_string_lossy().to_string())
    });
    let relay_input = cx.new(|cx| {
        InputState::new(window, cx).placeholder("wss://relay.example.com or relay.example.com")
    });
    let state = cx.new(|_| CreateRepoState::default());
    let grasp_settings = settings.read(cx).settings().grasp_servers.clone();
    let grasp_state = cx.new(|_| GraspServersState::new_default(&grasp_settings));

    load_user_grasp_servers(grasp_state.clone(), window, cx);

    window.open_dialog(cx, move |dialog, _window, _cx| {
        const DESC: &str = "Publish a new repository to your grasp servers.";
        const FOLDER_NOTE: &str = "Where the repository's working copy is created.";

        let name_input = name_input.clone();
        let desc_input = desc_input.clone();
        let folder_input = folder_input.clone();
        let relay_input = relay_input.clone();
        let state = state.clone();
        let grasp_state = grasp_state.clone();
        let dock_area = dock_area.clone();

        dialog
            .width(px(520.))
            .margin_top(px(50.))
            .content(move |content, _window, cx| {
                let busy = state.read(cx).busy;
                let error = state.read(cx).error.clone();

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
                            .child(grasp_servers_field(&grasp_state, &relay_input, cx)),
                    )
                    .children(error_row(&error, cx))
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
                                    let folder_input = folder_input.clone();
                                    let state = state.clone();
                                    let grasp_state = grasp_state.clone();
                                    let dock_area = dock_area.clone();

                                    move |_ev, window, cx| {
                                        create_repository(
                                            name_input.clone(),
                                            desc_input.clone(),
                                            folder_input.clone(),
                                            state.clone(),
                                            grasp_state.clone(),
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

/// Pick the repository's storage folder with the platform's native folder picker.
fn choose_folder(folder_input: &Entity<InputState>, window: &mut Window, cx: &mut App) {
    let handle = window.window_handle();
    let folder_input = folder_input.clone();
    let store = SettingsStore::global(cx);

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
                store.update(cx, |store, cx| {
                    store.edit(
                        |settings| {
                            settings.create_repository.default_folder = Some(path.clone().into())
                        },
                        cx,
                    );
                });
                folder_input.update(cx, |input, cx| input.set_value(path, window, cx));
            })
            .ok();
        }
    })
    .detach();
}

/// Run the create-repository flow.
///
/// Opens the new working copy and the repository panel on success.
#[allow(clippy::too_many_arguments)]
fn create_repository(
    name_input: Entity<InputState>,
    desc_input: Entity<TextareaState>,
    folder_input: Entity<InputState>,
    state: Entity<CreateRepoState>,
    grasp_state: Entity<GraspServersState>,
    dock_area: WeakEntity<DockArea>,
    window: &mut Window,
    cx: &mut App,
) {
    let name = name_input.read(cx).value().trim().to_owned();
    let description = desc_input.read(cx).value().trim().to_owned();
    let folder = PathBuf::from(folder_input.read(cx).value().trim());
    let servers = grasp_state.read(cx).grasp_servers.clone();

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
        backend.create_repository(&name, &description, folder, servers, cx)
    });

    let handle = window.window_handle();
    let state = state.clone();
    let dock_area = dock_area.clone();

    cx.spawn(async move |cx| match task.await {
        Ok((announcement, local_path)) => {
            cx.update_window(handle, |_, window, cx| {
                window.close_dialog(cx);
                // Record the new working copy as a checkout of this repository.
                // The New PR panel then pre-fills it.
                let checkouts = CheckoutsStore::global(cx);
                checkouts.update(cx, |store, cx| {
                    store.record(local_path.clone(), announcement.addr(), cx);
                });
                cx.open_with_system(&local_path);
                open_repo(dock_area, announcement, window, cx);
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

/// Open the newly created repository in the dock's center.
fn open_repo(
    dock_area: WeakEntity<DockArea>,
    announcement: Announcement,
    window: &mut Window,
    cx: &mut App,
) {
    open_repo_panel(&dock_area, &announcement, window, cx);
}
