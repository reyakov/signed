use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Axis, Context, Entity, EventEmitter, InteractiveElement as _,
    IntoElement, ListAlignment, ListState, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, WeakEntity, Window, div,
    list, px,
};
use signed_git::{CommitDiff, FileCommit, FileDiff, Repo};
use ui::avatar::Avatar;
use ui::badge::GroupedBadge;
use ui::button::{Button, ButtonVariant};
use ui::clipboard::ClipboardButton;
use ui::common::{Sizable as _, StyledExt as _, h_flex, v_flex};
use ui::empty_state::EmptyState;
use ui::icons::{Icon, IconRole};
use ui::layout::panel::Panel;
use ui::layout::resizable::{Resizable, ResizableState};
use ui::scrollbar::Scrollbar;
use ui::separator::Separator;
use ui::spinner::Spinner;
use ui::switch::Switch;
use ui::theme::ActiveTheme as _;
use ui::toggle::Toggle;
use ui::tooltip::Tooltip;
use ui::tree::{Tree, TreeItem, TreeItemSeed};
use utils::relative_time_secs;

use super::helpers::{DiffRow, co_authors, diff_rows, short_id};

/// Emitted when the user activates the Download commit button.
pub enum DiffPaneEvent {
    DownloadCommit,
}

impl EventEmitter<DiffPaneEvent> for DiffPane {}

/// One virtualized row of the multi-file diff list.
#[derive(Clone)]
enum PaneRow {
    Header {
        file_index: usize,
        gap: bool,
        expanded: bool,
    },
    Note {
        message: SharedString,
        last: bool,
    },
    Line {
        file_index: usize,
        row: DiffRow,
        last: bool,
    },
}

pub struct DiffPane {
    diff: Option<Arc<CommitDiff>>,
    seeds: Vec<TreeItemSeed>,
    expanded: HashSet<String>,
    rows: Arc<Vec<TreeItem>>,
    selected_index: Option<usize>,
    tree_visible: bool,
    split: Entity<ResizableState>,
    expanded_files: HashSet<String>,
    pane_rows: Arc<Vec<PaneRow>>,
    list: ListState,
    wrap: bool,
    downloadable: bool,
}

impl DiffPane {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let split = ResizableState::new(cx);
        split.update(cx, |split, cx| {
            split.set_sizes([px(260.)], cx);
        });

        Self {
            diff: None,
            seeds: Vec::new(),
            expanded: HashSet::new(),
            rows: Arc::default(),
            selected_index: None,
            tree_visible: false,
            split,
            expanded_files: HashSet::new(),
            pane_rows: Arc::default(),
            list: ListState::new(0, ListAlignment::Top, px(320.)),
            wrap: false,
            downloadable: false,
        }
    }

    pub fn diff(&self) -> Option<&CommitDiff> {
        self.diff.as_deref()
    }

    pub fn set_diff(&mut self, diff: CommitDiff, cx: &mut Context<Self>) {
        let mut paths: Vec<PathBuf> = diff
            .files
            .iter()
            .map(|file| PathBuf::from(&file.path))
            .collect();
        paths.sort();

        self.seeds = TreeItemSeed::from_paths(&paths);
        self.expanded = TreeItemSeed::collect_expanded(&self.seeds);
        self.rows = Arc::new(TreeItemSeed::flatten_all(&self.seeds, &self.expanded));
        self.selected_index = None;
        self.diff = Some(Arc::new(diff));

        self.expanded_files.clear();
        self.rebuild_rows();
        self.list = ListState::new(self.pane_rows.len(), ListAlignment::Top, px(320.));
        cx.notify();
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.diff = None;
        self.seeds.clear();
        self.expanded.clear();
        self.rows = Arc::default();
        self.selected_index = None;
        self.expanded_files.clear();
        self.pane_rows = Arc::default();
        self.list = ListState::new(0, ListAlignment::Top, px(320.));
        cx.notify();
    }

    pub fn set_tree_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        self.tree_visible = visible;
        cx.notify();
    }

    pub fn set_downloadable(&mut self, downloadable: bool, cx: &mut Context<Self>) {
        self.downloadable = downloadable;
        cx.notify();
    }

    pub fn toggle_wrap(&mut self, cx: &mut Context<Self>) {
        self.wrap = !self.wrap;
        self.rebuild_rows();
        cx.notify();
    }

    fn toggle_file(&mut self, path: &str, cx: &mut Context<Self>) {
        if !self.expanded_files.remove(path) {
            self.expanded_files.insert(path.to_owned());
        }
        self.rebuild_rows();
        cx.notify();
    }

    fn all_expanded(&self) -> bool {
        self.diff.as_ref().is_some_and(|diff| {
            !diff.files.is_empty()
                && diff
                    .files
                    .iter()
                    .all(|file| self.expanded_files.contains(&file.path))
        })
    }

    fn toggle_all_files(&mut self, cx: &mut Context<Self>) {
        if self.all_expanded() {
            self.expanded_files.clear();
        } else if let Some(diff) = self.diff.as_ref() {
            self.expanded_files = diff.files.iter().map(|file| file.path.clone()).collect();
        }
        self.rebuild_rows();
        cx.notify();
    }

    fn select_tree_file(&mut self, path: &str, cx: &mut Context<Self>) {
        self.selected_index = self
            .rows
            .iter()
            .position(|row| row.key().to_string() == path);
        if self.expanded_files.insert(path.to_owned()) {
            self.rebuild_rows();
        }

        let header_index = self.pane_rows.iter().position(|row| match row {
            PaneRow::Header { file_index, .. } => self
                .diff
                .as_ref()
                .and_then(|diff| diff.files.get(*file_index))
                .is_some_and(|file| file.path == path),
            _ => false,
        });
        if let Some(header_index) = header_index {
            self.list.scroll_to_reveal_item(header_index);
        }
        cx.notify();
    }

    fn rebuild_rows(&mut self) {
        let Some(diff) = self.diff.as_ref() else {
            self.pane_rows = Arc::default();
            self.list.reset(0);
            return;
        };

        let mut pane_rows = Vec::new();
        for (file_index, file) in diff.files.iter().enumerate() {
            let expanded = self.expanded_files.contains(&file.path);
            pane_rows.push(PaneRow::Header {
                file_index,
                gap: file_index > 0,
                expanded,
            });

            if !expanded {
                continue;
            }

            if file.binary {
                pane_rows.push(PaneRow::Note {
                    message: SharedString::from("Diff not available"),
                    last: true,
                });
                continue;
            }

            if file.hunks.is_empty() {
                pane_rows.push(PaneRow::Note {
                    message: SharedString::from("No content changes"),
                    last: true,
                });
                continue;
            }

            let rows = diff_rows(file);
            let last_row = rows.len().saturating_sub(1);

            pane_rows.extend(
                rows.into_iter()
                    .enumerate()
                    .map(|(row_index, row)| PaneRow::Line {
                        file_index,
                        row,
                        last: row_index == last_row,
                    }),
            );
        }

        self.pane_rows = Arc::new(pane_rows);
        self.list.reset(self.pane_rows.len());
    }

    fn toggle_folder(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.expanded.remove(id) {
            self.expanded.insert(id.to_owned());
        }

        self.rows = Arc::new(TreeItemSeed::flatten_all(&self.seeds, &self.expanded));
        self.selected_index = self
            .selected_index
            .and_then(|index| self.rows.get(index).map(|_| index));

        cx.notify();
    }

    fn render_tree_column(&self, cx: &mut Context<Self>) -> AnyElement {
        let rows = Arc::clone(&self.rows);
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
                    .items(move |_, _| Arc::clone(&rows))
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
                                let id = this.rows.get(index).map(|row| row.key().to_string());
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
        let id = row.key().to_string();
        let expandable = row.is_expandable();
        self.selected_index = Some(index);

        if expandable {
            self.toggle_folder(&id, cx);
        } else {
            self.select_tree_file(&id, cx);
        }
    }

    fn render_toolbar(
        &self,
        show_tree_toggle: bool,
        view: WeakEntity<Self>,
        cx: &App,
    ) -> AnyElement {
        let (insertions, deletions) = self.diff().map_or((0, 0), |diff| {
            (
                diff.files.iter().map(|file| file.insertions).sum(),
                diff.files.iter().map(|file| file.deletions).sum(),
            )
        });
        let files = self.diff().map_or(0, |diff| diff.files.len());
        let all_expanded = self.all_expanded();

        h_flex()
            .h_12()
            .px_2()
            .gap_2()
            .flex_none()
            .border_b_1()
            .border_color(cx.theme().colors.border)
            .when(show_tree_toggle, |this| {
                this.child(
                    Toggle::new("toggle-tree")
                        .icon(Icon::named(IconRole::PanelLeft))
                        .pressed(self.tree_visible)
                        .accessibility_label("Toggle file tree")
                        .on_change({
                            let view = view.clone();
                            move |pressed, _, cx| {
                                if let Err(error) =
                                    view.update(cx, |this, cx| this.set_tree_visible(pressed, cx))
                                {
                                    log::warn!("failed to toggle file tree: {error}");
                                }
                            }
                        }),
                )
            })
            .when(insertions > 0 || deletions > 0, |this| {
                this.child(GroupedBadge::new(
                    "toolbar-diff-stats",
                    insertions,
                    deletions,
                ))
            })
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().colors.muted_foreground)
                    .child(format!("{files} changed files")),
            )
            .child(div().flex_1())
            .child(
                Switch::new("wrap-lines")
                    .label("Wrap")
                    .checked(self.wrap)
                    .small()
                    .on_change({
                        let view = view.clone();
                        move |checked, _, cx| {
                            if let Err(error) = view.update(cx, |this, cx| {
                                if checked != this.wrap {
                                    this.toggle_wrap(cx);
                                }
                            }) {
                                log::warn!("failed to toggle wrap: {error}");
                            }
                        }
                    }),
            )
            .child(
                Button::new("expand-all")
                    .label(if all_expanded { "Collapse" } else { "Expand" })
                    .icon(Icon::named(IconRole::UnfoldVertical))
                    .variant(ButtonVariant::Ghost)
                    .small()
                    .on_click({
                        let view = view.clone();
                        move |_, _, cx| {
                            if let Err(error) =
                                view.update(cx, |this, cx| this.toggle_all_files(cx))
                            {
                                log::warn!("failed to toggle all diff files: {error}");
                            }
                        }
                    }),
            )
            .when(self.downloadable, |this| {
                this.child(
                    Button::new("download")
                        .label("Download")
                        .icon(Icon::named(IconRole::Download))
                        .variant(ButtonVariant::Ghost)
                        .tooltip(Tooltip::text("Download commit"))
                        .small()
                        .on_click(move |_, _, cx| {
                            if let Err(error) = view.update(cx, |_, cx| {
                                cx.emit(DiffPaneEvent::DownloadCommit);
                            }) {
                                log::warn!("failed to download commit patch: {error}");
                            }
                        }),
                )
            })
            .into_any_element()
    }

    fn render_file_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(diff) = self.diff.as_ref() else {
            return EmptyState::centered("No changes");
        };

        let diff = Arc::clone(diff);
        let pane_rows = Arc::clone(&self.pane_rows);
        let wrap = self.wrap;
        let view = cx.entity().downgrade();

        div()
            .relative()
            .flex_1()
            .min_h_0()
            .h_full()
            .debug_selector(|| "commit-diff-files-list".to_owned())
            .child(
                list(self.list.clone(), move |ix, _, cx| {
                    match pane_rows.get(ix) {
                        Some(row) => render_pane_row(row, &diff.files, &view, wrap, cx),
                        None => div().into_any_element(),
                    }
                })
                .size_full()
                .p_2(),
            )
            .child(Scrollbar::vertical("commit-diff-files", self.list.clone()))
            .into_any_element()
    }

    /// Renders every diff row inline, for embedding in a page that scrolls as a whole.
    pub fn render_flow(&self, view: WeakEntity<Self>, cx: &App) -> AnyElement {
        let Some(diff) = self.diff.as_ref() else {
            return div()
                .p_4()
                .text_sm()
                .text_color(cx.theme().colors.muted_foreground)
                .child("No changes")
                .into_any_element();
        };
        if diff.files.is_empty() {
            return div()
                .p_4()
                .text_sm()
                .text_color(cx.theme().colors.muted_foreground)
                .child("No files changed")
                .into_any_element();
        }

        let wrap = self.wrap;
        let rows = self
            .pane_rows
            .iter()
            .map(|row| render_pane_row(row, &diff.files, &view, wrap, cx))
            .collect::<Vec<_>>();

        v_flex()
            .w_full()
            .child(self.render_toolbar(false, view, cx))
            .child(v_flex().w_full().p_2().children(rows))
            .into_any_element()
    }
}

fn render_pane_row(
    row: &PaneRow,
    files: &[FileDiff],
    pane: &WeakEntity<DiffPane>,
    wrap: bool,
    cx: &App,
) -> AnyElement {
    let border = cx.theme().colors.border;
    let surface = cx.theme().colors.surface;

    match row {
        PaneRow::Header {
            file_index,
            gap,
            expanded,
        } => {
            let Some(file) = files.get(*file_index) else {
                return div().into_any_element();
            };
            let path = file.path.clone();
            let title = match &file.old_path {
                Some(old) => SharedString::from(format!("{old} → {}", file.path)),
                None => SharedString::from(file.path.clone()),
            };

            div()
                .w_full()
                .when(*gap, |this| this.pt_2())
                .child(
                    h_flex()
                        .id(("file-header", *file_index))
                        .debug_selector(move || format!("commit-diff-file-card-{file_index}"))
                        .h_10()
                        .px_2()
                        .gap_2()
                        .bg(surface)
                        .border_1()
                        .border_color(border)
                        .cursor_pointer()
                        .hover(|this| this.bg(cx.theme().colors.muted_hover))
                        .when(*expanded, |this| this.rounded_t_lg().border_b_0())
                        .when(!*expanded, |this| this.rounded_lg())
                        .child(
                            Icon::named(if *expanded {
                                IconRole::ChevronDown
                            } else {
                                IconRole::ChevronRight
                            })
                            .color(cx.theme().colors.muted_foreground),
                        )
                        .child(div().min_w_0().truncate().text_sm().child(title))
                        .when(file.insertions > 0 || file.deletions > 0, |this| {
                            this.child(GroupedBadge::new(
                                ("file-diff-stats", *file_index),
                                file.insertions,
                                file.deletions,
                            ))
                        })
                        .child(div().flex_1())
                        .child(
                            Button::new(("view-file", *file_index))
                                .label("View file")
                                .icon(Icon::named(IconRole::Eye))
                                .variant(ButtonVariant::Ghost)
                                .small()
                                .on_click(|_, _, cx| cx.stop_propagation()),
                        )
                        .on_click({
                            let pane = pane.clone();
                            move |_, _, cx| {
                                if let Err(error) =
                                    pane.update(cx, |this, cx| this.toggle_file(&path, cx))
                                {
                                    log::warn!("failed to toggle diff file: {error}");
                                }
                            }
                        }),
                )
                .into_any_element()
        }
        PaneRow::Note { message, last, .. } => div()
            .w_full()
            .border_x_1()
            .border_color(border)
            .bg(surface)
            .px_3()
            .py_2()
            .text_xs()
            .text_color(cx.theme().colors.muted_foreground)
            .when(*last, |this| this.border_b_1().rounded_b_lg())
            .child(message.clone())
            .into_any_element(),
        PaneRow::Line {
            file_index,
            row,
            last,
        } => {
            let Some(file) = files.get(*file_index) else {
                return div().into_any_element();
            };
            div()
                .w_full()
                .border_x_1()
                .border_color(border)
                .bg(surface)
                .when(*last, |this| this.border_b_1().rounded_b_lg())
                .child(row.render(&file.hunks, wrap, cx))
                .into_any_element()
        }
    }
}

impl Render for DiffPane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(diff) = self.diff.as_ref() else {
            return EmptyState::centered("No changes");
        };

        if diff.files.is_empty() {
            return EmptyState::centered("No files changed");
        }

        let toolbar = self.render_toolbar(true, cx.entity().downgrade(), cx);
        let file_list = self.render_file_list(cx);

        let content = if self.tree_visible {
            let tree = self.render_tree_column(cx);
            h_flex()
                .flex_1()
                .min_h_0()
                .child(
                    Resizable::new("commit-diff-split", self.split.clone())
                        .axis(Axis::Horizontal)
                        .pane_with_min(tree, px(120.))
                        .pane_with_min(file_list, px(240.))
                        .flex_1()
                        .min_w_0()
                        .h_full(),
                )
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_w_0()
                .h_full()
                .child(file_list)
                .into_any_element()
        };

        v_flex()
            .size_full()
            .min_h_0()
            .bg(cx.theme().colors.background)
            .child(toolbar)
            .child(content)
            .into_any_element()
    }
}

pub struct CommitDiffView {
    worktree: PathBuf,
    commit: FileCommit,
    /// The diff is being computed on a background task.
    loading: bool,
    error: Option<SharedString>,
    pane: Entity<DiffPane>,
    tasks: Vec<Task<Result<(), anyhow::Error>>>,
    _download_subscription: Subscription,
}

impl CommitDiffView {
    pub fn new(
        worktree: PathBuf,
        commit_id: String,
        downloadable: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let pane = cx.new(|cx| {
            let mut pane = DiffPane::new(cx);
            pane.set_downloadable(downloadable, cx);
            pane
        });
        let subscription = cx.subscribe(&pane, |this, _, DiffPaneEvent::DownloadCommit, cx| {
            this.download_patch(cx);
        });

        let weak = cx.entity().downgrade();
        window.defer(cx, move |window, cx| {
            if let Err(error) = weak.update(cx, |this, cx| this.load(window, cx)) {
                log::warn!("failed to load commit diff: {error}");
            }
        });

        Self {
            worktree,
            commit: FileCommit {
                id: commit_id,
                summary: String::new(),
                description: None,
                author: String::new(),
                time: 0,
                date_label: String::new(),
                parents: Vec::new(),
            },
            loading: true,
            error: None,
            pane,
            tasks: Vec::new(),
            _download_subscription: subscription,
        }
    }

    /// Reaps finished tasks, then queues another.
    fn push_task(&mut self, task: Task<Result<(), anyhow::Error>>) {
        self.tasks.retain(|task| !task.is_ready());
        self.tasks.push(task);
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

        self.push_task(task);
    }

    fn download_patch(&mut self, cx: &mut Context<Self>) {
        let worktree = self.worktree.clone();
        let id = self.commit.id.clone();

        let suggested_name = format!("{}.patch", short_id(&id));
        let prompt = cx.prompt_for_new_path(&worktree, Some(suggested_name.as_str()));

        let task: Task<Result<(), anyhow::Error>> = cx.spawn(async move |this, cx| {
            let destination = match prompt.await {
                Ok(Ok(Some(path))) => path,
                Ok(Ok(None)) => return Ok(()),
                Ok(Err(error)) => {
                    log::warn!("failed to open the commit patch save dialog: {error}");
                    return Ok(());
                }
                Err(_) => return Ok(()),
            };

            let saved = cx
                .background_spawn(async move {
                    let patch = Repo::open(&worktree).and_then(|repo| repo.commit_patch(&id))?;
                    std::fs::write(&destination, patch)?;
                    Ok::<(), anyhow::Error>(())
                })
                .await;

            if let Err(error) = saved {
                this.update(cx, |this, cx| {
                    this.error = Some(format!("{error:#}").into());
                    cx.notify();
                })?;
            }
            Ok(())
        });

        self.push_task(task);
    }

    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let commit = &self.commit;
        let muted = cx.theme().colors.muted_foreground;

        let row = |label: &'static str, value: AnyElement| -> AnyElement {
            h_flex()
                .h_6()
                .gap_2()
                .text_xs()
                .child(div().w_20().flex_none().text_color(muted).child(label))
                .child(div().flex_1().min_w_0().child(value))
                .into_any_element()
        };

        v_flex()
            .flex_none()
            .px_4()
            .py_3()
            .w_full()
            .gap_2()
            .border_b_1()
            .border_color(cx.theme().colors.border)
            // The top right corner is the panel toolbar's.
            .child(
                div()
                    .min_w_0()
                    .pr_16()
                    .font_semibold()
                    .truncate()
                    .text_ellipsis()
                    .child(SharedString::from(commit.summary.clone())),
            )
            .when_some(commit.description.as_ref(), |this, description| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().colors.muted_foreground)
                        .child(SharedString::from(description)),
                )
            })
            .child(Separator::horizontal())
            .child(row(
                "Author",
                h_flex()
                    .gap_2()
                    .child(
                        Avatar::new(commit.author.clone())
                            .name(commit.author.clone())
                            .small(),
                    )
                    .child(
                        div()
                            .truncate()
                            .child(SharedString::from(commit.author.clone())),
                    )
                    .into_any_element(),
            ))
            .children(
                commit
                    .description
                    .as_ref()
                    .map_or_else(Vec::new, |description| {
                        co_authors(description)
                            .into_iter()
                            .map(|name| {
                                row(
                                    "Co-author",
                                    h_flex()
                                        .gap_2()
                                        .child(Avatar::new(name.clone()).name(name.clone()).small())
                                        .child(
                                            div().min_w_0().truncate().text_ellipsis().child(name),
                                        )
                                        .into_any_element(),
                                )
                            })
                            .collect::<Vec<_>>()
                    }),
            )
            .child(row(
                "Date",
                h_flex()
                    .gap_2()
                    .child(relative_time_secs(commit.time))
                    .when(!commit.date_label.is_empty(), |this| {
                        this.child(
                            div()
                                .text_color(muted)
                                .child(SharedString::from(format!("({})", commit.date_label))),
                        )
                    })
                    .into_any_element(),
            ))
            .child(row(
                "Commit",
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .min_w_0()
                            .text_ellipsis()
                            .truncate()
                            .child(SharedString::from(commit.id.clone())),
                    )
                    .child(ClipboardButton::new("commit-id", &commit.id).xsmall())
                    .into_any_element(),
            ))
            .children(commit.parents.iter().enumerate().map(|(ix, parent)| {
                row(
                    "Parent",
                    h_flex()
                        .gap_2()
                        .child(
                            div()
                                .min_w_0()
                                .text_ellipsis()
                                .truncate()
                                .child(SharedString::from(parent.clone())),
                        )
                        .child(ClipboardButton::new(("commit-parent", ix), parent).xsmall())
                        .into_any_element(),
                )
            }))
            .into_any_element()
    }
}

impl Panel for CommitDiffView {
    fn title(&self, _cx: &App) -> SharedString {
        let short = self.commit.id.get(..8).unwrap_or(&self.commit.id);
        SharedString::from(format!("Commit {short}"))
    }

    fn loading(&self, _cx: &App) -> bool {
        self.loading
    }

    fn toolbar(&self, _cx: &App) -> bool {
        true
    }
}

impl Render for CommitDiffView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = if self.loading {
            Spinner::centered("commit-diff-loading")
        } else if let Some(error) = self.error.clone() {
            EmptyState::centered(error)
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
