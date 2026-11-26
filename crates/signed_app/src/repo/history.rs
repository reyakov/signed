use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Error;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Task,
    UniformListScrollHandle, WeakEntity, Window, div, uniform_list,
};
use signed_git::{CommitList, FileCommit, Repo};
use signed_state::{Backend, RepoStore, SyncStatusDirectory};
use ui::alert::{Alert, AlertSeverity, AlertStyle};
use ui::avatar::IdentityRow;
use ui::button::{Button, ButtonSide, ButtonVariant};
use ui::common::{Sizable as _, h_flex, v_flex};
use ui::empty_state::EmptyState;
use ui::icons::{Icon, IconRole};
use ui::layout::panel::PanelSlot;
use ui::scrollbar::Scrollbar;
use ui::spinner::Spinner;
use ui::theme::ActiveTheme as _;
use ui::tooltip::Tooltip;
use utils::relative_time_secs;

use super::RepoDetailView;
use super::commit_diff::CommitDiffView;
use super::helpers::short_id;
use crate::app::AppRoot;
use crate::shared::log_dropped;

pub struct RepoHistoryView {
    store: Entity<RepoStore>,
    workspace: WeakEntity<AppRoot>,
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
    /// The latest walk, assigning it cancels the previous one.
    load_task: Option<Task<Result<(), Error>>>,
}

impl RepoHistoryView {
    pub fn new(
        store: Entity<RepoStore>,
        workspace: WeakEntity<AppRoot>,
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
            load_task: None,
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

        self.load_task = Some(task);
    }

    /// The panel id of `commit_id` in this repository's worktree, namespaced by `kind`.
    fn panel_id(&self, kind: &str, commit_id: &str, worktree: &Path, cx: &App) -> SharedString {
        match self.store.read(cx).addr() {
            Some(addr) => SharedString::from(format!("{kind}:{addr}:{commit_id}")),
            None => SharedString::from(format!("{kind}:{}:{commit_id}", worktree.display())),
        }
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

        let commit_id = commit_id.to_owned();
        let id = self.panel_id("commit", &commit_id, &worktree, cx);
        let workspace = self.workspace.clone();

        log_dropped(workspace.update(cx, |root, cx| {
            root.panels().clone().update(cx, |panels, cx| {
                panels.open(
                    PanelSlot::Split,
                    id,
                    move |window, cx| {
                        cx.new(|cx| {
                            CommitDiffView::new(
                                worktree.clone(),
                                commit_id.clone(),
                                true,
                                window,
                                cx,
                            )
                        })
                    },
                    window,
                    cx,
                )
            })
        }));
    }

    /// Shows the files of the listed commit `commit_id` in the repository panel.
    fn browse_commit(&mut self, commit_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        // The list comes from this worktree, which may be another checkout than the panel shows.
        let Some(source) = self.worktree.clone() else {
            return;
        };

        let Some(commit) = self
            .commits
            .iter()
            .find(|commit| commit.id == commit_id)
            .cloned()
        else {
            return;
        };

        log_dropped(self.detail.update(cx, |detail, cx| {
            detail.browse_commit(commit, source, window, cx);
        }));
    }
}

impl Render for RepoHistoryView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(list) = self.all_commits.as_ref() else {
            return if self.loading_all_commits {
                Spinner::centered("commits-loading")
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
                EmptyState::centered("Failed to load commits")
            };
        };

        if self.commits.is_empty() {
            return EmptyState::centered("No commits found");
        }

        let commits = Arc::clone(&self.commits);
        let shown = commits.len();
        let total = list.total;

        let view = cx.entity().downgrade();
        let detail = self.detail.clone();

        let backend = Backend::global(cx);
        let user = backend.read(cx).current_user();

        let author = user.is_some_and(|user| self.store.read(cx).is_author(&user));
        let checkout = self.worktree.clone();

        let pushing = self.store.read(cx).pushing;
        let scroll_handle = self.scroll_handle.clone();

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

        v_flex()
            .relative()
            .flex_1()
            .w_full()
            .min_h_0()
            .child(
                v_flex()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        uniform_list("commits", shown, move |range, _, cx| {
                            range
                                .map(|ix| {
                                    let Some(commit) = commits.get(ix) else {
                                        return div().into_any_element();
                                    };

                                    let row_id = commit.id.clone();
                                    let browse_id = commit.id.clone();
                                    let view = view.clone();
                                    let browse_view = view.clone();

                                    let ahead = ahead
                                        .iter()
                                        .find(|(id, _)| id.starts_with(commit.id.as_str()));

                                    h_flex()
                                        .id(commit.id.clone())
                                        .px_4()
                                        .h_12()
                                        .w_full()
                                        .gap_2()
                                        .text_sm()
                                        .border_b_1()
                                        .border_color(cx.theme().colors.border)
                                        .hover(|this| this.bg(cx.theme().colors.muted))
                                        .child(
                                            IdentityRow::new(
                                                commit.author.clone(),
                                                commit.author.clone(),
                                                None,
                                            )
                                            .small()
                                            .truncate()
                                            .gap_2()
                                            .flex_none()
                                            .w_32()
                                            .text_color(cx.theme().colors.muted_foreground),
                                        )
                                        .child(
                                            h_flex().w_24().child(
                                                Button::new(format!("browse-commit:{}", commit.id))
                                                    .label(short_id(&commit.id))
                                                    .variant(ButtonVariant::Ghost)
                                                    .side(ButtonSide::Left)
                                                    .small()
                                                    .tooltip(Tooltip::text(
                                                        "Browse files at commit",
                                                    ))
                                                    .on_click(move |_, window, cx| {
                                                        cx.stop_propagation();
                                                        log_dropped(browse_view.update(
                                                            cx,
                                                            |this, cx| {
                                                                this.browse_commit(
                                                                    &browse_id, window, cx,
                                                                );
                                                            },
                                                        ));
                                                    }),
                                            ),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .text_ellipsis()
                                                .whitespace_nowrap()
                                                .child(commit.summary.clone()),
                                        )
                                        .child(
                                            div()
                                                .flex_none()
                                                .text_xs()
                                                .text_color(cx.theme().colors.muted_foreground)
                                                .whitespace_nowrap()
                                                .child(relative_time_secs(commit.time)),
                                        )
                                        .when_some(ahead, |row, (ahead_id, path)| {
                                            let detail = detail.clone();
                                            let path = path.clone();
                                            let target = ahead_id.clone();
                                            let short = short_id(&commit.id);

                                            row.child(
                                                Button::new(("sync-commit", ix))
                                                    .label("Sync")
                                                    .icon(Icon::named(IconRole::ArrowUp))
                                                    .variant(ButtonVariant::Secondary)
                                                    .small()
                                                    .disabled(pushing)
                                                    .tooltip(Tooltip::text(format!(
                                                        "Sync up to {short}"
                                                    )))
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
                                                    }),
                                            )
                                        })
                                        .on_click(move |_, window, cx| {
                                            if let Err(error) = view.update(cx, |this, cx| {
                                                this.open_commit_diff(&row_id, window, cx);
                                            }) {
                                                log::warn!("failed to open commit diff: {error}");
                                            }
                                        })
                                        .into_any_element()
                                })
                                .collect()
                        })
                        .track_scroll(&scroll_handle)
                        .size_full(),
                    )
                    .child(Scrollbar::vertical("commits-scroll", scroll_handle)),
            )
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
