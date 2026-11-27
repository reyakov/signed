mod helpers;

use std::path::PathBuf;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext as _, Context, ElementId, Entity, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Task, Window, px,
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

use crate::shared::labels::{RelayHostExt as _, ServerInputExt as _};

/// One grasp server row, with its display strings built when settings change.
#[derive(Clone)]
pub(crate) struct ServerRow {
    pub(crate) id: ElementId,
    /// The stored server URL.
    pub(crate) url: String,
    /// The host shown on the row.
    pub(crate) host: SharedString,
}

/// One scan path row, with its label built when settings change.
#[derive(Clone)]
pub(crate) struct PathRow {
    pub(crate) id: ElementId,
    pub(crate) path: PathBuf,
    pub(crate) label: SharedString,
}

pub struct SettingsDialog {
    handle: DialogHandle,
    grasp_input: Entity<InputState>,
    default_folder: Entity<InputState>,
    /// Grasp server rows, rebuilt when the settings change.
    servers: Vec<ServerRow>,
    /// Scan path rows, rebuilt when the settings change.
    scan_paths: Vec<PathRow>,
    /// Prompts this dialog started, cancelled when it drops.
    tasks: Vec<Task<()>>,
    /// Rebuilds the rows when the settings change.
    _subscription: Option<Subscription>,
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

        let dialog = cx.new(|cx| {
            let mut dialog = Self {
                handle: DialogHandle::new(true),
                grasp_input,
                default_folder,
                servers: Vec::new(),
                scan_paths: Vec::new(),
                tasks: Vec::new(),
                _subscription: None,
            };
            dialog.rebuild_rows(cx);
            dialog
        });

        dialog.update(cx, |dialog, cx| {
            dialog._subscription = Some(cx.observe(&store, |this, _store, cx| {
                this.rebuild_rows(cx);
                cx.notify();
            }));
        });

        dialog
    }

    /// Rebuilds the row caches from the current settings.
    fn rebuild_rows(&mut self, cx: &App) {
        let store = SettingsStore::global(cx);
        let settings = store.read(cx).settings();

        self.servers = settings
            .grasp_servers
            .default_servers
            .iter()
            .map(|url| ServerRow {
                id: ElementId::from((
                    ElementId::from("settings-remove-server"),
                    SharedString::from(url.clone()),
                )),
                url: url.clone(),
                host: url.normalize_server().map_or_else(
                    || SharedString::from(url.clone()),
                    |(_, relay)| relay.host_label(),
                ),
            })
            .collect();

        self.scan_paths = settings
            .local_repos
            .scan_paths
            .iter()
            .map(|path| PathRow {
                id: ElementId::from((
                    ElementId::from("settings-remove-path"),
                    SharedString::from(path.display().to_string()),
                )),
                path: path.clone(),
                label: SharedString::from(path.display().to_string()),
            })
            .collect();
    }
}

impl Render for SettingsDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        const APPEARANCE_DESC: &str =
            "Choose whether the app follows the system theme or uses a light/dark theme.";
        const FETCH_DESC: &str = "Curated fetches events from relays in the repository's announcement. Uncensored also fetches from every maintainer's relays.";
        const GRASP_DESC: &str =
            "Servers used to host your git repositories via the Grasp protocol";
        const SCAN_DESC: &str = "Directories scanned for local git repositories.";
        const FOLDER_DESC: &str = "The folder for newly created repositories.";

        let grasp_input = self.grasp_input.clone();
        let default_folder = self.default_folder.clone();
        let servers = self.servers.clone();
        let scan_paths = self.scan_paths.clone();
        let this = cx.weak_entity();

        Dialog::new("settings-dialog")
            .handle(self.handle.clone())
            .width(px(600.))
            .height(px(560.))
            .title("Settings")
            .content(move |_, cx| {
                let store = SettingsStore::global(cx);
                let error = store.read(cx).error.clone();

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
                        grasp_servers_editor(&grasp_input, &servers, cx),
                    ))
                    .child(Separator::horizontal())
                    .child(setting_block(
                        cx,
                        "Scan Directories",
                        SCAN_DESC,
                        scan_paths_editor(&this, &scan_paths, cx),
                    ))
                    .child(setting_block(
                        cx,
                        "Default Folder",
                        FOLDER_DESC,
                        default_folder_selector(&default_folder, &this),
                    ))
                    .into_any_element()
            })
    }
}
