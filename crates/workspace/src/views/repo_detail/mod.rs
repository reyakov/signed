use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use anyhow::Error;
use assets::CustomIconName;
use dock::{BasePanel, DockArea, Panel, PanelEvent, add_center_panel, panel_handle};
use gix::Repository;
use gpui::prelude::*;
use gpui::{
    Action, Anchor, AnyElement, App, ClipboardItem, Context, Entity, EventEmitter, FocusHandle,
    Focusable, PathPromptOptions, Pixels, Render, SharedString, Size, Subscription, WeakEntity,
    Window, div, px, relative, size, transparent_white,
};
use gpui_base::{Button as BaseButton, Disableable, Popover};
use gpui_component::alert::Alert;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::combobox::{Combobox, ComboboxEvent, ComboboxState};
use gpui_component::menu::DropdownMenu;
use gpui_component::searchable_list::SearchableVec;
use gpui_component::tree::TreeState;
use gpui_component::{
    ActiveTheme, Colorize, Icon, IconName, Sizable, StyledExt, ThemeStyled,
    VirtualListScrollHandle, h_flex, v_flex,
};
use nostr::prelude::{RelayUrl, ToBech32, Url};
use signed_core::{Announcement, RepoAddr, RepoStatus, filters};
use signed_git::{CommitList, FileCommit};
use signed_state::{
    Backend, CheckoutStatus, CheckoutsStore, GitStore, LocalReposStore, ProfileStore,
    RepoListStore, RepoStore, pr_proposes_checkout,
};
use signed_ui::{CountBadge, DropdownButton, PixelAvatar, UserAvatar, copy_row};

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
use helpers::{
    ShareTargets, TreeItemSeed, build_tree_items, is_markdown_path, ref_selector_trigger,
    tree_items,
};
use issues::{IssuesView, open_new_issue_dialog};
use pull_requests::PullRequestsView;
use send_patch::open_send_patch_panel;

use crate::views::repo_detail::new_pull_request::open_new_pull_panel;

/// What kind of ref the header selectors switch to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RefKind {
    /// A local branch `refs/heads/*`, HEAD stays attached.
    Branch,
    /// A tag `refs/tags/*`, HEAD becomes detached.
    Tag,
}

/// Header actions dispatched by the dropdown menus of the header buttons.
/// `pub(super)` because the pull-request list panel shares this action set.
/// It offers the New-PR and Send-patch actions in its own dropdown.
#[derive(Clone, Action, PartialEq, Eq)]
#[action(namespace = repo_detail, no_json)]
pub(super) enum RepoAction {
    /// Open the new issue dialog.
    NewIssue,
    /// Open the new pull request dialog.
    NewPR,
    /// Open the send patch panel.
    SendPatch,
    /// Open the about dialog.
    About,
    /// Re-push the repository to its grasp servers.
    Push,
    /// Delete the repository from nostr, owner only.
    Delete,
}

/// Everything loaded from the local clone for the explorer.
/// The tree seeds, README, refs and HEAD commit.
/// Computed on a background thread, see [`load_repo_data`].
/// Applied on the main thread.
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

/// Detail view of a repository, header, stats and metadata.
/// A file explorer with README preview, cloned from the announcement's `clone` URLs.
pub struct RepoDetailView {
    focus_handle: FocusHandle,
    /// Dock area the detail view lives in.
    /// New panels, commit diffs, are added there.
    dock_area: WeakEntity<DockArea>,
    /// Snapshot taken at open time.
    /// Shown until the store's first refresh completes.
    /// Also a fallback while the store has no announcement.
    /// `None` for local repositories that haven't been published yet.
    initial: Option<Announcement>,
    /// Per-repository nostr store, holding announcement, issues, PRs and statuses.
    /// `None` until a local repository is initialized to NIP-34.
    store: Option<Entity<RepoStore>>,
    /// Path of the local repository when opened from the scan.
    /// `None` once it is initialized to NIP-34, or for announced repositories.
    local_path: Option<PathBuf>,
    /// File explorer state, the worktree of the local clone.
    tree_state: Entity<TreeState>,
    /// Root of the local clone, for reading files on demand.
    worktree: Option<PathBuf>,
    /// Markdown document currently in the preview pane, README or a file.
    md: Option<MarkdownView>,
    /// Code file currently in the preview pane.
    code: Option<CodeView>,
    readme_name: Option<SharedString>,
    /// Currently previewed file, a relative path, and its contents.
    selected_file: Option<SharedString>,
    files: HashMap<String, FileContent>,
    /// Paths of cached previews, oldest first.
    /// Feeds the eviction caps in [`Self::evict_previews`].
    file_order: VecDeque<String>,
    /// Total text bytes held by [`Self::files`].
    preview_bytes: usize,
    /// Reads in flight, to avoid duplicate loads.
    loading_files: HashSet<String>,
    /// Latest commit touching a previewed file or the README, keyed by path.
    commits: HashMap<String, FileCommit>,
    /// Paths queued for the next batched commit query, see [`Self::load_commits`].
    pending_commits: Vec<String>,
    /// A batched commit query is in flight.
    loading_commits: bool,
    /// Active header tab, 0 = Files tree, 1 = Commits.
    active_tab: usize,
    /// Commits reachable from HEAD, newest first.
    /// `None` until the walk finishes or fails.
    /// [`CommitList`] caps the list, `total` feeds the tab badge.
    all_commits: Option<CommitList>,
    /// Commit walk in flight.
    loading_all_commits: bool,
    /// Virtual list state of the Commits tab.
    scroll_handle: VirtualListScrollHandle,
    item_sizes: Rc<Vec<Size<Pixels>>>,
    /// A clone/fetch is in flight.
    loading: bool,
    error: Option<SharedString>,
    /// Commit HEAD currently points to, shown in the header button.
    head_commit: Option<FileCommit>,
    /// Branch selector in the header, local branches, searchable.
    branch_select: Entity<ComboboxState<SearchableVec<SharedString>>>,
    /// Tag selector in the header, tags, searchable.
    tag_select: Entity<ComboboxState<SearchableVec<SharedString>>>,
    /// A branch/tag switch is in flight, checkout plus explorer reload.
    switching_ref: bool,
    /// Bumped on every branch/tag switch.
    /// In-flight loads with an older generation are discarded when they complete.
    ref_generation: u64,
    /// Subscriptions keeping the selectors' confirm events alive.
    _subscriptions: Vec<Subscription>,
    /// `(path, branch)` ready-suggestions dismissed by the user, per panel.
    banner_dismissed: HashSet<(PathBuf, String)>,
    /// The announced HEAD the ready statuses were last requested with.
    /// Whether they were requested at all.
    /// Re-requested only when the HEAD, the base default, changes.
    /// e.g. when the store's first refresh lands.
    ready_requested: bool,
    ready_head: Option<String>,
    /// The global checkouts store's ready-to-contribute statuses of this
    /// repository, last seen when they drove a render.
    ///
    /// The store notifies on any recompute pass; the observer re-renders this
    /// panel only when these slices changed.
    ready_statuses: Vec<CheckoutStatus>,
    /// The global checkouts store's ready-to-push statuses of this repository,
    /// last seen when they drove a render.
    push_statuses: Vec<CheckoutStatus>,
    /// Upstream repository, from this fork's `u` tag, the user asked to open.
    /// Its announcement is still being fetched.
    pending_upstream: Option<RepoAddr>,
}

impl RepoDetailView {
    /// Open a repository announced.
    ///
    /// The store connects to the announcement's relays and loads issues, PRs and statuses.
    pub fn new(
        dock_area: WeakEntity<DockArea>,
        initial: Announcement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // The announcement we opened from already carries the NIP-34 `relays` tag.
        // The store connects to those relays immediately, no bootstrap fetch wait.
        let addr = initial.addr();
        let relays = initial.relays.clone();
        let store = cx.new(|cx| RepoStore::new(addr, relays, cx));

        let mut view = Self::new_common(
            dock_area,
            Some(initial),
            Some(store.clone()),
            None,
            window,
            cx,
        );
        view.attach_store(&store, cx);
        view
    }

    /// Open a local repository discovered by the scan.
    pub fn new_local(
        dock_area: WeakEntity<DockArea>,
        local_path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_common(dock_area, None, None, Some(local_path), window, cx)
    }

    /// Shared construction.
    ///
    /// File explorer state, ref selectors and the deferred repository load.
    fn new_common(
        dock_area: WeakEntity<DockArea>,
        initial: Option<Announcement>,
        store: Option<Entity<RepoStore>>,
        local_path: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let tree_state = cx.new(|cx| TreeState::new(cx));

        // Empty until the clone completes, then filled with the local refs.
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

        let mut subscriptions = vec![
            cx.subscribe_in(&branch_select, window, |this, _state, event, window, cx| {
                // `Change` fires only when the selection actually changed.
                // Picking the already-selected branch emits nothing.
                // A confirmed value always means a switch.
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

        // The ready-to-contribute and ready-to-push banners are driven by the
        // global checkouts store. It notifies on every recompute; compare the
        // statuses of this repository so unrelated updates (the sidebar badges,
        // other open panels) do not re-render this panel.
        let checkouts = CheckoutsStore::global(cx);
        subscriptions.push(cx.observe(&checkouts, |this, _checkouts, cx| {
            if this.refresh_statuses(cx) {
                cx.notify();
            }
        }));

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
            error: None,
            head_commit: None,
            branch_select,
            tag_select,
            switching_ref: false,
            ref_generation: 0,
            banner_dismissed: HashSet::new(),
            ready_requested: false,
            ready_head: None,
            ready_statuses: Vec::new(),
            push_statuses: Vec::new(),
            pending_upstream: None,
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    /// Load the repository and populate the file explorer.
    ///
    /// A local, not yet published, repository opens straight from disk.
    /// An announced repository's clone, if any, loads first without touching the network.
    fn load_repo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.loading = true;
        self.error = None;
        cx.notify();

        // Local repositories live on disk at their scan path.
        // No clone step or network refresh applies here.
        if let Some(local_path) = self.local_path.clone() {
            let task: gpui::Task<Result<(), Error>> = cx.spawn_in(window, async move |this, cx| {
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

            task.detach();

            return;
        }

        let Some(initial) = self.initial.as_ref() else {
            return;
        };

        let cache = GitStore::global(cx).cache().clone();
        let addr = initial.addr();
        let clone_urls: Vec<Url> = initial.clone.clone();

        // Captured before the loads start.
        // A branch/tag switch bumps the generation, discarding the refresh below.
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

        let task: gpui::Task<Result<(), Error>> = cx.spawn_in(window, async move |this, cx| {
            let disk = disk.await;
            let had_clone = matches!(&disk, Ok(Some(_)));

            // No local clone yet, so clone from the network then load.
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

            // Refresh the clone from the network in the background.
            // When it completes, update the refs and commit list.
            // Loads started before a branch/tag switch are discarded via the generation.
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

                    // Best-effort, a fetch failure, e.g. offline, keeps the cached state.
                    // The state is already shown.
                    signed_git::fetch_all(&repo).ok();

                    let worktree = repo.workdir().map(Path::to_path_buf);
                    // A fetch never moves a mirror's local branches.
                    // A push landing on the grasp servers would never show up.
                    // That covers own repo pushes from a checkout and updates fetched here.
                    // Fast-forward branches from the remote, like `git pull --ff-only`.
                    // Only the checked-out branch's worktree can change on disk.
                    let moved = match &worktree {
                        Some(worktree) => {
                            signed_git::fast_forward_branches(worktree).unwrap_or(false)
                        }
                        None => false,
                    };

                    let (branches, tags) = match &worktree {
                        Some(_) => (
                            signed_git::repo_branches(&repo).unwrap_or_default(),
                            signed_git::repo_tags(&repo).unwrap_or_default(),
                        ),
                        None => (Vec::new(), Vec::new()),
                    };

                    let current_branch = signed_git::current_branch(&repo).unwrap_or(None);
                    let head_commit = signed_git::head_commit(&repo).unwrap_or(None);

                    Ok::<_, Error>(Some((moved, branches, tags, current_branch, head_commit)))
                })
            }
            .await;

            this.update_in(cx, |this, window, cx| {
                if refresh_generation != this.ref_generation {
                    return;
                }

                if let Ok(Some((moved, branches, tags, current_branch, head_commit))) = refresh {
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

                    if moved {
                        this.catch_up_worktree(cx);
                    }

                    cx.notify();
                }
            })?;

            Ok(())
        });

        task.detach();
    }

    /// Apply the loaded repository data.
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

        // Populate the branch/tag selectors with the local refs.
        // Select the branch HEAD points to.
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

    /// Clone the repository into a user-chosen folder outside the cache.
    fn clone_to_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(store) = self.store.clone() else {
            return;
        };

        let name = {
            let Some(announcement) = self.announcement(cx) else {
                return;
            };
            let addr = announcement.addr();
            // Directory name, the display name falling back to the repo id.
            // Both are sanitized to a safe single path component.
            let name = announcement
                .name
                .as_ref()
                .map(|name| name.to_string())
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| addr.identifier.clone());
            let name = signed_git::sanitize_path_component(&name);
            if name.is_empty() {
                "repository".to_owned()
            } else {
                name
            }
        };

        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Clone".into()),
        });

        let task: gpui::Task<Result<(), Error>> = cx.spawn_in(window, async move |this, cx| {
            // `Ok(Ok(Some(paths)))` means the user picked a folder.
            // A cancel or picker failure resolves to anything else.
            let picked = match prompt.await {
                Ok(Ok(Some(mut paths))) => paths.pop(),
                _ => None,
            };
            let Some(folder) = picked else {
                return Ok(());
            };

            let destination = folder.join(&name);
            let destination_for_open = destination.clone();

            // The store owns the clone, its busy flag and error reporting.
            let clone = this.update_in(cx, |_this, _window, cx| {
                store.update(cx, |store, cx| store.clone_to_folder(destination, cx))
            })?;

            // Reveal the new clone in the system file manager on success.
            // Failures already surfaced in the store's error banner.
            if let Ok(()) = clone.await {
                this.update_in(cx, |_this, _window, cx| {
                    cx.open_with_system(&destination_for_open);
                })?;
            }

            Ok(())
        });

        task.detach();
    }

    /// Preview the file at `path`, relative to the worktree root.
    fn open_file(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.selected_file = Some(path.into());

        if self.files.contains_key(path) {
            // The file is cached, but the markdown or code state may hold a different file.
            // Re-point it at this one, the parse runs on a background task either way.
            // Without this, the pane would show a spinner forever.
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

        // Paths come from our own tree walk, but never trust them.
        // Refuse anything that could escape the worktree.
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

        let task: gpui::Task<Result<(), Error>> = cx.spawn_in(window, async move |this, cx| {
            let path_for_read = path.clone();
            let content = cx
                .background_spawn(async move {
                    let full = worktree.join(&path_for_read);
                    // Refuse oversized files before reading them.
                    // Reading a multi-gigabyte file just to classify it is wasteful.
                    // It would burn disk and memory bandwidth.
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
                // The worktree was switched while this file was reading.
                // The result belongs to the previous branch.
                // Clear the in-flight marker either way.
                // Otherwise the path could never be loaded again.
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

        task.detach();
    }

    /// Queue `path` for the per-file commit query.
    /// Requests are batched into one history walk, see [`Self::load_commits`].
    fn load_commit(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.commits.contains_key(path) || self.pending_commits.iter().any(|p| p == path) {
            return;
        }
        self.pending_commits.push(path.to_string());
        if !self.loading_commits {
            self.load_commits(cx);
        }
    }

    /// Walk history once for every queued path on a background task.
    /// Cache the latest commit touching each path in [`Self::commits`].
    /// That feeds the file header in the content column.
    /// Batching shares one walk across paths queued while the previous walk ran.
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

        let task: gpui::Task<Result<(), Error>> = cx.spawn(async move |this, cx| {
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
                // Paths queued while the walk was in flight start the next batch.
                // A stale walk, branch switched mid-flight, must not strand them.
                // This runs under the current generation regardless of the result.
                if !this.pending_commits.is_empty() {
                    this.load_commits(cx);
                }
                cx.notify();
            })?;

            Ok(())
        });

        task.detach();
    }

    /// Walk all commits reachable from HEAD on a background task.
    /// For the Commits tab and its total-count badge.
    /// [`CommitList`] caps the list, only the newest commits are materialized.
    fn load_all_commits(&mut self, cx: &mut Context<Self>) {
        if self.loading_all_commits || self.all_commits.is_some() {
            return;
        }

        let Some(worktree) = self.worktree.clone() else {
            return;
        };

        self.loading_all_commits = true;
        let generation = self.ref_generation;

        let task: gpui::Task<Result<(), Error>> = cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { signed_git::worktree_all_commits(&worktree) })
                .await;

            this.update(cx, |this, cx| {
                // A stale walk, branch switched mid-flight, must not leave the flag set.
                // Otherwise the Commits tab would spin forever.
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

        task.detach();
    }

    /// Open a new panel showing the diff of `commit_id`.
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
            add_center_panel(dock_area, panel_handle(panel), window, cx);
        });
    }

    /// Re-push the repository's refs to its announced grasp servers.
    fn push_repository(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(store) = self.store.clone() else {
            return;
        };

        self.error = None;
        cx.notify();

        store
            .update(cx, |store, cx| store.push_repository(cx))
            .detach();
    }

    /// Push the unpushed commits of the local checkout at `path`.
    fn push_unpushed_checkout(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(store) = self.store.clone() else {
            return;
        };

        if store.read(cx).pushing {
            return;
        }

        self.error = None;
        cx.notify();

        let task: gpui::Task<Result<(), Error>> = cx.spawn_in(window, async move |this, cx| {
            // The store owns the push, its busy flag and error reporting.
            let push = this.update_in(cx, |_this, _window, cx| {
                store.update(cx, |store, cx| store.push_checkout(path.clone(), cx))
            })?;

            // The remote moved, refresh the mirror browsing.
            // Failures already surfaced in the store's error banner.
            if let Ok(()) = push.await {
                this.update_in(cx, |this, window, cx| {
                    this.load_repo(window, cx);
                })?;
            }

            Ok(())
        });

        task.detach();
    }

    /// Delete the repository from nostr, announcement, state and activity.
    fn delete_repository(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(store) = self.store.clone() else {
            return;
        };
        store
            .update(cx, |store, cx| store.delete_repository(cx))
            .detach();
    }

    /// Open the issues list panel in the dock area.
    fn open_issue_detail(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(store) = self.store.clone() else {
            return;
        };
        let Some(dock_area) = self.dock_area.upgrade() else {
            return;
        };

        let panel = cx.new(|cx| IssuesView::new(self.dock_area.clone(), store, window, cx));

        dock_area.update(cx, |dock_area, cx| {
            add_center_panel(dock_area, panel_handle(panel), window, cx);
        });
    }

    /// Open the pull requests list panel in the dock area.
    fn open_pull_request_detail(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(store) = self.store.clone() else {
            return;
        };
        let Some(dock_area) = self.dock_area.upgrade() else {
            return;
        };

        let panel = cx.new(|cx| PullRequestsView::new(self.dock_area.clone(), store, window, cx));

        dock_area.update(cx, |dock_area, cx| {
            add_center_panel(dock_area, panel_handle(panel), window, cx);
        });
    }

    /// Open the upstream repository, the `u` tag of this fork's announcement.
    /// The upstream announcement may not be in the local database yet.
    /// Subscribe for it and open the panel as soon as it lands.
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

        let task: gpui::Task<Result<(), Error>> = cx.spawn_in(window, async move |this, cx| {
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

        task.detach();
    }

    /// Check out `name`, a branch or tag picked in the header.
    /// Refresh the explorer once the switch completes.
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

        // Branches and tags are mutually exclusive states of HEAD.
        // Selecting one clears the other selector.
        // Remember the previous selections to restore them if the checkout fails.
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
        // In-flight loads of the previous branch are discarded when they complete.
        self.ref_generation += 1;
        cx.notify();

        let checkout_name = name.clone();
        let task: gpui::Task<Result<(), Error>> = cx.spawn_in(window, async move |this, cx| {
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

        task.detach();
    }

    /// Restore a selector to `previous`, or clear it after a failed switch.
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

    /// Refresh the file explorer, preview pane and commit list after a successful switch.
    /// The selectors were already updated by [`Self::switch_ref`].
    /// [`Self::switching_ref`] stays set until this reload finishes.
    /// A second switch cannot interleave.
    fn reload_worktree(&mut self, cx: &mut Context<Self>) {
        let Some(worktree) = self.worktree.clone() else {
            return;
        };

        let task: gpui::Task<Result<(), Error>> = cx.spawn(async move |this, cx| {
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
                        // Rebuild the tree from scratch.
                        // Entries of the previous branch are gone.
                        // The expansion state goes with them.
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

        task.detach();
    }

    /// Refresh the file explorer, previews and commit list after the mirror
    /// caught up with the remote.
    ///
    /// The checked-out branch fast-forwarded in place, so unlike
    /// [`Self::reload_worktree`] this keeps the panel's selection and previews:
    /// it rebuilds the tree, drops previews of files the refresh removed and
    /// re-renders the README when it is on screen.
    fn catch_up_worktree(&mut self, cx: &mut Context<Self>) {
        let Some(worktree) = self.worktree.clone() else {
            return;
        };

        let task: gpui::Task<Result<(), Error>> = cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let snapshot = signed_git::worktree_snapshot(&worktree)?;
                    let tree = build_tree_items(&snapshot.entries);
                    Ok::<_, Error>((snapshot, tree))
                })
                .await;

            this.update(cx, |this, cx| {
                match result {
                    Ok((snapshot, tree)) => {
                        let head_changed = snapshot.head_commit.as_ref().map(|c| &c.id)
                            != this.head_commit.as_ref().map(|c| &c.id);

                        this.head_commit = snapshot.head_commit;
                        this.tree_state.update(cx, |state, cx| {
                            state.set_items(tree_items(tree, false), cx);
                        });

                        // Drop previews of files the refresh removed from the  worktree,
                        // everything else stays put.
                        let present: HashSet<String> = snapshot
                            .entries
                            .iter()
                            .map(|path| path.to_string_lossy().into_owned())
                            .collect();

                        let mut previewed: Vec<String> = Vec::new();
                        previewed.extend(this.files.keys().cloned());
                        previewed.extend(this.selected_file.clone().map(|p| p.to_string()));

                        if let Some(path) = this.md.as_ref().and_then(|md| md.path.clone()) {
                            previewed.push(path.to_string());
                        }

                        if let Some(path) = this.code.as_ref().map(|code| code.path.clone()) {
                            previewed.push(path.to_string());
                        }

                        previewed.sort();
                        previewed.dedup();

                        for path in previewed {
                            if !present.contains(&path) {
                                this.drop_preview_of(&path);
                            }
                        }

                        // Re-render the README when it is on screen, i.e. when no file preview is open.
                        if this.selected_file.is_none() {
                            match snapshot.readme_path.zip(snapshot.readme) {
                                Some((path, bytes)) => {
                                    this.readme_name = Some(path.to_string_lossy().into());
                                    if let Ok(text) = String::from_utf8(bytes) {
                                        this.set_markdown(None, &text, cx);
                                    }
                                }
                                None => {
                                    this.md = None;
                                    this.readme_name = None;
                                }
                            }
                        }

                        if head_changed {
                            this.all_commits = None;
                            this.loading_all_commits = false;
                            this.load_all_commits(cx);
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

        task.detach();
    }

    /// Drop the cached preview, editor and commit state of `path`.
    fn drop_preview_of(&mut self, path: &str) {
        if let Some(FileContent::Text(text)) = self.files.remove(path) {
            self.preview_bytes -= text.len();
        }
        self.commits.remove(path);
        if self.selected_file.as_deref() == Some(path) {
            self.selected_file = None;
        }
        if self.md.as_ref().and_then(|md| md.path.as_deref()) == Some(path) {
            self.md = None;
        }
        if self.code.as_ref().map(|code| code.path.as_ref()) == Some(path) {
            self.code = None;
        }
    }

    /// Drop the oldest previews beyond the cache caps.
    /// Keep the currently selected file.
    /// An evicted file's parsed editor state drops with its entry.
    /// Re-opening it re-parses on a background task.
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

    /// The latest announcement from the store or the open-time snapshot.
    /// `None` for local repositories that haven't been published yet.
    fn announcement<'a>(&'a self, cx: &'a App) -> Option<&'a Announcement> {
        let store = self.store.as_ref()?;
        store
            .read(cx)
            .announcement
            .as_ref()
            .or(self.initial.as_ref())
    }

    /// Display name, the announcement's name or ID for announced repositories.
    /// The directory name for local ones.
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
                    .as_deref()
                    .map(SharedString::from)
                    .unwrap_or_else(|| SharedString::from(announcement.id.clone()))
            })
            .unwrap_or_default()
    }

    /// The NIP-34 header, actions and issues/PR counts.
    /// Or the local header with an Init button for an unpublished repository.
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

        // Busy flags are owned by the store; observers re-render on their changes.
        let pushing = store.pushing;
        let cloning = store.cloning;

        let Some(source) = store.announcement.as_ref().or(self.initial.as_ref()) else {
            return div().into_any_element();
        };

        // Derived NIP-34 header data, share targets and clone commands.
        // Rebuilt per frame: two bech32 encodes and a couple of format strings.
        let nip05 = ProfileStore::global(cx)
            .read(cx)
            .get(&source.owner)
            .metadata()
            .nip05
            .clone()
            .filter(|nip05| !nip05.trim().is_empty());

        let announcement = Rc::new(source.clone());
        let share = Rc::new(ShareTargets::from_announcement(&announcement));

        let nostr_url = nostr_clone_url(&announcement, nip05.as_deref());
        let ngit_command = SharedString::from(format!("git clone {nostr_url}"));
        let nak_command = SharedString::from(format!("nak git clone {nostr_url}"));
        let git_commands = Rc::new(announcement.clone_urls());

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
            .p_4()
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
                                    .loading(pushing)
                                    .disabled(pushing)
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
                                            .loading(cloning)
                                            .disabled(cloning)
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

    /// Header for a local, not yet published, repository.
    /// The directory name and path with an Init button instead of the NIP-34 actions.
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

    /// Open the dialog guiding the user through publishing the local repository to NIP-34.
    fn open_init_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(local_path) = self.local_path.clone() else {
            return;
        };
        let view = cx.entity().downgrade();
        init_dialog::open(local_path, view, window, cx);
    }

    /// Switch the repository into its NIP-34 mode after a successful init.
    /// Creates the nostr store for the announced repository.
    /// Drops the local scan identity.
    /// The worktree is unchanged, so the explorer keeps its loaded content.
    pub(crate) fn apply_announcement(
        &mut self,
        announcement: Announcement,
        cx: &mut Context<Self>,
    ) {
        // The repository is no longer a bare local repo.
        // Drop it from the scan results so it leaves the sidebar's local section.
        if let Some(path) = self.local_path.take() {
            LocalReposStore::global(cx).update(cx, |store, cx| store.remove(&path, cx));
        }
        let store =
            cx.new(|cx| RepoStore::new(announcement.addr(), announcement.relays.clone(), cx));
        // Re-render on store refreshes, issues, PRs and statuses.
        // Keep the ready-to-contribute statuses of this repository requested.
        self.attach_store(&store, cx);
        self.store = Some(store);
        self.initial = Some(announcement);
        cx.notify();
    }

    /// Observe the repository's store, re-render on refreshes.
    /// Request the ready-to-contribute statuses for it.
    fn attach_store(&mut self, store: &Entity<RepoStore>, cx: &mut Context<Self>) {
        self._subscriptions
            .push(cx.observe(store, |this, _store, cx| {
                this.refresh_ready_statuses(cx);
                cx.notify();
            }));
        self.refresh_ready_statuses(cx);
    }

    /// Request the statuses of this repository again when the announced HEAD changes.
    /// The HEAD is the base the checkouts are compared against.
    /// Owned repositories are watched for unpushed commits.
    /// Other repositories for ready-to-contribute checkouts.
    fn refresh_ready_statuses(&mut self, cx: &mut Context<Self>) {
        let Some(entity) = self.store.clone() else {
            return;
        };

        let head = entity.read(cx).head.clone();

        if self.ready_requested && self.ready_head == head {
            return;
        }

        self.ready_requested = true;
        self.ready_head = head.clone();

        let addr = entity.read(cx).addr().clone();
        let backend = Backend::global(cx);
        let checkout = CheckoutsStore::global(cx);

        let owned = backend
            .read(cx)
            .current_user()
            .is_some_and(|user| entity.read(cx).is_author(&user));

        checkout.update(cx, |store, cx| {
            // The ready statuses keep the fast poll running while the panel is open.
            // The sidebar's push watch alone polls slower.
            store.request_statuses(&addr, head, cx);

            if owned {
                store.request_push_statuses(&addr, cx);
            }
        });
    }

    /// The ready-to-push statuses of this repository in the global checkouts
    /// store changed since they last drove a render.
    ///
    /// Updates the cached slices. `None` store (a local, not yet published,
    /// repository) has no statuses.
    fn refresh_statuses(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(entity) = self.store.clone() else {
            return false;
        };
        let addr = entity.read(cx).addr().clone();
        let checkouts = CheckoutsStore::global(cx).read(cx);
        let ready_statuses = checkouts.ready_statuses_of(&addr);
        let push_statuses = checkouts.push_statuses_of(&addr);

        let changed = ready_statuses != self.ready_statuses || push_statuses != self.push_statuses;
        self.ready_statuses = ready_statuses;
        self.push_statuses = push_statuses;
        changed
    }

    /// The first checkout ready for a pull request on this repository.
    /// Not covered by an open PR of the signed-in user.
    /// Not dismissed in this panel.
    /// The repository's own checkouts are not suggested here.
    /// Their work is pushed, see [`Self::push_suggestion`].
    fn ready_suggestion(&self, cx: &App) -> Option<CheckoutStatus> {
        let store = self.store.as_ref()?;
        let addr = store.read(cx).addr().clone();
        let user = Backend::global(cx).read(cx).current_user()?;
        if store.read(cx).is_author(&user) {
            return None;
        }

        let statuses = CheckoutsStore::global(cx).read(cx).ready_statuses_of(&addr);

        'status: for status in statuses {
            if self
                .banner_dismissed
                .contains(&(status.path.clone(), status.branch.clone()))
            {
                continue;
            }
            let store = store.read(cx);
            for pr in &store.pull_requests {
                if pr_proposes_checkout(pr, store.status_of(pr) == RepoStatus::Open, user, &status)
                {
                    continue 'status;
                }
            }
            return Some(status);
        }

        None
    }

    /// The first checkout of this owned repository with unpushed commits.
    ///
    /// Not dismissed in this panel.
    fn push_suggestion(&self, cx: &App) -> Option<CheckoutStatus> {
        let entity = self.store.as_ref()?;
        let user = Backend::global(cx).read(cx).current_user()?;

        if !entity.read(cx).is_author(&user) {
            return None;
        }

        let addr = entity.read(cx).addr().clone();
        let statuses = CheckoutsStore::global(cx).read(cx).push_statuses_of(&addr);

        statuses.into_iter().find(|status| {
            !self
                .banner_dismissed
                .contains(&(status.path.clone(), status.branch.clone()))
        })
    }

    /// The ready-to-push banner of an owned repository.
    ///
    /// A local checkout has unpushed commits, with a Push action and a dismiss control.
    fn render_push_banner(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let status = self.push_suggestion(cx)?;
        let key = (status.path.clone(), status.branch.clone());
        let path = status.path.clone();
        // The push busy flag lives on the store; it disables the banner's triggers.
        let pushing = self
            .store
            .as_ref()
            .is_some_and(|store| store.read(cx).pushing);

        let commits = if status.ahead == 1 {
            SharedString::from("1 commit")
        } else {
            SharedString::from(format!("{} commits", status.ahead))
        };

        Some(
            h_flex()
                .p_4()
                .gap_2()
                .w_full()
                .items_center()
                .justify_between()
                .bg(cx.theme().muted)
                .child(
                    h_flex()
                        .gap_2()
                        .text_sm()
                        .text_color(cx.theme().info)
                        .child(
                            h_flex()
                                .px_1()
                                .rounded(cx.theme().radius)
                                .border_1()
                                .border_color(cx.theme().info)
                                .bg(cx.theme().info.mix_oklab(transparent_white(), 0.04))
                                .text_xs()
                                .font_semibold()
                                .font_family(cx.theme().mono_font_family.clone())
                                .child(status.branch),
                        )
                        .child("has")
                        .child(
                            h_flex()
                                .px_1()
                                .rounded(cx.theme().radius)
                                .border_1()
                                .border_color(cx.theme().info)
                                .bg(cx.theme().info.mix_oklab(transparent_white(), 0.04))
                                .text_xs()
                                .font_semibold()
                                .font_family(cx.theme().mono_font_family.clone())
                                .child(commits),
                        )
                        .child("ready to push"),
                )
                .child(
                    h_flex()
                        .gap_1()
                        .child(
                            Button::new("push-checkout-banner")
                                .icon(IconName::ArrowUp)
                                .label("Push")
                                .small()
                                .info()
                                .loading(pushing)
                                .disabled(pushing)
                                .on_click(cx.listener(move |this, _event, window, cx| {
                                    this.push_unpushed_checkout(path.clone(), window, cx);
                                })),
                        )
                        .child(
                            Button::new("close-repo")
                                .icon(IconName::Close)
                                .tooltip("Dismiss")
                                .small()
                                .ghost()
                                .disabled(pushing)
                                .on_click(cx.listener(move |this, _ev, _window, cx| {
                                    this.banner_dismissed.insert(key.clone());
                                    cx.notify();
                                })),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Warning after a push that only some grasp servers accepted.
    fn render_push_warning_banner(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let store = self.store.as_ref()?;
        let store = store.read(cx);
        let warning = store.last_push_warning.clone()?;
        let pushing = store.pushing;

        Some(
            h_flex()
                .p_4()
                .gap_2()
                .w_full()
                .items_start()
                .justify_between()
                .bg(cx.theme().warning.mix_oklab(transparent_white(), 0.08))
                .child(
                    h_flex()
                        .gap_2()
                        .min_w_0()
                        .flex_1()
                        .items_start()
                        .child(Icon::new(IconName::TriangleAlert).small().flex_shrink_0())
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_sm()
                                .text_color(cx.theme().warning)
                                .child(SharedString::from(warning)),
                        ),
                )
                .child(
                    h_flex()
                        .gap_1()
                        .flex_shrink_0()
                        .child(
                            Button::new("republish-after-partial-push")
                                .icon(CustomIconName::Init)
                                .label("Republish")
                                .small()
                                .info()
                                .loading(pushing)
                                .disabled(pushing)
                                .on_click(cx.listener(|this, _event, window, cx| {
                                    this.push_repository(window, cx);
                                })),
                        )
                        .child(
                            Button::new("dismiss-push-warning")
                                .icon(IconName::Close)
                                .tooltip("Dismiss")
                                .small()
                                .ghost()
                                .disabled(pushing)
                                .on_click(cx.listener(|this, _ev, _window, cx| {
                                    if let Some(store) = this.store.clone() {
                                        store.update(cx, |store, _| {
                                            store.last_push_warning = None;
                                        });
                                    }
                                    cx.notify();
                                })),
                        ),
                )
                .into_any_element(),
        )
    }

    /// The ready-to-contribute banner of the repository panel.
    fn render_ready_banner(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let status = self.ready_suggestion(cx)?;
        let key = (status.path.clone(), status.branch.clone());

        let commits = if status.ahead == 1 {
            SharedString::from("1 commit")
        } else {
            SharedString::from(format!("{} commits", status.ahead))
        };

        Some(
            h_flex()
                .p_4()
                .gap_2()
                .w_full()
                .items_center()
                .justify_between()
                .bg(cx.theme().muted)
                .child(
                    h_flex()
                        .gap_2()
                        .text_sm()
                        .text_color(cx.theme().info)
                        .child(
                            h_flex()
                                .px_1()
                                .rounded(cx.theme().radius)
                                .border_1()
                                .border_color(cx.theme().info)
                                .bg(cx.theme().info.mix_oklab(transparent_white(), 0.04))
                                .text_xs()
                                .font_semibold()
                                .font_family(cx.theme().mono_font_family.clone())
                                .child(status.branch),
                        )
                        .child("is")
                        .child(
                            h_flex()
                                .px_1()
                                .rounded(cx.theme().radius)
                                .border_1()
                                .border_color(cx.theme().info)
                                .bg(cx.theme().info.mix_oklab(transparent_white(), 0.04))
                                .text_xs()
                                .font_semibold()
                                .font_family(cx.theme().mono_font_family.clone())
                                .child(commits),
                        )
                        .child("ahead of")
                        .child(
                            h_flex()
                                .px_1()
                                .rounded(cx.theme().radius)
                                .border_1()
                                .border_color(cx.theme().info)
                                .bg(cx.theme().info.mix_oklab(transparent_white(), 0.04))
                                .text_xs()
                                .font_semibold()
                                .font_family(cx.theme().mono_font_family.clone())
                                .child(status.base),
                        ),
                )
                .child(
                    h_flex()
                        .gap_1()
                        .child(
                            Button::new("create-pr-from-banner")
                                .icon(IconName::Plus)
                                .label("Create")
                                .small()
                                .info()
                                .on_click(cx.listener(|this, _event, window, cx| {
                                    if let Some(store) = this.store.clone() {
                                        open_new_pull_panel(
                                            this.dock_area.clone(),
                                            store,
                                            window,
                                            cx,
                                        );
                                    }
                                })),
                        )
                        .child(
                            Button::new("dismiss-ready-banner")
                                .icon(IconName::Close)
                                .tooltip("Dismiss")
                                .small()
                                .ghost()
                                .on_click(cx.listener(move |this, _ev, _window, cx| {
                                    this.banner_dismissed.insert(key.clone());
                                    cx.notify();
                                })),
                        ),
                )
                .into_any_element(),
        )
    }

    /// The tab row shared by both header variants.
    /// Files and Commits tabs, the HEAD commit button and the branch/tag selectors.
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
                        this.child(CountBadge::new(count))
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
                                    ref_selector_trigger(ctx, CustomIconName::GitBranch, cx)
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
                                    ref_selector_trigger(ctx, CustomIconName::Tag, cx)
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

        let banner = self
            .render_ready_banner(cx)
            .or_else(|| self.render_push_banner(cx));

        // View-level load/switch errors, plus the errors of the store-owned
        // operations, republish, checkout push, delete and clone-to-folder.
        let error = self.error.clone().or_else(|| {
            self.store
                .as_ref()
                .and_then(|store| store.read(cx).last_error.clone().map(SharedString::from))
        });

        v_flex()
            .image_cache(gpui::retain_all("repo"))
            .id("repo")
            .size_full()
            .when_some(banner, |this, banner| this.child(banner))
            .when_some(self.render_push_warning_banner(cx), |this, banner| {
                this.child(banner)
            })
            .child(self.render_header(cx))
            .when_some(error, |this, error| {
                this.child(
                    Alert::error("repo-error", error)
                        .banner()
                        .on_close(cx.listener(|this, _event, _window, cx| {
                            this.error = None;
                            if let Some(store) = this.store.clone() {
                                store.update(cx, |store, _| store.last_error = None);
                            }
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

/// Read the worktree state of `repo`, no network.
///
/// Entries, README, refs and HEAD commit.
fn load_repo_data(repo: &Repository) -> Result<RepoData, Error> {
    let entries = signed_git::worktree_entries(repo)?;
    let tree = build_tree_items(&entries);
    let readme_path = signed_git::find_readme(repo)?;
    let readme = match &readme_path {
        Some(path) => signed_git::worktree_read(repo, path)?,
        None => None,
    };
    let worktree = repo.workdir().map(Path::to_path_buf);
    // Ref listing is auxiliary UI.
    // A broken ref must not prevent the explorer from loading.
    // Failures degrade to empty selectors.
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

/// The `nostr://...` clone URL of an announcement, NIP-34.
fn nostr_clone_url(announcement: &Announcement, nip05: Option<&str>) -> SharedString {
    let owner = announcement.owner;
    let user = nip05
        .map(str::to_owned)
        .unwrap_or_else(|| owner.to_bech32().unwrap());

    let mut url = format!("nostr://{user}");
    if let Some(hint) = announcement.relays.first().and_then(RelayUrl::domain) {
        url.push('/');
        url.push_str(hint);
    }
    url.push('/');
    url.push_str(&announcement.id);

    SharedString::from(url)
}

/// The forked-from row of the detail header.
///
/// Clickable link to the upstream repository when the `u` tag references a NIP-34 repo.
/// Plain text when it only carries a git URL.
fn fork_row(announcement: &Announcement, cx: &mut Context<RepoDetailView>) -> Option<AnyElement> {
    let upstream = announcement.upstream.as_ref()?;

    let (label, clickable) = match &upstream.addr {
        Some(addr) => {
            // Prefer the upstream's display name when its announcement is known locally.
            // Fall back to its repository id otherwise.
            let name = RepoListStore::global(cx)
                .read(cx)
                .announcements
                .iter()
                .find(|a| a.addr() == *addr)
                .map(|a| {
                    a.name
                        .as_deref()
                        .map(SharedString::from)
                        .unwrap_or_else(|| SharedString::from(a.id.clone()))
                })
                .unwrap_or_else(|| SharedString::from(addr.identifier.clone()));
            (SharedString::from(format!("Forked from {name}")), true)
        }
        None => (SharedString::from(upstream.display().as_str()), false),
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

/// Open `announcement` as a repository panel in the dock's center.
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
            add_center_panel(dock_area, panel_handle(detail.clone()), window, cx);
        });
    }

    detail
}
