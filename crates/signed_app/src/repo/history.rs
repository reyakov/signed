use std::path::{Path, PathBuf};

use anyhow::Error;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Task, UniformListScrollHandle, WeakEntity, Window, div, uniform_list,
};
use signed_git::{CommitList, Repo};
use signed_state::{Backend, RepoStore, SyncStatusDirectory};
use ui::button::{Button, ButtonVariant};
use ui::common::{Sizable as _, v_flex};
use ui::icons::{Icon, IconRole};
use ui::scrollbar::Scrollbar;
use ui::spinner::Spinner;
use ui::theme::{ActiveTheme as _, TextRole, Typeset as _};
use ui::tooltip::Tooltip;

use super::RepoDetailView;
use super::commit_diff::CommitDiffView;
use super::helpers::commit_row;
use crate::shared::panels;
use crate::workspace::Workspace;

pub struct RepoHistoryView {
    store: Entity<RepoStore>,
    workspace: WeakEntity<Workspace>,
    detail: WeakEntity<RepoDetailView>,
    sync_directory: Entity<SyncStatusDirectory>,
    worktree: Option<PathBuf>,
    all_commits: Option<CommitList>,
    loading_all_commits: bool,
    scroll_handle: UniformListScrollHandle,
    tasks: Vec<Task<Result<(), Error>>>,
}

impl RepoHistoryView {
    pub fn new(
        store: Entity<RepoStore>,
        workspace: WeakEntity<Workspace>,
        detail: WeakEntity<RepoDetailView>,
        sync_directory: Entity<SyncStatusDirectory>,
    ) -> Self {
        Self {
            store,
            workspace,
            detail,
            sync_directory,
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

    pub fn worktree(&self) -> Option<&Path> {
        self.worktree.as_deref()
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

        let store = self.store.clone();
        let commit_id = commit_id.to_owned();
        let id = match store.read(cx).addr() {
            Some(addr) => SharedString::from(format!("commit:{addr}:{commit_id}")),
            None => SharedString::from(format!("commit:{}:{}", worktree.display(), commit_id)),
        };
        let workspace = self.workspace.clone();

        panels::open_detail(
            &workspace,
            id,
            move |window, cx| {
                cx.new(|cx| CommitDiffView::new(worktree.clone(), commit_id.clone(), window, cx))
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
        let detail = self.detail.clone();

        let user = Backend::global(cx).read(cx).current_user();
        let author = user.is_some_and(|user| self.store.read(cx).is_author(&user));
        let checkout = self.worktree.clone();

        let ahead = self
            .store
            .read(cx)
            .addr()
            .cloned()
            .filter(|_| author)
            .map(|addr| {
                self.sync_directory.read_with(cx, |directory, cx| {
                    directory.ahead_commits_of(&addr, checkout.as_deref(), cx)
                })
            })
            .unwrap_or_default();

        let pushing = self.store.read(cx).pushing;

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

                            let action = ahead
                                .iter()
                                .find(|(id, _)| id.starts_with(commit.id.as_str()))
                                .map(|(ahead, path)| {
                                    let detail = detail.clone();
                                    let path = path.clone();
                                    let target = ahead.clone();
                                    let short = commit.id.clone();

                                    Button::new(("sync-commit", ix))
                                        .label("Sync")
                                        .icon(Icon::named(IconRole::ArrowUp))
                                        .variant(ButtonVariant::Secondary)
                                        .small()
                                        .disabled(pushing)
                                        .tooltip(Tooltip::text(format!("Sync up to {short}")))
                                        .on_click(move |_, window, cx| {
                                            cx.stop_propagation();
                                            if let Some(detail) = detail.upgrade() {
                                                detail.update(cx, |this, cx| {
                                                    this.sync_checkout_commit(
                                                        path.clone(),
                                                        target.clone(),
                                                        window,
                                                        cx,
                                                    );
                                                });
                                            }
                                        })
                                        .into_any_element()
                                });

                            commit_row(
                                ix,
                                &commit,
                                action,
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
