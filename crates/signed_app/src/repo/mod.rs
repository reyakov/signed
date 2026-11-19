use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Error;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, PathPromptOptions, Render, SharedString, Styled as _, Subscription, Task,
    WeakEntity, Window, div,
};
use metadata::ProfileDirectory;
use nostr::prelude::{PublicKey, Url};
use signed_core::{Announcement, RepoAddr};
use signed_git::{FileCommit, GitCache, Repo};
use signed_state::{
    Backend, LocalReposStore, Mirrors, Nip34Binding, RepoStore, SyncStatusDirectory,
};
use ui::alert::{Alert, AlertSeverity};
use ui::avatar::Avatar;
use ui::badge::Badge;
use ui::common::{Sizable as _, h_flex, v_flex};
use ui::icons::{Icon, IconRole};
use ui::layout::panel::Panel;
use ui::sidebar::{Sidebar, SidebarEntry};
use ui::spinner::Spinner;
use ui::theme::{ActiveTheme as _, TextRole, Typeset as _};

pub mod actions;
pub mod commit_diff;
pub mod dialogs;
pub mod files;
mod header;
pub mod helpers;
pub mod history;
pub mod refs;

use files::RepoFilesView;
use helpers::{RepoData, load_repo_data};
use history::RepoHistoryView;
use refs::RefSwitcher;

use crate::issues::IssuesView;
use crate::pull_requests::PullRequestsView;
use crate::shared::tree::{build_tree_items, sorted_worktree_paths};
use crate::workspace::Workspace;

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

pub struct RepoDetailView {
    pub(super) workspace: WeakEntity<Workspace>,
    pub(super) store: Entity<RepoStore>,
    about: Option<Entity<dialogs::about::AboutDialog>>,
    init: Option<Entity<dialogs::init::InitRepoDialog>>,
    reset: Option<Entity<dialogs::reset::ResetRemoteDialog>>,
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
    refs: RefSwitcher,
    tasks: Vec<Task<Result<(), Error>>>,
    /// Profiles rendered by the header and maintainers.
    pub(super) directory: Entity<ProfileDirectory>,
    /// Sync status feeding the per-commit sync buttons.
    sync_directory: Entity<SyncStatusDirectory>,
    pub(super) repo_menu_open: bool,
    pub(super) clone_open: bool,
    _subscriptions: Vec<Subscription>,
}

impl RepoDetailView {
    pub fn new(
        workspace: WeakEntity<Workspace>,
        addr: RepoAddr,
        hint: Option<Announcement>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let store = cx.new(|cx| RepoStore::new(addr, hint, cx));
        Self::new_common(workspace, store, window, cx)
    }

    pub fn new_local(
        workspace: WeakEntity<Workspace>,
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
        workspace: WeakEntity<Workspace>,
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
        workspace: WeakEntity<Workspace>,
        store: Entity<RepoStore>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let weak = cx.entity().downgrade();
        let sync_directory = SyncStatusDirectory::new(cx);
        let files = cx.new(RepoFilesView::new);
        let history = cx.new(|_| {
            RepoHistoryView::new(
                store.clone(),
                workspace.clone(),
                weak.clone(),
                sync_directory.clone(),
            )
        });
        let issues = cx.new(|cx| IssuesView::new(workspace.clone(), store.clone(), cx));
        let pulls = cx.new(|cx| PullRequestsView::new(workspace.clone(), store.clone(), cx));
        let refs = RefSwitcher::new(weak.clone(), window, cx);

        let subscriptions = vec![
            cx.observe(&sync_directory, |this, _directory, cx| {
                this.sync_history_source(cx);
            }),
            cx.subscribe(&files, |this, _files, _event, cx| {
                if this.active_section != RepoSection::Files {
                    this.active_section = RepoSection::Files;
                    cx.notify();
                }
            }),
        ];

        let store_for_attach = store.clone();
        window.defer(cx, move |window, cx| {
            crate::shared::log_dropped(weak.update(cx, |this, cx| {
                this.attach_store(&store_for_attach, window, cx);
                this.load_repo(window, cx);
            }));
        });

        Self {
            workspace,
            store: store.clone(),
            about: None,
            init: None,
            reset: None,
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
            refs,
            tasks: Vec::new(),
            directory: ProfileDirectory::new(cx),
            sync_directory,
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
                this.track_profiles(cx);
                this.track_sync_status(cx);
                if !this.repo_started && store.read(cx).announcement.is_some() {
                    this.load_repo(window, cx);
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

    fn sync_history_source(&mut self, cx: &mut Context<Self>) {
        let Some(displayed) = self.worktree.clone() else {
            return;
        };

        let source = self.history_source(&displayed, cx);
        self.history.update(cx, |history, cx| {
            if history.worktree() == Some(source.as_path()) {
                return;
            }
            history.set_worktree(Some(source));
            history.reload(cx);
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
        cx.notify();

        let task: Task<Result<(), Error>> = cx.spawn_in(window, async move |this, cx| {
            let push = this.update_in(cx, |_this, _window, cx| {
                store.update(cx, |store, cx| {
                    store.push_checkout_commit(path.clone(), commit.clone(), cx)
                })
            })?;

            push.await?;

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
                Ok(deleted) => {
                    this.reset = Some(dialogs::reset::ResetRemoteDialog::new(store, deleted, cx));
                    cx.notify();
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
    fn delete_repository(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let task = self
            .store
            .update(cx, |store, cx| store.delete_repository(cx));
        self.tasks.push(task);
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
                        .typeset(TextRole::Body, cx)
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
                crate::shared::log_dropped(weak.update(cx, |this, cx| {
                    this.active_section = section;
                    cx.notify();
                }));
            })
            .entries(move |_, cx| {
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
                    files.read(cx).render_tree(files.downgrade()),
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
}

impl Panel for RepoDetailView {
    fn title(&self, cx: &App) -> SharedString {
        let store = self.store.read(cx);
        if let Some(name) = store.path.as_ref().and_then(|path| path.file_name()) {
            return SharedString::from(name.to_string_lossy().into_owned());
        }
        if let Some(announcement) = store.announcement.as_ref() {
            return announcement
                .name
                .as_deref()
                .map(SharedString::from)
                .unwrap_or_else(|| SharedString::from(announcement.id.clone()));
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
        let weak = cx.entity().downgrade();
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
            .when_some(self.about.clone(), |el, view| el.child(view))
            .when_some(self.init.clone(), |el, view| el.child(view))
            .when_some(self.reset.clone(), |el, view| el.child(view))
    }
}
