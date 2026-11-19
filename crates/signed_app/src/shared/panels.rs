use std::sync::Arc;

use gpui::{App, Entity, SharedString, WeakEntity, Window};
use ui::layout::panel::{Panel, PanelView};

use crate::workspace::Workspace;

/// Opens a repository in the main panel, reusing it when it is already cached.
pub fn open_primary<P, F>(
    workspace: &WeakEntity<Workspace>,
    id: impl Into<SharedString>,
    build: F,
    window: &mut Window,
    cx: &mut App,
) where
    P: Panel + 'static,
    F: Fn(&mut Window, &mut App) -> Entity<P> + 'static,
{
    let id = id.into();

    if workspace
        .read_with(cx, |workspace, _| workspace.has_main(&id))
        .unwrap_or(false)
    {
        if let Err(error) = workspace.update(cx, |workspace, cx| workspace.activate_main(&id, cx)) {
            log::warn!("failed to activate repository {id}: {error}");
        }
        return;
    }

    let view: Arc<dyn PanelView> = Arc::new(build(window, cx));

    if let Err(error) = workspace.update(cx, |workspace, cx| {
        workspace.show_main(id.clone(), view, cx)
    }) {
        log::warn!("failed to open repository {id}: {error}");
    }
}

/// Opens a detail panel beside the main panel, reusing it when it is already cached.
pub fn open_detail<P, F>(
    workspace: &WeakEntity<Workspace>,
    id: impl Into<SharedString>,
    build: F,
    window: &mut Window,
    cx: &mut App,
) where
    P: Panel + 'static,
    F: Fn(&mut Window, &mut App) -> Entity<P> + 'static,
{
    let id = id.into();

    if workspace
        .read_with(cx, |workspace, _| workspace.has_detail(&id))
        .unwrap_or(false)
    {
        if let Err(error) = workspace.update(cx, |workspace, cx| workspace.activate_detail(&id, cx))
        {
            log::warn!("failed to activate detail panel {id}: {error}");
        }
        return;
    }

    let view: Arc<dyn PanelView> = Arc::new(build(window, cx));

    if let Err(error) = workspace.update(cx, |workspace, cx| {
        workspace.show_detail(id.clone(), view, cx)
    }) {
        log::warn!("failed to open detail panel {id}: {error}");
    }
}
