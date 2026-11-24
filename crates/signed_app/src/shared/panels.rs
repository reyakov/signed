use gpui::{App, AppContext, Entity, SharedString, WeakEntity, Window};
use ui::layout::panel::{Panel, PanelSlot};

use crate::app::AppRoot;
use crate::shared::log_dropped;

/// Opens a panel in `slot`, reusing it when it is already open.
fn open_slot<P, F>(
    slot: PanelSlot,
    workspace: &WeakEntity<AppRoot>,
    id: impl Into<SharedString>,
    build: F,
    window: &mut Window,
    cx: &mut App,
) where
    P: Panel + 'static,
    F: Fn(&mut Window, &mut App) -> Entity<P> + 'static,
{
    let result = workspace.update(cx, |workspace, cx| {
        workspace
            .panels()
            .clone()
            .update(cx, |panels, cx| panels.open(slot, id, build, window, cx))
    });
    if let Err(error) = result {
        log::warn!("failed to open panel: {error}");
    }
}

/// Opens a repository in the main panel, reusing it when it is already open.
pub fn open_primary<P, F>(
    workspace: &WeakEntity<AppRoot>,
    id: impl Into<SharedString>,
    build: F,
    window: &mut Window,
    cx: &mut App,
) where
    P: Panel + 'static,
    F: Fn(&mut Window, &mut App) -> Entity<P> + 'static,
{
    open_slot(PanelSlot::Main, workspace, id, build, window, cx);
}

/// Opens a detail panel beside the main panel, reusing it when it is already open.
pub fn open_detail<P, F>(
    workspace: &WeakEntity<AppRoot>,
    id: impl Into<SharedString>,
    build: F,
    window: &mut Window,
    cx: &mut App,
) where
    P: Panel + 'static,
    F: Fn(&mut Window, &mut App) -> Entity<P> + 'static,
{
    open_slot(PanelSlot::Split, workspace, id, build, window, cx);
}

/// Closes the detail panel open under `id`.
pub fn close_detail(workspace: &WeakEntity<AppRoot>, id: &SharedString, cx: &mut impl AppContext) {
    let result = workspace.update(cx, |workspace, cx| {
        workspace
            .panels()
            .clone()
            .update(cx, |panels, cx| panels.close(PanelSlot::Split, id, cx))
    });
    log_dropped(result);
}
