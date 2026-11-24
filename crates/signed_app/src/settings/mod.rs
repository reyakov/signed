mod helpers;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    Window, px,
};
use helpers::{
    appearance_select, default_folder_selector, event_fetching_select, grasp_servers_editor,
    scan_paths_editor, setting_block, setting_row,
};
use settings::SettingsStore;
use ui::alert::{Alert, AlertSeverity};
use ui::common::v_flex;
use ui::dialog::{Dialog, DialogHandle};
use ui::input::InputState;
use ui::separator::Separator;

pub struct SettingsDialog {
    handle: DialogHandle,
    grasp_input: Entity<InputState>,
    default_folder: Entity<InputState>,
}

impl SettingsDialog {
    pub fn new(cx: &mut App) -> Entity<Self> {
        let store = SettingsStore::global(cx);
        let settings = store.read(cx).settings().clone();
        let grasp_input = InputState::new(cx);

        let folder = settings
            .create_repository
            .default_folder
            .clone()
            .unwrap_or_else(paths::desktop_dir);

        let default_folder = InputState::builder()
            .text(folder.to_string_lossy())
            .read_only()
            .build(cx);

        cx.new(|_| Self {
            handle: DialogHandle::new(true),
            grasp_input,
            default_folder,
        })
    }
}

impl Render for SettingsDialog {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        const APPEARANCE_DESC: &str =
            "Choose whether the app follows the system theme or uses a light/dark theme.";
        const FETCH_DESC: &str = "Curated fetches events from relays in the repository's announcement. Uncensored also fetches from every maintainer's relays.";
        const GRASP_DESC: &str =
            "Servers used to host your git repositories via the Grasp protocol";
        const SCAN_DESC: &str = "Directories scanned for local git repositories.";
        const FOLDER_DESC: &str = "The folder for newly created repositories.";

        let grasp_input = self.grasp_input.clone();
        let default_folder = self.default_folder.clone();

        Dialog::new("settings-dialog")
            .handle(self.handle.clone())
            .width(px(600.))
            .height(px(560.))
            .title("Settings")
            .content(move |_, cx| {
                let error = SettingsStore::global(cx).read(cx).error.clone();

                v_flex()
                    .gap_4()
                    .when_some(error, |this, error| {
                        this.child(
                            Alert::new("settings-error")
                                .severity(AlertSeverity::Danger)
                                .style(ui::alert::AlertStyle::Inline)
                                .title("Settings are not saved")
                                .message(error),
                        )
                    })
                    .child(setting_row(
                        cx,
                        "Appearance",
                        APPEARANCE_DESC,
                        appearance_select(cx),
                    ))
                    .child(Separator::horizontal())
                    .child(setting_row(
                        cx,
                        "Event Fetching Strategy",
                        FETCH_DESC,
                        event_fetching_select(cx),
                    ))
                    .child(Separator::horizontal())
                    .child(setting_block(
                        cx,
                        "Grasp Servers",
                        GRASP_DESC,
                        grasp_servers_editor(&grasp_input, cx),
                    ))
                    .child(Separator::horizontal())
                    .child(setting_block(
                        cx,
                        "Scan Directories",
                        SCAN_DESC,
                        scan_paths_editor(cx),
                    ))
                    .child(setting_block(
                        cx,
                        "Default Folder",
                        FOLDER_DESC,
                        default_folder_selector(&default_folder),
                    ))
                    .into_any_element()
            })
    }
}
