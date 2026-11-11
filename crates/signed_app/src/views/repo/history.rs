use std::path::PathBuf;

use anyhow::Error;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Task,
    UniformListScrollHandle, WeakEntity, Window, div, px, uniform_list,
};
use gpui_native_kit::common::{h_flex, v_flex};
use gpui_native_kit::empty_state::EmptyState;
use gpui_native_kit::layout::dock::DockState;
use gpui_native_kit::scrollbar::Scrollbar;
use gpui_native_kit::spinner::Spinner;
use gpui_native_kit::theme::ActiveTheme as _;
use signed_git::{CommitList, FileCommit, Repo};
use signed_state::RepoStore;
use utils::relative_time_secs;

use super::commit_diff::{COMMIT_ROW_HEIGHT, CommitDiffView};
use super::repo_display_name;
use crate::panels;

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
                empty_state("Failed to load commits")
            };
        };

        if list.commits.is_empty() {
            return empty_state("No commits found");
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
                        .text_xs()
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

fn empty_state(message: &'static str) -> gpui::AnyElement {
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .child(EmptyState::new(message))
        .into_any_element()
}

/// One commit row shared by the repository history and pull-request commit list.
pub fn commit_row(
    ix: usize,
    commit: &FileCommit,
    on_click: impl Fn(&mut Window, &mut gpui::App) + 'static,
    cx: &gpui::App,
) -> gpui::AnyElement {
    h_flex()
        .id(ix)
        .px_4()
        .h(px(COMMIT_ROW_HEIGHT))
        .w_full()
        .gap_3()
        .border_b_1()
        .border_color(cx.theme().colors.border)
        .hover(|this| this.bg(cx.theme().colors.muted_hover))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .justify_center()
                .child(
                    h_flex()
                        .gap_2()
                        .overflow_hidden()
                        .child(
                            div()
                                .font_family(cx.theme().mono_font)
                                .text_xs()
                                .text_color(cx.theme().colors.muted_foreground)
                                .child(commit.id.clone()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_sm()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(commit.summary.clone()),
                        ),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .text_xs()
                        .text_color(cx.theme().colors.muted_foreground)
                        .child(commit.author.clone())
                        .child(relative_time_secs(commit.time)),
                ),
        )
        .on_click(move |_, window, cx| on_click(window, cx))
        .into_any_element()
}
