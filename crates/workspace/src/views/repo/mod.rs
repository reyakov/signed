use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::Error;
use assets::CustomIconName;
use dock::{BasePanel, DockArea, Panel, PanelEvent, add_center_panel, panel_handle};
use gix::Repository;
use gpui::prelude::*;
use gpui::{
    Action, Anchor, AnyElement, App, ClipboardItem, Context, Entity, EventEmitter, FocusHandle,
    Focusable, PathPromptOptions, Render, SharedString, Subscription, Task, WeakEntity, Window,
    div, px, relative, transparent_white,
};
use gpui_base::{Button as BaseButton, Disableable, Popover};
use gpui_component::alert::Alert;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::combobox::{Combobox, ComboboxEvent};
use gpui_component::menu::{DropdownMenu, PopupMenu};
use gpui_component::spinner::Spinner;
use gpui_component::{
    ActiveTheme, Colorize, Icon, IconName, Sizable, StyledExt, ThemeStyled, h_flex, v_flex,
};
use nostr::nips::nip19::Nip19Coordinate;
use nostr::prelude::{RelayUrl, ToBech32, Url};
use signed_core::{Announcement, RepoAddr, RepoStatus};
use signed_git::FileCommit;
use signed_state::{
    Backend, CheckoutStatus, CheckoutsStore, LocalReposStore, ProfileStore, RepoListStore,
    RepoStore, ensure_repo_mirror, open_repo_mirror, pr_proposes_checkout,
};
use signed_ui::{
    CountBadge, DropdownButton, PixelAvatar, UserAvatar, copy_row, menu_copy_row, middle_truncate,
    ref_selector_trigger,
};

mod about;
mod actions;
mod banners;
mod files;
mod history;
mod init_dialog;
mod refs;

pub(crate) use actions::{RepoItem, open_repo_item, open_repo_panel};

use self::banners::Banners;
use self::files::RepoFilesView;
use self::history::RepoHistoryView;
use self::refs::RefSwitcher;
use crate::views::issues::{IssuesView, open_new_issue_dialog};
use crate::views::pull_requests::PullRequestsView;
use crate::views::pull_requests::new::open_new_pull_panel;
use crate::views::repo::about::open_about_dialog;
use crate::views::send_patch::open_send_patch_panel;
use crate::views::tree::{TreeItemSeed, build_tree_items, sorted_worktree_paths};

#[derive(Clone, Copy, PartialEq, Eq)]
enum RefKind {
    /// HEAD stays attached.
    Branch,
    /// HEAD becomes detached.
    Tag,
}

#[derive(Clone, Action, PartialEq, Eq)]
#[action(namespace = repo, no_json)]
pub(crate) enum RepoAction {
    NewIssue,
    NewPR,
    SendPatch,
    About,
    Push,
    Delete,
}

pub struct RepoDetailView {
    focus_handle: FocusHandle,
    dock_area: WeakEntity<DockArea>,
    store: Entity<RepoStore>,
    /// A repository opened by address alone starts without an announcement.
    repo_started: bool,
    /// The Files tab, which owns the explorer, previews and the per-file commit map.
    files: Entity<RepoFilesView>,
    /// The checked-out worktree path, shared by the Files tab and the commit list.
    worktree: Option<PathBuf>,
    /// The active tab, either 0 (Files tree) or 1 (Commits).
    active_tab: usize,
    history: Entity<RepoHistoryView>,
    loading: bool,
    error: Option<SharedString>,
    head_commit: Option<FileCommit>,
    refs: RefSwitcher,
    banners: Banners,
    tasks: Vec<Task<Result<(), Error>>>,
    _subscriptions: Vec<Subscription>,
}

struct RepoData {
    tree: Vec<TreeItemSeed>,
    entries: Vec<PathBuf>,
    readme_path: Option<PathBuf>,
    readme: Option<Vec<u8>>,
    worktree: Option<PathBuf>,
    branches: Vec<String>,
    tags: Vec<String>,
    current_branch: Option<String>,
    head_commit: Option<FileCommit>,
}

impl RepoDetailView {
    pub fn new(
        dock_area: WeakEntity<DockArea>,
        addr: RepoAddr,
        hint: Option<Announcement>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let store = cx.new(|cx| RepoStore::new(addr, hint, cx));
        Self::new_common(dock_area, store, window, cx)
    }

    pub fn new_local(
        dock_area: WeakEntity<DockArea>,
        local_path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let store = cx.new(move |_cx| RepoStore::new_local(local_path));
        Self::new_common(dock_area, store, window, cx)
    }

    fn new_common(
        dock_area: WeakEntity<DockArea>,
        store: Entity<RepoStore>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let checkouts = CheckoutsStore::global(cx);
        let files = cx.new(RepoFilesView::new);
        let history = cx.new(|_cx| RepoHistoryView::new(store.clone(), dock_area.clone()));
        let refs = RefSwitcher::new(window, cx);

        let mut subscriptions = vec![
            cx.subscribe_in(
                &refs.branch_select,
                window,
                |this, _state, event, window, cx| {
                    if let ComboboxEvent::Change(values) = event
                        && let Some(name) = values.first()
                    {
                        this.switch_ref(RefKind::Branch, name.clone(), window, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &refs.tag_select,
                window,
                |this, _state, event, window, cx| {
                    if let ComboboxEvent::Change(values) = event
                        && let Some(name) = values.first()
                    {
                        this.switch_ref(RefKind::Tag, name.clone(), window, cx);
                    }
                },
            ),
        ];

        // The ready-to-contribute and ready-to-push banners are driven by the global checkouts store.
        subscriptions.push(cx.observe(&checkouts, |this, _checkouts, cx| {
            if this.refresh_statuses(cx) {
                cx.notify();
            }
        }));

        // Defer loading the repository until the window is ready.
        cx.defer_in(window, |this, window, cx| {
            this.load_repo(window, cx);
        });

        let mut view = Self {
            dock_area,
            store: store.clone(),
            repo_started: false,
            files,
            worktree: None,
            active_tab: 0,
            history,
            loading: true,
            error: None,
            head_commit: None,
            refs,
            tasks: Vec::new(),
            banners: Banners::default(),
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        };

        view.attach_store(&store, window, cx);
        view
    }

    pub(crate) fn apply_announcement(
        &mut self,
        announcement: Announcement,
        cx: &mut Context<Self>,
    ) {
        let path = self.store.read(cx).path.clone();

        if let Some(path) = path {
            LocalReposStore::global(cx).update(cx, |store, cx| store.remove(&path, cx));
        }

        self.store
            .update(cx, |store, cx| store.announce(announcement, cx));

        // The new address needs its ready-to-contribute statuses requested.
        self.refresh_ready_statuses(cx);
        cx.notify();
    }

    pub(super) fn attach_store(
        &mut self,
        store: &Entity<RepoStore>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self._subscriptions
            .push(cx.observe_in(store, window, |this, store, window, cx| {
                this.refresh_ready_statuses(cx);
                if !this.repo_started && store.read(cx).announcement.is_some() {
                    this.load_repo(window, cx);
                }
                cx.notify();
            }));
        self.refresh_ready_statuses(cx);
    }

    fn refresh_ready_statuses(&mut self, cx: &mut Context<Self>) {
        let Some(addr) = self.store.read(cx).addr().cloned() else {
            return;
        };

        let head = self.store.read(cx).head.clone();
        let (requested, requested_head) = self.banners.ready_requested_at();

        if requested && requested_head == &head {
            return;
        }

        self.banners.mark_ready_requested(head.clone());

        let backend = Backend::global(cx);
        let checkout = CheckoutsStore::global(cx);

        let owned = backend
            .read(cx)
            .current_user()
            .is_some_and(|user| self.store.read(cx).is_author(&user));

        checkout.update(cx, |store, cx| {
            // The ready statuses keep the fast poll running while the panel is open.
            // The sidebar's push watch alone polls slower.
            store.request_statuses(&addr, head, cx);

            if owned {
                store.request_push_statuses(&addr, cx);
            }
        });
    }

    pub(super) fn refresh_statuses(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(addr) = self.store.read(cx).addr().cloned() else {
            return false;
        };

        let checkouts = CheckoutsStore::global(cx).read(cx);
        let ready_statuses = checkouts.ready_statuses_of(&addr);
        let push_statuses = checkouts.push_statuses_of(&addr);

        self.banners.set_statuses(ready_statuses, push_statuses)
    }

    /// The latest announcement of the repository,
    ///
    /// `None` while local-only or until the store's first pass loads it.
    fn announcement<'a>(&'a self, cx: &'a App) -> Option<&'a Announcement> {
        self.store.read(cx).announcement.as_ref()
    }

    /// Load the repository and populate the file explorer.
    pub(super) fn load_repo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.loading = true;
        self.error = None;
        cx.notify();

        let (addr, announcement, local_path) = {
            let store = self.store.read(cx);
            (
                store.addr().cloned(),
                store.announcement.clone(),
                store.path.clone(),
            )
        };

        // Local repositories live on disk at their scan path.
        // No clone step or network refresh applies here.
        if addr.is_none() {
            self.repo_started = true;

            let Some(local_path) = local_path else {
                return;
            };

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

            self.tasks.push(task);

            return;
        }

        let Some(announcement) = announcement else {
            return;
        };

        self.repo_started = true;

        let addr = announcement.addr();
        let clone_urls: Vec<Url> = announcement.clone.clone();

        let disk = {
            let addr = addr.clone();
            cx.background_spawn(async move {
                match open_repo_mirror(&addr)? {
                    Some(repo) => Ok(Some(load_repo_data(&repo)?)),
                    None => Ok(None),
                }
            })
        };

        let task: Task<Result<(), Error>> = cx.spawn_in(window, async move |this, cx| {
            let disk = disk.await;
            let had_clone = matches!(&disk, Ok(Some(_)));

            let data = match disk {
                Ok(Some(data)) => Ok(data),
                Ok(None) => {
                    let addr = addr.clone();
                    let clone_urls = clone_urls.clone();
                    cx.background_spawn(async move {
                        let repo = ensure_repo_mirror(&addr, &clone_urls)?;
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
            if !had_clone {
                return Ok(());
            }

            let refresh = {
                let addr = addr.clone();

                cx.background_spawn(async move {
                    let Some(repo) = open_repo_mirror(&addr)? else {
                        return Ok::<_, Error>(None);
                    };

                    // Best-effort, a fetch failure, e.g. offline, keeps the cached state.
                    signed_git::fetch_all(&repo).ok();

                    let worktree = repo.workdir().map(Path::to_path_buf);

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
                if let Ok(Some((moved, branches, tags, current_branch, head_commit))) = refresh {
                    let branches: Vec<SharedString> = branches.iter().map(Into::into).collect();
                    let tags: Vec<SharedString> = tags.iter().map(Into::into).collect();

                    let branches_changed = this.refs.set_branches(
                        branches,
                        current_branch.map(Into::into),
                        window,
                        cx,
                    );

                    let tags_changed = this.refs.set_tags(tags, window, cx);
                    let new_head_commit = head_commit.as_ref().map(|c| &c.id);
                    let current_head_commit = this.head_commit.as_ref().map(|c| &c.id);

                    let head_changed = new_head_commit != current_head_commit;
                    this.head_commit = head_commit;

                    if head_changed {
                        this.history.update(cx, |history, cx| history.reload(cx));
                    }

                    if moved {
                        this.catch_up_worktree(cx);
                    }

                    if branches_changed || tags_changed || head_changed {
                        cx.notify();
                    }
                }
            })?;

            Ok(())
        });

        self.tasks.push(task);
    }

    fn apply_repo_data(&mut self, data: RepoData, window: &mut Window, cx: &mut Context<Self>) {
        log::debug!("repo detail: apply_repo_data");
        let RepoData {
            tree,
            entries,
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

        self.worktree = Some(worktree.clone());
        self.head_commit = head_commit;

        self.files.update(cx, |files, cx| {
            files.set_worktree(worktree.clone());
            files.apply_entries(tree, sorted_worktree_paths(&entries), cx);
            files.set_readme(readme_path, readme, cx);
        });

        self.history.update(cx, |history, cx| {
            history.set_worktree(Some(worktree));
            history.reload(cx);
        });

        let branches: Vec<SharedString> = branches.into_iter().map(Into::into).collect();
        let tags: Vec<SharedString> = tags.into_iter().map(Into::into).collect();

        self.refs
            .set_branches(branches, current_branch.map(Into::into), window, cx);

        self.refs.set_tags(tags, window, cx);
    }

    pub(super) fn clone_to_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let store = self.store.clone();

        let name = {
            let Some(announcement) = self.announcement(cx) else {
                return;
            };
            let addr = announcement.addr();
            // Directory name, the display name falling back to the repo id.
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

        self.tasks.push(task);
    }

    fn switch_ref<T>(&mut self, kind: RefKind, name: T, window: &mut Window, cx: &mut Context<Self>)
    where
        T: Into<SharedString>,
    {
        if self.refs.switching_ref {
            return;
        }

        let Some(worktree) = self.worktree.clone() else {
            return;
        };

        let name = name.into();
        let previous_branch = self.refs.branch_select.read(cx).selected_value();
        let previous_tag = self.refs.tag_select.read(cx).selected_value();

        match kind {
            RefKind::Branch => {
                self.refs
                    .tag_select
                    .update(cx, |state, cx| state.clear_selection(cx));
            }
            RefKind::Tag => {
                self.refs
                    .branch_select
                    .update(cx, |state, cx| state.clear_selection(cx));
            }
        }

        self.refs.switching_ref = true;
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
                        this.refs.switching_ref = false;
                        this.refs.restore_selection(
                            &this.refs.branch_select,
                            &previous_branch,
                            window,
                            cx,
                        );
                        this.refs.restore_selection(
                            &this.refs.tag_select,
                            &previous_tag,
                            window,
                            cx,
                        );
                    }
                }
                cx.notify();
            })?;

            Ok(())
        });

        self.tasks.push(task);
    }

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
                    let paths = sorted_worktree_paths(&snapshot.entries);
                    Ok::<_, Error>((snapshot, tree, paths))
                })
                .await;

            this.update(cx, |this, cx| {
                this.refs.switching_ref = false;

                match result {
                    Ok((snapshot, tree, paths)) => {
                        this.head_commit = snapshot.head_commit;
                        let readme_path = snapshot.readme_path;
                        let readme = snapshot.readme;
                        this.files.update(cx, |files, cx| {
                            files.clear_previews();
                            files.apply_entries(tree, paths, cx);
                            files.set_readme(readme_path, readme, cx);
                        });
                        this.history.update(cx, |history, cx| history.reload(cx));
                    }
                    Err(error) => {
                        this.error = Some(error.to_string().into());
                        this.head_commit = None;
                        // The tree may show files that no longer exist.
                        this.files.update(cx, |files, cx| {
                            files.apply_entries(Vec::new(), Vec::new(), cx);
                        });
                    }
                }

                cx.notify();
            })?;

            Ok(())
        });

        self.tasks.push(task);
    }

    /// Refresh the file explorer, previews and commit list after the mirror caught up with the remote.
    pub(super) fn catch_up_worktree(&mut self, cx: &mut Context<Self>) {
        let Some(worktree) = self.worktree.clone() else {
            return;
        };

        let task: gpui::Task<Result<(), Error>> = cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let snapshot = signed_git::worktree_snapshot(&worktree)?;
                    let tree = build_tree_items(&snapshot.entries);
                    let paths = sorted_worktree_paths(&snapshot.entries);
                    Ok::<_, Error>((snapshot, tree, paths))
                })
                .await;

            this.update(cx, |this, cx| {
                match result {
                    Ok((snapshot, tree, paths)) => {
                        let head_changed = snapshot.head_commit.as_ref().map(|c| &c.id)
                            != this.head_commit.as_ref().map(|c| &c.id);

                        let files_changed = this
                            .files
                            .update(cx, |files, cx| files.catch_up(&snapshot, tree, paths, cx));

                        // A fast-forward of a branch other than the checked-out
                        // one leaves the worktree untouched. Rebuilding the tree
                        // and re-parsing the README would flash the panel for
                        // nothing, so it is a no-op.
                        if !head_changed && !files_changed {
                            log::debug!("repo detail catch_up_worktree: no-op");
                            return;
                        }

                        this.head_commit = snapshot.head_commit;

                        if head_changed {
                            this.history.update(cx, |history, cx| history.reload(cx));
                        }

                        cx.notify();
                    }
                    Err(error) => {
                        this.error = Some(error.to_string().into());
                        cx.notify();
                    }
                }
            })?;

            Ok(())
        });

        self.tasks.push(task);
    }

    pub(super) fn push_repository(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.error = None;
        cx.notify();

        let task = self.store.update(cx, |store, cx| store.push_repository(cx));
        self.tasks.push(task);
    }

    pub(super) fn push_unpushed_checkout(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let store = self.store.clone();

        if store.read(cx).pushing {
            return;
        }

        self.error = None;
        cx.notify();

        let task: gpui::Task<Result<(), Error>> = cx.spawn_in(window, async move |this, cx| {
            let push = this.update_in(cx, |_this, _window, cx| {
                store.update(cx, |store, cx| store.push_checkout(path.clone(), cx))
            })?;

            if let Ok(()) = push.await {
                this.update_in(cx, |this, window, cx| {
                    this.load_repo(window, cx);
                })?;
            }

            Ok(())
        });

        self.tasks.push(task);
    }

    /// Delete the repository from nostr, announcement, state and activity.
    pub(super) fn delete_repository(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let task = self
            .store
            .update(cx, |store, cx| store.delete_repository(cx));
        self.tasks.push(task);
    }

    pub(super) fn open_issue_detail(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.store.read(cx).addr().is_none() {
            return;
        }

        let Some(dock_area) = self.dock_area.upgrade() else {
            return;
        };

        let store = self.store.clone();
        let panel = cx.new(|cx| IssuesView::new(self.dock_area.clone(), store, window, cx));

        dock_area.update(cx, |dock_area, cx| {
            add_center_panel(dock_area, panel_handle(panel), window, cx);
        });
    }

    pub(super) fn open_pull_request_detail(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.store.read(cx).addr().is_none() {
            return;
        }

        let Some(dock_area) = self.dock_area.upgrade() else {
            return;
        };

        let store = self.store.clone();
        let panel = cx.new(|cx| PullRequestsView::new(self.dock_area.clone(), store, window, cx));

        dock_area.update(cx, |dock_area, cx| {
            add_center_panel(dock_area, panel_handle(panel), window, cx);
        });
    }

    /// Open the upstream repository, the `u` tag of this fork's announcement.
    ///
    /// The announcement may not be in the local database yet. The panel opens
    /// from the address and fills in when the store loads it; the store's
    /// `subscribe_remote` fetches it from the bootstrap relays.
    pub(super) fn open_upstream(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(addr) = self
            .announcement(cx)
            .and_then(|announcement| announcement.upstream.as_ref())
            .and_then(|upstream| upstream.addr.clone())
        else {
            return;
        };

        open_repo_panel(&self.dock_area, &addr, None, window, &mut *cx);
    }

    /// Open the dialog that publishes the local repository to NIP-34.
    pub(super) fn open_init_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(local_path) = self.store.read(cx).path.clone() else {
            return;
        };
        let view = cx.entity().downgrade();
        init_dialog::open(local_path, view, window, cx);
    }

    pub(super) fn render_header(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if self.store.read(cx).addr().is_none() {
            return self.render_local_header(cx);
        }

        let store = self.store.read(cx);
        let issue_count = SharedString::from(store.issue_count().to_string());
        let pr_count = SharedString::from(store.pull_request_count().to_string());

        // Busy flags are owned by the store; observers re-render on their changes.
        let pushing = store.pushing;
        let cloning = store.cloning;

        let Some(source) = store.announcement.as_ref() else {
            return div().into_any_element();
        };

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

        let name = repo_display_name(self.store.read(cx));
        let description = announcement.description();
        let avatar = PixelAvatar::new(format!("{}:{}", announcement.owner, announcement.id));

        v_flex()
            .on_action(
                cx.listener(|this, action: &RepoAction, window, cx| match action {
                    RepoAction::NewIssue => {
                        open_new_issue_dialog(this.store.clone(), window, cx);
                    }
                    RepoAction::NewPR => {
                        open_new_pull_panel(this.dock_area.clone(), this.store.clone(), window, cx);
                    }
                    RepoAction::SendPatch => {
                        open_send_patch_panel(
                            this.dock_area.clone(),
                            this.store.clone(),
                            window,
                            cx,
                        );
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

    fn render_local_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let name = repo_display_name(self.store.read(cx));
        let path = self
            .store
            .read(cx)
            .path
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

    fn render_header_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let commits_count = self.history.read(cx).commit_count();
        let worktree_empty = self.refs.switching_ref || self.worktree.is_none();

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
                                    this.history.update(cx, |history, cx| {
                                        history.open_commit_diff(&id, window, cx)
                                    });
                                }
                            })),
                    )
                    .child(
                        div().w(px(120.)).child(
                            Combobox::new(&self.refs.branch_select)
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
                            Combobox::new(&self.refs.tag_select)
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

    fn ready_suggestion(&self, cx: &App) -> Option<CheckoutStatus> {
        let store = self.store.read(cx);
        let addr = store.addr()?;
        let user = Backend::global(cx).read(cx).current_user()?;

        if store.is_author(&user) {
            return None;
        }

        let statuses = CheckoutsStore::global(cx).read(cx).ready_statuses_of(addr);

        'status: for status in statuses {
            if self.banners.dismissal(&status) {
                continue;
            }
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
        let store = self.store.read(cx);
        let addr = store.addr()?;
        let user = Backend::global(cx).read(cx).current_user()?;

        if !store.is_author(&user) {
            return None;
        }

        let statuses = CheckoutsStore::global(cx).read(cx).push_statuses_of(addr);

        statuses
            .into_iter()
            .find(|status| !self.banners.dismissal(status))
    }

    pub(super) fn render_push_banner(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let status = self.push_suggestion(cx)?;
        let path = status.path.clone();
        let branch = status.branch.clone();
        // The push busy flag lives on the store; it disables the banner's triggers.
        let pushing = self.store.read(cx).pushing;

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
                                .child(branch),
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
                                    this.banners.dismiss(&status);
                                    cx.notify();
                                })),
                        ),
                )
                .into_any_element(),
        )
    }

    pub(super) fn render_push_warning_banner(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let store = self.store.read(cx);
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
                                    this.store.update(cx, |store, _| {
                                        store.last_push_warning = None;
                                    });
                                    cx.notify();
                                })),
                        ),
                )
                .into_any_element(),
        )
    }

    pub(super) fn render_ready_banner(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let status = self.ready_suggestion(cx)?;
        let branch = status.branch.clone();
        let base = status.base.clone();

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
                                .child(branch),
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
                                .child(base),
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
                                    open_new_pull_panel(
                                        this.dock_area.clone(),
                                        this.store.clone(),
                                        window,
                                        cx,
                                    );
                                })),
                        )
                        .child(
                            Button::new("dismiss-ready-banner")
                                .icon(IconName::Close)
                                .tooltip("Dismiss")
                                .small()
                                .ghost()
                                .on_click(cx.listener(move |this, _ev, _window, cx| {
                                    this.banners.dismiss(&status);
                                    cx.notify();
                                })),
                        ),
                )
                .into_any_element(),
        )
    }

    /// The Files tab body, or the clone/initial-load spinner.
    fn render_files_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.loading {
            return v_flex()
                .flex_1()
                .size_full()
                .items_center()
                .justify_center()
                .gap_2()
                .child(Spinner::new().small())
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("Cloning repository..."),
                )
                .into_any_element();
        }

        self.files.clone().into_any_element()
    }
}

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

struct ShareTargets {
    /// NIP-19 `naddr1...` of the announcement, with its announced relays.
    naddr: String,
    /// Hex ID of the announcement event itself.
    event_id: String,
    /// NIP-34 coordinate `30617:<pubkey>:<repo-id>`.
    coordinate: String,
    /// `https://gitworkshop.dev/<naddr>`
    gitworkshop: String,
    /// `https://ditto.pub/<naddr>`
    ditto: String,
}

impl ShareTargets {
    fn from_announcement(announcement: &Announcement) -> Self {
        let addr = announcement.addr();
        let coordinate = addr.to_string();
        let naddr = Nip19Coordinate::new(addr, announcement.relays.iter().cloned())
            .to_bech32()
            .expect("a complete coordinate always encodes to naddr");

        Self {
            naddr: naddr.clone(),
            event_id: announcement.event_id.to_bech32().unwrap(),
            coordinate,
            gitworkshop: format!("https://gitworkshop.dev/{naddr}"),
            ditto: format!("https://ditto.pub/{naddr}"),
        }
    }

    fn menu(&self, menu: PopupMenu) -> PopupMenu {
        menu.min_w(px(340.))
            .item(menu_copy_row(
                "copy-gitworkshop",
                "GitWorkshop",
                truncate_naddr_link(&self.gitworkshop, 4),
                self.gitworkshop.clone(),
            ))
            .item(menu_copy_row(
                "copy-ditto",
                "Ditto",
                truncate_naddr_link(&self.ditto, 4),
                self.ditto.clone(),
            ))
            .item(menu_copy_row(
                "copy-event-id",
                "Event ID",
                middle_truncate(&self.event_id, 10, 10),
                self.event_id.clone(),
            ))
            .item(menu_copy_row(
                "copy-coordinate",
                "Coordinate",
                middle_truncate(&self.coordinate, 10, 10),
                self.coordinate.clone(),
            ))
    }
}

fn truncate_naddr_link(url: &str, tail: usize) -> String {
    let Some(end) = url.find("naddr1").map(|i| i + "naddr1".len()) else {
        return url.to_string();
    };
    if url.len() - end <= tail + 3 {
        return url.to_string();
    }
    format!("{}...{}", &url[..end], &url[url.len() - tail..])
}

fn load_repo_data(repo: &Repository) -> Result<RepoData, Error> {
    let entries = signed_git::worktree_entries(repo)?;
    let tree = build_tree_items(&entries);
    let readme_path = signed_git::find_readme(repo)?;

    let readme = match &readme_path {
        Some(path) => signed_git::worktree_read(repo, path)?,
        None => None,
    };

    let worktree = repo.workdir().map(Path::to_path_buf);
    let head_commit = signed_git::head_commit(repo).unwrap_or(None);

    let (branches, tags, current_branch) = match &worktree {
        Some(_) => (
            signed_git::repo_branches(repo).unwrap_or_default(),
            signed_git::repo_tags(repo).unwrap_or_default(),
            signed_git::current_branch(repo).unwrap_or(None),
        ),
        None => (Vec::new(), Vec::new(), None),
    };

    Ok(RepoData {
        tree,
        entries,
        readme_path,
        readme,
        worktree,
        branches,
        tags,
        current_branch,
        head_commit,
    })
}

/// The announcement's name or ID for announced repositories, the directory name for local ones.
pub(super) fn repo_display_name(store: &RepoStore) -> SharedString {
    if store.addr().is_none() {
        return store
            .path
            .as_ref()
            .map(|path| {
                SharedString::from(
                    path.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.display().to_string()),
                )
            })
            .unwrap_or_default();
    }

    store
        .announcement
        .as_ref()
        .map(|announcement| {
            announcement
                .name
                .as_deref()
                .map(SharedString::from)
                .unwrap_or_else(|| SharedString::from(announcement.id.clone()))
        })
        .unwrap_or_default()
}

impl BasePanel for RepoDetailView {
    fn panel_name(&self) -> &'static str {
        "repo"
    }
}

impl Panel for RepoDetailView {
    fn title(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        repo_display_name(self.store.read(cx))
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
        let banner = self
            .render_ready_banner(cx)
            .or_else(|| self.render_push_banner(cx));

        let error = self.error.clone().or_else(|| {
            self.store
                .read(cx)
                .last_error
                .clone()
                .map(SharedString::from)
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
                            this.store.update(cx, |store, _| store.last_error = None);
                            cx.notify();
                        })),
                )
            })
            .map(|this| match self.active_tab {
                0 => this.child(self.render_files_tab(cx)),
                _ => this.child(self.history.clone()),
            })
    }
}
