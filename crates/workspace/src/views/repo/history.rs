use std::path::PathBuf;
use std::rc::Rc;

use anyhow::Error;
use dock::{DockArea, add_center_panel, panel_handle};
use gpui::prelude::*;
use gpui::{Context, Entity, Pixels, Render, Size, Task, WeakEntity, Window, div, px, size};
use gpui_component::scroll::Scrollbar;
use gpui_component::spinner::Spinner;
use gpui_component::{ActiveTheme, Sizable, VirtualListScrollHandle, v_flex, v_virtual_list};
use signed_git::CommitList;
use signed_state::RepoStore;
use signed_ui::placeholder;

use super::repo_display_name;
use crate::views::commit_diff::{COMMIT_ROW_HEIGHT, CommitDiffView, commit_row};

pub(super) struct RepoHistoryView {
    store: Entity<RepoStore>,
    dock_area: WeakEntity<DockArea>,
    worktree: Option<PathBuf>,
    all_commits: Option<CommitList>,
    loading_all_commits: bool,
    scroll_handle: VirtualListScrollHandle,
    item_sizes: Rc<Vec<Size<Pixels>>>,
    tasks: Vec<Task<Result<(), Error>>>,
}

impl RepoHistoryView {
    pub(super) fn new(store: Entity<RepoStore>, dock_area: WeakEntity<DockArea>) -> Self {
        Self {
            store,
            dock_area,
            worktree: None,
            all_commits: None,
            loading_all_commits: false,
            scroll_handle: VirtualListScrollHandle::new(),
            item_sizes: Rc::new(Vec::new()),
            tasks: Vec::new(),
        }
    }

    pub(super) fn set_worktree(&mut self, path: Option<PathBuf>) {
        self.worktree = path;
    }

    /// Number of commits reachable from HEAD, for the Commits tab badge.
    pub(super) fn commit_count(&self) -> Option<usize> {
        self.all_commits.as_ref().map(|list| list.total)
    }

    /// Drop the current list and walk HEAD again.
    pub(super) fn reload(&mut self, cx: &mut Context<Self>) {
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
                .background_spawn(async move { signed_git::worktree_all_commits(&worktree) })
                .await;

            this.update(cx, |this, cx| {
                if let Ok(list) = result {
                    let count = list.commits.len();
                    this.item_sizes = Rc::new(vec![size(px(0.), px(COMMIT_ROW_HEIGHT)); count]);
                    this.all_commits = Some(list);
                }

                this.loading_all_commits = false;
                cx.notify();
            })?;

            Ok(())
        });

        self.tasks.push(task);
    }

    pub(super) fn open_commit_diff(
        &mut self,
        commit_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(worktree) = self.worktree.clone() else {
            return;
        };

        let Some(dock_area) = self.dock_area.upgrade() else {
            return;
        };

        // Same display name as the repo detail panel's title.
        let repo_name = repo_display_name(self.store.read(cx));

        let panel =
            cx.new(|cx| CommitDiffView::new(worktree, repo_name, commit_id.into(), window, cx));

        dock_area.update(cx, |dock_area, cx| {
            add_center_panel(dock_area, panel_handle(panel), window, cx);
        });
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
                    .child(Spinner::new().small())
                    .into_any_element()
            } else {
                placeholder("Failed to load commits", cx)
            };
        };

        if list.commits.is_empty() {
            return placeholder("No commits found", cx);
        }

        let view = cx.entity().clone();
        let sizes = self.item_sizes.clone();
        let scroll_handle = self.scroll_handle.clone();
        let shown = list.commits.len();
        let total = list.total;

        v_flex()
            .relative()
            .flex_1()
            .w_full()
            .min_h_0()
            .child(
                v_virtual_list(view, "commits", sizes, move |this, range, _window, cx| {
                    let view = cx.entity().downgrade();
                    let commits = this
                        .all_commits
                        .as_ref()
                        .map(|list| list.commits.as_slice())
                        .unwrap_or(&[]);

                    range
                        .map(|ix| {
                            let id = commits[ix].id.clone();
                            let view = view.clone();

                            commit_row(
                                ix,
                                &commits[ix],
                                move |window, cx| {
                                    if let Some(view) = view.upgrade() {
                                        view.update(cx, |this, cx| {
                                            this.open_commit_diff(&id, window, cx)
                                        });
                                    }
                                },
                                cx,
                            )
                        })
                        .collect()
                })
                .track_scroll(&scroll_handle)
                .size_full(),
            )
            .when(shown < total, |this| {
                this.child(
                    div()
                        .py_2()
                        .w_full()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
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
                    .child(Scrollbar::vertical(&self.scroll_handle)),
            )
            .into_any_element()
    }
}
