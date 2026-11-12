use std::path::{Path, PathBuf};

use gpui::{App, AppContext as _, Context, PathPromptOptions, SharedString, Window};
use nostr::prelude::Url;
use signed_core::Announcement;
use signed_git::{GitCache, Repo};
use signed_state::{Backend, CheckoutsStore, Mirrors, RepoListStore};

use super::NewPullRequestView;
use crate::pull_requests::helpers::ForkCompare;

impl NewPullRequestView {
    pub(super) fn has_source(&self) -> bool {
        self.repo_path.is_some() || self.fork.is_some()
    }

    pub(super) fn work_path(&self) -> Option<PathBuf> {
        match &self.fork {
            Some(fork) => Some(fork.mirror_path.clone()),
            None => self.repo_path.clone(),
        }
    }

    pub(super) fn base_ref(&self) -> String {
        match &self.fork {
            Some(_) => ForkCompare::base_ref(&self.base),
            None => self.base.to_string(),
        }
    }

    pub(super) fn compare_ref(&self) -> String {
        match &self.fork {
            Some(fork) => fork.compare_ref(&self.compare),
            None => self.compare.to_string(),
        }
    }

    pub(crate) fn choose_checkout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose local checkout".into()),
        });
        let task: gpui::Task<Result<(), anyhow::Error>> =
            cx.spawn_in(window, async move |this, cx| {
                let picked = match prompt.await {
                    Ok(Ok(Some(mut paths))) => paths.pop(),
                    _ => None,
                };
                let Some(path) = picked else {
                    return Ok(());
                };
                this.update_in(cx, |this, window, cx| {
                    this.apply_folder_path(path, window, cx)
                })?;
                Ok(())
            });
        self.tasks.push(task);
    }

    pub(crate) fn apply_folder_path(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.fork_generation = self.fork_generation.wrapping_add(1);
        let generation = self.fork_generation;
        let path_string = path.to_string_lossy().to_string();
        let task: gpui::Task<Result<(), anyhow::Error>> =
            cx.spawn_in(window, async move |this, cx| {
                let info = cx
                    .background_spawn({
                        let path = path.clone();
                        async move {
                            let repo = Repo::try_open(&path)?;
                            match repo.branches() {
                                Ok(branches) => Some((branches, repo.current_branch())),
                                Err(error) => {
                                    log::warn!("failed to list branches: {error}");
                                    None
                                }
                            }
                        }
                    })
                    .await;
                this.update_in(cx, |this, window, cx| {
                    if generation != this.fork_generation {
                        return;
                    }
                    this.apply_checkout(path_string, info, window, cx);
                })?;
                Ok(())
            });
        self.tasks.push(task);
    }

    fn apply_checkout(
        &mut self,
        path: String,
        info: Option<(Vec<String>, Option<String>)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.fork = None;
        let Some((branches, current)) = info else {
            self.error = Some("The chosen folder is not a git repository".into());
            self.repo_path = None;
            self.branches.clear();
            self.merge_base = None;
            self.commits = None;
            self.pane.update(cx, |pane, cx| pane.clear(cx));
            cx.notify();
            return;
        };
        if branches.is_empty() {
            self.error = Some("The repository has no branches yet".into());
            self.repo_path = None;
            self.branches.clear();
            self.merge_base = None;
            self.commits = None;
            self.pane.update(cx, |pane, cx| pane.clear(cx));
            cx.notify();
            return;
        }

        let announced = self.store.read(cx).head.clone();
        let base = announced
            .as_ref()
            .filter(|branch| branches.contains(branch))
            .cloned()
            .or_else(|| branches.iter().find(|branch| *branch == "main").cloned())
            .unwrap_or_else(|| branches[0].clone());
        let compare = current
            .filter(|branch| branches.contains(branch))
            .unwrap_or_else(|| base.clone());

        self.repo_path = Some(PathBuf::from(&path));
        self.error = None;
        self.branches = branches.into_iter().map(SharedString::from).collect();

        if let Some(addr) = self.store.read(cx).addr().cloned() {
            CheckoutsStore::global(cx).update(cx, |store, cx| {
                store.record(PathBuf::from(&path), addr, cx);
            });
        }

        self.base = SharedString::from(base);
        self.compare = SharedString::from(compare);
        self.sync_selectors(window, cx);
        self.reload_compare(cx);
    }

    fn sync_selectors(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let options: Vec<(SharedString, SharedString)> = self
            .branches
            .iter()
            .map(|branch| (branch.clone(), branch.clone()))
            .collect();
        let base = self.base.clone();
        let compare = self.compare.clone();

        self.base_select.update(cx, |state, cx| {
            state.set_options(options.clone(), cx);
            state.set_selected(Some(base), window, cx);
        });
        self.compare_select.update(cx, |state, cx| {
            state.set_options(options, cx);
            state.set_selected(Some(compare), window, cx);
        });
    }

    pub(super) fn fork_candidates(&self, cx: &App) -> Vec<Announcement> {
        let Some(addr) = self.store.read(cx).addr().cloned() else {
            return Vec::new();
        };
        let euc = self
            .store
            .read(cx)
            .announcement
            .as_ref()
            .and_then(|a| a.euc.clone());
        let user = Backend::global(cx).read(cx).current_user();
        let announcements = RepoListStore::global(cx).read(cx).announcements.clone();
        Announcement::forks_in(&announcements, &addr, euc.as_deref(), user)
            .into_iter()
            .cloned()
            .collect()
    }

    pub(crate) fn choose_fork(
        &mut self,
        announcement: Announcement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(base) = self.store.read(cx).addr().cloned() else {
            return;
        };
        let mirror_path = Mirrors::path(&base);
        let namespace = GitCache::fork_namespace(&announcement);
        let clone_urls: Vec<Url> = announcement.clone.clone();
        let base_clone_urls: Vec<Url> = self
            .store
            .read(cx)
            .announcement
            .as_ref()
            .map(|a| a.clone.clone())
            .unwrap_or_default();
        self.fork_generation = self.fork_generation.wrapping_add(1);
        let generation = self.fork_generation;
        let handle = window.window_handle();

        self.loading = true;
        self.error = None;
        cx.notify();

        let task: gpui::Task<Result<(), anyhow::Error>> = cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn({
                    let namespace = namespace.clone();
                    let mirror_path = mirror_path.clone();
                    async move {
                        Mirrors::ensure(&base, &base_clone_urls)?;
                        let repo = Repo::open(&mirror_path)?;
                        repo.delete_refs_with_prefix("refs/fork")?;
                        repo.fetch_refs(
                            &clone_urls,
                            &format!("+refs/heads/*:refs/fork/{namespace}/*"),
                        )?;
                        let strip = |refs: Vec<String>, prefix: &str| {
                            let mut names: Vec<String> = refs
                                .into_iter()
                                .filter_map(|name| {
                                    name.strip_prefix(prefix)
                                        .map(|rest| rest.trim_start_matches('/').to_owned())
                                })
                                .filter(|name| !name.is_empty())
                                .collect();
                            names.sort();
                            names
                        };
                        let base_branches = strip(
                            repo.refs_with_prefix("refs/remotes/origin")?,
                            "refs/remotes/origin",
                        );
                        let compare_branches = strip(
                            repo.refs_with_prefix(&format!("refs/fork/{namespace}"))?,
                            &format!("refs/fork/{namespace}"),
                        );
                        Ok::<_, anyhow::Error>((base_branches, compare_branches))
                    }
                })
                .await;

            if let Err(error) = cx.update_window(handle, |_, window, cx| {
                this.update(cx, |this, cx| {
                    if generation != this.fork_generation {
                        return;
                    }
                    this.apply_fork(announcement, mirror_path, namespace, result, window, cx);
                })
            }) {
                log::warn!("failed to apply fork: {error}");
            }
            Ok(())
        });
        self.tasks.push(task);
    }

    fn apply_fork(
        &mut self,
        announcement: Announcement,
        mirror_path: PathBuf,
        namespace: String,
        result: Result<(Vec<String>, Vec<String>), anyhow::Error>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.loading = false;
        let (base_branches, compare_branches) = match result {
            Ok(branches) => branches,
            Err(error) => {
                self.error = Some(format!("Could not compare against the fork: {error}").into());
                cx.notify();
                return;
            }
        };
        if compare_branches.is_empty() {
            self.error = Some("The fork has no branches to compare".into());
            cx.notify();
            return;
        }
        if base_branches.is_empty() {
            self.error = Some("Could not list the target repository's branches.".into());
            cx.notify();
            return;
        }

        let announced = self.store.read(cx).head.clone();
        let base = announced
            .filter(|branch| base_branches.contains(branch))
            .map(SharedString::from)
            .or_else(|| {
                base_branches
                    .iter()
                    .find(|branch| branch.as_str() == "main")
                    .map(|branch| SharedString::from(branch.clone()))
            })
            .unwrap_or_else(|| SharedString::from(base_branches[0].clone()));
        let compare = compare_branches
            .iter()
            .find(|branch| branch.as_str() == "main")
            .map(|branch| SharedString::from(branch.clone()))
            .unwrap_or_else(|| SharedString::from(compare_branches[0].clone()));

        self.fork = Some(ForkCompare {
            announcement,
            namespace,
            mirror_path,
        });
        self.error = None;
        self.base = base;
        self.compare = compare;
        self.branches = base_branches.into_iter().map(SharedString::from).collect();
        self.sync_selectors(window, cx);
        self.reload_compare(cx);
    }

    pub(super) fn reload_compare(&mut self, cx: &mut Context<Self>) {
        let Some(repo_path) = self.work_path() else {
            return;
        };
        let base = self.base_ref();
        let compare = self.compare_ref();
        let base_name = self.base.to_string();
        let compare_name = self.compare.to_string();

        self.loading = true;
        self.error = None;
        self.compare_generation += 1;
        let generation = self.compare_generation;
        cx.notify();

        if base == compare {
            self.loading = false;
            self.merge_base = None;
            self.commits = None;
            self.pane.update(cx, |pane, cx| pane.clear(cx));
            self.error = Some("Choose different base and compare branches".into());
            cx.notify();
            return;
        }

        let task: gpui::Task<Result<(), anyhow::Error>> = cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn({
                    async move {
                        let repo = Repo::open(Path::new(&repo_path))?;
                        let merge_base = repo.merge_base(&base, &compare)?.ok_or_else(|| {
                            anyhow::anyhow!(
                                "{base_name} and {compare_name} share no common ancestor"
                            )
                        })?;
                        let commits = repo.commit_range(&merge_base, &compare)?;
                        let diff = repo.range_diff(&merge_base, &compare)?;
                        Ok::<_, anyhow::Error>((merge_base, commits, diff))
                    }
                })
                .await;

            this.update(cx, |this, cx| {
                if generation != this.compare_generation {
                    return;
                }
                this.loading = false;
                match result {
                    Ok((merge_base, commits, diff)) => {
                        this.merge_base = Some(merge_base);
                        this.commits = Some(commits);
                        this.pane.update(cx, |pane, cx| pane.set_diff(diff, cx));
                    }
                    Err(error) => {
                        this.merge_base = None;
                        this.commits = None;
                        this.pane.update(cx, |pane, cx| pane.clear(cx));
                        this.error = Some(error.to_string().into());
                    }
                }
                cx.notify();
            })?;
            Ok(())
        });
        self.tasks.push(task);
    }
}
