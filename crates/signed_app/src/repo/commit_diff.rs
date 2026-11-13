use std::collections::HashSet;
use std::path::PathBuf;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Axis, Context, Entity, IntoElement, ParentElement as _,
    Render, ScrollStrategy, SharedString, Styled as _, Task, UniformListScrollHandle, Window, div,
    px, uniform_list,
};
use gpui_native_kit::badge::{Badge, BadgeVariant};
use gpui_native_kit::clipboard::ClipboardButton;
use gpui_native_kit::common::{h_flex, v_flex};
use gpui_native_kit::data::tree::{Tree, TreeItem};
use gpui_native_kit::icons::{Icon, IconRole};
use gpui_native_kit::layout::dock::Panel;
use gpui_native_kit::layout::resizable::{Resizable, ResizableState};
use gpui_native_kit::scrollbar::Scrollbar;
use gpui_native_kit::spinner::Spinner;
use gpui_native_kit::theme::{ActiveTheme as _, TextRole, Typeset as _};
use metadata::ProfileDirectory;
use signed_git::{CommitDiff, DiffStatus, FileCommit, FileDiff, Repo};
use signed_state::RepoStore;
use utils::relative_time_secs;

use super::helpers::{DiffRow, diff_rows, render_diff_row};
use crate::shared::tree::{TreeItemSeed, TreeRow, build_tree_items, collect_expanded, flatten};
use crate::shared::widgets::{empty_state, repo_tab_avatar};

pub struct DiffPane {
    diff: Option<CommitDiff>,
    seeds: Vec<TreeItemSeed>,
    expanded: HashSet<String>,
    rows: Vec<TreeRow>,
    selected_file: Option<SharedString>,
    selected_index: Option<usize>,
    /// Rows of the selected file's diff, hunk headers and lines.
    diff_rows: Vec<DiffRow>,
    scroll_handle: UniformListScrollHandle,
    split: Entity<ResizableState>,
}

impl DiffPane {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            diff: None,
            seeds: Vec::new(),
            expanded: HashSet::new(),
            rows: Vec::new(),
            selected_file: None,
            selected_index: None,
            diff_rows: Vec::new(),
            scroll_handle: UniformListScrollHandle::new(),
            split: {
                let split = ResizableState::new(cx);
                split.update(cx, |split, cx| split.set_sizes([px(260.)], cx));
                split
            },
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
        self.seeds = build_tree_items(&paths);
        self.expanded = collect_expanded(&self.seeds);
        self.rows = flatten(&self.seeds, &self.expanded);

        let first = diff
            .files
            .first()
            .map(|file| SharedString::from(file.path.as_str()));

        self.selected_file = first.clone();
        self.selected_index = first
            .as_ref()
            .and_then(|path| self.rows.iter().position(|row| row.id == path.as_ref()));
        self.diff = Some(diff);

        if let Some(path) = first {
            self.set_diff_rows(path.as_ref());
        } else {
            self.diff_rows.clear();
        }
        cx.notify();
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.diff = None;
        self.seeds.clear();
        self.expanded.clear();
        self.rows.clear();
        self.selected_file = None;
        self.selected_index = None;
        self.diff_rows.clear();
        cx.notify();
    }

    fn select_file(&mut self, path: &str, cx: &mut Context<Self>) {
        self.selected_file = Some(path.into());
        self.selected_index = self.rows.iter().position(|row| row.id == path);
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
        self.diff_rows = diff_rows(file);
        self.scroll_handle.scroll_to_item(0, ScrollStrategy::Top);
    }

    fn toggle_folder(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.expanded.remove(id) {
            self.expanded.insert(id.to_owned());
        }
        self.rows = flatten(&self.seeds, &self.expanded);
        self.selected_index = self
            .selected_file
            .as_ref()
            .and_then(|path| self.rows.iter().position(|row| row.id == path.as_ref()));
        cx.notify();
    }

    fn render_tree_column(&self, cx: &mut Context<Self>) -> AnyElement {
        let rows = self.rows.clone();
        let selected = self.selected_index;
        let view = cx.entity().downgrade();
        let change_view = view.clone();

        v_flex()
            .h_full()
            .min_h_0()
            .p_2()
            .child(
                Tree::new("commit-diff-tree")
                    .selected(selected)
                    .items(move |_, _| {
                        rows.iter()
                            .map(|row| {
                                let item = TreeItem::new(row.id.clone(), row.label.clone())
                                    .depth(row.depth)
                                    .expandable(row.expandable)
                                    .expanded(row.expanded);

                                if row.expandable {
                                    item.icon(Icon::named(IconRole::Folder))
                                } else {
                                    item.icon(Icon::named(IconRole::File))
                                }
                            })
                            .collect()
                    })
                    .on_change(move |index, window, cx| {
                        if let Some(index) = index
                            && let Err(error) = change_view.update(cx, |this, cx| {
                                this.activate_row(index, window, cx);
                            })
                        {
                            log::warn!("failed to activate diff tree row: {error}");
                        }
                    })
                    .on_disclosure({
                        let view = view;
                        move |index, _, cx| {
                            if let Err(error) = view.update(cx, |this, cx| {
                                let id = this.rows.get(index).map(|row| row.id.clone());
                                if let Some(id) = id {
                                    this.toggle_folder(&id, cx);
                                }
                            }) {
                                log::warn!("failed to toggle diff folder: {error}");
                            }
                        }
                    }),
            )
            .into_any_element()
    }

    fn activate_row(&mut self, index: usize, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(index) else {
            return;
        };
        let id = row.id.clone();
        let expandable = row.expandable;
        self.selected_index = Some(index);

        if expandable {
            self.toggle_folder(&id, cx);
        } else {
            self.select_file(&id, cx);
        }
    }

    fn render_detail_column(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(diff) = self.diff.as_ref() else {
            return empty_state("No changes");
        };
        let Some(path) = self.selected_file.clone() else {
            return if diff.files.is_empty() {
                empty_state("No files changed")
            } else {
                empty_state("Select a file")
            };
        };
        let Some(file) = diff.files.iter().find(|file| file.path == path.as_ref()) else {
            return empty_state("File not found");
        };
        self.render_file_diff(file, cx)
    }

    fn render_file_diff(&self, file: &FileDiff, cx: &App) -> AnyElement {
        let status_label = match file.status {
            DiffStatus::Added => "A",
            DiffStatus::Modified => "M",
            DiffStatus::Deleted => "D",
            DiffStatus::Renamed => "R",
            DiffStatus::Copied => "C",
        };
        let status_color = match file.status {
            DiffStatus::Added => cx.theme().ramps.success.base,
            DiffStatus::Modified => cx.theme().ramps.info.base,
            DiffStatus::Deleted => cx.theme().ramps.destructive.base,
            DiffStatus::Renamed | DiffStatus::Copied => cx.theme().colors.muted_foreground,
        };
        let title = match &file.old_path {
            Some(old) => format!("{old} → {}", file.path),
            None => file.path.clone(),
        };

        let body: AnyElement = if file.binary {
            empty_state("Diff not available")
        } else if file.hunks.is_empty() {
            empty_state("No content changes")
        } else {
            let hunks = file.hunks.clone();
            let rows = self.diff_rows.clone();

            v_flex()
                .size_full()
                .relative()
                .child(
                    uniform_list("commit-diff-rows", rows.len(), move |range, _, cx| {
                        range
                            .map(|ix| render_diff_row(&hunks, rows[ix], cx))
                            .collect()
                    })
                    .track_scroll(&self.scroll_handle)
                    .size_full(),
                )
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .child(Scrollbar::vertical(
                            "commit-diff-scroll",
                            self.scroll_handle.clone(),
                        )),
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
                    .child(
                        div()
                            .typeset(TextRole::Caption2, cx)
                            .text_color(status_color)
                            .child(status_label),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .typeset(TextRole::Caption2, cx)
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(title),
                    )
                    .when(!file.binary, |this| {
                        this.child(
                            h_flex()
                                .gap_2()
                                .typeset(TextRole::Caption, cx)
                                .child(
                                    div()
                                        .text_color(cx.theme().ramps.success.base)
                                        .child(format!("+{}", file.insertions)),
                                )
                                .child(
                                    div()
                                        .text_color(cx.theme().ramps.destructive.base)
                                        .child(format!("-{}", file.deletions)),
                                ),
                        )
                    }),
            )
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }
}

impl Render for DiffPane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tree = self.render_tree_column(cx);
        let detail = self.render_detail_column(cx);

        h_flex()
            .size_full()
            .min_h_0()
            .bg(cx.theme().colors.background)
            .child(
                Resizable::new("commit-diff-split", self.split.clone())
                    .axis(Axis::Horizontal)
                    .pane_with_min(tree, px(120.))
                    .pane_with_min(detail, px(240.)),
            )
    }
}

pub struct CommitDiffView {
    store: Entity<RepoStore>,
    worktree: PathBuf,
    repo_name: SharedString,
    commit: FileCommit,
    /// The diff is being computed on a background task.
    loading: bool,
    error: Option<SharedString>,
    pane: Entity<DiffPane>,
    /// Profile for the repository owner in the tab title.
    directory: Entity<ProfileDirectory>,
    tasks: Vec<Task<Result<(), anyhow::Error>>>,
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
        let directory = ProfileDirectory::new(cx);

        if let Some(owner) = store.read(cx).announcement.as_ref().map(|a| a.owner) {
            directory.update(cx, |directory, cx| {
                directory.track([owner], cx);
            });
        }

        let weak = cx.entity().downgrade();
        window.defer(cx, move |window, cx| {
            if let Err(error) = weak.update(cx, |this, cx| this.load(window, cx)) {
                log::warn!("failed to load commit diff: {error}");
            }
        });

        Self {
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
            directory,
            tasks: Vec::new(),
        }
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.loading = true;
        self.error = None;
        cx.notify();

        let worktree = self.worktree.clone();
        let id = self.commit.id.clone();

        let task: Task<Result<(), anyhow::Error>> = cx.spawn_in(window, async move |this, cx| {
            let commit = cx
                .background_spawn({
                    let worktree = worktree.clone();
                    let id = id.clone();
                    async move { Repo::open(&worktree).and_then(|repo| repo.commit(&id)) }
                })
                .await;
            let diff = cx
                .background_spawn({
                    let worktree = worktree.clone();
                    let id = id.clone();
                    async move { Repo::open(&worktree).and_then(|repo| repo.commit_diff(&id)) }
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

        self.tasks.push(task);
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
            .flex_none()
            .px_4()
            .py_3()
            .w_full()
            .gap_2()
            .border_b_1()
            .border_color(cx.theme().colors.border)
            .child(
                v_flex()
                    .child(
                        div()
                            .typeset(TextRole::Headline, cx)
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(commit.summary.clone()),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .typeset(TextRole::Body, cx)
                            .child(SharedString::from(format!("{} committed", commit.author)))
                            .child(
                                h_flex()
                                    .gap_1()
                                    .text_color(cx.theme().colors.muted_foreground)
                                    .child(SharedString::from(&commit.id))
                                    .child(ClipboardButton::new("commit-id", &commit.id)),
                            )
                            .child(
                                div()
                                    .text_color(cx.theme().colors.muted_foreground)
                                    .child(relative_time_secs(commit.time)),
                            ),
                    ),
            )
            .when_some(commit.description.as_ref(), |this, description| {
                this.child(
                    div()
                        .typeset(TextRole::Body, cx)
                        .child(SharedString::from(description)),
                )
            })
            .child(
                h_flex()
                    .gap_2()
                    .typeset(TextRole::Caption, cx)
                    .child(Badge::new("commit-files", format!("{files} files changed")))
                    .when(insertions > 0, |this| {
                        this.child(
                            Badge::new("commit-insertions", format!("+ {insertions}"))
                                .variant(BadgeVariant::Success),
                        )
                    })
                    .when(deletions > 0, |this| {
                        this.child(
                            Badge::new("commit-deletions", format!("- {deletions}"))
                                .variant(BadgeVariant::Danger),
                        )
                    }),
            )
            .into_any_element()
    }
}

impl Panel for CommitDiffView {
    fn title(&self, _cx: &App) -> Option<SharedString> {
        Some(SharedString::from(format!(
            "{}/{}",
            self.repo_name, self.commit.id
        )))
    }

    fn lead(&self, cx: &App) -> Option<AnyElement> {
        Some(repo_tab_avatar(self.store.read(cx), &self.directory, cx))
    }
}

impl Render for CommitDiffView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body: AnyElement = if self.loading {
            v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .child(Spinner::new("commit-diff-loading"))
                .into_any_element()
        } else if let Some(error) = self.error.clone() {
            empty_state(error)
        } else {
            self.pane.clone().into_any_element()
        };

        v_flex()
            .size_full()
            .min_h_0()
            .child(self.render_header(cx))
            .child(div().flex_1().min_h_0().child(body))
    }
}
