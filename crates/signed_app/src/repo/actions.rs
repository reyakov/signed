use gpui::{App, AppContext as _, SharedString, WeakEntity, Window};
use gpui_native_kit::layout::dock::DockState;
use nostr::prelude::EventId;
use signed_core::{Announcement, RepoAddr};
use signed_state::RepoStore;

use crate::repo::RepoDetailView;
use crate::shared::panels;

/// An item of a repository to open from outside its detail panel.
pub enum RepoItem {
    Issue(EventId),
    PullRequest(EventId),
    Patch,
}

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

/// Opens an issue or pull request using a store built here.
pub fn open_repo_item(
    dock: &WeakEntity<DockState>,
    addr: &RepoAddr,
    hint: Option<&Announcement>,
    item: RepoItem,
    window: &mut Window,
    cx: &mut App,
) {
    let store = cx.new(|cx| RepoStore::new(addr.clone(), hint.cloned(), cx));
    match item {
        RepoItem::Issue(issue_id) => {
            let id = SharedString::from(format!("issue:{issue_id}"));
            let store = store.clone();
            panels::open(
                dock,
                id,
                move |_, cx| {
                    cx.new(|cx| {
                        crate::issues::detail::IssueDetailView::new(store.clone(), issue_id, cx)
                    })
                },
                window,
                cx,
            );
        }
        RepoItem::PullRequest(pr_id) => {
            let id = SharedString::from(format!("pr:{pr_id}"));
            let dock_for_build = dock.clone();
            let store = store.clone();
            panels::open(
                dock,
                id,
                move |window, cx| {
                    cx.new(|cx| {
                        crate::pull_requests::PullRequestDetailView::new(
                            dock_for_build.clone(),
                            store.clone(),
                            pr_id,
                            window,
                            cx,
                        )
                    })
                },
                window,
                cx,
            );
        }
        RepoItem::Patch => {}
    }
}
