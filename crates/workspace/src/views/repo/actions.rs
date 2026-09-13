use std::sync::Arc;

use dock::{DockArea, add_center_panel, panel_handle};
use gpui::prelude::*;
use gpui::{App, Entity, WeakEntity, Window};
use gpui_base::dock::PanelView;
use nostr::prelude::EventId;
use signed_core::{Announcement, RepoAddr};
use signed_state::RepoStore;

use super::RepoDetailView;
use crate::views::issues::detail::IssueDetailView;
use crate::views::pull_requests::detail::PullRequestDetailView;

/// Open repository as a panel in the dock's center.
pub(crate) fn open_repo_panel(
    dock_area: &WeakEntity<DockArea>,
    addr: &RepoAddr,
    hint: Option<&Announcement>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<RepoDetailView> {
    let detail = cx
        .new(|cx| RepoDetailView::new(dock_area.clone(), addr.clone(), hint.cloned(), window, cx));

    if let Some(dock_area) = dock_area.upgrade() {
        dock_area.update(cx, |dock_area, cx| {
            add_center_panel(dock_area, panel_handle(detail.clone()), window, cx);
        });
    }

    detail
}

/// The nostr store of `addr`'s repository, without opening a repository panel.
fn repo_store(addr: &RepoAddr, hint: Option<&Announcement>, cx: &mut App) -> Entity<RepoStore> {
    cx.new(|cx| RepoStore::new(addr.clone(), hint.cloned(), cx))
}

/// An item of a repository to open from outside its detail panel.
pub(crate) enum RepoItem {
    Issue(EventId),
    PullRequest(EventId),
    Patch,
}

/// The repository store is built here.
pub(crate) fn open_repo_item(
    dock_area: &WeakEntity<DockArea>,
    addr: &RepoAddr,
    hint: Option<&Announcement>,
    item: RepoItem,
    window: &mut Window,
    cx: &mut App,
) {
    let panel: Arc<dyn PanelView> =
        match item {
            RepoItem::Issue(issue_id) => {
                let store = repo_store(addr, hint, cx);
                panel_handle(cx.new(|cx| IssueDetailView::new(store, issue_id, window, cx)))
            }
            RepoItem::PullRequest(pr_id) => {
                let store = repo_store(addr, hint, cx);
                panel_handle(cx.new(|cx| {
                    PullRequestDetailView::new(dock_area.clone(), store, pr_id, window, cx)
                }))
            }
            RepoItem::Patch => return,
        };

    let Some(dock_area) = dock_area.upgrade() else {
        return;
    };

    dock_area.update(cx, |dock_area, cx| {
        add_center_panel(dock_area, panel, window, cx);
    });
}
