use std::path::{Path, PathBuf};

use assets::CustomIconName;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, PathPromptOptions, Render, SharedString, Styled as _, Subscription,
    UniformListScrollHandle, WeakEntity, Window, div, px, uniform_list,
};
use gpui_native_kit::button::Button;
use gpui_native_kit::combobox::{Combobox, ComboboxState};
use gpui_native_kit::common::{h_flex, v_flex};
use gpui_native_kit::data::tabs::{Tab, Tabs};
use gpui_native_kit::icons::{Icon, IconRole};
use gpui_native_kit::input::{Input, InputState};
use gpui_native_kit::layout::dock::{DockState, Panel};
use gpui_native_kit::menu::{Menu, MenuItem};
use gpui_native_kit::scrollbar::Scrollbar;
use gpui_native_kit::spinner::Spinner;
use gpui_native_kit::theme::ActiveTheme as _;
use metadata::ProfileDirectory;
use nostr::prelude::Url;
use signed_core::Announcement;
use signed_git::{GitCache, Repo};
use signed_state::{Backend, CheckoutsStore, Mirrors, RepoListStore, RepoStore};

use super::helpers::{ForkCompare, fork_display_name, shorten_owner, truncate_label};
use crate::repo::commit_diff::{CommitDiffView, DiffPane};
use crate::repo::helpers::commit_row;
use crate::shared::widgets::empty_state;

pub struct NewPullRequestView {
    dock: WeakEntity<DockState>,
    panel_id: SharedString,
    store: Entity<RepoStore>,
    repo_name: SharedString,
    repo_path: Option<PathBuf>,
    branches: Vec<SharedString>,
    fork: Option<ForkCompare>,
    base: SharedString,
    compare: SharedString,
    base_select: Entity<ComboboxState<SharedString>>,
    compare_select: Entity<ComboboxState<SharedString>>,
    subject: Entity<InputState>,
    description: Entity<InputState>,
    merge_base: Option<String>,
    commits: Option<Vec<signed_git::FileCommit>>,
    loading: bool,
    error: Option<SharedString>,
    submitting: bool,
    compare_generation: u64,
    active_tab: usize,
    pane: Entity<DiffPane>,
    scroll_handle: UniformListScrollHandle,
    tasks: Vec<gpui::Task<Result<(), anyhow::Error>>>,
    directory: Entity<ProfileDirectory>,
    source_open: bool,
    _subscriptions: Vec<Subscription>,
}

impl NewPullRequestView {
    pub fn new(
        dock: WeakEntity<DockState>,
        panel_id: SharedString,
        store: Entity<RepoStore>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let directory = ProfileDirectory::new(cx);
        let repo_name = store.read(cx).name();
        let pane = cx.new(DiffPane::new);

        let weak = cx.entity().downgrade();
        let subject = InputState::new(cx);
        let description = InputState::builder().multiline().build(cx);

        let base_select = cx.new(|cx| {
            ComboboxState::new(
                Combobox::new("pr-base", "Base", Vec::new()).on_change({
                    let weak = weak.clone();
                    move |value, _window, cx| {
                        if let Some(base) = value {
                            let _ = weak.update(cx, |this, cx| {
                                this.base = base;
                                this.reload_compare(cx);
                            });
                        }
                    }
                }),
                window,
                cx,
            )
        });

        let compare_select = cx.new(|cx| {
            ComboboxState::new(
                Combobox::new("pr-compare", "Compare", Vec::new()).on_change({
                    let weak = weak.clone();
                    move |value, _window, cx| {
                        if let Some(compare) = value {
                            let _ = weak.update(cx, |this, cx| {
                                this.compare = compare;
                                this.reload_compare(cx);
                            });
                        }
                    }
                }),
                window,
                cx,
            )
        });

        if let Some(owner) = store.read(cx).announcement.as_ref().map(|a| a.owner) {
            directory.update(cx, |directory, cx| directory.track([owner], cx));
        }

        window.defer(cx, move |window, cx| {
            let _ = weak.update(cx, |this, cx| {
                let Some(addr) = this.store.read(cx).addr().cloned() else {
                    return;
                };

                let Some(path) = CheckoutsStore::global(cx)
                    .read(cx)
                    .associations_of(&addr)
                    .into_iter()
                    .next()
                else {
                    return;
                };

                this.apply_folder_path(path, window, cx);
            });
        });

        Self {
            dock,
            panel_id,
            store,
            repo_name,
            repo_path: None,
            branches: Vec::new(),
            fork: None,
            base: SharedString::default(),
            compare: SharedString::default(),
            base_select,
            compare_select,
            subject,
            description,
            merge_base: None,
            commits: None,
            loading: false,
            error: None,
            submitting: false,
            compare_generation: 0,
            active_tab: 0,
            pane,
            scroll_handle: UniformListScrollHandle::new(),
            tasks: Vec::new(),
            directory,
            source_open: false,
            _subscriptions: Vec::new(),
        }
    }

    fn has_source(&self) -> bool {
        self.repo_path.is_some() || self.fork.is_some()
    }

    fn work_path(&self) -> Option<PathBuf> {
        match &self.fork {
            Some(fork) => Some(fork.mirror_path.clone()),
            None => self.repo_path.clone(),
        }
    }

    fn base_ref(&self) -> String {
        match &self.fork {
            Some(_) => ForkCompare::base_ref(&self.base),
            None => self.base.to_string(),
        }
    }

    fn compare_ref(&self) -> String {
        match &self.fork {
            Some(fork) => fork.compare_ref(&self.compare),
            None => self.compare.to_string(),
        }
    }

    fn choose_checkout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    fn apply_folder_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let path_string = path.to_string_lossy().to_string();
        let info = Repo::try_open(&path).and_then(|repo| {
            let branches = repo.branches().ok()?;
            let current = repo.current_branch();
            Some((branches, current))
        });
        self.apply_checkout(path_string, info, window, cx);
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

    fn fork_candidates(&self, cx: &App) -> Vec<Announcement> {
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

    fn choose_fork(
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
        let expected = self.fork.as_ref().map(|fork| fork.announcement.addr());
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
                    let applied = this.fork.as_ref().map(|fork| fork.announcement.addr());
                    if applied != expected {
                        this.loading = false;
                        cx.notify();
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

    fn reload_compare(&mut self, cx: &mut Context<Self>) {
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

    fn submit(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.submitting || self.loading {
            return;
        }
        let Some(merge_base) = self.merge_base.clone() else {
            return;
        };
        let Some(repo_path) = self.work_path() else {
            return;
        };
        let subject = self.subject.read(cx).value().to_string();
        let description = self.description.read(cx).value().to_string();
        let branch_name = self.compare.to_string();
        let compare_ref = self.compare_ref();
        let store = self.store.clone();
        let dock = self.dock.clone();
        let panel_id = self.panel_id.clone();

        self.submitting = true;
        self.error = None;
        cx.notify();

        let task: gpui::Task<Result<(), anyhow::Error>> = cx.spawn(async move |this, cx| {
            let publish = store.update(cx, |store, cx| {
                store.open_pull_request_from_refs(
                    repo_path,
                    merge_base,
                    compare_ref,
                    (!subject.is_empty()).then_some(subject),
                    description,
                    Some(branch_name),
                    false,
                    cx,
                )
            });
            if let Err(error) = publish.await {
                this.update(cx, |this, cx| {
                    this.submitting = false;
                    this.error = Some(error.to_string().into());
                    cx.notify();
                })?;
                return Ok(());
            }
            this.update(cx, |this, _cx| {
                this.submitting = false;
            })?;
            let _ = dock.update(cx, |dock, cx| dock.close_panel(&panel_id, cx));
            Ok(())
        });
        self.tasks.push(task);
    }

    fn open_commit_diff(&mut self, commit_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo_path) = self.work_path() else {
            return;
        };
        let id = match self.store.read(cx).addr() {
            Some(addr) => SharedString::from(format!("commit:{addr}:{commit_id}")),
            None => SharedString::from(format!("commit:{}:{commit_id}", repo_path.display())),
        };
        let dock = self.dock.clone();
        let store = self.store.clone();
        let repo_name = self.repo_name.clone();
        let commit_id = commit_id.to_owned();
        crate::shared::panels::open(
            &dock,
            id,
            move |window, cx| {
                cx.new(|cx| {
                    CommitDiffView::new(
                        store.clone(),
                        repo_path.clone(),
                        repo_name.clone(),
                        commit_id.clone(),
                        window,
                        cx,
                    )
                })
            },
            window,
            cx,
        );
    }

    fn submit_enabled(&self, cx: &App) -> bool {
        self.has_source()
            && !self.loading
            && !self.submitting
            && self.merge_base.is_some()
            && self
                .commits
                .as_ref()
                .is_some_and(|commits| !commits.is_empty())
            && !self.subject.read(cx).value().is_empty()
    }

    fn render_compare_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let weak = cx.entity().downgrade();
        let weak_menu = cx.entity().downgrade();
        let source_items = self.source_menu_items(cx, cx.entity().downgrade());
        let can_submit = self.submit_enabled(cx);
        let source_label = match &self.fork {
            Some(fork) => truncate_label(&fork_display_name(&fork.announcement)),
            None => self.repo_path.as_ref().map_or_else(
                || SharedString::from("No source"),
                |path| truncate_label(&path.display().to_string()),
            ),
        };
        let refresh_fork = self.fork.as_ref().map(|fork| fork.announcement.clone());

        h_flex()
            .px_4()
            .py_2()
            .w_full()
            .gap_3()
            .items_end()
            .child(
                v_flex()
                    .gap_1()
                    .child(label("Merge Into"))
                    .child(div().w(px(160.)).child(self.base_select.clone())),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(label("Pull From"))
                    .child(div().w(px(160.)).child(self.compare_select.clone())),
            )
            .child(
                v_flex().gap_1().child(label("Source")).child(
                    Menu::new("pr-source")
                        .open(self.source_open)
                        .on_change(move |open, _, cx| {
                            let _ = weak_menu.update(cx, |this, cx| {
                                this.source_open = open;
                                cx.notify();
                            });
                        })
                        .trigger(move |_, _| {
                            Button::new("compare-source")
                                .label(source_label.clone())
                                .variant(gpui_native_kit::button::ButtonVariant::Secondary)
                                .into_any_element()
                        })
                        .items(move |_, _| source_items.clone()),
                ),
            )
            .when_some(refresh_fork, |this, fork| {
                this.child(
                    Button::new("refresh-fork")
                        .icon(Icon::named(CustomIconName::Refresh))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.choose_fork(fork.clone(), window, cx);
                        })),
                )
            })
            .child(div().flex_1())
            .child(
                Button::new("create-pr")
                    .label("Create pull request")
                    .icon(Icon::named(IconRole::Plus))
                    .loading(self.submitting)
                    .disabled(!can_submit)
                    .on_click(move |_, window, cx| {
                        let _ = weak.update(cx, |this, cx| this.submit(window, cx));
                    }),
            )
            .into_any_element()
    }

    fn source_menu_items(&self, cx: &App, view: WeakEntity<Self>) -> Vec<MenuItem> {
        let associated = self
            .store
            .read(cx)
            .addr()
            .map(|addr| CheckoutsStore::global(cx).read(cx).associations_of(addr))
            .unwrap_or_default();
        let active_path = (self.fork.is_none())
            .then(|| self.repo_path.clone())
            .flatten();
        let active_fork = self.fork.as_ref().map(|fork| fork.announcement.addr());

        let mut items = Vec::new();
        for path in associated {
            let label = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            let view = view.clone();
            items.push(
                MenuItem::new(truncate_label(&label))
                    .checked(active_path.as_ref() == Some(&path))
                    .handler(move |window, cx| {
                        let path = path.clone();
                        let _ =
                            view.update(cx, |this, cx| this.apply_folder_path(path, window, cx));
                    }),
            );
        }
        let folder_view = view.clone();
        items.push(
            MenuItem::new("Choose another folder…").handler(move |window, cx| {
                let _ = folder_view.update(cx, |this, cx| this.choose_checkout(window, cx));
            }),
        );
        items.push(MenuItem::separator());

        let candidates = self.fork_candidates(cx);
        if candidates.is_empty() {
            items.push(MenuItem::new("No announced forks of this repository").disabled(true));
        } else {
            let user = Backend::global(cx).read(cx).current_user();
            for candidate in candidates {
                let subtitle = if Some(candidate.owner) == user {
                    SharedString::from("Your fork")
                } else {
                    SharedString::from(format!("by {}", shorten_owner(&candidate.owner)))
                };
                let title = format!("{} — {}", fork_display_name(&candidate), subtitle);
                let view = view.clone();
                items.push(
                    MenuItem::new(truncate_label(&title))
                        .checked(active_fork == Some(candidate.addr()))
                        .handler(move |window, cx| {
                            let announcement = candidate.clone();
                            let _ = view
                                .update(cx, |this, cx| this.choose_fork(announcement, window, cx));
                        }),
                );
            }
        }
        items
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let files = self.pane.read(cx).diff().map_or(0, |diff| diff.files.len());
        let commits = self.commits.as_ref().map_or(0, |commits| commits.len());
        let weak = cx.entity().downgrade();

        Tabs::new("new-pr-tabs")
            .selected(self.active_tab)
            .tabs(move |_, _| {
                vec![
                    Tab::new("files", format!("Files ({files})"))
                        .lead(Icon::named(CustomIconName::GitFile)),
                    Tab::new("commits", format!("Commits ({commits})"))
                        .lead(Icon::named(CustomIconName::GitCommit)),
                ]
            })
            .on_change(move |index, _, cx| {
                let _ = weak.update(cx, |this, cx| {
                    this.active_tab = index;
                    cx.notify();
                });
            })
            .into_any_element()
    }

    fn render_content(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.loading {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .child(Spinner::new("new-pr-loading"))
                .into_any_element();
        }
        if !self.has_source() {
            return empty_state("Choose a local checkout or an announced fork to compare");
        }
        if self.commits.is_none() && self.error.is_some() {
            return empty_state("Nothing to compare");
        }
        match self.active_tab {
            0 => self.pane.clone().into_any_element(),
            _ => self.render_commits(cx),
        }
    }

    fn render_commits(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(commits) = self.commits.as_ref() else {
            return empty_state("No commits");
        };
        if commits.is_empty() {
            return empty_state("No commits between the branches");
        }
        let commits = commits.clone();
        let view = cx.entity().downgrade();

        v_flex()
            .relative()
            .flex_1()
            .min_h_0()
            .child(
                uniform_list("new-pr-commits", commits.len(), move |range, _, cx| {
                    range
                        .map(|ix| {
                            let commit = commits[ix].clone();
                            let id = commit.id.clone();
                            let view = view.clone();
                            commit_row(
                                ix,
                                &commit,
                                move |window, cx| {
                                    let _ = view.update(cx, |this, cx| {
                                        this.open_commit_diff(&id, window, cx);
                                    });
                                },
                                cx,
                            )
                        })
                        .collect()
                })
                .track_scroll(&self.scroll_handle)
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .child(Scrollbar::vertical(
                        "new-pr-commits-scroll",
                        self.scroll_handle.clone(),
                    )),
            )
            .into_any_element()
    }
}

impl Panel for NewPullRequestView {
    fn title(&self, _cx: &gpui::App) -> Option<SharedString> {
        Some(SharedString::from(format!(
            "{}/new-pull-request",
            self.repo_name
        )))
    }

    fn lead(&self, cx: &gpui::App) -> Option<AnyElement> {
        Some(crate::shared::widgets::repo_tab_avatar(
            self.store.read(cx),
            &self.directory,
            cx,
        ))
    }
}

impl Render for NewPullRequestView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("new-pr")
            .size_full()
            .child(
                v_flex()
                    .gap_3()
                    .child(self.render_compare_bar(cx))
                    .child(
                        v_flex()
                            .px_4()
                            .gap_2()
                            .child(
                                Input::new("pr-title", self.subject.clone()).placeholder("Title"),
                            )
                            .child(
                                Input::new("pr-description", self.description.clone())
                                    .rows(3)
                                    .placeholder("Describe..."),
                            )
                            .when_some(self.error.clone(), |this, error| {
                                this.child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().ramps.destructive.base)
                                        .child(error),
                                )
                            }),
                    )
                    .child(self.render_tabs(cx)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(self.render_content(cx)),
            )
    }
}

fn label(text: &'static str) -> AnyElement {
    div()
        .text_xs()
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .child(text)
        .into_any_element()
}
