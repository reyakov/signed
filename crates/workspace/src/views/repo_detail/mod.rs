use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

use anyhow::Error;
use assets::CustomIconName;
use dock::{BasePanel, DockArea, DockPlacement, Panel, PanelEvent, panel_handle};
use gix::Repository;
use gpui::prelude::*;
use gpui::{
    Action, Anchor, AnyElement, App, ClipboardItem, Context, Div, ElementId, Entity, EventEmitter,
    FocusHandle, Focusable, PathPromptOptions, Pixels, Render, SharedString, Size, Subscription,
    Task, WeakEntity, Window, div, px, relative, size,
};
use gpui_base::{Button as BaseButton, Disableable, Popover};
use gpui_component::avatar::Avatar;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::clipboard::Clipboard;
use gpui_component::combobox::{
    Caret, Combobox, ComboboxEvent, ComboboxState, ComboboxTriggerContext,
};
use gpui_component::searchable_list::SearchableVec;
use gpui_component::tree::TreeState;
use gpui_component::{
    ActiveTheme, Colorize, Icon, IconName, Sizable, StyledExt, ThemeStyled,
    VirtualListScrollHandle, h_flex, v_flex,
};
use nostr::prelude::{RelayUrl, ToBech32};
use signed_core::Announcement;
use signed_git::{CommitList, FileCommit};
use signed_state::{GitStore, ProfileStore, RepoStore};

use crate::image_cache::{MAX_IMAGES, image_cache};
use crate::pixel_avatar::PixelAvatar;

mod about;
mod browser;
mod commits;
mod diff;
mod helpers;
mod issue_detail;
mod issues;
mod pull_request_detail;
mod pull_requests;

use about::open_about_dialog;
use browser::{
    CodeView, FileContent, MAX_PREVIEW_BYTES, MAX_PREVIEW_CACHE_BYTES, MAX_PREVIEWED_FILES,
    MarkdownView,
};
use commits::COMMIT_ROW_HEIGHT;
use diff::CommitDiffView;
use helpers::{
    BaseDropdownButton, ShareTargets, TreeItemSeed, build_tree_items, is_markdown_path, tree_items,
};
use issues::{IssuesView, open_new_issue_dialog};
use pull_requests::{PullRequestsView, open_new_pull_request_dialog};

/// What kind of ref the header selectors switch to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RefKind {
    /// A local branch (`refs/heads/*`); HEAD stays attached.
    Branch,
    /// A tag (`refs/tags/*`); HEAD becomes detached.
    Tag,
}

/// Header actions dispatched by the dropdown menus of the header buttons.
#[derive(Clone, Action, PartialEq, Eq)]
#[action(namespace = repo_detail, no_json)]
enum RepoAction {
    /// Open the "new issue" dialog.
    NewIssue,
    /// Open the "new pull request" dialog.
    NewPR,
}

/// Everything loaded from the local clone for the explorer: the tree seeds,
/// README, refs and HEAD commit. Computed on a background thread (see
/// [`load_repo_data`]) and applied on the main thread.
struct RepoData {
    tree: Vec<TreeItemSeed>,
    readme_path: Option<PathBuf>,
    readme: Option<Vec<u8>>,
    worktree: Option<PathBuf>,
    branches: Vec<String>,
    tags: Vec<String>,
    current_branch: Option<String>,
    head_commit: Option<FileCommit>,
}

/// Detail view of a repository: header, stats, a file explorer with README
/// preview (cloned from the announcement's `clone` URLs), and metadata.
pub struct RepoDetailView {
    focus_handle: FocusHandle,
    /// Dock area the detail view lives in; new panels (commit diffs) are
    /// added there.
    dock_area: WeakEntity<DockArea>,
    /// Snapshot taken at open time, shown until the store's first refresh
    /// completes (and as a fallback while the store has no announcement).
    initial: Announcement,
    /// Per-repository nostr store (announcement, issues, PRs, statuses).
    store: Entity<RepoStore>,
    /// File explorer state (worktree of the local clone).
    tree_state: Entity<TreeState>,
    /// Root of the local clone, for reading files on demand.
    worktree: Option<PathBuf>,
    /// Markdown document currently in the preview pane (README or a file).
    md: Option<MarkdownView>,
    /// Code file currently in the preview pane.
    code: Option<CodeView>,
    readme_name: Option<SharedString>,
    /// Currently previewed file (relative path) and its contents.
    selected_file: Option<SharedString>,
    files: HashMap<String, FileContent>,
    /// Paths of cached previews, oldest first; feeds the eviction caps in
    /// [`Self::evict_previews`].
    file_order: VecDeque<String>,
    /// Total text bytes held by [`Self::files`].
    preview_bytes: usize,
    /// Reads in flight, to avoid duplicate loads.
    loading_files: HashSet<String>,
    /// Latest commit touching a previewed file (or the README), keyed by path.
    commits: HashMap<String, FileCommit>,
    /// Paths queued for the next batched commit query (see [`Self::load_commits`]).
    pending_commits: Vec<String>,
    /// A batched commit query is in flight.
    loading_commits: bool,
    /// Active header tab: 0 = Files (tree), 1 = Commits.
    active_tab: usize,
    /// Commits reachable from HEAD, newest first; `None` until the walk
    /// finishes (or fails). `commits` may be capped by
    /// [`CommitList`]; `total` feeds the tab badge.
    all_commits: Option<CommitList>,
    /// Commit walk in flight.
    loading_all_commits: bool,
    /// Virtual list state of the Commits tab.
    scroll_handle: VirtualListScrollHandle,
    item_sizes: Rc<Vec<Size<Pixels>>>,
    /// A clone/fetch is in flight.
    loading: bool,
    /// The header clone button is cloning into a user-chosen folder.
    cloning: bool,
    error: Option<SharedString>,
    /// Commit HEAD currently points to, shown in the header button.
    head_commit: Option<FileCommit>,
    /// Branch selector (header): local branches, searchable.
    branch_select: Entity<ComboboxState<SearchableVec<SharedString>>>,
    /// Tag selector (header): tags, searchable.
    tag_select: Entity<ComboboxState<SearchableVec<SharedString>>>,
    /// A branch/tag switch is in flight (checkout plus explorer reload).
    switching_ref: bool,
    /// Bumped on every branch/tag switch; in-flight loads tagged with an
    /// older generation are discarded when they complete.
    ref_generation: u64,
    /// In-flight tasks; finished tasks are pruned on every push, so the vec
    /// stays bounded by the number of concurrent loads.
    tasks: Vec<Task<Result<(), Error>>>,
    /// Subscriptions keeping the selectors' confirm events alive.
    _subscriptions: Vec<Subscription>,
}

impl RepoDetailView {
    pub fn new(
        dock_area: WeakEntity<DockArea>,
        initial: Announcement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // The announcement we opened from already carries the repository's
        // NIP-34 `relays` tag, so the store can connect to those relays
        // immediately instead of waiting for the bootstrap fetch.
        let store = cx.new(|cx| RepoStore::new(initial.addr(), initial.relays.clone(), cx));
        let tree_state = cx.new(|cx| TreeState::new(cx));

        // Empty until the clone completes; populated with the local refs.
        let branch_select: Entity<ComboboxState<SearchableVec<SharedString>>> = cx.new(|cx| {
            ComboboxState::new(
                SearchableVec::new(Vec::<SharedString>::new()),
                Vec::new(),
                window,
                cx,
            )
            .searchable(true)
        });
        let tag_select: Entity<ComboboxState<SearchableVec<SharedString>>> = cx.new(|cx| {
            ComboboxState::new(
                SearchableVec::new(Vec::<SharedString>::new()),
                Vec::new(),
                window,
                cx,
            )
            .searchable(true)
        });

        let subscriptions = vec![
            cx.subscribe_in(&branch_select, window, |this, _state, event, window, cx| {
                // `Change` fires only when the selection actually changed
                // (picking the already-selected branch emits nothing), so a
                // confirmed value always means a switch.
                if let ComboboxEvent::Change(values) = event
                    && let Some(name) = values.first()
                {
                    this.switch_ref(RefKind::Branch, name.clone(), window, cx);
                }
            }),
            cx.subscribe_in(&tag_select, window, |this, _state, event, window, cx| {
                if let ComboboxEvent::Change(values) = event
                    && let Some(name) = values.first()
                {
                    this.switch_ref(RefKind::Tag, name.clone(), window, cx);
                }
            }),
        ];

        // Defer loading the repository until the window is ready.
        cx.defer_in(window, |this, window, cx| {
            this.load_repo(window, cx);
        });

        Self {
            initial,
            dock_area,
            store,
            tree_state,
            worktree: None,
            md: None,
            code: None,
            readme_name: None,
            selected_file: None,
            files: HashMap::new(),
            file_order: VecDeque::new(),
            preview_bytes: 0,
            loading_files: HashSet::new(),
            commits: HashMap::new(),
            pending_commits: Vec::new(),
            loading_commits: false,
            active_tab: 0,
            all_commits: None,
            loading_all_commits: false,
            scroll_handle: VirtualListScrollHandle::new(),
            item_sizes: Rc::new(Vec::new()),
            loading: true,
            cloning: false,
            error: None,
            head_commit: None,
            branch_select,
            tag_select,
            switching_ref: false,
            ref_generation: 0,
            focus_handle: cx.focus_handle(),
            tasks: Vec::new(),
            _subscriptions: subscriptions,
        }
    }

    /// Load the repository and populate the file explorer. The local clone
    /// (if any) is loaded first without touching the network, so an
    /// unreachable server can't block the panel; a background fetch then
    /// refreshes the refs and commit list (a fetch never changes the
    /// checked-out files, so the tree and previews are left alone).
    fn load_repo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.loading = true;
        self.error = None;
        cx.notify();

        let cache = GitStore::global(cx).cache().clone();
        let addr = self.initial.addr();
        let clone_urls: Vec<String> = self.initial.clone.iter().map(ToString::to_string).collect();
        // Captured before the loads start: a branch/tag switch bumps it, and
        // the refresh below is discarded when that happens.
        let refresh_generation = self.ref_generation;

        let disk = {
            let cache = cache.clone();
            let addr = addr.clone();
            cx.background_spawn(async move {
                match cache.open(&addr)? {
                    Some(repo) => Ok(Some(load_repo_data(&repo)?)),
                    None => Ok(None),
                }
            })
        };

        let task = cx.spawn_in(window, async move |this, cx| {
            let disk = disk.await;
            let had_clone = matches!(&disk, Ok(Some(_)));

            // No local clone yet: clone from the network (blocking), then load.
            let data = match disk {
                Ok(Some(data)) => Ok(data),
                Ok(None) => {
                    let cache = cache.clone();
                    let addr = addr.clone();
                    let clone_urls = clone_urls.clone();
                    cx.background_spawn(async move {
                        let repo = cache.ensure_clone(&addr, &clone_urls)?;
                        load_repo_data(&repo)
                    })
                    .await
                }
                Err(error) => Err(error),
            };

            this.update_in(cx, |this, window, cx| {
                match data {
                    Ok(data) => this.apply_repo_data(data, window, cx),
                    Err(error) => this.error = Some(error.to_string().into()),
                }
                this.loading = false;
                cx.notify();
            })?;

            // Refresh the clone from the network in the background; when it
            // completes, update the refs and commit list. Loads started
            // before a branch/tag switch are discarded via the generation.
            if !had_clone {
                return Ok(());
            }
            let refresh = {
                let cache = cache.clone();
                let addr = addr.clone();
                cx.background_spawn(async move {
                    let Some(repo) = cache.open(&addr)? else {
                        return Ok::<_, Error>(None);
                    };
                    // Best-effort: a failed fetch (e.g. offline) keeps the
                    // cached state, which is already shown.
                    signed_git::fetch_all(&repo).ok();
                    let worktree = repo.workdir().map(Path::to_path_buf);
                    let (branches, tags) = match &worktree {
                        Some(_) => (
                            signed_git::repo_branches(&repo).unwrap_or_default(),
                            signed_git::repo_tags(&repo).unwrap_or_default(),
                        ),
                        None => (Vec::new(), Vec::new()),
                    };
                    let current_branch = signed_git::current_branch(&repo).unwrap_or(None);
                    let head_commit = signed_git::head_commit(&repo).unwrap_or(None);
                    Ok::<_, Error>(Some((branches, tags, current_branch, head_commit)))
                })
            }
            .await;

            this.update_in(cx, |this, window, cx| {
                if refresh_generation != this.ref_generation {
                    return;
                }
                if let Ok(Some((branches, tags, current_branch, head_commit))) = refresh {
                    let branches: Vec<SharedString> =
                        branches.into_iter().map(Into::into).collect();
                    let tags: Vec<SharedString> = tags.into_iter().map(Into::into).collect();
                    this.branch_select.update(cx, |state, cx| {
                        state.set_items(SearchableVec::from(branches), window, cx);
                        if let Some(branch) = current_branch {
                            let branch: SharedString = branch.into();
                            state.set_selected_values(&[branch], window, cx);
                        }
                    });
                    this.tag_select.update(cx, |state, cx| {
                        state.set_items(SearchableVec::from(tags), window, cx);
                    });
                    this.head_commit = head_commit;
                    // The fetch may have brought new commits: reload the list.
                    this.all_commits = None;
                    this.loading_all_commits = false;
                    this.load_all_commits(cx);
                    cx.notify();
                }
            })?;

            Ok(())
        });

        self.tasks.push(task);
    }

    /// Apply the loaded repository data: explorer tree, README preview, ref
    /// selectors and HEAD commit, then start the commit-list walk.
    fn apply_repo_data(&mut self, data: RepoData, window: &mut Window, cx: &mut Context<Self>) {
        let RepoData {
            tree,
            readme_path,
            readme,
            worktree,
            branches,
            tags,
            current_branch,
            head_commit,
        } = data;
        let Some(worktree) = worktree else {
            self.error = Some("Repository has no worktree".into());
            return;
        };

        self.worktree = Some(worktree);
        self.head_commit = head_commit;
        self.tree_state.update(cx, |state, cx| {
            state.set_items(tree_items(tree, false), cx);
        });

        // Populate the branch/tag selectors with the local refs, selecting
        // the branch HEAD points to.
        let branches: Vec<SharedString> = branches.into_iter().map(Into::into).collect();
        let tags: Vec<SharedString> = tags.into_iter().map(Into::into).collect();
        self.branch_select.update(cx, |state, cx| {
            state.set_items(SearchableVec::from(branches), window, cx);
            if let Some(branch) = current_branch {
                let branch: SharedString = branch.into();
                state.set_selected_values(&[branch], window, cx);
            }
        });
        self.tag_select.update(cx, |state, cx| {
            state.set_items(SearchableVec::from(tags), window, cx);
        });

        self.load_all_commits(cx);
        if let Some((path, bytes)) = readme_path.zip(readme) {
            self.readme_name = Some(path.to_string_lossy().into());
            self.load_commit(&path.to_string_lossy(), cx);
            if let Ok(text) = String::from_utf8(bytes) {
                self.set_markdown(None, &text, cx);
            }
        }
    }

    /// Clone the repository into a folder chosen by the user (outside the
    /// cache), then open the new clone in the system file manager. Like
    /// ngit's clone, this resolves the announcement's `clone` URLs and
    /// clones from the first working git server.
    fn clone_to_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.cloning {
            return;
        }

        let (clone_urls, name) = {
            let announcement = self.announcement(cx);
            let addr = announcement.addr();
            let clone_urls: Vec<String> =
                announcement.clone.iter().map(ToString::to_string).collect();
            // Directory name: the display name, falling back to the repo id;
            // both sanitized to a safe single path component.
            let name = announcement
                .name
                .as_ref()
                .map(|name| name.to_string())
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| addr.identifier.clone());
            let name = signed_git::sanitize_path_component(&name);
            let name = if name.is_empty() {
                "repository".to_owned()
            } else {
                name
            };
            (clone_urls, name)
        };

        self.cloning = true;
        cx.notify();

        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Clone".into()),
        });

        let task = cx.spawn_in(window, async move |this, cx| {
            // `Ok(Ok(Some(paths)))` means the user picked a folder; a
            // cancel (or a picker failure) resolves to anything else.
            let picked = match prompt.await {
                Ok(Ok(Some(mut paths))) => paths.pop(),
                _ => None,
            };
            let Some(folder) = picked else {
                this.update_in(cx, |this, _window, cx| {
                    this.cloning = false;
                    cx.notify();
                })?;
                return Ok(());
            };

            let destination = folder.join(&name);
            let destination_for_open = destination.clone();
            let result = cx
                .background_spawn(async move { signed_git::clone_repo(&clone_urls, &destination) })
                .await;

            this.update_in(cx, |this, _window, cx| {
                this.cloning = false;
                match result {
                    Ok(_) => cx.open_with_system(&destination_for_open),
                    Err(error) => {
                        this.error = Some(format!("Failed to clone: {error}").into());
                    }
                }
                cx.notify();
            })?;

            Ok(())
        });

        self.tasks.push(task);
    }

    /// Preview the file at `path` (relative to the worktree root).
    fn open_file(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.selected_file = Some(path.into());

        if self.files.contains_key(path) {
            // The file is cached, but the persistent markdown/code state may
            // still hold a different file; re-point it at this one (the parse
            // runs on a background task either way). Without this, the pane
            // would show a spinner forever.
            if let Some(FileContent::Text(text)) = self.files.get(path) {
                let text = text.clone();
                if is_markdown_path(path) {
                    if self.md.as_ref().map(|md| md.path.as_deref()) != Some(Some(path)) {
                        self.set_markdown(Some(path.into()), &text, cx);
                    }
                } else if self.code.as_ref().map(|code| code.path.as_str()) != Some(path) {
                    self.set_code(path.into(), &text, window, cx);
                }
            }
            cx.notify();
            return;
        }
        if self.loading_files.contains(path) {
            cx.notify();
            return;
        }

        // Paths come from our own tree walk, but never trust them: refuse
        // anything that could escape the worktree.
        let rel = Path::new(path);
        let unsafe_path = rel.is_absolute()
            || rel.components().any(|c| {
                matches!(
                    c,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            });

        let Some(worktree) = self.worktree.clone() else {
            return;
        };

        if unsafe_path {
            return;
        }

        self.loading_files.insert(path.to_string());
        let path = path.to_string();
        self.load_commit(&path, cx);
        let generation = self.ref_generation;

        let task = cx.spawn_in(window, async move |this, cx| {
            let path_for_read = path.clone();
            let content = cx
                .background_spawn(async move {
                    let full = worktree.join(&path_for_read);
                    // Refuse oversized files before reading them: reading a
                    // multi-gigabyte file just to classify it as too large
                    // would waste the disk and memory bandwidth.
                    let metadata = match std::fs::metadata(&full) {
                        Ok(metadata) => metadata,
                        Err(error) => return Err(anyhow::anyhow!("{}", error)),
                    };
                    if metadata.len() > MAX_PREVIEW_BYTES as u64 {
                        return Ok(FileContent::TooLarge);
                    }
                    let bytes = match std::fs::read(&full) {
                        Ok(bytes) => bytes,
                        Err(error) => return Err(anyhow::anyhow!("{}", error)),
                    };
                    match String::from_utf8(bytes) {
                        Ok(text) => Ok(FileContent::Text(text)),
                        Err(_) => Ok(FileContent::Binary),
                    }
                })
                .await;

            this.update_in(cx, |this, window, cx| {
                // The worktree was switched while this file was reading;
                // the result belongs to the previous branch. Clear the
                // in-flight marker either way, or the path could never be
                // loaded again.
                if generation != this.ref_generation {
                    this.loading_files.remove(&path);
                    return;
                }
                this.loading_files.remove(&path);
                match content {
                    Ok(kind) => {
                        if let FileContent::Text(text) = &kind {
                            if is_markdown_path(&path) {
                                let same = this.md.as_ref().map(|md| md.path.as_deref())
                                    == Some(Some(path.as_str()));
                                if !same {
                                    this.set_markdown(Some(path.clone().into()), text, cx);
                                }
                            } else {
                                let same = this.code.as_ref().map(|code| code.path.as_str())
                                    == Some(path.as_str());
                                if !same {
                                    this.set_code(path.clone().into(), text, window, cx);
                                }
                            }
                            this.preview_bytes += text.len();
                        }
                        this.files.insert(path.clone(), kind);
                        this.file_order.push_back(path);
                        this.evict_previews();
                    }
                    Err(error) => {
                        this.files
                            .insert(path, FileContent::Failed(error.to_string()));
                    }
                }
                cx.notify();
            })?;

            Ok(())
        });

        self.tasks.push(task);
    }

    /// Queue `path` for the per-file commit query; requests are batched into
    /// one history walk (see [`Self::load_commits`]).
    fn load_commit(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.commits.contains_key(path) || self.pending_commits.iter().any(|p| p == path) {
            return;
        }
        self.pending_commits.push(path.to_string());
        if !self.loading_commits {
            self.load_commits(cx);
        }
    }

    /// Walk history once for every queued path on a background task, and
    /// cache the latest commit touching each of them in [`Self::commits`]
    /// (for the file header in the content column).
    ///
    /// Batching shares one walk (and its object decodes) across all paths
    /// queued while the previous walk was in flight, instead of walking the
    /// full history per file.
    fn load_commits(&mut self, cx: &mut Context<Self>) {
        if self.pending_commits.is_empty() || self.loading_commits {
            return;
        }
        let Some(worktree) = self.worktree.clone() else {
            self.pending_commits.clear();
            return;
        };

        self.loading_commits = true;
        let paths = std::mem::take(&mut self.pending_commits);
        let generation = self.ref_generation;

        let task = cx.spawn(async move |this, cx| {
            let rels: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
            let result = cx
                .background_spawn(
                    async move { signed_git::worktree_last_commits(&worktree, &rels) },
                )
                .await;

            this.update(cx, |this, cx| {
                this.loading_commits = false;
                if generation == this.ref_generation
                    && let Ok(found) = result
                {
                    for (path, commit) in found {
                        this.commits
                            .insert(path.to_string_lossy().into_owned(), commit);
                    }
                }
                // Paths queued while the walk was in flight start the next
                // batch. A stale walk (branch switched mid-flight) must not
                // strand them, so this runs under the current generation
                // regardless of whether the result was applied.
                if !this.pending_commits.is_empty() {
                    this.load_commits(cx);
                }
                cx.notify();
            })?;

            Ok(())
        });

        self.tasks.push(task);
    }

    /// Walk all commits reachable from HEAD on a background task, for the
    /// Commits tab and its total-count badge. The list is capped by
    /// [`CommitList`]; only the newest commits are materialized.
    fn load_all_commits(&mut self, cx: &mut Context<Self>) {
        if self.loading_all_commits || self.all_commits.is_some() {
            return;
        }

        let Some(worktree) = self.worktree.clone() else {
            return;
        };

        self.loading_all_commits = true;
        let generation = self.ref_generation;

        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { signed_git::worktree_all_commits(&worktree) })
                .await;

            this.update(cx, |this, cx| {
                // A stale walk (branch switched mid-flight) must not leave
                // the flag set, or the Commits tab would spin forever.
                if generation != this.ref_generation {
                    this.loading_all_commits = false;
                    return;
                }
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

    /// Open a new panel showing the diff of `commit_id` (all files it
    /// changed, with the line diff of each). Called from the Commits tab
    /// rows and the latest-commit button in the header.
    fn open_commit_diff(&mut self, commit_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(worktree) = self.worktree.clone() else {
            return;
        };

        let Some(dock_area) = self.dock_area.upgrade() else {
            return;
        };

        // Same display name as the repo detail panel's title.
        let repo_name = self.display_name(cx);

        let panel =
            cx.new(|cx| CommitDiffView::new(worktree, repo_name, commit_id.into(), window, cx));

        dock_area.update(cx, |dock_area, cx| {
            dock_area.add_panel_view(panel_handle(panel), DockPlacement::Center, None, window, cx);
        });
    }

    /// Open the issues panel at the bottom of the dock area.
    fn open_issue_detail(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dock_area) = self.dock_area.upgrade() else {
            return;
        };

        let panel = cx.new(|cx| {
            IssuesView::new(
                self.dock_area.clone(),
                self.store.clone(),
                self.display_name(cx),
                window,
                cx,
            )
        });

        dock_area.update(cx, |dock_area, cx| {
            dock_area.add_panel_view(panel_handle(panel), DockPlacement::Center, None, window, cx);
        });
    }

    /// Open the pull requests panel at the bottom of the dock area.
    fn open_pull_request_detail(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dock_area) = self.dock_area.upgrade() else {
            return;
        };

        let panel = cx.new(|cx| {
            PullRequestsView::new(
                self.dock_area.clone(),
                self.store.clone(),
                self.display_name(cx),
                window,
                cx,
            )
        });

        dock_area.update(cx, |dock_area, cx| {
            dock_area.add_panel_view(panel_handle(panel), DockPlacement::Center, None, window, cx);
        });
    }

    /// Check out `name` (a branch or tag picked in the header) and refresh
    /// the explorer once the switch completes.
    fn switch_ref(
        &mut self,
        kind: RefKind,
        name: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.switching_ref {
            return;
        }
        let Some(worktree) = self.worktree.clone() else {
            return;
        };

        // Branches and tags are mutually exclusive states of HEAD: selecting
        // one clears the other selector. Remember the previous selections so
        // they can be restored if the checkout fails.
        let previous_branch = self.branch_select.read(cx).selected_value();
        let previous_tag = self.tag_select.read(cx).selected_value();

        match kind {
            RefKind::Branch => {
                self.tag_select
                    .update(cx, |state, cx| state.clear_selection(cx));
            }
            RefKind::Tag => {
                self.branch_select
                    .update(cx, |state, cx| state.clear_selection(cx));
            }
        }
        self.switching_ref = true;
        // In-flight loads of the previous branch are discarded when they
        // complete.
        self.ref_generation += 1;
        cx.notify();

        let checkout_name = name.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    match kind {
                        RefKind::Branch => {
                            signed_git::worktree_checkout_branch(&worktree, &checkout_name)
                        }
                        RefKind::Tag => {
                            signed_git::worktree_checkout_tag(&worktree, &checkout_name)
                        }
                    }
                })
                .await;

            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(()) => this.reload_worktree(cx),
                    Err(error) => {
                        this.error = Some(format!("Failed to check out {name}: {error}").into());
                        this.switching_ref = false;
                        this.restore_selection(&this.branch_select, &previous_branch, window, cx);
                        this.restore_selection(&this.tag_select, &previous_tag, window, cx);
                    }
                }
                cx.notify();
            })?;

            Ok(())
        });

        self.tasks.push(task);
    }

    /// Restore a selector to `previous`, or clear it (after a failed switch).
    fn restore_selection(
        &self,
        select: &Entity<ComboboxState<SearchableVec<SharedString>>>,
        previous: &Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        select.update(cx, |state, cx| match previous {
            Some(value) => state.set_selected_values(std::slice::from_ref(value), window, cx),
            None => state.clear_selection(cx),
        });
    }

    /// Trigger body for the branch/tag selectors: the kind icon, the
    /// selection (or placeholder) and the caret. `Combobox` replaces its
    /// default trigger entirely, which is the only way to show an icon
    /// inside the trigger label.
    fn render_ref_trigger(
        ctx: &ComboboxTriggerContext<SearchableVec<SharedString>>,
        icon: CustomIconName,
        cx: &App,
    ) -> AnyElement {
        let muted = cx.theme().muted_foreground;

        h_flex()
            .w_full()
            .min_w_0()
            .gap_1()
            .items_center()
            .child(Icon::new(icon).small().flex_shrink_0())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .when(ctx.selection().is_empty(), |this| this.text_color(muted))
                    .child(
                        ctx.selection()
                            .first()
                            .map(|(_, item)| item.clone())
                            .or_else(|| ctx.placeholder().cloned())
                            .unwrap_or_default(),
                    ),
            )
            .child(Caret::new(ctx.size()).text_color(muted))
            .into_any_element()
    }

    /// Refresh the file explorer, preview pane and commit list after a
    /// successful branch or tag switch. The selectors were already updated
    /// by [`Self::switch_ref`]; [`Self::switching_ref`] stays set until this
    /// reload finishes, so a second switch cannot interleave.
    fn reload_worktree(&mut self, cx: &mut Context<Self>) {
        let Some(worktree) = self.worktree.clone() else {
            return;
        };

        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let snapshot = signed_git::worktree_snapshot(&worktree)?;
                    // Build the tree off the main thread, like [`Self::load_repo`].
                    let tree = build_tree_items(&snapshot.entries);
                    Ok::<_, Error>((snapshot, tree))
                })
                .await;

            this.update(cx, |this, cx| {
                this.switching_ref = false;
                match result {
                    Ok((snapshot, tree)) => {
                        this.head_commit = snapshot.head_commit;
                        // Rebuild the tree from scratch: entries of the
                        // previous branch are gone, and with them the
                        // expansion state.
                        this.tree_state.update(cx, |state, cx| {
                            state.set_items(tree_items(tree, false), cx);
                        });

                        // Drop cached previews and commits of the old branch.
                        this.selected_file = None;
                        this.files.clear();
                        this.file_order.clear();
                        this.preview_bytes = 0;
                        this.loading_files.clear();
                        this.commits.clear();
                        this.pending_commits.clear();
                        this.loading_commits = false;
                        this.md = None;
                        this.code = None;
                        this.readme_name = None;
                        this.all_commits = None;
                        this.loading_all_commits = false;

                        if let Some((path, bytes)) = snapshot.readme_path.zip(snapshot.readme) {
                            this.readme_name = Some(path.to_string_lossy().into());
                            this.load_commit(&path.to_string_lossy(), cx);
                            if let Ok(text) = String::from_utf8(bytes) {
                                this.set_markdown(None, &text, cx);
                            }
                        }
                        this.load_all_commits(cx);
                    }
                    Err(error) => {
                        this.error = Some(error.to_string().into());
                        this.head_commit = None;
                        // The tree may show files that no longer exist.
                        this.tree_state.update(cx, |state, cx| {
                            state.set_items(Vec::new(), cx);
                        });
                    }
                }
                cx.notify();
            })?;

            Ok(())
        });

        self.tasks.push(task);
    }

    /// Drop the oldest previews beyond the cache caps, keeping the currently
    /// selected file. The parsed editor state of an evicted file is dropped
    /// along with its entry, so re-opening it re-parses on a background task.
    fn evict_previews(&mut self) {
        while (self.files.len() > MAX_PREVIEWED_FILES
            || self.preview_bytes > MAX_PREVIEW_CACHE_BYTES)
            && self.file_order.len() > 1
        {
            let path = self.file_order.pop_front().expect("non-empty");
            if Some(path.as_str()) == self.selected_file.as_deref() {
                self.file_order.push_back(path);
                continue;
            }
            if let Some(FileContent::Text(text)) = self.files.remove(&path) {
                self.preview_bytes -= text.len();
            }
            if self.md.as_ref().map(|md| md.path.as_deref()) == Some(Some(path.as_str())) {
                self.md = None;
            }
            if self
                .code
                .as_ref()
                .is_some_and(|code| code.path.as_ref() == path.as_str())
            {
                self.code = None;
            }
            self.commits.remove(&path);
        }
    }

    /// The latest announcement from the store, or the open-time snapshot.
    fn announcement<'a>(&'a self, cx: &'a App) -> &'a Announcement {
        self.store
            .read(cx)
            .announcement
            .as_ref()
            .unwrap_or(&self.initial)
    }

    /// Display name: the announcement's name, or the ID if no name is set.
    fn display_name(&self, cx: &App) -> SharedString {
        let announcement = self.announcement(cx);
        announcement
            .name
            .clone()
            .unwrap_or_else(|| SharedString::from(announcement.id.clone()))
    }

    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let store = self.store.read(cx);
        let announcement = store.announcement.as_ref().unwrap_or(&self.initial);
        let issue_count = SharedString::from(store.issue_count().to_string());
        let pr_count = SharedString::from(store.pull_request_count().to_string());

        let name = self.display_name(cx);
        let description = announcement.description();
        let avatar = PixelAvatar::new(format!("{}:{}", announcement.owner, announcement.id));
        let share = ShareTargets::from_announcement(announcement);

        let commits_count = self.all_commits.as_ref().map(|list| list.total);
        let worktree_empty = self.switching_ref || self.worktree.is_none();

        let nostr_url = nostr_clone_url(announcement, cx);
        let ngit_command = SharedString::from(format!("git clone {nostr_url}"));
        let nak_command = SharedString::from(format!("nak git clone {nostr_url}"));
        let git_commands = announcement.clone_urls();

        v_flex()
            .on_action(
                cx.listener(|this, action: &RepoAction, window, cx| match action {
                    RepoAction::NewIssue => {
                        open_new_issue_dialog(this.store.clone(), window, cx);
                    }
                    RepoAction::NewPR => {
                        open_new_pull_request_dialog(this.store.clone(), window, cx);
                    }
                }),
            )
            .px_4()
            .pb_4()
            .w_full()
            .gap_8()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .w_full()
                    .gap_4()
                    .items_start()
                    .justify_between()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .min_h_8()
                                    .font_semibold()
                                    .child(avatar.size_6())
                                    .child(name),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .line_clamp(2)
                                    .line_height(relative(1.25))
                                    .text_ellipsis()
                                    .child(description),
                            )
                            .child(
                                h_flex()
                                    .mt_2()
                                    .w_full()
                                    .gap_0p5()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .font_semibold()
                                            .child("Maintainers:"),
                                    )
                                    .child(self.render_maintainers(cx)),
                            ),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .gap_2()
                            .justify_end()
                            .child(
                                BaseDropdownButton::new("issues")
                                    .action(
                                        BaseButton::new("issues-open")
                                            .child(
                                                h_flex()
                                                    .h_8()
                                                    .px_2()
                                                    .gap_1()
                                                    .rounded(cx.theme().radius)
                                                    .bg(cx.theme().secondary)
                                                    .hover(|this| {
                                                        this.bg(cx.theme().secondary_hover)
                                                    })
                                                    .text_sm()
                                                    .text_color(cx.theme().secondary_foreground)
                                                    .child(Icon::new(CustomIconName::GitIssueDone))
                                                    .child("Issues")
                                                    .child(
                                                        div()
                                                            .mx_1()
                                                            .h_5()
                                                            .w_px()
                                                            .bg(cx.theme().border.darken(0.1)),
                                                    )
                                                    .child(issue_count),
                                            )
                                            .on_click(cx.listener(|this, _event, window, cx| {
                                                this.open_issue_detail(window, cx);
                                            })),
                                    )
                                    .dropdown_menu(|menu, _, _| {
                                        menu.menu_element_with_icon(
                                            IconName::Plus,
                                            Box::new(RepoAction::NewIssue),
                                            |_, _| div().text_xs().child("New issue"),
                                        )
                                    }),
                            )
                            .child(
                                BaseDropdownButton::new("prs")
                                    .action(
                                        BaseButton::new("prs-open")
                                            .child(
                                                h_flex()
                                                    .h_8()
                                                    .px_2()
                                                    .gap_1()
                                                    .rounded(cx.theme().radius)
                                                    .bg(cx.theme().secondary)
                                                    .hover(|this| {
                                                        this.bg(cx.theme().secondary_hover)
                                                    })
                                                    .text_sm()
                                                    .text_color(cx.theme().secondary_foreground)
                                                    .child(Icon::new(
                                                        CustomIconName::GitPullRequest,
                                                    ))
                                                    .child("Pull Requests")
                                                    .child(
                                                        div()
                                                            .mx_1()
                                                            .h_5()
                                                            .w_px()
                                                            .bg(cx.theme().border.darken(0.1)),
                                                    )
                                                    .child(pr_count),
                                            )
                                            .on_click(cx.listener(|this, _event, window, cx| {
                                                this.open_pull_request_detail(window, cx);
                                            })),
                                    )
                                    .dropdown_menu(|menu, _, _| {
                                        menu.menu_element_with_icon(
                                            IconName::Plus,
                                            Box::new(RepoAction::NewPR),
                                            |_, _| div().text_xs().child("New PR"),
                                        )
                                    }),
                            )
                            .child(
                                BaseDropdownButton::new("share")
                                    .action(
                                        Button::new("link")
                                            .icon(IconName::Copy)
                                            .tooltip("Copy ID")
                                            .secondary()
                                            .on_click({
                                                let naddr = share.naddr.clone();
                                                move |_, _, cx| {
                                                    cx.write_to_clipboard(
                                                        ClipboardItem::new_string(naddr.clone()),
                                                    );
                                                }
                                            }),
                                    )
                                    .dropdown_menu(move |menu, _, _| share.menu(menu)),
                            )
                            .child(
                                Button::new("info")
                                    .icon(IconName::Info)
                                    .tooltip("About")
                                    .secondary()
                                    .on_click(cx.listener(|this, _event, window, cx| {
                                        open_about_dialog(
                                            this.announcement(cx).clone(),
                                            window,
                                            cx,
                                        );
                                    })),
                            )
                            .child({
                                let view = cx.entity();
                                let ngit_command = ngit_command.clone();
                                let nak_command = nak_command.clone();
                                let git_commands = git_commands.clone();

                                Popover::new("clone")
                                    .anchor(Anchor::TopRight)
                                    .trigger(
                                        Button::new("clone")
                                            .icon(CustomIconName::GitClone)
                                            .tooltip("Clone")
                                            .loading(self.cloning)
                                            .disabled(self.cloning)
                                            .primary(),
                                    )
                                    .content(move |_, _window, cx| {
                                        let state = cx.entity();
                                        let ngit_row = command_row("copy-ngit", &ngit_command, cx);
                                        let nak_row = command_row("copy-nak", &nak_command, cx);

                                        v_flex()
                                            .w(px(440.))
                                            .mt_1()
                                            .p_3()
                                            .gap_4()
                                            .popover_style(cx)
                                            .child(
                                                v_flex()
                                                    .gap_1()
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .font_semibold()
                                                            .text_color(cx.theme().muted_foreground)
                                                            .child("Clone with ngit"),
                                                    )
                                                    .child(ngit_row),
                                            )
                                            .child(
                                                v_flex()
                                                    .gap_1()
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .font_semibold()
                                                            .text_color(cx.theme().muted_foreground)
                                                            .child("Clone with nak"),
                                                    )
                                                    .child(nak_row),
                                            )
                                            .child(
                                                v_flex()
                                                    .gap_1()
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .font_semibold()
                                                            .text_color(cx.theme().muted_foreground)
                                                            .child("Grasp Servers"),
                                                    )
                                                    .when(!git_commands.is_empty(), |this| {
                                                        this.children(
                                                            git_commands.iter().enumerate().map(
                                                                |(ix, cmd)| {
                                                                    command_row(
                                                                        format!("copy-git-{ix}"),
                                                                        cmd,
                                                                        cx,
                                                                    )
                                                                },
                                                            ),
                                                        )
                                                    })
                                                    .when(git_commands.is_empty(), |this| {
                                                        this.child(
                                                            div()
                                                                .text_xs()
                                                                .child("No git clone urls."),
                                                        )
                                                    }),
                                            )
                                            .child(div().h_px().w_full().bg(cx.theme().border))
                                            .child(
                                                h_flex().gap_1().justify_end().child(
                                                    Button::new("download")
                                                        .icon(CustomIconName::GitClone)
                                                        .label("Download")
                                                        .primary()
                                                        .on_click(move |_event, window, cx| {
                                                            state.update(cx, |state, cx| {
                                                                state.dismiss(window, cx);
                                                            });
                                                            view.update(cx, |this, cx| {
                                                                this.clone_to_folder(window, cx);
                                                            });
                                                        }),
                                                ),
                                            )
                                    })
                            }),
                    ),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        BaseButton::new("files-tab")
                            .flex()
                            .items_center()
                            .h_8()
                            .px_2()
                            .gap_2()
                            .child(
                                h_flex()
                                    .gap_1()
                                    .text_sm()
                                    .child(Icon::new(CustomIconName::GitFile).small())
                                    .child("Files"),
                            )
                            .text_color(cx.theme().button_foreground)
                            .rounded(cx.theme().radius)
                            .hover(|this| this.bg(cx.theme().button_hover))
                            .active(|this| this.bg(cx.theme().button_active))
                            .selected(self.active_tab == 0)
                            .when(self.active_tab == 0, |this| {
                                this.bg(cx.theme().button_active)
                            })
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.active_tab = 0;
                                cx.notify();
                            })),
                    )
                    .child(
                        BaseButton::new("commits-tab")
                            .flex()
                            .items_center()
                            .h_8()
                            .px_2()
                            .gap_2()
                            .child(
                                h_flex()
                                    .gap_1()
                                    .text_sm()
                                    .child(Icon::new(CustomIconName::GitCommit).small())
                                    .child("Commits"),
                            )
                            .when_some(commits_count, |this, count| {
                                this.child(
                                    h_flex()
                                        .justify_center()
                                        .px_1()
                                        .py_0p5()
                                        .min_w_4()
                                        .text_size(px(8.))
                                        .bg(cx.theme().muted)
                                        .text_color(cx.theme().muted_foreground)
                                        .rounded(cx.theme().radius)
                                        .line_height(relative(1.))
                                        .child(SharedString::from(count.to_string())),
                                )
                            })
                            .text_color(cx.theme().button_foreground)
                            .rounded(cx.theme().radius)
                            .hover(|this| this.bg(cx.theme().button_hover))
                            .active(|this| this.bg(cx.theme().button_active))
                            .selected(self.active_tab == 1)
                            .when(self.active_tab == 1, |this| {
                                this.bg(cx.theme().button_active)
                            })
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.active_tab = 1;
                                cx.notify();
                            })),
                    )
                    .child(
                        h_flex()
                            .flex_1()
                            .gap_2()
                            .justify_end()
                            .child(
                                Button::new("enc")
                                    .ghost()
                                    .when_some(self.head_commit.as_ref(), |this, commit| {
                                        this.child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(SharedString::from(&commit.id)),
                                        )
                                        .child(
                                            div()
                                                .max_w(px(200.))
                                                .overflow_hidden()
                                                .text_ellipsis()
                                                .whitespace_nowrap()
                                                .text_xs()
                                                .child(SharedString::from(&commit.summary)),
                                        )
                                    })
                                    .tooltip(
                                        self.head_commit
                                            .as_ref()
                                            .map_or_else(SharedString::default, |commit| {
                                                commit.summary.clone().into()
                                            }),
                                    )
                                    .on_click(cx.listener(|this, _event, window, cx| {
                                        if let Some(commit) = &this.head_commit {
                                            let id = commit.id.clone();
                                            this.open_commit_diff(&id, window, cx);
                                        }
                                    })),
                            )
                            .child(
                                div().w(px(120.)).child(
                                    Combobox::new(&self.branch_select)
                                        .placeholder("Branch")
                                        .appearance(false)
                                        .menu_width(px(200.))
                                        .disabled(worktree_empty)
                                        .bg(cx.theme().muted)
                                        .rounded(cx.theme().radius)
                                        .render_trigger(|ctx, _window, cx| {
                                            Self::render_ref_trigger(
                                                ctx,
                                                CustomIconName::GitBranch,
                                                cx,
                                            )
                                        }),
                                ),
                            )
                            .child(
                                div().w(px(120.)).child(
                                    Combobox::new(&self.tag_select)
                                        .placeholder("Tag")
                                        .appearance(false)
                                        .menu_width(px(200.))
                                        .disabled(worktree_empty)
                                        .bg(cx.theme().muted)
                                        .rounded(cx.theme().radius)
                                        .render_trigger(|ctx, _window, cx| {
                                            Self::render_ref_trigger(ctx, CustomIconName::Tag, cx)
                                        }),
                                ),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_maintainers(&self, cx: &mut Context<Self>) -> AnyElement {
        let announcement = self.announcement(cx);
        let profile_store = ProfileStore::global(cx);

        let mut seen = HashSet::new();
        let rest: Vec<_> = announcement
            .maintainers
            .iter()
            .copied()
            .filter(|key| key != &announcement.owner && seen.insert(*key))
            .collect();

        let owner = profile_store.read(cx).get(&announcement.owner);
        let owner_name = owner.name();
        let owner_picture = owner.picture();

        h_flex()
            .w_full()
            .gap_3()
            .child(
                Button::new("maintainers").compact().ghost().child(
                    h_flex()
                        .gap_2()
                        .child(
                            h_flex()
                                .gap_1()
                                .child(
                                    Avatar::new()
                                        .name(owner_name.clone())
                                        .when_some(owner_picture, |this, url| this.src(url))
                                        .rounded(cx.theme().radius)
                                        .small(),
                                )
                                .child(div().text_xs().whitespace_nowrap().child(owner_name)),
                        )
                        .when(!rest.is_empty(), |this| {
                            this.child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(SharedString::from(format!("+{}", rest.len()))),
                            )
                        }),
                ),
            )
            .into_any_element()
    }
}

impl BasePanel for RepoDetailView {
    fn panel_name(&self) -> &'static str {
        "repo_detail"
    }
}

impl Panel for RepoDetailView {
    fn title(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.display_name(cx)
    }
}

impl EventEmitter<PanelEvent> for RepoDetailView {}

impl Focusable for RepoDetailView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for RepoDetailView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tree_state = self.tree_state.clone();
        let view = cx.entity().downgrade();

        let pane_title = self
            .selected_file
            .clone()
            .or_else(|| self.readme_name.clone())
            .unwrap_or_else(|| "Overview".into());

        v_flex()
            .image_cache(image_cache("repo", MAX_IMAGES))
            .id("repo")
            .size_full()
            .child(self.render_header(cx))
            .map(|this| match self.active_tab {
                0 => this.child(
                    h_flex()
                        .flex_1()
                        .w_full()
                        .overflow_hidden()
                        .child(Self::render_tree_column(tree_state, view, cx))
                        .child(self.render_content_column(pane_title, cx))
                        .into_any_element(),
                ),
                _ => this.child(self.render_commits_tab(cx)),
            })
    }
}

/// Read the worktree state of `repo` (no network): entries, README, refs
/// and HEAD commit. The tree is built off the main thread; the seeds are
/// plain owned strings and convert to `TreeItem`s (which hold `Rc` state)
/// on the main thread.
fn load_repo_data(repo: &Repository) -> Result<RepoData, Error> {
    let entries = signed_git::worktree_entries(repo)?;
    let tree = build_tree_items(&entries);
    let readme_path = signed_git::find_readme(repo)?;
    let readme = match &readme_path {
        Some(path) => signed_git::worktree_read(repo, path)?,
        None => None,
    };
    let worktree = repo.workdir().map(Path::to_path_buf);
    // Ref listing is auxiliary UI: a broken ref must not prevent the
    // explorer from loading, so failures degrade to empty selectors.
    let (branches, tags, current_branch) = match &worktree {
        Some(_) => (
            signed_git::repo_branches(repo).unwrap_or_default(),
            signed_git::repo_tags(repo).unwrap_or_default(),
            signed_git::current_branch(repo).unwrap_or(None),
        ),
        None => (Vec::new(), Vec::new(), None),
    };
    let head_commit = signed_git::head_commit(repo).unwrap_or(None);

    Ok(RepoData {
        tree,
        readme_path,
        readme,
        worktree,
        branches,
        tags,
        current_branch,
        head_commit,
    })
}

/// The `nostr://...` clone URL of an announcement (NIP-34): the owner as a
/// NIP-05 identifier when known (npub otherwise), the first announced relay
/// as a hint, and the repository identifier.
fn nostr_clone_url(announcement: &Announcement, cx: &App) -> SharedString {
    let owner = announcement.owner;
    let user = ProfileStore::global(cx)
        .read(cx)
        .get(&owner)
        .metadata()
        .nip05
        .as_deref()
        .filter(|nip05| !nip05.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| owner.to_bech32().unwrap_or_else(|_| owner.to_hex()));

    let mut url = format!("nostr://{user}");
    if let Some(hint) = announcement.relays.first().and_then(RelayUrl::domain) {
        url.push('/');
        url.push_str(hint);
    }
    url.push('/');
    url.push_str(&announcement.id);

    SharedString::from(url)
}

fn command_row<E>(copy_id: E, command: &SharedString, cx: &mut App) -> Div
where
    E: Into<ElementId>,
{
    h_flex()
        .h_8()
        .w_full()
        .px_2()
        .gap_2()
        .items_center()
        .bg(cx.theme().muted)
        .rounded(cx.theme().radius)
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_ellipsis()
                .text_xs()
                .child(command.clone()),
        )
        .child(
            Clipboard::new(copy_id)
                .tooltip("Copy")
                .value(command.clone()),
        )
}
