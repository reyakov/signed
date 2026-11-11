use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Error;
use assets::CustomIconName;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, ClipboardItem, Context, Entity, InteractiveElement as _,
    IntoElement, ParentElement as _, PathPromptOptions, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, WeakEntity, Window, div, px,
};
use gpui_native_kit::alert::{Alert, AlertSeverity};
use gpui_native_kit::avatar::Avatar;
use gpui_native_kit::badge::Badge;
use gpui_native_kit::button::{Button, ButtonVariant};
use gpui_native_kit::clipboard::ClipboardButton;
use gpui_native_kit::common::{Sizable as _, h_flex, v_flex};
use gpui_native_kit::dropdown_menu::DropdownMenu;
use gpui_native_kit::icons::{Icon, IconRole};
use gpui_native_kit::layout::dock::{DockState, Panel};
use gpui_native_kit::menu::MenuItem;
use gpui_native_kit::popover::Popover;
use gpui_native_kit::spinner::Spinner;
use gpui_native_kit::theme::ActiveTheme as _;
use gpui_native_kit::tooltip::Tooltip;
use metadata::ProfileDirectory;
use nostr::nips::nip19::Nip19Coordinate;
use nostr::prelude::{PublicKey, RelayUrl, ToBech32, Url};
use signed_core::{Announcement, RepoAddr};
use signed_git::{FileCommit, GitCache, Repo};
use signed_state::{
    Backend, CheckoutStatus, CheckoutSyncStatus, CheckoutsStore, LocalReposStore, Mirrors,
    Nip34Binding, Nip34Kind, PushStepStatus, RepoListStore, RepoStore, SyncStatusDirectory,
};
use utils::middle_truncate;

pub mod actions;
pub mod banners;
pub mod files;
pub mod refs;

use super::tree::{TreeItemSeed, build_tree_items, sorted_worktree_paths};
use crate::dialogs;
use banners::Banners;
use files::RepoFilesView;
use refs::RefSwitcher;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    /// HEAD stays attached.
    Branch,
    /// HEAD becomes detached.
    Tag,
}

pub struct RepoDetailView {
    dock: WeakEntity<DockState>,
    store: Entity<RepoStore>,
    /// A repository opened by address alone starts without an announcement.
    repo_started: bool,
    /// The Files tab, which owns the explorer, previews and the per-file commit map.
    files: Entity<RepoFilesView>,
    /// The checked-out worktree path.
    worktree: Option<PathBuf>,
    loading: bool,
    error: Option<SharedString>,
    head_commit: Option<FileCommit>,
    refs: RefSwitcher,
    banners: Banners,
    tasks: Vec<Task<Result<(), Error>>>,
    /// Profiles rendered by the header and maintainers.
    directory: Entity<ProfileDirectory>,
    /// Sync status feeding the sync-to-Nostr banner.
    sync_directory: Entity<SyncStatusDirectory>,
    share_open: bool,
    repo_menu_open: bool,
    clone_open: bool,
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
        dock: WeakEntity<DockState>,
        addr: RepoAddr,
        hint: Option<Announcement>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let store = cx.new(|cx| RepoStore::new(addr, hint, cx));
        Self::new_common(dock, store, window, cx)
    }

    pub fn new_local(
        dock: WeakEntity<DockState>,
        local_path: PathBuf,
        nip34: Option<Nip34Binding>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let store = cx.new(move |_cx| RepoStore::new_local(local_path, nip34));
        Self::new_common(dock, store, window, cx)
    }

    /// A local repository whose detected binding matches an announcement.
    pub fn new_local_announced(
        dock: WeakEntity<DockState>,
        announcement: Announcement,
        local_path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let addr = announcement.addr();
        let store = cx.new(move |cx| RepoStore::from_worktree(addr, announcement, local_path, cx));
        Self::new_common(dock, store, window, cx)
    }

    fn new_common(
        dock: WeakEntity<DockState>,
        store: Entity<RepoStore>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let checkouts = CheckoutsStore::global(cx);
        let files = cx.new(RepoFilesView::new);
        let weak = cx.entity().downgrade();
        let refs = RefSwitcher::new(weak.clone(), window, cx);

        let subscriptions = vec![cx.observe(&checkouts, |this, _checkouts, cx| {
            if this.refresh_statuses(cx) {
                cx.notify();
            }
        })];

        window.defer(cx, move |window, cx| {
            if let Err(error) = weak.update(cx, |this, cx| this.load_repo(window, cx)) {
                log::warn!("failed to load repository: {error}");
            }
        });

        let mut view = Self {
            dock,
            store: store.clone(),
            repo_started: false,
            files,
            worktree: None,
            loading: true,
            error: None,
            head_commit: None,
            refs,
            tasks: Vec::new(),
            banners: Banners::default(),
            directory: ProfileDirectory::new(cx),
            sync_directory: SyncStatusDirectory::new(cx),
            share_open: false,
            repo_menu_open: false,
            clone_open: false,
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

        self.refresh_ready_statuses(cx);
        cx.notify();
    }

    fn attach_store(
        &mut self,
        store: &Entity<RepoStore>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self._subscriptions
            .push(cx.observe_in(store, window, |this, store, window, cx| {
                this.refresh_ready_statuses(cx);
                this.track_profiles(cx);
                this.track_sync_status(cx);
                if !this.repo_started && store.read(cx).announcement.is_some() {
                    this.load_repo(window, cx);
                }
                cx.notify();
            }));
        self.refresh_ready_statuses(cx);
    }

    fn track_profiles(&mut self, cx: &mut Context<Self>) {
        let store = self.store.read(cx);

        let mut keys: Vec<PublicKey> = store
            .announcement
            .as_ref()
            .map(|announcement| {
                let mut keys = vec![announcement.owner];
                keys.extend(
                    announcement
                        .maintainers
                        .iter()
                        .copied()
                        .filter(|key| *key != announcement.owner),
                );
                keys
            })
            .unwrap_or_default();

        if let Some(owner) = store.nip34.as_ref().and_then(|binding| binding.owner) {
            keys.push(owner);
        }

        self.directory.update(cx, |directory, cx| {
            directory.track(keys, cx);
        });
    }

    fn track_sync_status(&mut self, cx: &mut Context<Self>) {
        let Some(addr) = self.store.read(cx).addr().cloned() else {
            return;
        };

        self.sync_directory.update(cx, |directory, cx| {
            directory.track([addr], cx);
        });
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
            store.request_statuses(&addr, head, cx);
            if owned {
                store.request_push_statuses(&addr, cx);
            }
        });
    }

    fn refresh_statuses(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(addr) = self.store.read(cx).addr().cloned() else {
            return false;
        };

        let checkouts = CheckoutsStore::global(cx).read(cx);
        let ready_statuses = checkouts.ready_statuses_of(&addr);
        let push_statuses = checkouts.push_statuses_of(&addr);

        self.banners.set_statuses(ready_statuses, push_statuses)
    }

    fn announcement<'a>(&'a self, cx: &'a App) -> Option<&'a Announcement> {
        self.store.read(cx).announcement.as_ref()
    }

    /// Load the repository and populate the file explorer.
    fn load_repo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.loading = true;
        self.error = None;
        self.track_profiles(cx);
        self.track_sync_status(cx);
        cx.notify();

        let (announcement, local_path) = {
            let store = self.store.read(cx);
            (store.announcement.clone(), store.path.clone())
        };

        if let Some(local_path) = local_path {
            self.repo_started = true;

            let task: Task<Result<(), Error>> = cx.spawn_in(window, async move |this, cx| {
                let data = cx
                    .background_spawn(async move {
                        let repo = Repo::open(&local_path)?;
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

        let store = self.store.downgrade();
        let addr = announcement.addr();
        let clone_urls: Vec<Url> = announcement.clone.clone();

        let disk = {
            let addr = addr.clone();
            cx.background_spawn(async move {
                match Mirrors::open(&addr)? {
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
                        let repo = Mirrors::ensure(&addr, &clone_urls)?;
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

            if !had_clone {
                return Ok(());
            }

            let pushing = store.read_with(cx, |this, _| this.pushing)?;
            if pushing {
                return Ok(());
            }

            let refresh = {
                let addr = addr.clone();

                cx.background_spawn(async move {
                    let Some(repo) = Mirrors::open(&addr)? else {
                        return Ok::<_, Error>(None);
                    };

                    repo.fetch().ok();
                    let moved = repo.fast_forward_branches().unwrap_or(false);
                    Ok::<_, Error>(Some((moved, repo.snapshot()?)))
                })
                .await
            };

            this.update_in(cx, |this, window, cx| {
                if let Ok(Some((moved, snapshot))) = refresh {
                    let branches: Vec<SharedString> =
                        snapshot.branches.iter().map(Into::into).collect();
                    let tags: Vec<SharedString> = snapshot.tags.iter().map(Into::into).collect();
                    let current_branch = snapshot.current_branch.clone();
                    let head_commit = snapshot.head_commit.clone();

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

        let branches: Vec<SharedString> = branches.into_iter().map(Into::into).collect();
        let tags: Vec<SharedString> = tags.into_iter().map(Into::into).collect();

        self.refs
            .set_branches(branches, current_branch.map(Into::into), window, cx);
        self.refs.set_tags(tags, window, cx);
    }

    fn clone_to_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let store = self.store.clone();

        let name = {
            let Some(announcement) = self.announcement(cx) else {
                return;
            };
            let addr = announcement.addr();
            let name = announcement
                .name
                .as_ref()
                .map(|name| name.to_string())
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| addr.identifier().to_owned());
            let name = GitCache::sanitize_path_component(&name);

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

        let task: Task<Result<(), Error>> = cx.spawn_in(window, async move |this, cx| {
            let picked = match prompt.await {
                Ok(Ok(Some(mut paths))) => paths.pop(),
                _ => None,
            };
            let Some(folder) = picked else {
                return Ok(());
            };

            let destination = folder.join(&name);
            let destination_for_open = destination.clone();

            let clone = this.update_in(cx, |_this, _window, cx| {
                store.update(cx, |store, cx| store.clone_to_folder(destination, cx))
            })?;

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
        let previous_branch = self.refs.branch_select.read(cx).selected().cloned();
        let previous_tag = self.refs.tag_select.read(cx).selected().cloned();

        match kind {
            RefKind::Branch => {
                self.refs
                    .tag_select
                    .update(cx, |state, cx| state.set_selected(None, window, cx));
            }
            RefKind::Tag => {
                self.refs
                    .branch_select
                    .update(cx, |state, cx| state.set_selected(None, window, cx));
            }
        }

        self.refs.switching_ref = true;
        cx.notify();

        let checkout_name = name.clone();
        let task: Task<Result<(), Error>> = cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let repo = Repo::open(&worktree);
                    match kind {
                        RefKind::Branch => {
                            repo.and_then(|repo| repo.checkout_branch(&checkout_name))
                        }
                        RefKind::Tag => repo.and_then(|repo| repo.checkout_tag(&checkout_name)),
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

        let task: Task<Result<(), Error>> = cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let snapshot = Repo::open(&worktree)?.snapshot()?;
                    let tree = build_tree_items(&snapshot.entries);
                    let paths = sorted_worktree_paths(&snapshot.entries);
                    Ok::<_, Error>((snapshot, tree, paths))
                })
                .await;

            this.update(cx, |this, cx| {
                this.refs.switching_ref = false;

                match result {
                    Ok((snapshot, tree, paths)) => {
                        this.head_commit = snapshot.head_commit.clone();
                        let readme_path = snapshot.readme_path;
                        let readme = snapshot.readme;
                        this.files.update(cx, |files, cx| {
                            files.clear_previews();
                            files.apply_entries(tree, paths, cx);
                            files.set_readme(readme_path, readme, cx);
                        });
                    }
                    Err(error) => {
                        this.error = Some(error.to_string().into());
                        this.head_commit = None;
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

    /// Refresh the file explorer and previews after the mirror caught up with the remote.
    fn catch_up_worktree(&mut self, cx: &mut Context<Self>) {
        let Some(worktree) = self.worktree.clone() else {
            return;
        };

        let task: Task<Result<(), Error>> = cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let snapshot = Repo::open(&worktree)?.snapshot()?;
                    let tree = build_tree_items(&snapshot.entries);
                    let paths = sorted_worktree_paths(&snapshot.entries);
                    Ok::<_, Error>((snapshot, tree, paths))
                })
                .await;

            this.update(cx, |this, cx| match result {
                Ok((snapshot, tree, paths)) => {
                    this.head_commit = snapshot.head_commit.clone();
                    this.files.update(cx, |files, cx| {
                        files.catch_up(&snapshot, tree, paths, cx);
                    });
                    cx.notify();
                }
                Err(error) => {
                    this.error = Some(error.to_string().into());
                    cx.notify();
                }
            })?;

            Ok(())
        });

        self.tasks.push(task);
    }

    fn push_repository(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.error = None;
        cx.notify();

        let task = self.store.update(cx, |store, cx| store.push_repository(cx));
        self.tasks.push(task);
    }

    fn push_unpushed_checkout(
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

        let task: Task<Result<(), Error>> = cx.spawn_in(window, async move |this, cx| {
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

    fn reset_remote(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(announcement) = self.announcement(cx).cloned() else {
            return;
        };

        let store = self.store.clone();
        let mirror = Mirrors::path(&announcement.addr());
        let urls: Vec<String> = announcement
            .clone
            .iter()
            .map(|url| url.to_string())
            .collect();

        let probe = cx.background_spawn(async move {
            let mut deleted: Vec<String> = Vec::new();
            let repo = Repo::open(&mirror)?;

            let local: HashSet<String> = repo
                .ref_state()?
                .refs
                .into_iter()
                .map(|(name, _)| name)
                .collect();

            for url in &urls {
                let Ok(remote_refs) = repo.ls_remote(url) else {
                    continue;
                };
                for (name, _) in remote_refs {
                    if !local.contains(&name) && !deleted.contains(&name) {
                        deleted.push(name);
                    }
                }
            }

            deleted.sort();
            Ok::<_, Error>(deleted)
        });

        cx.spawn_in(window, async move |this, cx| {
            let deleted = probe.await;

            this.update_in(cx, |this, _window, cx| match deleted {
                Ok(deleted) => dialogs::reset_remote::open(store, deleted, cx),
                Err(error) => {
                    this.error = Some(error.to_string().into());
                    cx.notify();
                }
            })?;

            Ok::<(), Error>(())
        })
        .detach();
    }

    /// Delete the repository from nostr, announcement, state and activity.
    fn delete_repository(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let task = self
            .store
            .update(cx, |store, cx| store.delete_repository(cx));
        self.tasks.push(task);
    }

    /// Open the upstream repository, the `u` tag of this fork's announcement.
    fn open_upstream(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(addr) = self
            .announcement(cx)
            .and_then(|announcement| announcement.upstream.as_ref())
            .and_then(|upstream| upstream.addr.clone())
        else {
            return;
        };

        actions::open_repo_panel(&self.dock, &addr, None, window, cx);
    }

    /// Open the dialog that publishes the local repository to NIP-34.
    fn open_init_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(local_path) = self.store.read(cx).path.clone() else {
            return;
        };
        dialogs::init_repo::open(local_path, cx.entity().downgrade(), window, cx);
    }

    fn render_header(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if self.store.read(cx).addr().is_none() {
            return self.render_local_header(cx);
        }

        let store = self.store.read(cx);
        let pushing = store.pushing;
        let cloning = store.cloning;

        let Some(announcement) = store.announcement.clone() else {
            return div().into_any_element();
        };

        let nip05 = self
            .directory
            .read(cx)
            .profile(&announcement.owner, cx)
            .metadata()
            .nip05
            .clone()
            .filter(|nip05| !nip05.trim().is_empty());

        let share = ShareTargets::from_announcement(&announcement);
        let nostr_url = nostr_clone_url(&announcement, nip05.as_deref());
        let ngit_command = SharedString::from(format!("git clone {nostr_url}"));
        let nak_command = SharedString::from(format!("nak git clone {nostr_url}"));
        let git_commands: Vec<String> = announcement.clone_urls();

        let name = repo_display_name(self.store.read(cx));
        let description = announcement.description();
        let avatar = Avatar::new(format!("{}:{}", announcement.owner, announcement.id));

        let share_naddr = share.naddr.clone();
        let share_for_menu = share.clone();
        let announcement_for_menu = announcement.clone();
        let weak_share = cx.entity().downgrade();
        let weak_menu_open = cx.entity().downgrade();
        let weak_menu_items = cx.entity().downgrade();
        let weak_clone = cx.entity().downgrade();
        let weak_download = cx.entity().downgrade();
        let ngit_for_popover = ngit_command.clone();
        let nak_for_popover = nak_command.clone();
        let git_for_popover = git_commands.clone();

        v_flex()
            .p_4()
            .w_full()
            .gap_6()
            .border_b_1()
            .border_color(cx.theme().colors.border)
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
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(avatar)
                                    .child(name),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .text_sm()
                                    .text_color(cx.theme().colors.muted_foreground)
                                    .line_clamp(2)
                                    .child(description),
                            )
                            .when_some(fork_row(&announcement, cx), |this, row| this.child(row))
                            .child(
                                h_flex()
                                    .mt_2()
                                    .w_full()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().colors.muted_foreground)
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
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
                                DropdownMenu::new("share")
                                    .icon(Icon::named(IconRole::Copy))
                                    .variant(ButtonVariant::Secondary)
                                    .open(self.share_open)
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            share_naddr.clone(),
                                        ));
                                    })
                                    .on_open_change(move |open, _, cx| {
                                        if let Err(error) = weak_share.update(cx, |this, cx| {
                                            this.share_open = open;
                                            cx.notify();
                                        }) {
                                            log::warn!("failed to update share menu: {error}");
                                        }
                                    })
                                    .items(move |_, _| share_for_menu.menu()),
                            )
                            .child(
                                DropdownMenu::new("repo-menu")
                                    .icon(Icon::named(CustomIconName::EllipsisVertical))
                                    .variant(ButtonVariant::Secondary)
                                    .loading(pushing)
                                    .disabled(pushing)
                                    .open(self.repo_menu_open)
                                    .on_open_change(move |open, _, cx| {
                                        if let Err(error) = weak_menu_open.update(cx, |this, cx| {
                                            this.repo_menu_open = open;
                                            cx.notify();
                                        }) {
                                            log::warn!("failed to update repo menu: {error}");
                                        }
                                    })
                                    .items(move |_, cx| {
                                        let owner = Backend::global(cx)
                                            .read(cx)
                                            .current_user()
                                            == Some(announcement_for_menu.owner);

                                        let mut items = vec![MenuItem::new("About").handler({
                                            let weak = weak_menu_items.clone();
                                            let announcement = announcement_for_menu.clone();
                                            move |_, cx| {
                                                if let Err(error) = weak.update(cx, |this, cx| {
                                                    dialogs::about::open(
                                                        announcement.clone(),
                                                        this.directory.clone(),
                                                        cx,
                                                    );
                                                }) {
                                                    log::warn!("failed to open about: {error}");
                                                }
                                            }
                                        })];

                                        if owner {
                                            items.push(MenuItem::new("Republish").handler({
                                                let weak = weak_menu_items.clone();
                                                move |window, cx| {
                                                    if let Err(error) = weak.update(cx, |this, cx| {
                                                        this.push_repository(window, cx);
                                                    }) {
                                                        log::warn!("failed to republish: {error}");
                                                    }
                                                }
                                            }));
                                            items.push(MenuItem::new("Reset remote to local").handler({
                                                let weak = weak_menu_items.clone();
                                                move |window, cx| {
                                                    if let Err(error) = weak.update(cx, |this, cx| {
                                                        this.reset_remote(window, cx);
                                                    }) {
                                                        log::warn!("failed to reset remote: {error}");
                                                    }
                                                }
                                            }));
                                            items.push(MenuItem::separator());
                                            items.push(
                                                MenuItem::new("Delete")
                                                    .destructive(true)
                                                    .handler({
                                                        let weak = weak_menu_items.clone();
                                                        move |window, cx| {
                                                            if let Err(error) =
                                                                weak.update(cx, |this, cx| {
                                                                    this.delete_repository(window, cx);
                                                                })
                                                            {
                                                                log::warn!(
                                                                    "failed to delete repository: {error}"
                                                                );
                                                            }
                                                        }
                                                    }),
                                            );
                                        }

                                        items
                                    }),
                            )
                            .child(
                                Popover::new("clone")
                                    .open(self.clone_open)
                                    .on_change(move |open, _, cx| {
                                        if let Err(error) = weak_clone.update(cx, |this, cx| {
                                            this.clone_open = open;
                                            cx.notify();
                                        }) {
                                            log::warn!("failed to update clone popover: {error}");
                                        }
                                    })
                                    .trigger(move |_, _| {
                                        Button::new("clone")
                                            .icon(Icon::named(CustomIconName::GitClone))
                                            .tooltip(Tooltip::text("Clone"))
                                            .loading(cloning)
                                            .disabled(cloning)
                                            .into_any_element()
                                    })
                                    .content(move |_, cx| {
                                        v_flex()
                                            .w(px(440.))
                                            .p_3()
                                            .gap_4()
                                            .child(clone_group(
                                                "Clone with ngit",
                                                "copy-ngit",
                                                &ngit_for_popover,
                                            ))
                                            .child(clone_group(
                                                "Clone with nak",
                                                "copy-nak",
                                                &nak_for_popover,
                                            ))
                                            .child(
                                                v_flex()
                                                    .gap_1()
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .text_color(
                                                                cx.theme().colors.muted_foreground,
                                                            )
                                                            .child("Grasp Servers"),
                                                    )
                                                    .when(git_for_popover.is_empty(), |this| {
                                                        this.child(
                                                            div()
                                                                .text_xs()
                                                                .child("No git clone urls."),
                                                        )
                                                    })
                                                    .children(git_for_popover.iter().enumerate().map(
                                                        |(ix, command)| {
                                                            ClipboardButton::new(
                                                                SharedString::from(format!(
                                                                    "copy-git-{ix}"
                                                                )),
                                                                command.clone(),
                                                            )
                                                        },
                                                    )),
                                            )
                                            .child(
                                                h_flex().justify_end().child(
                                                    Button::new("download")
                                                        .icon(Icon::named(CustomIconName::GitClone))
                                                        .label("Download")
                                                        .on_click({
                                                            let weak = weak_download.clone();
                                                            move |_, window, cx| {
                                                                if let Err(error) =
                                                                    weak.update(cx, |this, cx| {
                                                                        this.clone_to_folder(window, cx);
                                                                    })
                                                                {
                                                                    log::warn!(
                                                                        "failed to clone: {error}"
                                                                    );
                                                                }
                                                            }
                                                        }),
                                                ),
                                            )
                                            .into_any_element()
                                    }),
                            ),
                    ),
            )
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
        let avatar = Avatar::new(path.clone());

        let bound = self.store.read(cx).nip34.clone();
        let action = match bound
            .as_ref()
            .filter(|binding| binding.kind == Nip34Kind::Initialized)
        {
            Some(binding) => bound_repo_label(binding, &self.directory, cx),
            None => Button::new("init")
                .icon(Icon::named(CustomIconName::Init))
                .label("Initialize on Nostr")
                .tooltip(Tooltip::text("Publish this repository to Nostr"))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.open_init_dialog(window, cx);
                }))
                .into_any_element(),
        };

        v_flex()
            .px_4()
            .pb_4()
            .w_full()
            .gap_6()
            .border_b_1()
            .border_color(cx.theme().colors.border)
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
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(avatar)
                                    .child(name),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .text_sm()
                                    .text_color(cx.theme().colors.muted_foreground)
                                    .child(path),
                            ),
                    )
                    .child(action),
            )
            .into_any_element()
    }

    fn render_maintainers(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(announcement) = self.announcement(cx) else {
            return div().into_any_element();
        };

        let mut seen = HashSet::new();
        let rest: Vec<_> = announcement
            .maintainers
            .iter()
            .copied()
            .filter(|key| key != &announcement.owner && seen.insert(*key))
            .collect();

        let owner = self.directory.read(cx).profile(&announcement.owner, cx);
        let owner_name = owner.name();

        h_flex()
            .w_full()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Avatar::new(announcement.owner)
                                    .name(owner_name.clone())
                                    .picture(owner.picture())
                                    .small(),
                            )
                            .child(div().text_xs().whitespace_nowrap().child(owner_name)),
                    )
                    .when(!rest.is_empty(), |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().colors.muted_foreground)
                                .child(SharedString::from(format!("+{}", rest.len()))),
                        )
                    }),
            )
            .into_any_element()
    }

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

    fn sync_suggestion(&self, cx: &App) -> Option<CheckoutSyncStatus> {
        let store = self.store.read(cx);
        let addr = store.addr()?;
        let user = Backend::global(cx).read(cx).current_user()?;

        if !store.is_author(&user) {
            return None;
        }

        let statuses = self.sync_directory.read(cx).statuses_of(addr, cx);

        statuses.into_iter().find(|checkout| {
            checkout.status.ahead_total > 0 && !self.banners.sync_dismissal(checkout)
        })
    }

    fn render_push_banner(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let status = self.push_suggestion(cx)?;
        let path = status.path.clone();
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
                .justify_between()
                .bg(cx.theme().colors.muted)
                .child(
                    h_flex()
                        .gap_2()
                        .text_sm()
                        .text_color(cx.theme().colors.muted_foreground)
                        .child(Badge::new("push-branch", status.branch.clone()))
                        .child(commits)
                        .child("ready to push"),
                )
                .child(
                    h_flex()
                        .gap_1()
                        .child(
                            Button::new("push-checkout-banner")
                                .icon(Icon::named(CustomIconName::ArrowUp))
                                .label("Push")
                                .small()
                                .loading(pushing)
                                .disabled(pushing)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.push_unpushed_checkout(path.clone(), window, cx);
                                })),
                        )
                        .child(
                            Button::new("dismiss-push-banner")
                                .icon(Icon::named(IconRole::Close))
                                .variant(ButtonVariant::Ghost)
                                .small()
                                .disabled(pushing)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.banners.dismiss(&status);
                                    cx.notify();
                                })),
                        ),
                )
                .into_any_element(),
        )
    }

    fn render_sync_banner(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let checkout = self.sync_suggestion(cx)?;
        let path = checkout.path.clone();
        let pushing = self.store.read(cx).pushing;

        let (branch, ahead) = checkout
            .status
            .refs
            .iter()
            .filter_map(|(name, sync)| match sync {
                signed_git::RefSync::LocalAhead { ahead } => Some((name, *ahead)),
                signed_git::RefSync::Diverged { ahead, .. } => Some((name, *ahead)),
                _ => None,
            })
            .max_by_key(|(_, ahead)| *ahead)?;

        let branch = SharedString::from(branch.clone());
        let commits = if ahead == 1 {
            SharedString::from("1 commit")
        } else {
            SharedString::from(format!("{} commits", ahead))
        };

        Some(
            h_flex()
                .p_4()
                .gap_2()
                .w_full()
                .justify_between()
                .bg(cx.theme().colors.muted)
                .child(
                    h_flex()
                        .gap_2()
                        .text_sm()
                        .text_color(cx.theme().colors.muted_foreground)
                        .child(Badge::new("sync-branch", branch))
                        .child(commits)
                        .child("ahead of Nostr"),
                )
                .child(
                    h_flex()
                        .gap_1()
                        .child(
                            Button::new("sync-banner")
                                .icon(Icon::named(CustomIconName::Refresh))
                                .label("Sync now")
                                .small()
                                .loading(pushing)
                                .disabled(pushing)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.push_unpushed_checkout(path.clone(), window, cx);
                                })),
                        )
                        .child(
                            Button::new("dismiss-sync-banner")
                                .icon(Icon::named(IconRole::Close))
                                .variant(ButtonVariant::Ghost)
                                .small()
                                .disabled(pushing)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.banners.sync_dismiss(&checkout);
                                    cx.notify();
                                })),
                        ),
                )
                .into_any_element(),
        )
    }

    fn render_push_progress_banner(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let store = self.store.read(cx);
        if !store.pushing || store.push_steps.is_empty() {
            return None;
        }

        let steps = store
            .push_steps
            .iter()
            .map(|status| {
                let label = SharedString::from(status.step().label());

                match status {
                    PushStepStatus::Running(_) => h_flex()
                        .gap_2()
                        .text_sm()
                        .text_color(cx.theme().colors.muted_foreground)
                        .child(Spinner::new("push-step"))
                        .child(label)
                        .into_any_element(),
                    PushStepStatus::Done(_) => h_flex()
                        .gap_2()
                        .text_sm()
                        .child(Icon::named(IconRole::Check).size(px(14.)))
                        .child(label)
                        .into_any_element(),
                    PushStepStatus::Failed(_, reason) => h_flex()
                        .gap_2()
                        .text_sm()
                        .text_color(cx.theme().ramps.destructive.base)
                        .child(Icon::named(IconRole::Close).size(px(14.)))
                        .child(SharedString::from(format!(
                            "{}: {reason}",
                            status.step().label()
                        )))
                        .into_any_element(),
                }
            })
            .collect::<Vec<_>>();

        Some(
            v_flex()
                .p_4()
                .gap_1()
                .w_full()
                .bg(cx.theme().colors.muted)
                .child(div().text_sm().child("Pushing"))
                .children(steps)
                .into_any_element(),
        )
    }

    fn render_push_warning_banner(&self, cx: &Context<Self>) -> Option<AnyElement> {
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
                .bg(cx.theme().colors.muted)
                .child(
                    h_flex()
                        .gap_2()
                        .min_w_0()
                        .flex_1()
                        .items_start()
                        .child(Icon::named(IconRole::Warning).size(px(14.)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_sm()
                                .text_color(cx.theme().ramps.warning.base)
                                .child(SharedString::from(warning)),
                        ),
                )
                .child(
                    h_flex()
                        .gap_1()
                        .child(
                            Button::new("republish-after-partial-push")
                                .icon(Icon::named(CustomIconName::Init))
                                .label("Republish")
                                .small()
                                .loading(pushing)
                                .disabled(pushing)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.push_repository(window, cx);
                                })),
                        )
                        .child(
                            Button::new("dismiss-push-warning")
                                .icon(Icon::named(IconRole::Close))
                                .variant(ButtonVariant::Ghost)
                                .small()
                                .disabled(pushing)
                                .on_click(cx.listener(|this, _, _, cx| {
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

    fn render_files_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.loading {
            let label = if self.store.read(cx).path.is_some() {
                "Loading repository..."
            } else {
                "Cloning repository..."
            };

            return v_flex()
                .flex_1()
                .size_full()
                .items_center()
                .justify_center()
                .gap_2()
                .child(Spinner::new("repo-loading"))
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().colors.muted_foreground)
                        .child(label),
                )
                .into_any_element();
        }

        self.files.clone().into_any_element()
    }
}

impl Panel for RepoDetailView {
    fn title(&self, cx: &App) -> Option<SharedString> {
        Some(repo_display_name(self.store.read(cx)))
    }

    fn lead(&self, cx: &App) -> Option<AnyElement> {
        Some(repo_tab_avatar(self.store.read(cx), &self.directory, cx))
    }
}

impl Render for RepoDetailView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let banner = self
            .render_push_banner(cx)
            .or_else(|| self.render_sync_banner(cx));

        let error = self.error.clone().or_else(|| {
            self.store
                .read(cx)
                .last_error
                .clone()
                .map(SharedString::from)
        });

        let weak = cx.entity().downgrade();

        v_flex()
            .id("repo")
            .size_full()
            .when_some(banner, |this, banner| this.child(banner))
            .when_some(self.render_push_progress_banner(cx), |this, banner| {
                this.child(banner)
            })
            .when_some(self.render_push_warning_banner(cx), |this, banner| {
                this.child(banner)
            })
            .child(self.render_header(cx))
            .when_some(error, |this, error| {
                this.child(
                    Alert::new("repo-error")
                        .severity(AlertSeverity::Danger)
                        .title("Repository error")
                        .message(error)
                        .on_dismiss(move |cx| {
                            if let Err(error) = weak.update(cx, |this, cx| {
                                this.error = None;
                                this.store.update(cx, |store, _| store.last_error = None);
                                cx.notify();
                            }) {
                                log::warn!("failed to dismiss repo error: {error}");
                            }
                        }),
                )
            })
            .child(self.render_files_tab(cx))
    }
}

fn clone_group(label: &str, id: &'static str, command: &SharedString) -> AnyElement {
    v_flex()
        .gap_1()
        .child(div().text_xs().child(SharedString::from(label)))
        .child(ClipboardButton::new(id, command.clone()))
        .into_any_element()
}

fn nostr_clone_url(announcement: &Announcement, nip05: Option<&str>) -> SharedString {
    let owner = announcement.owner;
    let user = nip05
        .map(str::to_owned)
        .unwrap_or_else(|| owner.to_bech32().unwrap_or_else(|_| owner.to_string()));

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
                .unwrap_or_else(|| SharedString::from(addr.identifier().to_owned()));
            (SharedString::from(format!("Forked from {name}")), true)
        }
        None => (SharedString::from(upstream.display().as_str()), false),
    };

    let row = h_flex()
        .gap_1()
        .min_w_0()
        .text_sm()
        .text_color(cx.theme().colors.muted_foreground)
        .child(Icon::named(CustomIconName::GitBranch).size(px(14.)))
        .child(div().whitespace_nowrap().text_ellipsis().child(label));

    Some(if clickable {
        row.id("fork-upstream")
            .cursor_pointer()
            .on_click(cx.listener(|this, _, window, cx| this.open_upstream(window, cx)))
            .into_any_element()
    } else {
        row.into_any_element()
    })
}

#[derive(Clone)]
struct ShareTargets {
    /// NIP-19 `naddr1...` of the announcement, with its announced relays.
    naddr: String,
    /// Hex ID of the announcement event itself.
    event_id: String,
    /// NIP-34 coordinate `30617:<pubkey>:<repo-id>`.
    coordinate: String,
    gitworkshop: String,
    ditto: String,
}

impl ShareTargets {
    fn from_announcement(announcement: &Announcement) -> Self {
        let addr = announcement.addr();
        let coordinate = addr.to_string();
        let naddr = Nip19Coordinate::new(addr.into(), announcement.relays.iter().cloned())
            .to_bech32()
            .unwrap_or_else(|_| coordinate.clone());

        Self {
            naddr: naddr.clone(),
            event_id: announcement
                .event_id
                .to_bech32()
                .unwrap_or_else(|_| announcement.event_id.to_string()),
            coordinate,
            gitworkshop: format!("https://gitworkshop.dev/{naddr}"),
            ditto: format!("https://ditto.pub/{naddr}"),
        }
    }

    fn menu(&self) -> Vec<MenuItem> {
        let items = [
            ("GitWorkshop", truncate_naddr_link(&self.gitworkshop, 4)),
            ("Ditto", truncate_naddr_link(&self.ditto, 4)),
            ("Event ID", middle_truncate(&self.event_id, 10, 10)),
            ("Coordinate", middle_truncate(&self.coordinate, 10, 10)),
        ];

        let values = [
            self.gitworkshop.clone(),
            self.ditto.clone(),
            self.event_id.clone(),
            self.coordinate.clone(),
        ];

        items
            .into_iter()
            .zip(values)
            .map(|((label, detail), value)| {
                MenuItem::new(SharedString::from(format!("{label}  {detail}"))).handler(
                    move |_, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(value.clone()));
                    },
                )
            })
            .collect()
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

fn load_repo_data(repo: &Repo) -> Result<RepoData, Error> {
    let snapshot = repo.snapshot()?;
    let worktree = repo.workdir().map(Path::to_path_buf);
    let tree = build_tree_items(&snapshot.entries);

    Ok(RepoData {
        tree,
        entries: snapshot.entries,
        readme_path: snapshot.readme_path,
        readme: snapshot.readme,
        worktree,
        branches: snapshot.branches,
        tags: snapshot.tags,
        current_branch: snapshot.current_branch,
        head_commit: snapshot.head_commit,
    })
}

/// The announcement's name or ID for announced repositories, the directory name for local ones.
pub fn repo_display_name(store: &RepoStore) -> SharedString {
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

/// The owner and identifier a local repository is already bound to.
fn bound_repo_label(
    binding: &Nip34Binding,
    directory: &Entity<ProfileDirectory>,
    cx: &App,
) -> AnyElement {
    let profile = binding.owner.map(|pk| directory.read(cx).profile(&pk, cx));

    h_flex()
        .flex_shrink_0()
        .gap_2()
        .text_sm()
        .child("Initialized by")
        .text_color(cx.theme().colors.muted_foreground)
        .when_some(profile, |this, profile| {
            let name = profile.name();
            this.child(
                h_flex()
                    .gap_1()
                    .text_color(cx.theme().colors.foreground)
                    .child(
                        Avatar::new(profile.public_key())
                            .name(name.clone())
                            .picture(profile.picture())
                            .small(),
                    )
                    .child(name),
            )
        })
        .when_some(binding.identifier.as_deref(), |this, ident| {
            this.child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(SharedString::from(format!("/{ident}"))),
            )
        })
        .into_any_element()
}

/// The profile avatar shown on the repository tab.
fn repo_tab_avatar(
    store: &RepoStore,
    directory: &Entity<ProfileDirectory>,
    cx: &App,
) -> AnyElement {
    let seed = store
        .announcement
        .as_ref()
        .map(|announcement| format!("{}:{}", announcement.owner, announcement.id))
        .or_else(|| store.addr().map(|addr| addr.to_string()))
        .or_else(|| {
            store
                .path
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned())
        })
        .unwrap_or_default();

    let owner = store
        .announcement
        .as_ref()
        .map(|announcement| directory.read(cx).profile(&announcement.owner, cx));

    let (name, picture) = owner
        .map(|profile| (Some(profile.name()), profile.picture()))
        .unwrap_or((None, None));

    let avatar = match name {
        Some(name) => Avatar::new(seed).name(name),
        None => Avatar::new(seed),
    };

    avatar.picture(picture).small().into_any_element()
}
