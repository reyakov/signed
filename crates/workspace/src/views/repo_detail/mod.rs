use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use anyhow::Error;
use assets::CustomIconName;
use dock::{BasePanel, DockArea, DockPlacement, Panel, PanelEvent, panel_handle};
use gix::Repository;
use gpui::prelude::*;
use gpui::{
    Action, Anchor, AnyElement, App, ClipboardItem, Context, Entity, EventEmitter, FocusHandle,
    Focusable, PathPromptOptions, Pixels, Render, SharedString, Size, Subscription, Task,
    WeakEntity, Window, div, px, relative, size,
};
use gpui_base::{Button as BaseButton, Disableable, Popover};
use gpui_component::alert::Alert;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::combobox::{
    Caret, Combobox, ComboboxEvent, ComboboxState, ComboboxTriggerContext,
};
use gpui_component::menu::DropdownMenu;
use gpui_component::searchable_list::SearchableVec;
use gpui_component::tree::TreeState;
use gpui_component::{
    ActiveTheme, Colorize, Icon, IconName, Sizable, StyledExt, ThemeStyled,
    VirtualListScrollHandle, h_flex, v_flex,
};
use nostr::prelude::{EventId, RelayUrl, ToBech32};
use signed_core::{Announcement, RepoAddr, filters};
use signed_git::{CommitList, FileCommit};
use signed_state::{Backend, GitStore, LocalReposStore, ProfileStore, RepoListStore, RepoStore};
use signed_ui::image_cache::{MAX_IMAGES, image_cache};
use signed_ui::{DropdownButton, PixelAvatar, UserAvatar, copy_row};

mod about;
mod browser;
mod commits;
mod diff;
mod helpers;
mod init_dialog;
mod issue_detail;
mod issues;
mod new_pull_request;
mod pull_request_detail;
mod pull_requests;
mod send_patch;

use about::open_about_dialog;
use browser::{
    CodeView, FileContent, MAX_PREVIEW_BYTES, MAX_PREVIEW_CACHE_BYTES, MAX_PREVIEWED_FILES,
    MarkdownView,
};
use commits::COMMIT_ROW_HEIGHT;
use diff::CommitDiffView;
use helpers::{ShareTargets, TreeItemSeed, build_tree_items, is_markdown_path, tree_items};
use issues::{IssuesView, open_new_issue_dialog};
use pull_requests::PullRequestsView;
use send_patch::open_send_patch_panel;

use crate::views::repo_detail::new_pull_request::open_new_pull_panel;

/// What kind of ref the header selectors switch to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RefKind {
    /// A local branch (`refs/heads/*`); HEAD stays attached.
    Branch,
    /// A tag (`refs/tags/*`); HEAD becomes detached.
    Tag,
}

/// Header actions dispatched by the dropdown menus of the header buttons.
/// `pub(super)`: the pull-request list panel offers the same New-PR / Send-
/// patch actions in its own dropdown.
#[derive(Clone, Action, PartialEq, Eq)]
#[action(namespace = repo_detail, no_json)]
pub(super) enum RepoAction {
    /// Open the "new issue" dialog.
    NewIssue,
    /// Open the "new pull request" dialog.
    NewPR,
    /// Open the "send patch" panel.
    SendPatch,
    /// Open the about dialog.
    About,
    /// Re-push the repository to its grasp servers.
    Push,
    /// Delete the repository from nostr (owner only).
    Delete,
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

/// Derived NIP-34 header data, cached so renders don't re-encode bech32
/// share targets and rebuild clone command strings on every frame.
struct HeaderCache {
    /// Announcement event ID and owner NIP-05 this cache was built from;
    /// rebuilt when either changes (a new announcement version, or the
    /// owner's profile arriving with a NIP-05 identifier).
    key: (EventId, Option<String>),
    announcement: Rc<Announcement>,
    share: Rc<ShareTargets>,
    ngit_command: SharedString,
    nak_command: SharedString,
    git_commands: Rc<Vec<SharedString>>,
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
    /// `None` for local repositories that haven't been published yet.
    initial: Option<Announcement>,
    /// Per-repository nostr store (announcement, issues, PRs, statuses).
    /// `None` until a local repository is initialized (published) to
    /// NIP-34.
    store: Option<Entity<RepoStore>>,
    /// Path of the local repository when opened from the scan; `None` once
    /// it has been initialized to NIP-34 (or for announced repositories).
    local_path: Option<PathBuf>,
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
    /// A push to the grasp servers is in flight.
    pushing: bool,
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
    /// Derived NIP-34 header data (share targets, clone commands),
    /// rebuilt only when the announcement or the owner's NIP-05 changes
    /// instead of on every render.
    header_cache: Option<HeaderCache>,
    /// In-flight tasks; finished tasks are pruned on every push, so the vec
    /// stays bounded by the number of concurrent loads.
    tasks: Vec<Task<Result<(), Error>>>,
    /// Subscriptions keeping the selectors' confirm events alive.
    _subscriptions: Vec<Subscription>,
    /// Upstream repository (from this fork's `u` tag) the user asked to
    /// open, while its announcement is still being fetched.
    pending_upstream: Option<RepoAddr>,
}

impl RepoDetailView {
    /// Open a repository announced on NIP-34: the store connects to the
    /// announcement's relays and loads issues, PRs and statuses.
    pub fn new(
        dock_area: WeakEntity<DockArea>,
        initial: Announcement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // The announcement we opened from already carries the repository's
        // NIP-34 `relays` tag, so the store can connect to those relays
        // immediately instead of waiting for the bootstrap fetch.
        let addr = initial.addr();
        let relays = initial.relays.clone();
        let store = cx.new(|cx| RepoStore::new(addr, relays, cx));

        Self::new_common(dock_area, Some(initial), Some(store), None, window, cx)
    }

    /// Open a local repository discovered by the scan. There is no
    /// announcement and no nostr store until the user initializes
    /// (publishes) it to NIP-34, so the header shows an Init button
    /// instead of the NIP-34 actions.
    pub fn new_local(
        dock_area: WeakEntity<DockArea>,
        local_path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_common(dock_area, None, None, Some(local_path), window, cx)
    }

    /// Shared construction: file explorer state, ref selectors and the
    /// deferred repository load.
    fn new_common(
        dock_area: WeakEntity<DockArea>,
        initial: Option<Announcement>,
        store: Option<Entity<RepoStore>>,
        local_path: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
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
            local_path,
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
            pushing: false,
            error: None,
            head_commit: None,
            branch_select,
            tag_select,
            switching_ref: false,
            ref_generation: 0,
            header_cache: None,
            focus_handle: cx.focus_handle(),
            tasks: Vec::new(),
            _subscriptions: subscriptions,
            pending_upstream: None,
        }
    }

    /// Load the repository and populate the file explorer. A local
    /// (not yet published) repository is opened straight from disk. An
    /// announced repository's local clone (if any) is loaded first without
    /// touching the network, so an unreachable server can't block the
    /// panel; a background fetch then refreshes the refs and commit list.
    fn load_repo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.loading = true;
        self.error = None;
        cx.notify();

        // Local repositories live on disk at their scan path; there is no
        // clone to ensure and no network refresh.
        if let Some(local_path) = self.local_path.clone() {
            let task = cx.spawn_in(window, async move |this, cx| {
                let data = cx
                    .background_spawn(async move {
                        let repo = gix::open(&local_path)?;
                        load_repo_data(&repo)
                    })
                    .await;

                this.update_in(cx, |this, window, cx| {
                    match data {
                        Ok(data) => this.apply_repo_data(data, window, cx),
                        Err(error) => this.error = Some(error.to_string().into()),
                    }
                    this.loading = false;
                    cx.notify();
                })?;
                Ok(())
            });
            self.tasks.push(task);
            return;
        }

        let Some(initial) = self.initial.as_ref() else {
            return;
        };
        let cache = GitStore::global(cx).cache().clone();
        let addr = initial.addr();
        let clone_urls: Vec<String> = initial.clone.iter().map(ToString::to_string).collect();
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
                    let branches: Vec<SharedString> = branches.iter().map(Into::into).collect();
                    let tags: Vec<SharedString> = tags.iter().map(Into::into).collect();

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

                    let new_head_commit = head_commit.as_ref().map(|c| &c.id);
                    let current_head_commit = this.head_commit.as_ref().map(|c| &c.id);
                    let head_changed = new_head_commit != current_head_commit;
                    this.head_commit = head_commit;

                    if head_changed || this.all_commits.is_none() {
                        this.all_commits = None;
                        this.loading_all_commits = false;
                        this.load_all_commits(cx);
                    }

                    cx.notify();
                }
            })?;

            Ok(())
        });

        self.tasks.push(task);
    }

    /// Apply the loaded repository data: explorer tree, README preview,
    /// ref selectors and HEAD commit, then start the commit-list walk.
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

        // Populate the branch/tag selectors with the local refs,
        // selecting the branch HEAD points to.
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

    /// Clone the repository into a folder chosen by the user (outside the cache),
    /// then open the new clone in the system file manager.
    fn clone_to_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.cloning {
            return;
        }

        let (clone_urls, name) = {
            let Some(announcement) = self.announcement(cx) else {
                return;
            };
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
    /// (for the file header in the content column). Batching shares one
    /// walk across all paths queued while the previous walk was in flight.
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

    /// Re-push the repository's refs to its announced grasp servers; the
    /// menu trigger shows a spinner while the push is in flight, failures
    /// appear in the panel's error banner.
    fn push_repository(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pushing {
            return;
        }
        let Some(announcement) = self.announcement(cx).cloned() else {
            return;
        };
        self.pushing = true;
        self.error = None;
        cx.notify();

        let backend = Backend::global(cx);
        let task = backend.update(cx, |backend, cx| backend.push_repository(announcement, cx));

        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, _window, cx| {
                if let Err(error) = result {
                    this.error = Some(format!("Push failed: {error}").into());
                }
                this.pushing = false;
                cx.notify();
            })?;
            Ok(())
        }));
    }

    /// Delete the repository from nostr (announcement, state and activity);
    /// only offered to the repository owner. The sidebar list updates when
    /// the deletion events arrive.
    fn delete_repository(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(announcement) = self.announcement(cx).cloned() else {
            return;
        };
        let backend = Backend::global(cx);
        let task = backend.update(cx, |backend, cx| {
            backend.delete_repository(announcement.addr(), cx)
        });

        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, _window, cx| {
                if let Err(error) = result {
                    this.error = Some(format!("Delete failed: {error}").into());
                }
                cx.notify();
            })?;
            Ok(())
        }));
    }

    /// Open the issues panel at the bottom of the dock area.
    fn open_issue_detail(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(store) = self.store.clone() else {
            return;
        };
        let Some(dock_area) = self.dock_area.upgrade() else {
            return;
        };

        let panel = cx.new(|cx| {
            IssuesView::new(
                self.dock_area.clone(),
                store,
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
        let Some(store) = self.store.clone() else {
            return;
        };
        let Some(dock_area) = self.dock_area.upgrade() else {
            return;
        };

        let panel = cx.new(|cx| {
            PullRequestsView::new(
                self.dock_area.clone(),
                store,
                self.display_name(cx),
                window,
                cx,
            )
        });

        dock_area.update(cx, |dock_area, cx| {
            dock_area.add_panel_view(panel_handle(panel), DockPlacement::Center, None, window, cx);
        });
    }

    /// Open the upstream repository (the `u` tag of this fork's announcement).
    /// When the upstream announcement is not in the local database yet,
    /// subscribe for it and open the panel as soon as it lands.
    fn open_upstream(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending_upstream.is_some() {
            return;
        }

        let Some(announcement) = self.announcement(cx).cloned() else {
            return;
        };

        let Some(addr) = announcement.upstream.and_then(|upstream| upstream.addr) else {
            return;
        };

        if let Some(found) = RepoListStore::global(cx)
            .read(cx)
            .announcements
            .iter()
            .find(|a| a.addr() == addr)
            .cloned()
        {
            open_repo_panel(&self.dock_area, &found, window, &mut *cx);
            return;
        }

        let backend = Backend::global(cx);
        backend.update(cx, |backend, cx| {
            backend.subscribe_bootstrap(vec![filters::announcement(&addr)], cx);
        });
        self.pending_upstream = Some(addr);

        let task = cx.spawn_in(window, async move |this, cx| {
            for _ in 0..60 {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;

                let opened = this.update_in(cx, |this, window, cx| {
                    let Some(addr) = this.pending_upstream.clone() else {
                        return true;
                    };
                    let found = RepoListStore::global(cx)
                        .read(cx)
                        .announcements
                        .iter()
                        .find(|a| a.addr() == addr)
                        .cloned();
                    match found {
                        Some(found) => {
                            this.pending_upstream = None;
                            open_repo_panel(&this.dock_area, &found, window, &mut *cx);
                            true
                        }
                        None => false,
                    }
                })?;

                if opened {
                    return Ok(());
                }
            }

            this.update(cx, |this, _cx| this.pending_upstream = None)?;
            Ok(())
        });

        self.tasks.push(task);
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
    /// default trigger entirely, the only way to show an icon inside it.
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

    /// The latest announcement from the store, or the open-time snapshot;
    /// `None` for local repositories that haven't been published yet.
    fn announcement<'a>(&'a self, cx: &'a App) -> Option<&'a Announcement> {
        let store = self.store.as_ref()?;
        store
            .read(cx)
            .announcement
            .as_ref()
            .or(self.initial.as_ref())
    }

    /// Display name: the announcement's name (or ID) for announced
    /// repositories, the directory name for local ones.
    fn display_name(&self, cx: &App) -> SharedString {
        if let Some(path) = &self.local_path {
            return SharedString::from(
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string()),
            );
        }
        self.announcement(cx)
            .map(|announcement| {
                announcement
                    .name
                    .clone()
                    .unwrap_or_else(|| SharedString::from(announcement.id.clone()))
            })
            .unwrap_or_default()
    }

    /// The NIP-34 header (actions, issues/PR counts) or, for a local
    /// repository that hasn't been published yet, the local header with an
    /// Init button.
    fn render_header(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if self.local_path.is_some() {
            return self.render_local_header(cx);
        }

        let Some(store_entity) = self.store.as_ref() else {
            return div().into_any_element();
        };
        let store = store_entity.read(cx);
        let issue_count = SharedString::from(store.issue_count().to_string());
        let pr_count = SharedString::from(store.pull_request_count().to_string());

        let Some(source) = store.announcement.as_ref().or(self.initial.as_ref()) else {
            return div().into_any_element();
        };

        // The header derives bech32 share targets and clone command strings
        // from the announcement; rebuild them only when the announcement or
        // the owner's NIP-05 changes, not on every render.
        let nip05 = ProfileStore::global(cx)
            .read(cx)
            .get(&source.owner)
            .metadata()
            .nip05
            .clone()
            .filter(|nip05| !nip05.trim().is_empty());
        let key = (source.event_id, nip05);

        if self
            .header_cache
            .as_ref()
            .is_none_or(|cache| cache.key != key)
        {
            let announcement = source.clone();
            let share = ShareTargets::from_announcement(&announcement);
            let nostr_url = nostr_clone_url(&announcement, key.1.as_deref());
            self.header_cache = Some(HeaderCache {
                ngit_command: SharedString::from(format!("git clone {nostr_url}")),
                nak_command: SharedString::from(format!("nak git clone {nostr_url}")),
                git_commands: Rc::new(announcement.clone_urls()),
                share: Rc::new(share),
                announcement: Rc::new(announcement),
                key,
            });
        }

        let cache = self.header_cache.as_ref().expect("cache just built");
        let announcement = cache.announcement.clone();
        let share = cache.share.clone();
        let ngit_command = cache.ngit_command.clone();
        let nak_command = cache.nak_command.clone();
        let git_commands = cache.git_commands.clone();

        let name = self.display_name(cx);
        let description = announcement.description();
        let avatar = PixelAvatar::new(format!("{}:{}", announcement.owner, announcement.id));

        v_flex()
            .on_action(
                cx.listener(|this, action: &RepoAction, window, cx| match action {
                    RepoAction::NewIssue => {
                        if let Some(store) = this.store.clone() {
                            open_new_issue_dialog(store, window, cx);
                        }
                    }
                    RepoAction::NewPR => {
                        if let Some(store) = this.store.clone() {
                            open_new_pull_panel(this.dock_area.clone(), store, window, cx);
                        }
                    }
                    RepoAction::SendPatch => {
                        if let Some(store) = this.store.clone() {
                            open_send_patch_panel(this.dock_area.clone(), store, window, cx);
                        }
                    }
                    RepoAction::About => {
                        if let Some(announcement) = this.announcement(cx) {
                            open_about_dialog(announcement.clone(), window, cx);
                        }
                    }
                    RepoAction::Push => this.push_repository(window, cx),
                    RepoAction::Delete => this.delete_repository(window, cx),
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
                            .when_some(fork_row(&announcement, cx), |this, row| this.child(row))
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
                                DropdownButton::new("issues")
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
                                        menu.menu_element(Box::new(RepoAction::NewIssue), |_, _| {
                                            h_flex()
                                                .gap_2()
                                                .text_sm()
                                                .child(Icon::new(IconName::Plus))
                                                .child("New issue")
                                        })
                                    }),
                            )
                            .child(
                                DropdownButton::new("prs")
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
                                        menu.menu_element(Box::new(RepoAction::NewPR), |_, _| {
                                            h_flex()
                                                .gap_2()
                                                .text_sm()
                                                .child(Icon::new(IconName::Plus))
                                                .child("New Pull Request")
                                        })
                                        .menu_element(
                                            Box::new(RepoAction::SendPatch),
                                            |_, _| {
                                                h_flex()
                                                    .gap_2()
                                                    .text_sm()
                                                    .child(Icon::new(IconName::File))
                                                    .child("Send Patch")
                                            },
                                        )
                                    }),
                            )
                            .child(
                                DropdownButton::new("share")
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
                                Button::new("repo-menu-open")
                                    .icon(IconName::EllipsisVertical)
                                    .tooltip("Repository management")
                                    .compact()
                                    .secondary()
                                    .loading(self.pushing)
                                    .disabled(self.pushing)
                                    .dropdown_menu(move |menu, _, cx| {
                                        let backend = Backend::global(cx);
                                        let current_user = backend.read(cx).current_user();
                                        let owner = current_user == Some(announcement.owner);

                                        let menu = menu.menu_element(
                                            Box::new(RepoAction::About),
                                            |_, _| {
                                                h_flex()
                                                    .gap_2()
                                                    .text_sm()
                                                    .child(Icon::new(IconName::Info))
                                                    .child("About")
                                            },
                                        );

                                        if owner {
                                            menu.menu_element(Box::new(RepoAction::Push), |_, _| {
                                                h_flex()
                                                    .gap_2()
                                                    .text_sm()
                                                    .child(Icon::new(CustomIconName::Init))
                                                    .child("Republish")
                                            })
                                            .separator()
                                            .menu_element(Box::new(RepoAction::Delete), |_, cx| {
                                                h_flex()
                                                    .gap_2()
                                                    .text_sm()
                                                    .text_color(cx.theme().danger)
                                                    .child(Icon::new(IconName::Delete))
                                                    .child("Delete")
                                            })
                                        } else {
                                            menu
                                        }
                                    }),
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
                                        let ngit_row = copy_row("copy-ngit", &ngit_command, cx);
                                        let nak_row = copy_row("copy-nak", &nak_command, cx);

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
                                                                    copy_row(
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
            .child(self.render_header_tabs(cx))
            .into_any_element()
    }

    /// Header for a local (not yet published) repository: the directory
    /// name and path with an Init button instead of the NIP-34 actions
    /// (issues, pull requests, share, info, clone).
    fn render_local_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let name = self.display_name(cx);
        let path = self
            .local_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        let avatar = PixelAvatar::new(path.clone());

        v_flex()
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
                                    .child(path),
                            ),
                    )
                    .child(
                        Button::new("init")
                            .icon(CustomIconName::Init)
                            .label("Initialize on Nostr")
                            .primary()
                            .tooltip("Publish this repository to Nostr")
                            .on_click(cx.listener(|this, _event, window, cx| {
                                this.open_init_dialog(window, cx);
                            })),
                    ),
            )
            .child(self.render_header_tabs(cx))
            .into_any_element()
    }

    /// Open the dialog guiding the user through publishing the local
    /// repository to NIP-34.
    fn open_init_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(local_path) = self.local_path.clone() else {
            return;
        };
        let view = cx.entity().downgrade();
        init_dialog::open(local_path, view, window, cx);
    }

    /// Switch the repository into its NIP-34 mode after a successful init:
    /// create the nostr store for the announced repository and drop the
    /// local (scan) identity. The worktree is unchanged, so the file
    /// explorer keeps its loaded content.
    pub(crate) fn apply_announcement(
        &mut self,
        announcement: Announcement,
        cx: &mut Context<Self>,
    ) {
        // The repository is no longer a bare local repo: drop it from the
        // scan results so it leaves the sidebar's local section immediately.
        if let Some(path) = self.local_path.take() {
            LocalReposStore::global(cx).update(cx, |store, cx| store.remove(&path, cx));
        }
        let store =
            cx.new(|cx| RepoStore::new(announcement.addr(), announcement.relays.clone(), cx));
        // Re-render when the store refreshes (issues, PRs, statuses).
        self._subscriptions
            .push(cx.observe(&store, |_this, _store, cx| cx.notify()));
        self.store = Some(store);
        self.initial = Some(announcement);
        cx.notify();
    }

    /// The tab row shared by both header variants: Files/Commits tabs, the
    /// HEAD commit button and the branch/tag selectors.
    fn render_header_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let commits_count = self.all_commits.as_ref().map(|list| list.total);
        let worktree_empty = self.switching_ref || self.worktree.is_none();

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
                                    Self::render_ref_trigger(ctx, CustomIconName::GitBranch, cx)
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
            )
            .into_any_element()
    }

    fn render_maintainers(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(announcement) = self.announcement(cx) else {
            return div().into_any_element();
        };
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
                                .child(UserAvatar::new(owner_name.clone()).picture(owner_picture))
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
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    Alert::error("repo-error", error)
                        .banner()
                        .on_close(cx.listener(|this, _event, _window, cx| {
                            this.error = None;
                            cx.notify();
                        })),
                )
            })
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
/// and HEAD commit.
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
/// as a hint, and the repository identifier. `nip05` is the owner's
/// NIP-05 identifier from the profile store, already blank-filtered.
fn nostr_clone_url(announcement: &Announcement, nip05: Option<&str>) -> SharedString {
    let owner = announcement.owner;
    let user = nip05
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

/// The "Forked from …" row of the detail header: a clickable link to the
/// upstream repository when the `u` tag references a NIP-34 repo,
/// plain text when it only carries a git URL.
fn fork_row(announcement: &Announcement, cx: &mut Context<RepoDetailView>) -> Option<AnyElement> {
    let upstream = announcement.upstream.as_ref()?;

    let (label, clickable) = match &upstream.addr {
        Some(addr) => {
            // Prefer the upstream's display name when its announcement
            // is already known locally fall back to its repository id.
            let name = RepoListStore::global(cx)
                .read(cx)
                .announcements
                .iter()
                .find(|a| a.addr() == *addr)
                .map(|a| {
                    a.name
                        .clone()
                        .unwrap_or_else(|| SharedString::from(a.id.clone()))
                })
                .unwrap_or_else(|| SharedString::from(addr.identifier.clone()));
            (SharedString::from(format!("Forked from {name}")), true)
        }
        None => (upstream.display(), false),
    };

    let row = h_flex()
        .gap_1()
        .items_center()
        .min_w_0()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(Icon::new(CustomIconName::GitBranch).small())
        .child(div().whitespace_nowrap().text_ellipsis().child(label));

    Some(if clickable {
        row.id("fork-upstream")
            .cursor_pointer()
            .hover(|this| this.text_color(cx.theme().foreground))
            .on_click(cx.listener(|this, _ev, window, cx| this.open_upstream(window, cx)))
            .into_any_element()
    } else {
        row.into_any_element()
    })
}

/// Open `announcement` as a repository panel in the dock's center, returning
/// the new detail view. Shared by the explore list, the sidebar and fork
/// links so every entry point opens repositories identically.
pub(crate) fn open_repo_panel(
    dock_area: &WeakEntity<DockArea>,
    announcement: &Announcement,
    window: &mut Window,
    cx: &mut App,
) -> Entity<RepoDetailView> {
    let detail =
        cx.new(|cx| RepoDetailView::new(dock_area.clone(), announcement.clone(), window, cx));

    if let Some(dock_area) = dock_area.upgrade() {
        dock_area.update(cx, |dock_area, cx| {
            dock_area.add_panel_view(
                panel_handle(detail.clone()),
                DockPlacement::Center,
                None,
                window,
                cx,
            );
        });
    }

    detail
}
