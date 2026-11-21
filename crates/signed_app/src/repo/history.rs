use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Error;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Task, UniformListScrollHandle, WeakEntity, Window, div,
};
use signed_git::{CommitList, FileCommit, Repo};
use signed_state::{Backend, RepoStore, SyncStatusDirectory};
use ui::alert::{Alert, AlertSeverity, AlertStyle};
use ui::button::{Button, ButtonVariant};
use ui::common::{Sizable as _, v_flex};
use ui::icons::{Icon, IconRole};
use ui::theme::ActiveTheme as _;
use ui::tooltip::Tooltip;

use super::RepoDetailView;
use super::commit_diff::CommitDiffView;
use crate::shared::event_list::commit_list;
use crate::shared::panels;
use crate::workspace::Workspace;

pub struct RepoHistoryView {
    store: Entity<RepoStore>,
    workspace: WeakEntity<Workspace>,
    detail: WeakEntity<RepoDetailView>,
    sync_directory: Entity<SyncStatusDirectory>,
    worktree: Option<PathBuf>,
    all_commits: Option<Arc<CommitList>>,
    loading_all_commits: bool,
    /// The loaded commit list, shared with the render without a per-frame copy.
    commits: Arc<Vec<FileCommit>>,
    /// The last load failure, shown instead of the empty list.
    error: Option<SharedString>,
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
            commits: Arc::new(Vec::new()),
            error: None,
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

    /// Reaps finished tasks, then queues another.
    fn push_task(&mut self, task: Task<Result<(), Error>>) {
        self.tasks.retain(|task| !task.is_ready());
        self.tasks.push(task);
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
                match result {
                    Ok(list) => {
                        this.commits = Arc::new(list.commits.clone());
                        this.all_commits = Some(Arc::new(list));
                        this.error = None;
                    }
                    Err(error) => this.error = Some(error.to_string().into()),
                }
                this.loading_all_commits = false;
                cx.notify();
            })?;

            Ok(())
        });

        self.push_task(task);
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
                cx.new(|cx| {
                    CommitDiffView::new(worktree.clone(), commit_id.clone(), true, window, cx)
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
                crate::shared::widgets::centered_spinner("commits-loading")
            } else if let Some(error) = self.error.clone() {
                let weak = cx.entity().downgrade();
                v_flex()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .child(
                        Alert::new("commits-error")
                            .severity(AlertSeverity::Danger)
                            .title("Failed to load commits")
                            .message(error)
                            .style(AlertStyle::Inline)
                            .on_dismiss(move |cx| {
                                if let Err(error) = weak.update(cx, |this, cx| {
                                    this.error = None;
                                    cx.notify();
                                }) {
                                    log::warn!("failed to dismiss commits error: {error}");
                                }
                            }),
                    )
                    .into_any_element()
            } else {
                crate::shared::widgets::empty_state("Failed to load commits")
            };
        };

        if self.commits.is_empty() {
            return crate::shared::widgets::empty_state("No commits found");
        }

        let commits = Arc::clone(&self.commits);
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

        let list = commit_list(
            "commits",
            "commits-scroll",
            commits,
            self.scroll_handle.clone(),
            move |ix, commit, _| {
                ahead
                    .iter()
                    .find(|(id, _)| id.starts_with(commit.id.as_str()))
                    .map(|(ahead_id, path)| {
                        let detail = detail.clone();
                        let path = path.clone();
                        let target = ahead_id.clone();
                        let short = super::helpers::short_id(&commit.id);

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
                    })
            },
            move |commit, window, cx| {
                if let Err(error) = view.update(cx, |this, cx| {
                    this.open_commit_diff(&commit.id, window, cx);
                }) {
                    log::warn!("failed to open commit diff: {error}");
                }
            },
            cx,
        );

        v_flex()
            .relative()
            .flex_1()
            .w_full()
            .min_h_0()
            .child(list)
            .when(shown < total, |this| {
                this.child(
                    div()
                        .py_2()
                        .w_full()
                        .text_xs()
                        .text_color(cx.theme().colors.muted_foreground)
                        .child(format!("Showing {shown} of {total} commits")),
                )
            })
            .into_any_element()
    }
}
