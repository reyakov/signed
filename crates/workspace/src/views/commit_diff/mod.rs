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
use gpui_component::tree::{TreeEntry, TreeItem, TreeState, tree};
use gpui_component::{
    ActiveTheme, Sizable, StyledExt, VirtualListScrollHandle, h_flex, v_flex, v_virtual_list,
};
use signed_git::{CommitDiff, DiffHunk, DiffLine, DiffLineKind, DiffStatus, FileCommit, FileDiff};
use signed_state::RepoStore;
use signed_ui::{placeholder, tree_row};
use utils::relative_time_secs;

use crate::views::tree::{build_tree_items, tree_items};
use crate::views::{repo_tab_avatar, tab_title};

const TREE_WIDTH: f32 = 260.;

pub struct DiffPane {
    diff: Option<CommitDiff>,
    tree_state: Entity<TreeState>,
    selected_file: Option<SharedString>,
    /// Rows of the selected file's diff, hunk headers and lines.
    rows: Vec<DiffRow>,
    item_sizes: Rc<Vec<Size<Pixels>>>,
    scroll_handle: VirtualListScrollHandle,
}

impl DiffPane {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            diff: None,
            tree_state: cx.new(|cx| TreeState::new(cx)),
            selected_file: None,
            rows: Vec::new(),
            item_sizes: Rc::new(Vec::new()),
            scroll_handle: VirtualListScrollHandle::new(),
        }
    }

    pub fn diff(&self) -> Option<&CommitDiff> {
        self.diff.as_ref()
    }

    pub fn set_diff(&mut self, diff: CommitDiff, cx: &mut Context<Self>) {
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
        self.tree_state.update(cx, |state, cx| {
            state.set_items(items.clone(), cx);
            let item = find_item(&items, first.as_deref());
            state.set_selected_item(item, cx);
        });
        self.selected_file = first.clone();
        self.diff = Some(diff);
        if let Some(path) = first {
            self.set_diff_rows(path.as_ref());
        }
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.diff = None;
        self.selected_file = None;
        self.rows = Vec::new();
        self.item_sizes = Rc::new(Vec::new());
        self.tree_state.update(cx, |state, cx| {
            state.set_items(Vec::new(), cx);
        });
    }

    fn select_file(&mut self, path: &str, cx: &mut Context<Self>) {
        self.selected_file = Some(path.into());
        self.set_diff_rows(path);
        cx.notify();
    }

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
                    .when(self.diff.is_none(), |this| {
                        this.child(placeholder("No changes", cx))
                    }),
            )
            .into_any_element()
    }

    fn render_detail_column(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(diff) = self.diff.as_ref() else {
            return placeholder("No changes", cx);
        };
        let Some(path) = self.selected_file.clone() else {
            return if diff.files.is_empty() {
                placeholder("No files changed", cx)
            } else {
                placeholder("Select a file", cx)
            };
        };
        let Some(file) = diff.files.iter().find(|file| file.path == path.as_ref()) else {
            return placeholder("File not found", cx);
        };
        self.render_file_diff(file, cx.entity(), cx)
    }

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
}

impl Render for DiffPane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .size_full()
            .min_h_0()
            .bg(cx.theme().background)
            .child(self.render_tree_column(cx))
            .child(self.render_detail_column(cx))
    }
}

pub struct CommitDiffView {
    focus_handle: FocusHandle,
    store: Entity<RepoStore>,
    worktree: PathBuf,
    repo_name: SharedString,
    commit: FileCommit,
    /// The diff is being computed on a background task.
    loading: bool,
    error: Option<SharedString>,
    pane: Entity<DiffPane>,
}

impl CommitDiffView {
    pub fn new(
        store: Entity<RepoStore>,
        worktree: PathBuf,
        repo_name: SharedString,
        commit_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let pane = cx.new(DiffPane::new);

        // Defer until the window is ready, like the repository detail view.
        cx.defer_in(window, |this, window, cx| {
            this.load(window, cx);
        });

        Self {
            focus_handle: cx.focus_handle(),
            store,
            worktree,
            repo_name,
            commit: FileCommit {
                id: commit_id,
                summary: String::new(),
                description: None,
                author: String::new(),
                time: 0,
            },
            loading: true,
            error: None,
            pane,
        }
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.loading = true;
        self.error = None;
        cx.notify();

        let worktree = self.worktree.clone();
        let id = self.commit.id.clone();

        let task: gpui::Task<Result<(), anyhow::Error>> =
            cx.spawn_in(window, async move |this, cx| {
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
                            this.pane.update(cx, |pane, cx| pane.set_diff(diff, cx));
                        }
                        Err(error) => {
                            this.error = Some(error.to_string().into());
                        }
                    }
                    cx.notify();
                })?;

                Ok(())
            });

        task.detach();
    }

    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let commit = &self.commit;
        let (files, insertions, deletions) = self.pane.read(cx).diff().map_or((0, 0, 0), |diff| {
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
    fn title(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let avatar = repo_tab_avatar(self.store.read(cx), cx);
        let label = SharedString::from(format!("{}/{}", self.repo_name, self.commit.id));

        tab_title(avatar, label)
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
        let body: AnyElement = if self.loading {
            v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .child(Spinner::new().small())
                .into_any_element()
        } else if let Some(error) = self.error.clone() {
            placeholder(&error, cx)
        } else {
            self.pane.clone().into_any_element()
        };

        v_resizable("commit-diff")
            .child(
                resizable_panel()
                    .size(px(180.))
                    .size_range(px(120.)..px(420.))
                    .flex_none()
                    .bg(cx.theme().background)
                    .child(self.render_header(cx)),
            )
            .child(resizable_panel().child(body))
    }
}

const GUTTER_WIDTH: f32 = 44.;
const DIFF_ROW_HEIGHT: f32 = 20.;

#[derive(Clone, Copy)]
enum DiffRow {
    Hunk {
        old_start: u32,
        old_lines: u32,
        new_start: u32,
        new_lines: u32,
    },
    Line {
        hunk: usize,
        line: usize,
    },
}

fn diff_rows(file: &FileDiff) -> Vec<DiffRow> {
    let mut rows = Vec::new();
    for (hunk_ix, hunk) in file.hunks.iter().enumerate() {
        rows.push(DiffRow::Hunk {
            old_start: hunk.old_start,
            old_lines: hunk.old_lines,
            new_start: hunk.new_start,
            new_lines: hunk.new_lines,
        });
        rows.extend((0..hunk.lines.len()).map(|line| DiffRow::Line {
            hunk: hunk_ix,
            line,
        }));
    }
    rows
}

fn render_diff_row(hunks: &[DiffHunk], row: DiffRow, cx: &App) -> AnyElement {
    match row {
        DiffRow::Hunk {
            old_start,
            old_lines,
            new_start,
            new_lines,
        } => div()
            .px_2()
            .w_full()
            .h(px(DIFF_ROW_HEIGHT))
            .font_family(cx.theme().mono_font_family.clone())
            .text_xs()
            .bg(cx.theme().muted)
            .border_y(px(1.))
            .border_color(cx.theme().border)
            .text_color(cx.theme().muted_foreground)
            .child(SharedString::from(format!(
                "@@ -{},{} +{},{} @@",
                old_start, old_lines, new_start, new_lines
            )))
            .into_any_element(),
        DiffRow::Line { hunk, line } => render_diff_line(&hunks[hunk].lines[line], cx),
    }
}

fn render_diff_line(line: &DiffLine, cx: &App) -> AnyElement {
    let bg = match line.kind {
        DiffLineKind::Addition => Some(cx.theme().success.opacity(0.2)),
        DiffLineKind::Deletion => Some(cx.theme().danger.opacity(0.2)),
        DiffLineKind::Context => None,
    };
    let gutter = cx.theme().muted_foreground;

    // Fixed height and nowrap, the virtual list assumes every row has the same height.
    // Long lines are clipped instead of wrapped.
    h_flex()
        .w_full()
        .h(px(DIFF_ROW_HEIGHT))
        .items_center()
        .font_family(cx.theme().mono_font_family.clone())
        .text_xs()
        .when_some(bg, |this, bg| this.bg(bg))
        .child(
            div()
                .w(px(GUTTER_WIDTH))
                .flex_none()
                .pr_2()
                .text_right()
                .text_color(gutter)
                .child(line.old.map(|n| n.to_string()).unwrap_or_default()),
        )
        .child(
            div()
                .w(px(GUTTER_WIDTH))
                .flex_none()
                .pr_2()
                .text_right()
                .text_color(gutter)
                .child(line.new.map(|n| n.to_string()).unwrap_or_default()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_color(cx.theme().foreground)
                .child(line.text.clone()),
        )
        .into_any_element()
}

fn find_item<'a>(items: &'a [TreeItem], id: Option<&str>) -> Option<&'a TreeItem> {
    let id = id?;
    items.iter().find_map(|item| {
        if item.id.as_ref() == id {
            Some(item)
        } else {
            find_item(&item.children, Some(id))
        }
    })
}

pub(crate) const COMMIT_ROW_HEIGHT: f32 = 56.;

pub(crate) fn commit_row(
    ix: usize,
    commit: &FileCommit,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    h_flex()
        .id(ix)
        .px_4()
        .h(px(COMMIT_ROW_HEIGHT))
        .w_full()
        .gap_3()
        .items_center()
        .border_b(px(1.))
        .border_color(cx.theme().border)
        .hover(|this| this.bg(cx.theme().list_hover))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .justify_center()
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .overflow_hidden()
                        .child(
                            div()
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
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
                        .text_color(cx.theme().muted_foreground)
                        .child(commit.author.clone())
                        .child(relative_time_secs(commit.time)),
                ),
        )
        .on_click(move |_event, window, cx| on_click(window, cx))
        .into_any_element()
}
