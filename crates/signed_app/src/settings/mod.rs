mod helpers;

use gpui::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    Window, px,
};
use gpui_native_kit::common::v_flex;
use gpui_native_kit::dialog::Dialog;
use gpui_native_kit::input::InputState;
use gpui_native_kit::separator::Separator;
use helpers::{
    appearance_select, default_folder_selector, event_fetching_select, grasp_servers_editor,
    scan_paths_editor, setting_block, setting_row,
};
use settings::SettingsStore;

use crate::shared::dialogs::open_global;

pub struct SettingsDialog {
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
            grasp_input,
            default_folder,
        })
    }
}

impl Render for SettingsDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let grasp_input = self.grasp_input.clone();
        let default_folder = self.default_folder.clone();
        let weak = cx.entity().downgrade();

        Dialog::new("settings-dialog")
            .open(true)
            .min_width(px(650.))
            .height(px(560.))
            .on_change(|open, _, cx| {
                if !open {
                    crate::shared::dialogs::close_global(cx);
                }
            })
            .title("Settings")
            .content(move |_, cx| {
                v_flex()
                            .gap_4()
                            .child(setting_row(
                                cx,
                                "Appearance",
                                "Choose whether the app follows the system theme or uses a light/dark theme.",
                                appearance_select(cx),
                            ))
                            .child(Separator::horizontal())
                            .child(setting_row(
                                cx,
                                "Event Fetching Strategy",
                                "Curated fetches events from relays in the repository's announcement. Uncensored also fetches from every maintainer's relays.",
                                event_fetching_select(cx),
                            ))
                            .child(Separator::horizontal())
                            .child(setting_block(
                                cx,
                                "Grasp Servers",
                                "Servers used to host your git repositories via the Grasp protocol",
                                grasp_servers_editor(&grasp_input, cx),
                            ))
                            .child(Separator::horizontal())
                            .child(setting_block(
                                cx,
                                "Scan Directories",
                                "Directories scanned for local git repositories.",
                                scan_paths_editor(cx),
                            ))
                            .child(setting_block(
                                cx,
                                "Default Folder",
                                "The folder for newly created repositories.",
                                default_folder_selector(&default_folder, weak.clone()),
                            ))
                    .into_any_element()
            })
    }
}

/// Opens the settings dialog.
pub fn open(cx: &mut App) {
    open_global(SettingsDialog::new(cx), cx);
}
