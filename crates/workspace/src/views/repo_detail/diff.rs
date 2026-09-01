use std::path::PathBuf;
use std::rc::Rc;

use dock::{BasePanel, Panel, PanelEvent};
use gpui::prelude::*;
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, Pixels, Render,
    ScrollStrategy, SharedString, Size, WeakEntity, Window, div, px, size,
};
use gpui_component::clipboard::Clipboard;
use gpui_component::list::ListItem;
use gpui_component::resizable::{resizable_panel, v_resizable};
use gpui_component::scroll::{ScrollableElement, Scrollbar};
use gpui_component::spinner::Spinner;
use gpui_component::tag::Tag;
use gpui_component::tree::{TreeEntry, TreeState, tree};
use gpui_component::{
    ActiveTheme, Sizable, StyledExt, VirtualListScrollHandle, h_flex, v_flex, v_virtual_list,
};
use signed_git::{CommitDiff, DiffStatus, FileCommit, FileDiff};
use signed_ui::{placeholder, tree_row};
use utils::relative_time_secs;

use super::helpers::{
    DIFF_ROW_HEIGHT, DiffRow, build_tree_items, diff_rows, find_item, render_diff_row, tree_items,
};

/// Width of the changed-files column.
const TREE_WIDTH: f32 = 260.;

/// Detail panel showing the diff of one commit.
pub struct CommitDiffView {
    focus_handle: FocusHandle,
    /// Local clone the commit lives in.
    worktree: PathBuf,
    /// Display name of the repository the commit belongs to.
    repo_name: SharedString,
    /// The commit being shown (header and tab title). Starts as an id-only
    /// stub; [`Self::load`] replaces it with the full metadata, which the
    /// history list intentionally omits.
    commit: FileCommit,
    /// Loaded diff; `None` while loading or after a failure.
    diff: Option<CommitDiff>,
    /// The diff is being computed on a background task.
    loading: bool,
    error: Option<SharedString>,
    /// Changed-files explorer state.
    tree_state: Entity<TreeState>,
    /// Path of the file whose diff is shown in the detail column.
    selected_file: Option<SharedString>,
    /// Rows of the selected file's diff (hunk headers + lines), backing the
    /// virtual list in the detail column.
    rows: Vec<DiffRow>,
    /// Per-row heights of [`Self::rows`].
    item_sizes: Rc<Vec<Size<Pixels>>>,
    /// Virtual list state of the diff rows.
    scroll_handle: VirtualListScrollHandle,
    /// In-flight tasks; pruned on every push (see [`helpers::track`]).
    tasks: Vec<gpui::Task<Result<(), anyhow::Error>>>,
}

impl CommitDiffView {
    pub fn new(
        worktree: PathBuf,
        repo_name: SharedString,
        commit_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let tree_state = cx.new(|cx| TreeState::new(cx));

        // Defer until the window is ready, like the repository detail view.
        cx.defer_in(window, |this, window, cx| {
            this.load(window, cx);
        });

        Self {
            focus_handle: cx.focus_handle(),
            worktree,
            repo_name,
            commit: FileCommit {
                id: commit_id,
                summary: String::new(),
                description: None,
                author: String::new(),
                time: 0,
            },
            diff: None,
            loading: true,
            error: None,
            tree_state,
            selected_file: None,
            rows: Vec::new(),
            item_sizes: Rc::new(Vec::new()),
            scroll_handle: VirtualListScrollHandle::new(),
            tasks: Vec::new(),
        }
    }

    /// Load the commit diff (and the full commit metadata) on a background
    /// task and populate the tree.
    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.loading = true;
        self.error = None;
        cx.notify();

        let worktree = self.worktree.clone();
        let id = self.commit.id.clone();

        let task = cx.spawn_in(window, async move |this, cx| {
            let commit = cx
                .background_spawn({
                    let worktree = worktree.clone();
                    let id = id.clone();
                    async move { signed_git::worktree_commit(&worktree, &id) }
                })
                .await;
            let diff = cx
                .background_spawn({
                    let worktree = worktree.clone();
                    let id = id.clone();
                    async move { signed_git::worktree_commit_diff(&worktree, &id) }
                })
                .await;

            this.update_in(cx, |this, _window, cx| {
                this.loading = false;
                if let Ok(Some(commit)) = commit {
                    this.commit = commit;
                }
                match diff {
                    Ok(diff) => {
                        let mut paths: Vec<PathBuf> = diff
                            .files
                            .iter()
                            .map(|file| PathBuf::from(&file.path))
                            .collect();
                        paths.sort();
                        let items = tree_items(build_tree_items(&paths), true);
                        let first = diff
                            .files
                            .first()
                            .map(|file| SharedString::from(file.path.as_str()));
                        this.tree_state.update(cx, |state, cx| {
                            state.set_items(items.clone(), cx);
                            let item = find_item(&items, first.as_deref());
                            state.set_selected_item(item, cx);
                        });
                        this.selected_file = first.clone();
                        this.diff = Some(diff);
                        if let Some(path) = first {
                            this.set_diff_rows(path.as_ref());
                        }
                    }
                    Err(error) => {
                        this.error = Some(error.to_string().into());
                    }
                }
                cx.notify();
            })?;

            Ok(())
        });

        self.tasks.push(task);
    }

    /// Show the diff of the file at `path` (selected in the tree).
    fn select_file(&mut self, path: &str, cx: &mut Context<Self>) {
        self.selected_file = Some(path.into());
        self.set_diff_rows(path);
        cx.notify();
    }

    /// Rebuild the virtual list state for the file at `path` and scroll back
    /// to the top.
    fn set_diff_rows(&mut self, path: &str) {
        let Some(diff) = self.diff.as_ref() else {
            return;
        };
        let Some(file) = diff.files.iter().find(|file| file.path == path) else {
            return;
        };
        self.rows = diff_rows(file);
        self.item_sizes = Rc::new(vec![size(px(0.), px(DIFF_ROW_HEIGHT)); self.rows.len()]);
        self.scroll_handle.scroll_to_item(0, ScrollStrategy::Top);
    }

    /// One row of the changed-files tree: icon + name, indented by depth.
    fn render_tree_item(
        ix: usize,
        entry: &TreeEntry,
        selected: bool,
        view: &WeakEntity<Self>,
    ) -> ListItem {
        let view = view.clone();
        let id = entry.item().id.clone();

        tree_row(ix, entry, selected, move |_window, cx| {
            if let Some(view) = view.upgrade() {
                view.update(cx, |this, cx| this.select_file(&id, cx));
            }
        })
    }

    /// Left column: the changed-files tree.
    fn render_tree_column(&self, cx: &mut Context<Self>) -> AnyElement {
        let tree_state = self.tree_state.clone();
        let view = cx.entity().downgrade();

        v_flex()
            .h_full()
            .w(px(TREE_WIDTH))
            .flex_none()
            .border_r_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .when(self.diff.is_some(), |this| {
                        this.child(
                            tree(&tree_state, move |ix, entry, selected, _window, _cx| {
                                Self::render_tree_item(ix, entry, selected, &view)
                            })
                            .p_2(),
                        )
                    })
                    .when(self.diff.is_none() && !self.loading, |this| {
                        this.child(placeholder("Failed to load diff", cx))
                    }),
            )
            .into_any_element()
    }

    /// Right column: header of the selected file plus its diff.
    fn render_detail_column(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.loading {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .child(Spinner::new().small())
                .into_any_element();
        }
        if let Some(error) = self.error.clone() {
            return placeholder(&error, cx);
        }
        let Some(diff) = self.diff.as_ref() else {
            return placeholder("Failed to load diff", cx);
        };
        let Some(path) = self.selected_file.clone() else {
            return if diff.files.is_empty() {
                placeholder("No files changed in this commit", cx)
            } else {
                placeholder("Select a file", cx)
            };
        };
        let Some(file) = diff.files.iter().find(|file| file.path == path.as_ref()) else {
            return placeholder("File not found", cx);
        };
        self.render_file_diff(file, cx.entity(), cx)
    }

    /// The diff of one file: a header with status and stats, then the hunks
    /// in a virtual list (a large diff is never materialized per frame).
    fn render_file_diff(&self, file: &FileDiff, view: Entity<Self>, cx: &App) -> AnyElement {
        let status_label = match file.status {
            DiffStatus::Added => "A",
            DiffStatus::Modified => "M",
            DiffStatus::Deleted => "D",
            DiffStatus::Renamed => "R",
            DiffStatus::Copied => "C",
        };
        let status_color = match file.status {
            DiffStatus::Added => cx.theme().success,
            DiffStatus::Modified => cx.theme().info,
            DiffStatus::Deleted => cx.theme().danger,
            DiffStatus::Renamed | DiffStatus::Copied => cx.theme().muted_foreground,
        };
        let title = match &file.old_path {
            Some(old) => format!("{old} → {}", file.path),
            None => file.path.clone(),
        };

        let body: AnyElement = if file.binary {
            placeholder("Diff not available", cx)
        } else if file.hunks.is_empty() {
            placeholder("No content changes", cx)
        } else {
            let sizes = self.item_sizes.clone();
            let scroll_handle = self.scroll_handle.clone();
            v_flex()
                .size_full()
                .relative()
                .child(
                    v_virtual_list(
                        view,
                        "commit-diff-rows",
                        sizes,
                        move |this, range, _window, cx| {
                            let Some(diff) = this.diff.as_ref() else {
                                return Vec::new();
                            };
                            let Some(path) = this.selected_file.as_deref() else {
                                return Vec::new();
                            };
                            let Some(file) = diff.files.iter().find(|file| file.path == path)
                            else {
                                return Vec::new();
                            };
                            range
                                .map(|ix| render_diff_row(&file.hunks, this.rows[ix], cx))
                                .collect()
                        },
                    )
                    .track_scroll(&scroll_handle)
                    .size_full(),
                )
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .child(Scrollbar::vertical(&scroll_handle)),
                )
                .into_any_element()
        };

        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                h_flex()
                    .px_3()
                    .h_9()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(status_color)
                            .child(status_label),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .font_semibold()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(title),
                    )
                    .when(!file.binary, |this| {
                        this.child(
                            h_flex()
                                .gap_2()
                                .text_xs()
                                .child(
                                    div()
                                        .text_color(cx.theme().success)
                                        .child(format!("+{}", file.insertions)),
                                )
                                .child(
                                    div()
                                        .text_color(cx.theme().danger)
                                        .child(format!("-{}", file.deletions)),
                                ),
                        )
                    }),
            )
            .child(div().id("commit-diff-body").flex_1().min_h_0().child(body))
            .into_any_element()
    }

    /// Header: commit id, summary, author/time and overall change stats.
    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let commit = &self.commit;
        let (files, insertions, deletions) = self.diff.as_ref().map_or((0, 0, 0), |diff| {
            (
                diff.files.len(),
                diff.files.iter().map(|file| file.insertions).sum(),
                diff.files.iter().map(|file| file.deletions).sum(),
            )
        });

        v_flex()
            .px_4()
            .pb_4()
            .w_full()
            .gap_4()
            .child(
                v_flex()
                    .child(
                        div()
                            .font_semibold()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(commit.summary.clone()),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .text_sm()
                            .child(h_flex().child(format!("{} committed", commit.author)))
                            .child(
                                h_flex()
                                    .gap_0p5()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(SharedString::from(&commit.id))
                                    .child(Clipboard::new("commit").value(&commit.id)),
                            )
                            .child(
                                div()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(relative_time_secs(commit.time)),
                            ),
                    ),
            )
            .when_some(commit.description.as_ref(), |this, description| {
                this.child(div().text_sm().child(SharedString::from(description)))
            })
            .child(
                h_flex()
                    .gap_2()
                    .text_xs()
                    .child(
                        Tag::primary()
                            .small()
                            .child(format!("{files} files changed")),
                    )
                    .when(insertions > 0, |this| {
                        this.child(
                            Tag::success()
                                .outline()
                                .small()
                                .child(format!("+ {insertions}")),
                        )
                    })
                    .when(deletions > 0, |this| {
                        this.child(
                            Tag::danger()
                                .outline()
                                .small()
                                .child(format!("- {deletions}")),
                        )
                    }),
            )
            .overflow_y_scrollbar()
            .into_any_element()
    }
}

impl BasePanel for CommitDiffView {
    fn panel_name(&self) -> &'static str {
        "commit_diff"
    }
}

impl Panel for CommitDiffView {
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().text_sm().child(SharedString::from(format!(
            "{}/{}",
            self.repo_name, self.commit.id
        )))
    }
}

impl EventEmitter<PanelEvent> for CommitDiffView {}

impl Focusable for CommitDiffView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for CommitDiffView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_resizable("commit-diff")
            .child(
                resizable_panel()
                    .size(px(180.))
                    .size_range(px(120.)..px(420.))
                    .flex_none()
                    .bg(cx.theme().background)
                    .child(self.render_header(cx)),
            )
            .child(
                resizable_panel().child(
                    h_flex()
                        .size_full()
                        .min_h_0()
                        .bg(cx.theme().background)
                        .child(self.render_tree_column(cx))
                        .child(self.render_detail_column(cx)),
                ),
            )
    }
}
