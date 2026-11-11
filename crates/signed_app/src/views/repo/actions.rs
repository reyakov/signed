use gpui::{App, AppContext as _, SharedString, WeakEntity, Window};
use gpui_native_kit::layout::dock::DockState;
use signed_core::{Announcement, RepoAddr};

use crate::panels;
use crate::views::repo::RepoDetailView;

/// Opens a repository as a dock panel, keyed by its address.
pub fn open_repo_panel(
    dock: &WeakEntity<DockState>,
    addr: &RepoAddr,
    hint: Option<&Announcement>,
    window: &mut Window,
    cx: &mut App,
) {
    let id = SharedString::from(format!("repo:{addr}"));
    let addr = addr.clone();
    let hint = hint.cloned();
    let dock_for_build = dock.clone();

    panels::open(
        dock,
        id,
        move |window, cx| {
            let addr = addr.clone();
            let hint = hint.clone();
            cx.new(|cx| RepoDetailView::new(dock_for_build.clone(), addr, hint, window, cx))
        },
        window,
        cx,
    );
}
