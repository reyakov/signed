use gpui::{App, AppContext as _, SharedString, WeakEntity, Window};
use nostr::prelude::EventId;
use signed_core::{Announcement, RepoAddr};
use signed_state::RepoStore;

use crate::repo::RepoDetailView;
use crate::shared::panels;
use crate::workspace::Workspace;

/// An item of a repository to open from outside its detail panel.
pub enum RepoItem {
    Issue(EventId),
    PullRequest(EventId),
    Patch,
}

/// Opens a repository as the main panel, keyed by its address.
pub fn open_repo_panel(
    workspace: &WeakEntity<Workspace>,
    addr: &RepoAddr,
    hint: Option<&Announcement>,
    window: &mut Window,
    cx: &mut App,
) {
    let id = SharedString::from(format!("repo:{addr}"));
    let addr = addr.clone();
    let hint = hint.cloned();
    let workspace_for_build = workspace.clone();

    panels::open_primary(
        workspace,
        id,
        move |window, cx| {
            let addr = addr.clone();
            let hint = hint.clone();
            cx.new(|cx| RepoDetailView::new(workspace_for_build.clone(), addr, hint, window, cx))
        },
        window,
        cx,
    );
}

/// Opens an issue or pull request using a store built here.
pub fn open_repo_item(
    workspace: &WeakEntity<Workspace>,
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
            panels::open_detail(
                workspace,
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
            let workspace_for_build = workspace.clone();
            let store = store.clone();
            panels::open_detail(
                workspace,
                id,
                move |window, cx| {
                    cx.new(|cx| {
                        crate::pull_requests::PullRequestDetailView::new(
                            workspace_for_build.clone(),
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
        // Opening a patch series directly is not a feature.
        _ => {}
    }
}
