use gpui::{App, Entity, SharedString, WeakEntity, Window};
use gpui_native_kit::layout::dock::{DockState, Panel};

/// Registers a panel builder under `id` and opens it in the dock.
pub fn open<P, F>(
    dock: &WeakEntity<DockState>,
    id: impl Into<SharedString>,
    build: F,
    window: &mut Window,
    cx: &mut App,
) where
    P: Panel + 'static,
    F: Fn(&mut Window, &mut App) -> Entity<P> + 'static,
{
    let id = id.into();

    if let Err(error) = dock.update(cx, |dock, cx| {
        dock.register_panel(id.clone(), build);
        dock.open_panel(&id, window, cx);
    }) {
        log::warn!("failed to open panel {id}: {error}");
    }
}
