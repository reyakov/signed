use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use assets::CustomIconName;
use dock::{BasePanel, DockArea, DockPlacement, Panel, PanelEvent, panel_handle};
use gpui::prelude::*;
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, Pixels, Render,
    ScrollStrategy, SharedString, Size, Subscription, Task, WeakEntity, Window, div, px, relative,
    size,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::clipboard::Clipboard;
use gpui_component::input::{Textarea, TextareaState};
use gpui_component::list::ListItem;
use gpui_component::scroll::{ScrollableElement, Scrollbar};
use gpui_component::spinner::Spinner;
use gpui_component::tab::{Tab, TabBar};
use gpui_component::tag::Tag;
use gpui_component::tree::{TreeEntry, TreeState, tree};
use gpui_component::{
    ActiveTheme, Icon, Sizable, StyledExt, VirtualListScrollHandle, h_flex, v_flex, v_virtual_list,
};
use nostr::prelude::{Event, EventId, Kind, Nip34Tag, PublicKey};
use signed_core::{activity_subject, pull_request_patch};
use signed_git::{CommitDiff, FileCommit, FileDiff, patch_commits, patch_diffs};
use signed_state::{GitStore, ProfileStore, RepoStore};
use signed_ui::image_cache::{MAX_IMAGES, image_cache};
use signed_ui::{UserAvatar, placeholder, status_badge, tree_row};
use utils::{relative_time, relative_time_secs};

use super::diff::CommitDiffView;
use super::helpers::{
    DIFF_ROW_HEIGHT, DiffRow, build_tree_items, diff_rows, find_item, render_diff_row, tree_items,
};

/// Width of the changed-files column.
const TREE_WIDTH: f32 = 260.;

/// Height of one commit row in the commits tab's virtual list: a single
/// text line plus the 1px bottom border.
const PR_COMMIT_ROW_HEIGHT: f32 = 37.;

/// Detail panel of a single pull request.
pub struct PullRequestDetailView {
    focus_handle: FocusHandle,
    /// Dock area new panels (commit diffs) are added to.
    dock_area: WeakEntity<DockArea>,
    /// Repo store holding the PR, its status and comments.
    store: Entity<RepoStore>,
    /// Event id of the root PR event (kind 1618; updates are revisions).
    pr_id: EventId,
    /// Input state of the "leave a comment" textarea.
    comment_input: Entity<TextareaState>,
    /// Display name of the repository, for panels opened from here.
    repo_name: SharedString,
    /// Local clone the PR's git changes come from; `None` while the diff is
    /// parsed from the nostr patch set (no commit diff viewer then).
    worktree: Option<PathBuf>,
    /// Root PR's content, shown as plain text.
    description: SharedString,
    /// Tip commit of the PR: the latest update's `c` tag, else the root's.
    current_commit: Option<SharedString>,
    /// Commits of the patch series, in patch order (oldest first).
    commits: Vec<FileCommit>,
    /// Parsed file changes of the patch; `None` while loading or on failure.
    diff: Option<CommitDiff>,
    /// The patch is being parsed on a background task.
    loading: bool,
    error: Option<SharedString>,
    /// Active header tab: 0 = Discussion, 1 = Files, 2 = Commits.
    active_tab: usize,
    /// Changed-files explorer state.
    tree_state: Entity<TreeState>,
    /// Path of the file whose diff is shown in the detail column.
    selected_file: Option<SharedString>,
    /// Rows of the selected file's diff (hunk headers + lines).
    rows: Vec<DiffRow>,
    /// Per-row heights of [`Self::rows`].
    item_sizes: Rc<Vec<Size<Pixels>>>,
    /// Virtual list state of the diff rows.
    scroll_handle: VirtualListScrollHandle,
    /// Per-row heights of the commits tab's virtual list, built when the
    /// patch series is loaded.
    commit_item_sizes: Rc<Vec<Size<Pixels>>>,
    /// Virtual list state of the commits tab.
    commit_scroll_handle: VirtualListScrollHandle,
    /// Comment bodies as shared strings, keyed by comment event ID, so
    /// re-renders don't clone full contents again (events are immutable,
    /// so the cache never needs invalidation).
    contents: HashMap<EventId, SharedString>,
    /// In-flight tasks; finished tasks are pruned on every push, so the vec
    /// stays bounded by the number of concurrent loads.
    tasks: Vec<Task<Result<(), anyhow::Error>>>,
    /// Subscriptions keeping the view live as the store refreshes.
    _subscriptions: Vec<Subscription>,
}

impl PullRequestDetailView {
    pub fn new(
        dock_area: WeakEntity<DockArea>,
        store: Entity<RepoStore>,
        pr_id: EventId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let tree_state = cx.new(|cx| TreeState::new(cx));
        let comment_input =
            cx.new(|cx| TextareaState::new(window, cx).placeholder("Leave a comment..."));

        // Same display name as the repo detail panel's title.
        let repo_name = store
            .read(cx)
            .announcement
            .as_ref()
            .map(|announcement| {
                announcement
                    .name
                    .clone()
                    .unwrap_or_else(|| SharedString::from(announcement.id.clone()))
            })
            .unwrap_or_default();

        // Re-render when the store refreshes (new comments, status changes).
        let subscriptions = vec![cx.observe(&store, |_this, _store, cx| cx.notify())];

        // Defer loading until the window is ready, like the commit diff view.
        cx.defer_in(window, |this, window, cx| {
            this.load(window, cx);
        });

        Self {
            focus_handle: cx.focus_handle(),
            dock_area,
            store,
            pr_id,
            comment_input,
            repo_name,
            worktree: None,
            description: SharedString::default(),
            current_commit: None,
            commits: Vec::new(),
            diff: None,
            loading: true,
            error: None,
            active_tab: 0,
            tree_state,
            selected_file: None,
            rows: Vec::new(),
            item_sizes: Rc::new(Vec::new()),
            scroll_handle: VirtualListScrollHandle::new(),
            commit_item_sizes: Rc::new(Vec::new()),
            commit_scroll_handle: VirtualListScrollHandle::new(),
            contents: HashMap::new(),
            tasks: Vec::new(),
            _subscriptions: subscriptions,
        }
    }

    /// Snapshot the PR events from the store, then compute the file changes
    /// and commit list on a background task and populate the tree.
    ///
    /// The changes come from the PR's patch set (NIP-34 `e`-linked patch
    /// events) when present; otherwise from the git repository (`c`,
    /// `clone` and `merge-base` tags), diffing the `merge-base..tip` range.
    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.loading = true;
        self.error = None;
        cx.notify();

        let cache = GitStore::global(cx).cache().clone();

        let (description, patch, current_commit, merge_base, clone_urls, addr, has_patch_link) = {
            let store = self.store.read(cx);
            let Some(root) = store
                .pull_requests
                .iter()
                .find(|pr| pr.id == self.pr_id && pr.kind == Kind::GitPullRequest)
            else {
                self.loading = false;
                self.error = Some("Pull request not found".into());
                cx.notify();
                return;
            };
            let update = latest_update(store.pull_requests.iter(), &root.id);
            let tip = update
                .and_then(current_commit_of)
                .or_else(|| current_commit_of(root));
            let base = update
                .and_then(merge_base_of)
                .or_else(|| merge_base_of(root));
            let clone_urls = clone_urls_of(root).or_else(|| {
                store
                    .announcement
                    .as_ref()
                    .map(|a| a.clone.iter().map(ToString::to_string).collect())
            });
            (
                root.content.clone(),
                pull_request_patch(root, store.patches.iter()),
                tip,
                base,
                clone_urls.unwrap_or_default(),
                store.addr().clone(),
                root.tags.event_ids().next().is_some(),
            )
        };
        self.description = description.into();

        let task = cx.spawn_in(window, async move |this, cx| {
            // Parse the nostr patch set first.
            let nostr_diff = cx
                .background_spawn({
                    let patch = patch.clone();
                    async move { patch_diffs(&patch) }
                })
                .await;
            let nostr_commits = cx
                .background_spawn({
                    let patch = patch.clone();
                    async move { patch_commits(&patch) }
                })
                .await;

            // PRs without patch events (e.g. published by ngit) carry their
            // changes in the git repository: fetch the clone and diff the
            // `merge-base..tip` range.
            let use_nostr = match &nostr_diff {
                Ok(diff) => has_patch_link || !diff.files.is_empty(),
                Err(_) => true,
            };
            let git = if use_nostr {
                None
            } else {
                let cache = cache.clone();
                let addr = addr.clone();
                let clone_urls = clone_urls.clone();
                let base = merge_base.clone();
                let tip = current_commit.clone();
                Some(
                    cx.background_spawn(async move {
                        let repo = cache.ensure_clone(&addr, &clone_urls)?;
                        let workdir = repo
                            .workdir()
                            .ok_or_else(|| anyhow::anyhow!("repository has no worktree"))?
                            .to_path_buf();
                        let tip =
                            tip.ok_or_else(|| anyhow::anyhow!("pull request has no tip commit"))?;
                        let base = match base {
                            Some(base) => base,
                            // No `merge-base` tag: use the merge base of the
                            // tip with the default branch.
                            None => {
                                let head = repo
                                    .head_id()
                                    .map_err(|_| anyhow::anyhow!("repository has no HEAD"))?;
                                let tip_id = repo.rev_parse_single(tip.as_bytes())?;
                                repo.merge_base(tip_id, head)?.to_string()
                            }
                        };
                        let diff = signed_git::worktree_commit_range_diff(&workdir, &base, &tip)?;
                        let commits =
                            signed_git::worktree_commit_range_commits(&workdir, &base, &tip)?;
                        Ok::<_, anyhow::Error>((diff, commits, workdir))
                    })
                    .await,
                )
            };

            let (diff, commits, worktree) = match git {
                Some(Ok((diff, commits, worktree))) => (Ok(diff), commits, Some(worktree)),
                Some(Err(error)) => (Err(error), Vec::new(), None),
                None => (nostr_diff, nostr_commits, None),
            };

            this.update_in(cx, |this, _window, cx| {
                this.loading = false;
                this.worktree = worktree;
                this.current_commit = current_commit.map(SharedString::from);
                this.commit_item_sizes =
                    Rc::new(vec![size(px(0.), px(PR_COMMIT_ROW_HEIGHT)); commits.len()]);
                this.commits = commits;
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

        self.tasks.retain(|task| !task.is_ready());
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

    /// Open the diff of `commit_id` in the bottom dock of the area.
    fn open_commit_diff(
        &mut self,
        worktree: PathBuf,
        commit_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(dock_area) = self.dock_area.upgrade() else {
            return;
        };

        let panel = cx.new(|cx| {
            CommitDiffView::new(
                worktree,
                self.repo_name.clone(),
                commit_id.into(),
                window,
                cx,
            )
        });

        dock_area.update(cx, |dock_area, cx| {
            dock_area.add_panel_view(panel_handle(panel), DockPlacement::Bottom, None, window, cx);
        });
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
                placeholder("No files changed in this pull request", cx)
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
            signed_git::DiffStatus::Added => "A",
            signed_git::DiffStatus::Modified => "M",
            signed_git::DiffStatus::Deleted => "D",
            signed_git::DiffStatus::Renamed => "R",
            signed_git::DiffStatus::Copied => "C",
        };
        let status_color = match file.status {
            signed_git::DiffStatus::Added => cx.theme().success,
            signed_git::DiffStatus::Modified => cx.theme().info,
            signed_git::DiffStatus::Deleted => cx.theme().danger,
            signed_git::DiffStatus::Renamed | signed_git::DiffStatus::Copied => {
                cx.theme().muted_foreground
            }
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
                        "pr-diff-rows",
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
            .child(div().id("pr-diff-body").flex_1().min_h_0().child(body))
            .into_any_element()
    }

    /// Underline tab bar: Discussion, Files and Commits.
    fn render_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let active = self.active_tab;
        let files_count = self.diff.as_ref().map(|diff| diff.files.len());
        let commits_count = if self.commits.is_empty() {
            None
        } else {
            Some(self.commits.len())
        };

        TabBar::new("pr-tabs")
            .underline()
            .small()
            .px_4()
            .w_full()
            .selected_index(active)
            .on_click(cx.listener(|this, index, _window, cx| {
                this.active_tab = *index;
                cx.notify();
            }))
            .child(Tab::new().label("Discussion"))
            .child(
                Tab::new()
                    .label("Files")
                    .when_some(files_count, |this, count| {
                        this.suffix(
                            Tag::secondary()
                                .xsmall()
                                .child(SharedString::from(count.to_string())),
                        )
                    }),
            )
            .child(
                Tab::new()
                    .label("Commits")
                    .when_some(commits_count, |this, count| {
                        this.suffix(
                            Tag::secondary()
                                .xsmall()
                                .child(SharedString::from(count.to_string())),
                        )
                    }),
            )
            .into_any_element()
    }

    /// Discussion tab: author, description and comments like the issue
    /// panel, with the comment form at the end and a sidebar on the right.
    fn render_discussion(&mut self, cx: &mut Context<Self>) -> AnyElement {
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

        let (author, picture, age, root_id) = {
            let store = self.store.read(cx);
            let Some(root) = store
                .pull_requests
                .iter()
                .find(|pr| pr.id == self.pr_id && pr.kind == Kind::GitPullRequest)
            else {
                return placeholder("Pull request not found", cx);
            };
            let profile = ProfileStore::global(cx).read(cx).get(&root.pubkey);
            (
                profile.name(),
                profile.picture(),
                relative_time(root.created_at),
                root.id,
            )
        };

        h_flex()
            .flex_1()
            .w_full()
            .min_h_0()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .p_4()
                    .gap_6()
                    .overflow_y_scrollbar()
                    .child(
                        v_flex()
                            .px_4()
                            .gap_8()
                            .child(
                                v_flex()
                                    .gap_4()
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .text_sm()
                                            .child(
                                                h_flex()
                                                    .gap_1()
                                                    .child(
                                                        UserAvatar::new(author.clone())
                                                            .picture(picture),
                                                    )
                                                    .child(author),
                                            )
                                            .child(SharedString::from("commented"))
                                            .child(
                                                div()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(SharedString::from(age)),
                                            ),
                                    )
                                    .when(!self.description.is_empty(), |this| {
                                        this.child(div().text_sm().child(self.description.clone()))
                                    }),
                            )
                            .child(self.render_comments(&root_id, cx))
                            .child(self.render_form(&root_id, cx)),
                    ),
            )
            .child(self.render_sidebar(cx))
            .into_any_element()
    }

    /// Right sidebar: participants and labels, like the issue panel.
    fn render_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let profile_store = ProfileStore::global(cx);
        let store = self.store.read(cx);

        let Some(root) = store
            .pull_requests
            .iter()
            .find(|pr| pr.id == self.pr_id && pr.kind == Kind::GitPullRequest)
        else {
            // `render_discussion` already bails out when the PR is missing.
            return div().into_any_element();
        };

        // Participants: the PR author plus everyone who commented.
        let mut participants: Vec<PublicKey> = vec![root.pubkey];
        participants.extend(store.comments_of(&root.id).map(|comment| comment.pubkey));
        participants.sort_by_key(PublicKey::to_hex);
        participants.dedup();

        // PR labels are NIP-34 `t` hashtag tags on the event.
        let labels: Vec<String> = root.tags.hashtags().map(|tag| tag.to_string()).collect();

        v_flex()
            .w(px(240.))
            .h_full()
            .flex_none()
            .px_4()
            .gap_4()
            .border_l(px(1.))
            .border_color(cx.theme().sidebar_border)
            .child(
                v_flex()
                    .mt_4()
                    .gap_2()
                    .child(sidebar_title("Participants", cx))
                    .children(participants.iter().map(|pubkey| {
                        let profile = profile_store.read(cx).get(pubkey);
                        let name = profile.name();
                        let picture = profile.picture();

                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(UserAvatar::new(name.clone()).picture(picture))
                            .child(div().text_sm().truncate().text_ellipsis().child(name))
                            .into_any_element()
                    })),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(sidebar_title("Labels", cx))
                    .map(|this| {
                        if labels.is_empty() {
                            this.child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("None yet."),
                            )
                        } else {
                            this.child(h_flex().gap_1().children({
                                let mut items = vec![];

                                for label in labels.iter() {
                                    items.push(
                                        Tag::secondary()
                                            .outline()
                                            .xsmall()
                                            .child(SharedString::from(label)),
                                    );
                                }

                                items
                            }))
                        }
                    }),
            )
            .into_any_element()
    }

    /// Files tab: the changed-files tree on the left, the diff of the
    /// selected file on the right.
    fn render_files_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .flex_1()
            .w_full()
            .min_h_0()
            .overflow_hidden()
            .child(self.render_tree_column(cx))
            .child(self.render_detail_column(cx))
            .into_any_element()
    }

    /// Full-height Commits tab: every commit of the patch series, or a
    /// status message while loading / when there are none.
    fn render_commits_tab(&self, cx: &mut Context<Self>) -> AnyElement {
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
        if self.commits.is_empty() {
            return placeholder("No commits found", cx);
        }

        v_flex()
            .relative()
            .flex_1()
            .w_full()
            .min_h_0()
            .child(
                v_virtual_list(
                    cx.entity().clone(),
                    "pr-commits",
                    self.commit_item_sizes.clone(),
                    move |this, range, _window, cx| {
                        range
                            .map(|ix| this.render_commit_row(ix, &this.commits[ix], cx))
                            .collect()
                    },
                )
                .track_scroll(&self.commit_scroll_handle)
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .child(Scrollbar::vertical(&self.commit_scroll_handle)),
            )
            .into_any_element()
    }

    /// One row of the commits tab: id, summary, author and time. Clicking a
    /// row opens the commit's diff in the bottom dock.
    fn render_commit_row(
        &self,
        ix: usize,
        commit: &FileCommit,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let meta = commit_meta(commit);
        let id = commit.id.clone();

        h_flex()
            .id(ix)
            .px_4()
            .h(px(PR_COMMIT_ROW_HEIGHT))
            .gap_2()
            .items_center()
            .text_sm()
            .border_b(px(1.))
            .border_color(cx.theme().border)
            .hover(|this| this.bg(cx.theme().list_hover))
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
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(commit.summary.clone()),
            )
            .when(!meta.is_empty(), |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(meta)),
                )
            })
            // Commits parsed from the nostr patch set may not exist in any
            // local clone; only git-backed PRs open a diff viewer.
            .when_some(self.worktree.clone(), |this, worktree| {
                this.on_click(cx.listener(move |this, _event, window, cx| {
                    this.open_commit_diff(worktree.clone(), &id, window, cx);
                }))
            })
            .into_any_element()
    }

    /// One comment card, same design as the issue panel: avatar, author,
    /// "commented" and age on the header row, content below.
    fn render_comments(&mut self, id: &EventId, cx: &mut Context<Self>) -> AnyElement {
        let store = self.store.read(cx);
        let comments: Vec<&Event> = store.comments_of(id).collect();
        let title = SharedString::from(format!("Discussions {}", comments.len()));

        v_flex()
            .gap_4()
            .child(div().text_xs().font_semibold().child(title))
            .children(comments.iter().map(|comment| {
                let profile = ProfileStore::global(cx).read(cx).get(&comment.pubkey);
                let author = profile.name();
                let picture = profile.picture();
                let age = relative_time(comment.created_at);
                // Comment bodies are cloned into shared strings once per
                // comment, not on every render.
                let content = self
                    .contents
                    .entry(comment.id)
                    .or_insert_with(|| SharedString::from(comment.content.clone()))
                    .clone();

                v_flex()
                    .gap_1()
                    .p_3()
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded(cx.theme().radius)
                    .child(
                        h_flex()
                            .gap_2()
                            .text_sm()
                            .child(
                                h_flex()
                                    .gap_1()
                                    .child(UserAvatar::new(author.clone()).picture(picture))
                                    .child(author),
                            )
                            .child(
                                div()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("commented"),
                            )
                            .child(
                                div()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(SharedString::from(age)),
                            ),
                    )
                    .child(div().text_sm().child(content))
            }))
            .into_any_element()
    }

    fn render_form(&mut self, id: &EventId, cx: &mut Context<Self>) -> AnyElement {
        let comment_input = self.comment_input.clone();
        let store = self.store.clone();
        let id = id.to_owned();

        v_flex()
            .gap_2()
            .child(
                Textarea::new(&self.comment_input)
                    .h_24()
                    .text_color(cx.theme().muted_foreground)
                    .bg(cx.theme().muted),
            )
            .child(
                h_flex()
                    .justify_between()
                    .child(
                        h_flex()
                            .gap_1()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(Icon::new(CustomIconName::Markdown).small())
                            .child("Markdown is supported"),
                    )
                    .child(
                        Button::new("pr-comment")
                            .primary()
                            .label("Comment")
                            .tooltip("Post comment")
                            .on_click(move |_event, window, cx| {
                                let content = comment_input.read(cx).value().trim().to_string();
                                if content.is_empty() {
                                    return;
                                }
                                let Some(root) = store
                                    .read(cx)
                                    .pull_requests
                                    .iter()
                                    .find(|pr| pr.id == id)
                                    .cloned()
                                else {
                                    return;
                                };
                                store.update(cx, |store, cx| {
                                    store.comment(&root, content, cx);
                                });
                                comment_input.update(cx, |input, cx| {
                                    input.set_value("", window, cx);
                                });
                            }),
                    ),
            )
            .into_any_element()
    }

    /// Always-visible header: status badge and title, like the issue panel.
    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let current_commit = self.current_commit.clone();
        let (title, status, branch) = {
            let store = self.store.read(cx);
            let Some(root) = store
                .pull_requests
                .iter()
                .find(|pr| pr.id == self.pr_id && pr.kind == Kind::GitPullRequest)
            else {
                return div().into_any_element();
            };
            (
                activity_subject(root),
                store.status_of(root),
                branch_name_of(root),
            )
        };

        v_flex()
            .px_4()
            .mb_4()
            .child(
                h_flex()
                    .w_full()
                    .min_h_16()
                    .gap_2()
                    .child(status_badge(status, cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_semibold()
                            .line_height(relative(1.2))
                            .child(title),
                    ),
            )
            .child(
                h_flex()
                    .gap_3()
                    .when_some(branch, |this, branch| {
                        this.child(
                            Button::new("branch")
                                .ghost()
                                .small()
                                .icon(CustomIconName::GitBranch)
                                .label(branch),
                        )
                    })
                    .when_some(current_commit, |this, id| {
                        this.child(
                            h_flex()
                                .gap_1()
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(id.clone())
                                .child(Clipboard::new("pr-commit").value(&id)),
                        )
                    }),
            )
            .into_any_element()
    }
}

/// One sidebar section title.
fn sidebar_title(text: &str, cx: &App) -> AnyElement {
    div()
        .text_xs()
        .font_semibold()
        .text_color(cx.theme().muted_foreground)
        .child(text.to_string())
        .into_any_element()
}

/// The `c` tag of a PR event (tip of the proposed branch), as hex.
fn current_commit_of(event: &Event) -> Option<String> {
    event
        .tags
        .iter()
        .find_map(|tag| match Nip34Tag::parse(tag.as_slice()) {
            Ok(Nip34Tag::CurrentCommit(commit)) => Some(commit.to_string()),
            _ => None,
        })
}

/// The `merge-base` tag of a PR event (most recent common ancestor with the
/// target branch), as hex.
fn merge_base_of(event: &Event) -> Option<String> {
    event
        .tags
        .iter()
        .find_map(|tag| match Nip34Tag::parse(tag.as_slice()) {
            Ok(Nip34Tag::MergeBase(commit)) => Some(commit.to_string()),
            _ => None,
        })
}

/// The `clone` tag of a PR event (URLs where the proposed branch can be
/// fetched), or `None` if the PR has none.
fn clone_urls_of(event: &Event) -> Option<Vec<String>> {
    event
        .tags
        .iter()
        .find_map(|tag| match Nip34Tag::parse(tag.as_slice()) {
            Ok(Nip34Tag::Clone(urls)) => Some(urls.iter().map(ToString::to_string).collect()),
            _ => None,
        })
}

/// The `branch-name` tag of a PR event, if any.
fn branch_name_of(event: &Event) -> Option<String> {
    event
        .tags
        .iter()
        .find_map(|tag| match Nip34Tag::parse(tag.as_slice()) {
            Ok(Nip34Tag::BranchName(name)) => Some(name),
            _ => None,
        })
}

/// The latest PR update (kind 1619) revising `root`, found via its NIP-22
/// `E` tag pointing at the root PR event.
fn latest_update<'a>(events: impl Iterator<Item = &'a Event>, root: &EventId) -> Option<&'a Event> {
    let root_hex = root.to_hex();
    events
        .filter(|e| e.kind == Kind::GitPullRequestUpdate)
        .filter(|e| {
            e.tags
                .iter()
                .any(|t| t.kind() == "E" && t.content() == Some(root_hex.as_str()))
        })
        .max_by_key(|e| e.created_at)
}

/// One-line commit metadata for the commits list: author and relative time,
/// whichever is available.
fn commit_meta(commit: &FileCommit) -> String {
    let author = commit.author.trim();
    let time = commit.time > 0;
    match (author.is_empty(), time) {
        (false, true) => format!("{author} · {}", relative_time_secs(commit.time)),
        (false, false) => author.to_string(),
        (true, true) => relative_time_secs(commit.time),
        (true, false) => String::new(),
    }
}

impl BasePanel for PullRequestDetailView {
    fn panel_name(&self) -> &'static str {
        "pull-request-detail"
    }
}

impl Panel for PullRequestDetailView {
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let hex = self.pr_id.to_hex();
        let id = SharedString::from(&hex[..8]);
        let title = if self.repo_name.is_empty() {
            id
        } else {
            SharedString::from(format!("{}/{}", self.repo_name, id))
        };
        div().text_sm().child(title)
    }
}

impl EventEmitter<PanelEvent> for PullRequestDetailView {}

impl Focusable for PullRequestDetailView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PullRequestDetailView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .image_cache(image_cache("pull-request-detail", MAX_IMAGES))
            .id("pull-request-detail")
            .size_full()
            .min_h_0()
            .child(self.render_header(cx))
            .child(self.render_tabs(cx))
            .map(|this| match self.active_tab {
                0 => this.child(self.render_discussion(cx)),
                1 => this.child(self.render_files_tab(cx)),
                _ => this.child(self.render_commits_tab(cx)),
            })
    }
}

#[cfg(test)]
mod tests {
    use nostr::prelude::{Tag, *};

    use super::*;

    const COMMIT_HEX: &str = "1111111111111111111111111111111111111111";
    const OTHER_ROOT_HEX: &str = "2222222222222222222222222222222222222222";

    fn keys() -> Keys {
        Keys::new(
            SecretKey::from_hex("0000000000000000000000000000000000000000000000000000000000000001")
                .expect("valid secret key"),
        )
    }

    /// Build a signed event with a controlled `created_at`.
    fn signed(kind: Kind, tags: Vec<Tag>, created_at: u64) -> Event {
        EventBuilder::new(kind, "")
            .tags(tags)
            .custom_created_at(Timestamp::from(created_at))
            .finalize(&keys())
            .expect("signed event")
    }

    fn pr_root() -> Event {
        signed(
            Kind::GitPullRequest,
            vec![
                Tag::parse(["c", COMMIT_HEX]).expect("valid tag"),
                Tag::parse(["branch-name", "feature/x"]).expect("valid tag"),
            ],
            100,
        )
    }

    #[test]
    fn reads_current_commit_and_branch_name() {
        let pr = pr_root();
        assert_eq!(current_commit_of(&pr).as_deref(), Some(COMMIT_HEX));
        assert_eq!(branch_name_of(&pr).as_deref(), Some("feature/x"));
    }

    #[test]
    fn returns_none_without_pr_tags() {
        let pr = signed(Kind::GitPullRequest, vec![], 100);
        assert_eq!(current_commit_of(&pr), None);
        assert_eq!(branch_name_of(&pr), None);
    }

    #[test]
    fn latest_update_picks_newest_revision_of_the_root() {
        let root = pr_root();
        let root_hex = root.id.to_hex();

        let revision = |created_at: u64| {
            signed(
                Kind::GitPullRequestUpdate,
                vec![Tag::parse(["E", &root_hex]).expect("valid tag")],
                created_at,
            )
        };
        // An update revising a different PR must be ignored even though it
        // is newer.
        let unrelated = signed(
            Kind::GitPullRequestUpdate,
            vec![Tag::parse(["E", OTHER_ROOT_HEX]).expect("valid tag")],
            999,
        );

        let events = [unrelated, revision(200), root.clone(), revision(300)];
        let latest = latest_update(events.iter(), &root.id).expect("an update");

        assert_eq!(latest.created_at.as_secs(), 300);
        assert_eq!(latest.kind, Kind::GitPullRequestUpdate);
    }

    #[test]
    fn latest_update_ignores_roots_without_revisions() {
        let root = pr_root();
        assert!(latest_update([&root].into_iter(), &root.id).is_none());
    }

    #[test]
    fn commit_meta_combines_author_and_time() {
        let commit = |author: &str, time: i64| FileCommit {
            id: COMMIT_HEX.into(),
            summary: "summary".into(),
            description: None,
            author: author.into(),
            time,
        };

        assert_eq!(commit_meta(&commit("Alice", 0)), "Alice");
        assert_eq!(commit_meta(&commit("", 0)), "");
        assert!(!commit_meta(&commit("", 1_000_000)).is_empty());
        assert!(!commit_meta(&commit("Alice", 1_000_000)).is_empty());
    }
}
