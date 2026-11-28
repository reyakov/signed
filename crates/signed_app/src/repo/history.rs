use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use anyhow::Error;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext as _, Context, ElementId, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Task,
    UniformListScrollHandle, WeakEntity, Window, div, uniform_list,
};
use signed_git::{CommitList, FileCommit, Repo};
use signed_state::RepoStore;
use ui::alert::{Alert, AlertSeverity, AlertStyle};
use ui::avatar::Avatar;
use ui::button::{Button, ButtonVariant};
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
use crate::app::AppRoot;
use crate::shared::commit_diff::CommitDiffView;
use crate::shared::labels::ShortIdExt as _;
use crate::shared::log_dropped;

/// One commit row, with its display strings built when the list loads.
struct HistoryRow {
    commit: FileCommit,
    /// The commit's hex id.
    id: SharedString,
    /// The abbreviated id shown on the browse button.
    short: SharedString,
    /// The browse button's element id.
    browse: ElementId,
    /// The row's age label.
    age: SharedString,
}

pub struct RepoHistoryView {
    store: Entity<RepoStore>,
    workspace: WeakEntity<AppRoot>,
    detail: WeakEntity<RepoDetailView>,
    worktree: Option<PathBuf>,
    all_commits: Option<Arc<CommitList>>,
    loading_all_commits: bool,
    /// The loaded commit list, shared with the render without a per-frame copy.
    commits: Arc<Vec<FileCommit>>,
    rows: Arc<Vec<HistoryRow>>,
    /// Commit ids that exist in a local checkout but not on Nostr.
    ahead: Arc<HashMap<String, PathBuf>>,
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
    ) -> Self {
        Self {
            store,
            workspace,
            detail,
            worktree: None,
            all_commits: None,
            loading_all_commits: false,
            commits: Arc::new(Vec::new()),
            rows: Arc::new(Vec::new()),
            ahead: Arc::new(HashMap::new()),
            error: None,
            scroll_handle: UniformListScrollHandle::new(),
            load_task: None,
        }
    }

    pub fn set_worktree(&mut self, path: Option<PathBuf>) {
        self.worktree = path;
    }

    pub fn set_ahead(&mut self, ahead: HashMap<String, PathBuf>, cx: &mut Context<Self>) {
        let next = Arc::new(ahead);
        if *self.ahead != *next {
            self.ahead = next;
            cx.notify();
        }
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
        self.loading_all_commits = true;

        let Some(worktree) = self.worktree.clone() else {
            return;
        };

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
                        this.rows = Arc::new(
                            list.commits
                                .iter()
                                .map(|commit| HistoryRow {
                                    id: SharedString::from(commit.id.clone()),
                                    short: commit.id.short_id(),
                                    browse: ElementId::from((
                                        ElementId::from("browse-commit"),
                                        SharedString::from(commit.id.clone()),
                                    )),
                                    age: SharedString::from(relative_time_secs(commit.time)),
                                    commit: commit.clone(),
                                })
                                .collect(),
                        );
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
        let scroll_handle = self.scroll_handle.clone();

        let Some(list) = self.all_commits.as_ref() else {
            return if self.loading_all_commits {
                Spinner::centered("commits-loading").into_any_element()
            } else if let Some(error) = self.error.clone() {
                let weak = cx.weak_entity();

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
                EmptyState::centered("Failed to load commits").into_any_element()
            };
        };

        if self.commits.is_empty() {
            return EmptyState::centered("No commits found").into_any_element();
        }

        let commits = Arc::clone(&self.rows);
        let shown = commits.len();
        let total = list.total;

        let open_commit = Rc::new(cx.processor(|this, commit_id: SharedString, window, cx| {
            this.open_commit_diff(&commit_id, window, cx);
        }));

        let browse_commit = Rc::new(cx.processor(|this, commit_id: SharedString, window, cx| {
            this.browse_commit(&commit_id, window, cx);
        }));
        let detail = self.detail.clone();

        let pushing = self.store.read(cx).pushing;
        let ahead = Arc::clone(&self.ahead);

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
                                    let open_commit = open_commit.clone();
                                    let browse_commit = browse_commit.clone();

                                    let ahead = ahead
                                        .get(commit.id.as_str())
                                        .map(|path| (commit.id.to_string(), path.clone()));

                                    h_flex()
                                        .id(row_id.clone())
                                        .px_4()
                                        .h_12()
                                        .w_full()
                                        .gap_2()
                                        .border_b_1()
                                        .border_color(cx.theme().colors.border)
                                        .hover(|this| this.bg(cx.theme().colors.muted))
                                        .child(
                                            h_flex()
                                                .gap_2()
                                                .flex_none()
                                                .w_40()
                                                .text_xs()
                                                .text_color(cx.theme().colors.muted_foreground)
                                                .child(
                                                    Avatar::new(commit.commit.author.clone())
                                                        .name(commit.commit.author.clone())
                                                        .small(),
                                                )
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .overflow_hidden()
                                                        .whitespace_nowrap()
                                                        .text_ellipsis()
                                                        .child(commit.commit.author.clone()),
                                                ),
                                        )
                                        .child(
                                            h_flex().w_20().child(
                                                Button::new(commit.browse.clone())
                                                    .label(commit.short.clone())
                                                    .variant(ButtonVariant::Ghost)
                                                    .small()
                                                    .tooltip(Tooltip::text(
                                                        "Browse files at commit",
                                                    ))
                                                    .on_click(move |_, window, cx| {
                                                        cx.stop_propagation();
                                                        browse_commit(
                                                            browse_id.clone(),
                                                            window,
                                                            cx,
                                                        );
                                                    }),
                                            ),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .text_ellipsis()
                                                .text_sm()
                                                .whitespace_nowrap()
                                                .child(commit.commit.summary.clone()),
                                        )
                                        .child(
                                            div()
                                                .flex_none()
                                                .text_xs()
                                                .text_color(cx.theme().colors.muted_foreground)
                                                .whitespace_nowrap()
                                                .child(commit.age.clone()),
                                        )
                                        .when_some(ahead, |row, (ahead_id, path)| {
                                            let path = path.clone();
                                            let target = ahead_id.clone();
                                            let detail = detail.clone();
                                            let tooltip = SharedString::from(format!(
                                                "Sync up to {}",
                                                commit.short
                                            ));

                                            row.child(
                                                Button::new(("sync-commit", ix))
                                                    .label("Sync")
                                                    .icon(Icon::named(IconRole::ArrowUp))
                                                    .variant(ButtonVariant::Secondary)
                                                    .small()
                                                    .loading(pushing)
                                                    .disabled(pushing)
                                                    .tooltip(Tooltip::text(tooltip))
                                                    .on_click(move |_, window, cx| {
                                                        cx.stop_propagation();
                                                        log_dropped(detail.update(
                                                            cx,
                                                            |this, cx| {
                                                                this.sync_checkout_commit(
                                                                    path.clone(),
                                                                    target.clone(),
                                                                    window,
                                                                    cx,
                                                                );
                                                            },
                                                        ));
                                                    }),
                                            )
                                        })
                                        .on_click(move |_, window, cx| {
                                            open_commit(row_id.clone(), window, cx);
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
