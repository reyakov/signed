use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Error;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, PathPromptOptions, Render, SharedString, StyleRefinement, Styled as _,
    Subscription, SystemNotification, Task, WeakEntity, Window, div, px,
};
use metadata::ProfileDirectory;
use nostr::prelude::{PublicKey, Url};
use signed_core::{Announcement, RepoAddr};
use signed_git::{FileCommit, GitCache, Repo};
use signed_state::{
    Backend, BackendEvent, LocalReposStore, Mirrors, Nip34Binding, Nip34Kind, PushStepStatus,
    RefreshGate, RefreshRequest, RepoStore, SyncStatusDirectory,
};
use ui::alert::{Alert, AlertSeverity};
use ui::avatar::Avatar;
use ui::badge::Badge;
use ui::button::{Button, ButtonVariant};
use ui::clipboard::CopyRow;
use ui::common::{Sizable as _, StyledExt as _, h_flex, v_flex};
use ui::dropdown_menu::DropdownMenu;
use ui::icons::{Icon, IconRole};
use ui::layout::panel::Panel;
use ui::layout::root::RootContext as _;
use ui::menu::MenuItem;
use ui::popover::Popover;
use ui::sidebar::{Sidebar, SidebarEntry};
use ui::spinner::Spinner;
use ui::theme::ActiveTheme as _;
use ui::tooltip::Tooltip;

pub mod actions;
pub mod commit_diff;
pub mod dialogs;
pub mod files;
pub mod helpers;
pub mod history;
pub mod refs;

use files::RepoFilesView;
use helpers::{
    RepoData, announcement_display_name, bound_repo_label, clone_group, fork_row, load_repo_data,
    nostr_clone_url, repo_display_name, snapshot_commit, snapshot_worktree,
};
use history::RepoHistoryView;
use refs::RefSwitcher;
use utils::relative_time_secs;

use crate::app::AppRoot;
use crate::issues::IssuesView;
use crate::pull_requests::PullRequestsView;
use crate::repo::dialogs::DeleteRepositoryDialog;
use crate::repo::dialogs::init::InitRepoDialog;
use crate::repo::helpers::{short_id, sorted_worktree_paths};
use crate::shared::log_dropped;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RepoSection {
    Files,
    Commits,
    Issues,
    PullRequests,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    /// HEAD stays attached.
    Branch,
    /// HEAD becomes detached.
    Tag,
}

/// The announcement and clone commands the repository header renders.
struct AboutModel {
    announcement: Arc<Announcement>,
    git_commands: Arc<Vec<String>>,
}

impl AboutModel {
    /// Rebuilds the model only when the store's announcement changed.
    fn sync(header: &mut Option<Self>, store: &Entity<RepoStore>, cx: &App) {
        let announcement = store.read(cx).announcement.as_ref();
        if header.as_ref().map(|header| header.announcement.as_ref()) == announcement {
            return;
        }
        *header = announcement.map(|announcement| Self {
            announcement: Arc::new(announcement.clone()),
            git_commands: Arc::new(announcement.clone_urls()),
        });
    }
}

pub struct RepoDetailView {
    pub(super) workspace: WeakEntity<AppRoot>,
    pub(super) store: Entity<RepoStore>,
    /// The announcement data the header renders, synced from the store.
    header: Option<AboutModel>,
    /// A repository opened by address alone starts without an announcement.
    repo_started: bool,
    /// The file explorer and preview.
    files: Entity<RepoFilesView>,
    /// The commit history section.
    pub(super) history: Entity<RepoHistoryView>,
    /// The issues list shown in the repository panel.
    issues: Entity<IssuesView>,
    /// The pull request list shown in the repository panel.
    pulls: Entity<PullRequestsView>,
    /// The section selected in the repository sidebar.
    active_section: RepoSection,
    /// The checked-out worktree path.
    worktree: Option<PathBuf>,
    loading: bool,
    error: Option<SharedString>,
    head_commit: Option<FileCommit>,
    /// The commit whose files are shown in place of the worktree.
    browsed_commit: Option<FileCommit>,
    refs: RefSwitcher,
    tasks: Vec<Task<Result<(), Error>>>,
    /// Profiles rendered by the header and maintainers.
    pub(super) directory: Entity<ProfileDirectory>,
    /// Sync status feeding the per-commit sync buttons.
    sync_directory: Entity<SyncStatusDirectory>,
    /// Only a commit sync reports push steps; republish stays silent.
    sync_progress: bool,
    /// Folds mirror refreshes requested while one is running.
    mirror_refresh: RefreshGate,
    pub(super) repo_menu_open: bool,
    pub(super) clone_open: bool,
    _subscriptions: Vec<Subscription>,
}

impl RepoDetailView {
    pub fn new(
        workspace: WeakEntity<AppRoot>,
        addr: RepoAddr,
        hint: Option<Announcement>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let store = cx.new(|cx| RepoStore::new(addr, hint, cx));
        Self::new_common(workspace, store, window, cx)
    }

    pub fn new_local(
        workspace: WeakEntity<AppRoot>,
        local_path: PathBuf,
        nip34: Option<Nip34Binding>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let store = cx.new(move |_cx| RepoStore::new_local(local_path, nip34));
        Self::new_common(workspace, store, window, cx)
    }

    /// A local repository whose detected binding matches an announcement.
    pub fn new_local_announced(
        workspace: WeakEntity<AppRoot>,
        announcement: Announcement,
        local_path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let addr = announcement.addr();
        let store = cx.new(move |cx| RepoStore::from_worktree(addr, announcement, local_path, cx));
        Self::new_common(workspace, store, window, cx)
    }

    fn new_common(
        workspace: WeakEntity<AppRoot>,
        store: Entity<RepoStore>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let backend = Backend::global(cx);
        let weak = cx.entity().downgrade();

        let sync_directory = SyncStatusDirectory::new(cx);
        let files = cx.new(RepoFilesView::new);

        let issues = cx.new(|cx| IssuesView::new(workspace.clone(), store.clone(), cx));
        let pulls = cx.new(|cx| PullRequestsView::new(workspace.clone(), store.clone(), cx));
        let refs = RefSwitcher::new(weak.clone(), window, cx);

        let history = cx.new(|_| {
            RepoHistoryView::new(
                store.clone(),
                workspace.clone(),
                weak.clone(),
                sync_directory.clone(),
            )
        });

        let mut subscriptions = vec![];

        subscriptions.push(
            cx.observe_in(&sync_directory, window, |this, _, window, cx| {
                this.sync_history_source(window, cx);
            }),
        );

        subscriptions.push(cx.subscribe(&backend, |this, _, event, cx| {
            let BackendEvent::PushProgress { addr, status } = event else {
                return;
            };

            if !this.sync_progress {
                return;
            }

            let current = this.store.read(cx).addr().cloned();

            if current.as_ref() != Some(addr) {
                return;
            }

            cx.show_system_notification(Self::sync_progress_notification(addr, status));
        }));

        subscriptions.push(cx.subscribe(&files, |this, _, _, cx| {
            if this.active_section != RepoSection::Files {
                this.active_section = RepoSection::Files;
                cx.notify();
            }
        }));

        let store_for_attach = store.clone();
        let mut header = None;
        AboutModel::sync(&mut header, &store, cx);

        cx.defer_in(window, move |this, window, cx| {
            this.attach_store(&store_for_attach, window, cx);
            this.load_repo(window, cx);
        });

        Self {
            workspace,
            store: store.clone(),
            header,
            repo_started: false,
            files,
            history,
            issues,
            pulls,
            active_section: RepoSection::Files,
            worktree: None,
            loading: true,
            error: None,
            head_commit: None,
            browsed_commit: None,
            refs,
            tasks: Vec::new(),
            directory: ProfileDirectory::new(cx),
            sync_directory,
            sync_progress: false,
            mirror_refresh: RefreshGate::default(),
            repo_menu_open: false,
            clone_open: false,
            _subscriptions: subscriptions,
        }
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
                AboutModel::sync(&mut this.header, &store, cx);
                this.track_profiles(cx);
                this.track_sync_status(cx);
                if !this.repo_started && store.read(cx).announcement.is_some() {
                    this.load_repo(window, cx);
                } else if this.loading
                    && store.read(cx).loaded
                    && store.read(cx).announcement.is_none()
                    && store.read(cx).path.is_none()
                {
                    // The store finished loading with nothing to clone from.
                    this.loading = false;
                    this.error = Some("Repository announcement not found".into());
                }
                cx.notify();
            }));
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

    /// Commits list a local checkout with unpushed commits, so they can be synced one by one.
    fn history_source(&self, displayed: &Path, cx: &App) -> PathBuf {
        let store = self.store.read(cx);

        if store.path.is_some() {
            return displayed.to_path_buf();
        }

        let Some(addr) = store.addr() else {
            return displayed.to_path_buf();
        };
        let owned = Backend::global(cx)
            .read(cx)
            .current_user()
            .is_some_and(|user| store.is_author(&user));

        if !owned {
            return displayed.to_path_buf();
        }

        self.sync_directory
            .read_with(cx, |directory, cx| directory.statuses_of(addr, cx))
            .into_iter()
            .find(|checkout| checkout.status.head.is_some() && checkout.status.ahead_total > 0)
            .map(|checkout| checkout.path)
            .unwrap_or_else(|| displayed.to_path_buf())
    }

    fn sync_history_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(displayed) = self.worktree.clone() else {
            return;
        };

        let reads_mirror = self.history.read(cx).worktree() == Some(displayed.as_path());
        let lists_mirror = self.history_source(&displayed, cx) == displayed;

        // A checkout that just caught up hands the list to a mirror lagging behind the sync.
        if lists_mirror && !reads_mirror {
            self.refresh_mirror(window, cx);
        } else {
            self.reload_history_source(false, cx);
        }
    }

    /// Points the commit list at its source again, reloading a mirror whose HEAD moved.
    fn reload_history_source(&mut self, mirror_head_moved: bool, cx: &mut Context<Self>) {
        let Some(displayed) = self.worktree.clone() else {
            return;
        };

        let source = self.history_source(&displayed, cx);
        self.history.update(cx, |history, cx| {
            if history.worktree() != Some(source.as_path()) {
                history.set_worktree(Some(source));
                history.reload(cx);
            } else if mirror_head_moved && source == displayed {
                history.reload(cx);
            }
        });
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

            self.push_task(task);
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

                // A cached mirror may lag behind the grasp servers since the last visit.
                if had_clone && !this.store.read(cx).pushing {
                    this.refresh_mirror(window, cx);
                }
            })?;

            Ok(())
        });

        self.push_task(task);
    }

    /// Fetches the mirror and fast-forwards its branches, showing the caught-up state.
    fn refresh_mirror(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.mirror_refresh.request() != RefreshRequest::Schedule {
            return;
        }

        let Some(addr) = self.store.read(cx).addr().cloned() else {
            return;
        };

        self.mirror_refresh.begin();

        let task: Task<Result<(), Error>> = cx.spawn_in(window, async move |this, cx| {
            let refresh = cx
                .background_spawn(async move {
                    let Some(repo) = Mirrors::open(&addr)? else {
                        return Ok::<_, Error>(None);
                    };

                    if let Err(error) = repo.fetch() {
                        log::debug!("best-effort fetch failed: {error}");
                    }
                    let moved = match repo.fast_forward_branches() {
                        Ok(moved) => moved,
                        Err(error) => {
                            log::warn!("failed to fast-forward branches: {error}");
                            false
                        }
                    };
                    Ok::<_, Error>(Some((moved, repo.snapshot()?)))
                })
                .await;

            this.update_in(cx, |this, window, cx| {
                let again = this.mirror_refresh.finish();
                let mut head_changed = false;

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
                    head_changed = new_head_commit != current_head_commit;
                    this.head_commit = head_commit;

                    if moved {
                        this.catch_up_worktree(cx);
                    }

                    if branches_changed || tags_changed || head_changed {
                        cx.notify();
                    }
                }

                this.reload_history_source(head_changed, cx);

                if again {
                    this.refresh_mirror(window, cx);
                }
            })?;

            Ok(())
        });

        self.push_task(task);
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
        self.browsed_commit = None;
        self.refs.commit_label = None;

        self.files.update(cx, |files, cx| {
            files.set_worktree(worktree.clone());
            files.pin_commit(None);
            files.apply_entries(tree, sorted_worktree_paths(&entries), cx);
            files.set_readme(readme_path, readme, cx);
        });

        let history_source = self.history_source(&worktree, cx);

        self.history.update(cx, |history, cx| {
            history.set_worktree(Some(history_source));
            history.reload(cx);
        });

        let branches: Vec<SharedString> = branches.into_iter().map(Into::into).collect();
        let tags: Vec<SharedString> = tags.into_iter().map(Into::into).collect();

        self.refs
            .set_branches(branches, current_branch.map(Into::into), window, cx);
        self.refs.set_tags(tags, window, cx);
    }

    /// Shows the files of `commit`, read from the repository at `source`, in place of the worktree.
    fn browse_commit(
        &mut self,
        commit: FileCommit,
        source: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.active_section = RepoSection::Files;
        self.browsed_commit = Some(commit.clone());
        self.refs.show_commit(short_id(&commit.id), window, cx);
        cx.notify();

        let loaded_id = commit.id.clone();
        let shown = source.clone();

        let task: Task<Result<(), Error>> = cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move { snapshot_commit(&source, &loaded_id) })
                .await;

            this.update_in(cx, |this, window, cx| {
                // A later pick replaced this one while it loaded.
                let current = this
                    .browsed_commit
                    .as_ref()
                    .is_some_and(|current| current.id == commit.id);
                if !current {
                    return;
                }

                match result {
                    Ok(tree) => this.files.update(cx, |files, cx| {
                        files.set_worktree(shown);
                        files.clear_previews();
                        files.apply_entries(tree.tree, tree.paths, cx);
                        files.pin_commit(Some(commit));
                        files.set_readme(tree.readme_path, tree.readme, cx);
                    }),
                    Err(error) => {
                        this.error = Some(error.to_string().into());
                        this.reload_worktree(window, cx);
                    }
                }
                cx.notify();
            })?;

            Ok(())
        });

        self.push_task(task);
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

        self.push_task(task);
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
        let browsing = self.browsed_commit.is_some();
        self.refs.commit_label = None;

        // Restored if the checkout fails while HEAD stays detached.
        let previous_tag = match kind {
            RefKind::Branch => self.refs.tag_select.read(cx).selected().cloned(),
            RefKind::Tag => None,
        };

        match kind {
            RefKind::Branch => {
                self.refs
                    .tag_select
                    .update(cx, |state, cx| state.set_selected_silent(None, window, cx));
            }
            RefKind::Tag => {
                self.refs
                    .branch_select
                    .update(cx, |state, cx| state.set_selected_silent(None, window, cx));
            }
        }

        self.refs.switching_ref = true;
        cx.notify();

        let checkout_name = name.clone();
        let task: Task<Result<(), Error>> = cx.spawn_in(window, async move |this, cx| {
            let outcome = cx
                .background_spawn(async move {
                    let repo = Repo::open(&worktree);
                    // A failed checkout leaves HEAD alone; report the branch it is still on.
                    let current = repo.as_ref().ok().and_then(|repo| repo.current_branch());
                    let checkout = match kind {
                        // Browsing a commit leaves HEAD alone, so picking its branch again only
                        // returns to the worktree.
                        RefKind::Branch
                            if browsing && current.as_deref() == Some(checkout_name.as_ref()) =>
                        {
                            Ok(())
                        }
                        RefKind::Branch => {
                            repo.and_then(|repo| repo.checkout_branch(&checkout_name))
                        }
                        RefKind::Tag => repo.and_then(|repo| repo.checkout_tag(&checkout_name)),
                    };
                    match checkout {
                        Ok(()) => Ok(current),
                        Err(error) => Err((error, current)),
                    }
                })
                .await;

            this.update_in(cx, |this, window, cx| {
                match outcome {
                    Ok(_) => this.reload_worktree(window, cx),
                    Err((error, current)) => {
                        this.error = Some(format!("Failed to check out {name}: {error}").into());
                        this.refs.switching_ref = false;

                        let detached = current.is_none();
                        this.refs.restore_selection(
                            &this.refs.branch_select,
                            &current.map(Into::into),
                            window,
                            cx,
                        );

                        let tag = if detached { previous_tag } else { None };
                        this.refs
                            .restore_selection(&this.refs.tag_select, &tag, window, cx);

                        if browsing {
                            this.reload_worktree(window, cx);
                        }
                    }
                }
                cx.notify();
            })?;

            Ok(())
        });

        self.push_task(task);
    }

    /// Reloads the worktree, ending a browsed commit.
    fn reload_worktree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(worktree) = self.worktree.clone() else {
            return;
        };

        let displayed = worktree.clone();

        let task: Task<Result<(), Error>> = cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move { snapshot_worktree(worktree) })
                .await;

            this.update_in(cx, |this, window, cx| {
                this.refs.switching_ref = false;
                this.browsed_commit = None;
                this.refs.commit_label = None;
                this.files.update(cx, |files, _| files.pin_commit(None));

                match result {
                    Ok((snapshot, tree, paths)) => {
                        this.head_commit = snapshot.head_commit.clone();
                        let current_branch = snapshot.current_branch.map(Into::into);
                        let readme_path = snapshot.readme_path;
                        let readme = snapshot.readme;
                        this.files.update(cx, |files, cx| {
                            files.set_worktree(displayed);
                            files.clear_previews();
                            files.apply_entries(tree, paths, cx);
                            files.set_readme(readme_path, readme, cx);
                        });
                        this.refs.restore_selection(
                            &this.refs.branch_select,
                            &current_branch,
                            window,
                            cx,
                        );
                        this.history.update(cx, |history, cx| history.reload(cx));
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

        self.push_task(task);
    }

    /// Refresh the file explorer and previews after the mirror caught up with the remote.
    fn catch_up_worktree(&mut self, cx: &mut Context<Self>) {
        let Some(worktree) = self.worktree.clone() else {
            return;
        };

        let task: Task<Result<(), Error>> = cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { snapshot_worktree(worktree) })
                .await;

            this.update(cx, |this, cx| match result {
                Ok((snapshot, tree, paths)) => {
                    this.head_commit = snapshot.head_commit.clone();
                    // A browsed commit's tree does not follow the worktree.
                    if this.browsed_commit.is_none() {
                        this.files.update(cx, |files, cx| {
                            files.catch_up(&snapshot, tree, paths, cx);
                        });
                    }
                    cx.notify();
                }
                Err(error) => {
                    this.error = Some(error.to_string().into());
                    cx.notify();
                }
            })?;

            Ok(())
        });

        self.push_task(task);
    }

    /// Reaps finished tasks, then queues another.
    fn push_task(&mut self, task: Task<Result<(), Error>>) {
        self.tasks.retain(|task| !task.is_ready());
        self.tasks.push(task);
    }

    fn push_repository(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.error = None;
        cx.notify();

        let task = self.store.update(cx, |store, cx| store.push_repository(cx));
        self.push_task(task);
    }

    fn sync_notification_tag(addr: &RepoAddr) -> SharedString {
        SharedString::from(format!("signed-sync-commit:{addr}"))
    }

    fn sync_progress_notification(addr: &RepoAddr, status: &PushStepStatus) -> SystemNotification {
        let label = status.step().label();

        let (title, body) = match status {
            PushStepStatus::Running(_) => ("Syncing commit", format!("{label}...")),
            PushStepStatus::Done(_) => ("Syncing commit", format!("{label} done")),
            PushStepStatus::Failed(_, reason) => ("Sync warning", format!("{label}: {reason}")),
        };

        SystemNotification {
            tag: Self::sync_notification_tag(addr),
            title: title.into(),
            body: body.into(),
            actions: Vec::new(),
        }
    }

    fn sync_checkout_commit(
        &mut self,
        path: PathBuf,
        commit: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let store = self.store.clone();

        if store.read(cx).pushing {
            return;
        }

        self.error = None;
        self.sync_progress = true;

        let tag = store
            .read(cx)
            .addr()
            .cloned()
            .map(|addr| Self::sync_notification_tag(&addr));

        // The first backend step may lag behind event signing; post feedback immediately.
        if let Some(tag) = &tag {
            cx.show_system_notification(SystemNotification {
                tag: tag.clone(),
                title: "Syncing commit".into(),
                body: "Preparing the push...".into(),
                actions: Vec::new(),
            });
        }
        cx.notify();

        let push = store.update(cx, |store, cx| {
            store.push_checkout_commit(path, commit.clone(), cx)
        });

        let task: Task<Result<(), Error>> = cx.spawn_in(window, async move |this, cx| {
            let result = push.await;

            this.update_in(cx, |this, _window, cx| {
                this.sync_progress = false;

                match result {
                    Ok(()) => {
                        if let Some(tag) = &tag {
                            let short = short_id(&commit);
                            let (title, body) = match store.read(cx).last_push_warning.clone() {
                                Some(warning) => ("Sync partially failed", warning),
                                None => ("Sync complete", format!("Synced up to {short}")),
                            };

                            cx.show_system_notification(SystemNotification {
                                tag: tag.clone(),
                                title: title.into(),
                                body: body.into(),
                                actions: Vec::new(),
                            });
                        }
                    }
                    Err(error) => {
                        if let Some(tag) = &tag {
                            cx.dismiss_system_notification(tag.as_ref()).detach();
                        }
                        store.update(cx, |store, _| store.last_error = None);
                        let dialog = dialogs::SyncErrorDialog::new(format!("{error:#}").into(), cx);
                        cx.open_dialog(dialog);
                    }
                }

                cx.notify();
            })?;

            Ok(())
        });

        self.push_task(task);
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
            // A server that cannot be listed may still hold refs the reset deletes.
            let mut unreachable: Vec<String> = Vec::new();
            let repo = Repo::open(&mirror)?;

            let local: HashSet<String> = repo
                .ref_state()?
                .refs
                .into_iter()
                .map(|(name, _)| name)
                .collect();

            for url in &urls {
                match repo.ls_remote(url) {
                    Ok(remote_refs) => {
                        for (name, _) in remote_refs {
                            if !local.contains(&name) && !deleted.contains(&name) {
                                deleted.push(name);
                            }
                        }
                    }
                    Err(_) => unreachable.push(url.clone()),
                }
            }

            deleted.sort();
            Ok::<_, Error>((deleted, unreachable))
        });

        cx.spawn_in(window, async move |this, cx| {
            let probe = probe.await;

            this.update_in(cx, |this, _window, cx| match probe {
                Ok((deleted, unreachable)) => {
                    let dialog =
                        dialogs::reset::ResetRemoteDialog::new(store, deleted, unreachable, cx);
                    cx.open_dialog(dialog);
                }
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
    fn delete_repository(&mut self, cx: &mut Context<Self>) {
        let task = self
            .store
            .update(cx, |store, cx| store.delete_repository(cx));
        self.push_task(task);
    }

    fn render_files_tab(&self, cx: &App) -> AnyElement {
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

        self.files.read(cx).render_content(cx)
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let files = self.files.clone();
        let commit_count = self.history.read(cx).commit_count();
        let include_issues = self.store.read(cx).addr().is_some();
        let issue_count = include_issues.then(|| self.issues.read(cx).total_count());
        let pull_count = include_issues.then(|| self.pulls.read(cx).total_count());
        let selected = match self.active_section {
            RepoSection::Commits => Some(0),
            RepoSection::Issues if include_issues => Some(1),
            RepoSection::PullRequests if include_issues => Some(2),
            _ => None,
        };
        let weak = cx.entity().downgrade();

        Sidebar::new("repo-sidebar")
            .selected(selected)
            .border_right(true)
            .transparent(false)
            .on_change(move |index, _, cx| {
                let section = match (index, include_issues) {
                    (Some(0), _) => RepoSection::Commits,
                    (Some(1), true) => RepoSection::Issues,
                    (Some(2), true) => RepoSection::PullRequests,
                    _ => return,
                };
                log_dropped(weak.update(cx, |this, cx| {
                    this.active_section = section;
                    cx.notify();
                }));
            })
            .entries(move |_, _| {
                let mut entries = vec![
                    SidebarEntry::row("repo-nav-commits", "Commits")
                        .prefix(Icon::named(IconRole::GitCommit))
                        .when_some(commit_count, |entry, count| {
                            entry.suffix(Badge::new(count, count.to_string()).xsmall())
                        }),
                ];

                if include_issues {
                    entries.push(
                        SidebarEntry::row("repo-nav-issues", "Issues")
                            .prefix(Icon::named(IconRole::GitIssueDone))
                            .when_some(issue_count, |entry, count| {
                                entry.suffix(Badge::new(count, count.to_string()).xsmall())
                            }),
                    );
                    entries.push(
                        SidebarEntry::row("repo-nav-pulls", "Pull requests")
                            .prefix(Icon::named(IconRole::GitPullRequest))
                            .when_some(pull_count, |entry, count| {
                                entry.suffix(Badge::new(count, count.to_string()).xsmall())
                            }),
                    );
                }

                entries.push(SidebarEntry::header("Files"));

                entries.push(SidebarEntry::custom(
                    files
                        .clone()
                        .cached(StyleRefinement::default().flex_1().min_h_0().w_full())
                        .into_any_element(),
                ));

                entries
            })
            .into_any_element()
    }

    fn render_pane(&mut self, cx: &mut Context<Self>) -> AnyElement {
        match self.active_section {
            RepoSection::Files => self.render_files_tab(cx),
            RepoSection::Commits => self.history.clone().into_any_element(),
            RepoSection::Issues => self.issues.clone().into_any_element(),
            RepoSection::PullRequests => self.pulls.clone().into_any_element(),
        }
    }

    pub(super) fn open_upstream(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(addr) = self
            .announcement(cx)
            .and_then(|announcement| announcement.upstream.as_ref())
            .and_then(|upstream| upstream.addr.clone())
        else {
            return;
        };
        actions::open_repo_panel(&self.workspace, &addr, None, window, cx);
    }

    /// Open the dialog that publishes the local repository to NIP-34.
    fn open_init_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(local_path) = self.store.read(cx).path.clone() else {
            return;
        };
        let view = cx.entity().downgrade();
        let dialog = InitRepoDialog::new(local_path, view, window, cx);
        cx.open_dialog(dialog);
    }

    pub(super) fn render_about(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if self.store.read(cx).addr().is_none() {
            return self.render_init_banner(cx);
        }

        let backend = Backend::global(cx);
        let store = self.store.read(cx);
        let pushing = store.pushing;
        let cloning = store.cloning;

        let Some(header) = &self.header else {
            return div().into_any_element();
        };
        let announcement = header.announcement.as_ref();

        let nip05 = self
            .directory
            .read(cx)
            .profile(&announcement.owner, cx)
            .metadata()
            .nip05
            .clone()
            .filter(|nip05| !nip05.trim().is_empty());

        let nostr_url = nostr_clone_url(announcement, nip05.as_deref());
        let ngit_command = SharedString::from(format!("git clone {nostr_url}"));
        let nak_command = SharedString::from(format!("nak git clone {nostr_url}"));

        let name = repo_display_name(self.store.read(cx));
        let description = announcement.description();
        let avatar = Avatar::new(format!("{}:{}", announcement.owner, announcement.id));

        let announcement_for_menu = Arc::clone(&header.announcement);
        let weak_menu_open = cx.entity().downgrade();
        let weak_menu_items = cx.entity().downgrade();
        let weak_clone = cx.entity().downgrade();
        let weak_download = cx.entity().downgrade();
        let ngit_for_popover = ngit_command.clone();
        let nak_for_popover = nak_command.clone();
        let git_for_popover = Arc::clone(&header.git_commands);

        h_flex()
            .p_6()
            .w_full()
            .gap_6()
            .items_start()
            .justify_between()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_2()
                    .child(
                        h_flex()
                            .gap_2()
                            .text_sm()
                            .font_semibold()
                            .child(avatar)
                            .child(
                                div()
                                    .min_w_0()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(name),
                            ),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .text_sm()
                            .text_color(cx.theme().colors.muted_foreground)
                            .line_clamp(3)
                            .child(description),
                    )
                    .when_some(fork_row(announcement, cx), |this, row| this.child(row)),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap_2()
                    .justify_end()
                    .child(
                        Popover::new("code-popover")
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
                                Button::new("code")
                                    .label("Code")
                                    .icon(Icon::named(IconRole::GitClone))
                                    .tooltip(Tooltip::text("Clone repository"))
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
                                        "copy-ngit".into(),
                                        &ngit_for_popover,
                                    ))
                                    .child(clone_group(
                                        "Clone with nak",
                                        "copy-nak".into(),
                                        &nak_for_popover,
                                    ))
                                    .child(
                                        v_flex()
                                            .gap_1()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .font_semibold()
                                                    .child("Grasp Servers"),
                                            )
                                            .when(git_for_popover.is_empty(), |this| {
                                                this.child(
                                                    div()
                                                        .text_xs()
                                                        .text_center()
                                                        .text_color(
                                                            cx.theme().colors.muted_foreground,
                                                        )
                                                        .child("No git clone urls."),
                                                )
                                            })
                                            .children(git_for_popover.iter().enumerate().map(
                                                |(ix, command)| {
                                                    CopyRow::new(
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
                                                .icon(Icon::named(IconRole::GitClone))
                                                .label("Download")
                                                .on_click({
                                                    let weak = weak_download.clone();
                                                    move |_, window, cx| {
                                                        if let Err(error) =
                                                            weak.update(cx, |this, cx| {
                                                                this.clone_to_folder(window, cx);
                                                            })
                                                        {
                                                            log::warn!("failed to clone: {error}");
                                                        }
                                                    }
                                                }),
                                        ),
                                    )
                                    .into_any_element()
                            }),
                    )
                    .child(
                        DropdownMenu::new("repo-menu")
                            .icon(IconRole::EllipsisVertical)
                            .variant(ButtonVariant::Ghost)
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
                                let current_user = backend.read(cx).current_user();
                                let owner = current_user == Some(announcement_for_menu.owner);

                                let mut items = vec![MenuItem::new("About").handler({
                                    let weak = weak_menu_items.clone();
                                    let announcement = Arc::clone(&announcement_for_menu);

                                    move |_, cx| {
                                        if let Err(error) = weak.update(cx, |this, cx| {
                                            let dialog = dialogs::about::AboutDialog::new(
                                                Arc::clone(&announcement),
                                                this.directory.clone(),
                                                cx,
                                            );
                                            cx.open_dialog(dialog);
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
                                    items.push(MenuItem::new("Delete").destructive(true).handler(
                                        {
                                            let weak = weak_menu_items.clone();
                                            move |_window, cx| {
                                                if let Err(error) = weak.update(cx, |_, cx| {
                                                    let view = cx.entity().downgrade();
                                                    let dialog =
                                                        DeleteRepositoryDialog::new(view, cx);
                                                    cx.open_dialog(dialog);
                                                }) {
                                                    log::warn!("failed to ask to delete: {error}");
                                                }
                                            }
                                        },
                                    ));
                                }

                                items
                            }),
                    ),
            )
            .into_any_element()
    }

    fn render_init_banner(&self, cx: &mut Context<Self>) -> AnyElement {
        let binding = self
            .store
            .read(cx)
            .nip34
            .as_ref()
            .filter(|binding| binding.kind == Nip34Kind::Initialized);

        let path = self
            .store
            .read(cx)
            .path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_default();

        v_flex()
            .flex_none()
            .p_6()
            .h_32()
            .w_full()
            .items_center()
            .justify_center()
            .gap_2()
            .map(|this| match binding {
                Some(binding) => this.child(bound_repo_label(binding, &self.directory, cx)),
                None => this.child(
                    Button::new("init")
                        .icon(Icon::named(IconRole::ArrowUp))
                        .label("Initialize on Nostr")
                        .tooltip(Tooltip::text("Publish this repository to Nostr"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_init_dialog(window, cx);
                        })),
                ),
            })
            .child(
                div()
                    .min_w_0()
                    .text_xs()
                    .text_color(cx.theme().colors.muted_foreground)
                    .child(path),
            )
            .into_any_element()
    }

    pub(super) fn render_metadata(&self, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .px_6()
            .h_12()
            .w_full()
            .gap_4()
            .items_center()
            .justify_between()
            .child(self.render_maintainers(cx))
            .child(self.render_ref_controls(cx))
            .into_any_element()
    }

    fn render_ref_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .justify_end()
            .flex_none()
            .gap_2()
            .when_some(self.render_last_commit(cx), |this, commit| {
                this.child(commit)
            })
            .child(self.refs.branch_select.clone())
            .child(self.refs.tag_select.clone())
            .into_any_element()
    }

    fn render_last_commit(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let commit = self.browsed_commit.as_ref().or(self.head_commit.as_ref())?;
        let short = commit.id.get(..8).unwrap_or(&commit.id).to_owned();
        let history = self.history.clone();
        let commit_id = commit.id.clone();
        let tooltip = match self.browsed_commit {
            Some(_) => "View commit",
            None => "View latest commit",
        };

        Some(
            Button::new("last-commit")
                .variant(ButtonVariant::Ghost)
                .fully_rounded(true)
                .tooltip(Tooltip::text(tooltip))
                .on_click(move |_, window, cx| {
                    history.update(cx, |history, cx| {
                        history.open_commit_diff(&commit_id, window, cx);
                    });
                })
                .child(
                    h_flex()
                        .px_2()
                        .w_full()
                        .gap_2()
                        .child(
                            Icon::named(IconRole::GitCommit)
                                .color(cx.theme().colors.muted_foreground),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().colors.muted_foreground)
                                .child(SharedString::from(&short)),
                        )
                        .child(
                            div()
                                .max_w(px(220.))
                                .text_sm()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(SharedString::from(&commit.summary)),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().colors.muted_foreground)
                                .child(SharedString::from(relative_time_secs(commit.time))),
                        ),
                )
                .flex_1()
                .into_any_element(),
        )
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
            .min_w_0()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
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
            })
            .into_any_element()
    }
}

impl Panel for RepoDetailView {
    fn title(&self, cx: &App) -> SharedString {
        let store = self.store.read(cx);
        if let Some(name) = store.path.as_ref().and_then(|path| path.file_name()) {
            return SharedString::from(name.to_string_lossy().into_owned());
        }
        if let Some(announcement) = store.announcement.as_ref() {
            return announcement_display_name(announcement);
        }
        SharedString::from("Repository")
    }

    fn prefix(&self, cx: &App) -> Option<AnyElement> {
        let store = self.store.read(cx);

        let key = match (store.announcement.as_ref(), store.path.as_ref()) {
            (Some(announcement), _) => SharedString::from(format!("repo:{}", announcement.addr())),
            (None, Some(path)) => SharedString::from(format!("repo-local:{}", path.display())),
            (None, None) => SharedString::from("repo"),
        };

        let owner = store
            .nip34
            .as_ref()
            .and_then(|binding| binding.owner)
            .or_else(|| {
                store
                    .announcement
                    .as_ref()
                    .map(|announcement| announcement.owner)
            });

        let mut avatar = Avatar::new(key);
        if let Some(owner) = owner {
            let profile = self.directory.read(cx).profile(&owner, cx);
            avatar = avatar.name(profile.name()).picture(profile.picture());
        }

        Some(avatar.small().into_any_element())
    }

    fn loading(&self, _cx: &App) -> bool {
        self.loading
    }
}

impl Render for RepoDetailView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let error = self.error.clone().or_else(|| {
            self.store
                .read(cx)
                .last_error
                .clone()
                .map(SharedString::from)
        });

        v_flex()
            .id("repo")
            .size_full()
            .child(self.render_about(cx))
            .child(self.render_metadata(cx))
            .when_some(error, |this, error| {
                let weak = cx.entity().downgrade();
                this.child(
                    Alert::new("repo-error")
                        .severity(AlertSeverity::Danger)
                        .title("Repository error")
                        .message(error)
                        .style(ui::alert::AlertStyle::Inline)
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
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    .border_t_1()
                    .border_color(cx.theme().colors.border)
                    .child(self.render_sidebar(cx))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .overflow_hidden()
                            .child(self.render_pane(cx)),
                    ),
            )
    }
}
