use dock::{DockArea, DockPlacement, panel_handle};
use gpui::prelude::*;
use gpui::{App, Entity, PathPromptOptions, SharedString, WeakEntity, Window, div, px};
use gpui_base::input::TextareaState;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::dialog::{DialogDescription, DialogFooter, DialogHeader, DialogTitle};
use gpui_component::form::{field, v_form};
use gpui_component::input::{Input, InputState, Textarea};
use gpui_component::{ActiveTheme, Disableable, IconName, WindowExt, h_flex};
use settings::SettingsStore;
use signed_core::Announcement;
use signed_state::Backend;

use super::super::RepoDetailView;
use super::grasp_servers::{GraspServersState, grasp_servers_field, load_user_grasp_servers};

/// Shared state for the Create Repository dialog, so async results can be rendered.
#[derive(Default)]
pub struct CreateRepoState {
    pub busy: bool,
    pub error: Option<SharedString>,
}

/// Open the Create Repository dialog.
///
/// The dialog loads the user's default grasp servers (kind `10317` grasp
/// list) and falls back to the shared defaults when none are set. On
/// success the dialog closes and the new repository opens in the dock.
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
        const FOLDER_NOTE: &str = "Where the repository is stored.";

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
                                    let grasp_state = grasp_state.clone();
                                    let dock_area = dock_area.clone();

                                    move |_ev, window, cx| {
                                        create_repository(
                                            name_input.clone(),
                                            desc_input.clone(),
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

/// Prompt the user to pick the folder the repository will be stored in, using
/// the platform's native folder picker, and show the result in the disabled
/// folder input. The picked folder is remembered in the settings so it
/// becomes the default next time.
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

/// Run the create-repository flow; closes the dialog and opens the new repository on success.
fn create_repository(
    name_input: Entity<InputState>,
    desc_input: Entity<TextareaState>,
    state: Entity<CreateRepoState>,
    grasp_state: Entity<GraspServersState>,
    dock_area: WeakEntity<DockArea>,
    window: &mut Window,
    cx: &mut App,
) {
    let name = name_input.read(cx).value().trim().to_owned();
    let description = desc_input.read(cx).value().trim().to_owned();
    let servers = grasp_state.read(cx).grasp_servers.clone();

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
