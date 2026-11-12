use std::path::PathBuf;

use anyhow::Error;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Task, UniformListScrollHandle, WeakEntity, Window, div, uniform_list,
};
use gpui_native_kit::common::v_flex;
use gpui_native_kit::layout::dock::DockState;
use gpui_native_kit::scrollbar::Scrollbar;
use gpui_native_kit::spinner::Spinner;
use gpui_native_kit::theme::{ActiveTheme as _, TextRole, Typeset as _};
use signed_git::{CommitList, Repo};
use signed_state::RepoStore;

use super::commit_diff::CommitDiffView;
use super::helpers::commit_row;
use super::helpers::repo_display_name;
use crate::shared::panels;

pub struct RepoHistoryView {
    store: Entity<RepoStore>,
    dock: WeakEntity<DockState>,
    worktree: Option<PathBuf>,
    all_commits: Option<CommitList>,
    loading_all_commits: bool,
    scroll_handle: UniformListScrollHandle,
    tasks: Vec<Task<Result<(), Error>>>,
}

impl RepoHistoryView {
    pub fn new(store: Entity<RepoStore>, dock: WeakEntity<DockState>) -> Self {
        Self {
            store,
            dock,
            worktree: None,
            all_commits: None,
            loading_all_commits: false,
            scroll_handle: UniformListScrollHandle::new(),
            tasks: Vec::new(),
        }
    }

    pub fn set_worktree(&mut self, path: Option<PathBuf>) {
        self.worktree = path;
    }

    /// Number of commits reachable from HEAD, for the Commits tab label.
    pub fn commit_count(&self) -> Option<usize> {
        self.all_commits.as_ref().map(|list| list.total)
    }

    /// Drop the current list and walk HEAD again.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.all_commits = None;
        self.loading_all_commits = false;
        self.load(cx);
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        if self.loading_all_commits || self.all_commits.is_some() {
            return;
        }

        let Some(worktree) = self.worktree.clone() else {
            return;
        };

        self.loading_all_commits = true;

        let task: Task<Result<(), Error>> = cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    Repo::open_cached(&worktree).and_then(|repo| repo.all_commits())
                })
                .await;

            this.update(cx, |this, cx| {
                if let Ok(list) = result {
                    this.all_commits = Some(list);
                }
                this.loading_all_commits = false;
                cx.notify();
            })?;

            Ok(())
        });

        self.tasks.push(task);
    }

    pub fn open_commit_diff(
        &mut self,
        commit_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(worktree) = self.worktree.clone() else {
            return;
        };

        // Same display name as the repo detail panel's title.
        let repo_name = repo_display_name(self.store.read(cx));
        let store = self.store.clone();
        let commit_id = commit_id.to_owned();
        let id = match store.read(cx).addr() {
            Some(addr) => SharedString::from(format!("commit:{addr}:{commit_id}")),
            None => SharedString::from(format!("commit:{}:{}", worktree.display(), commit_id)),
        };
        let dock = self.dock.clone();

        panels::open(
            &dock,
            id,
            move |window, cx| {
                cx.new(|cx| {
                    CommitDiffView::new(
                        store.clone(),
                        worktree.clone(),
                        repo_name.clone(),
                        commit_id.clone(),
                        window,
                        cx,
                    )
                })
            },
            window,
            cx,
        );
    }
}

impl Render for RepoHistoryView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(list) = self.all_commits.as_ref() else {
            return if self.loading_all_commits {
                v_flex()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .child(Spinner::new("commits-loading"))
                    .into_any_element()
            } else {
                crate::shared::widgets::empty_state("Failed to load commits")
            };
        };

        if list.commits.is_empty() {
            return crate::shared::widgets::empty_state("No commits found");
        }

        let commits = list.commits.clone();
        let shown = commits.len();
        let total = list.total;
        let view = cx.entity().downgrade();

        v_flex()
            .relative()
            .flex_1()
            .w_full()
            .min_h_0()
            .child(
                uniform_list("commits", shown, move |range, _, cx| {
                    range
                        .map(|ix| {
                            let commit = commits[ix].clone();
                            let id = commit.id.clone();
                            let view = view.clone();
                            commit_row(
                                ix,
                                &commit,
                                move |window, cx| {
                                    if let Err(error) = view.update(cx, |this, cx| {
                                        this.open_commit_diff(&id, window, cx);
                                    }) {
                                        log::warn!("failed to open commit diff: {error}");
                                    }
                                },
                                cx,
                            )
                        })
                        .collect()
                })
                .track_scroll(&self.scroll_handle)
                .size_full(),
            )
            .when(shown < total, |this| {
                this.child(
                    div()
                        .py_2()
                        .w_full()
                        .typeset(TextRole::Caption, cx)
                        .text_color(cx.theme().colors.muted_foreground)
                        .child(format!("Showing {shown} of {total} commits")),
                )
            })
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .child(Scrollbar::vertical(
                        "commits-scroll",
                        self.scroll_handle.clone(),
                    )),
            )
            .into_any_element()
    }
}
