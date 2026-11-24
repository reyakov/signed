use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, PathPromptOptions, Render, SharedString, StyleRefinement, Styled as _,
    Subscription, UniformListScrollHandle, WeakEntity, Window, div, px,
};
use nostr::prelude::{PublicKey, Url};
use signed_core::Announcement;
use signed_git::{GitCache, Repo};
use signed_state::{Backend, BackendEvent, CheckoutsStore, Mirrors, RepoListStore, RepoStore};
use ui::button::{Button, ButtonVariant};
use ui::combobox::{Combobox, ComboboxState};
use ui::common::{StyledExt as _, h_flex, v_flex};
use ui::empty_state::EmptyState;
use ui::icons::{Icon, IconRole};
use ui::input::{Input, InputState};
use ui::layout::panel::Panel;
use ui::menu::Menu;
use ui::spinner::Spinner;
use ui::tabs::{Tab, Tabs};

use super::helpers::{ForkCompare, truncate_label};
use crate::app::AppRoot;
use crate::repo::commit_diff::{CommitDiffView, DiffPane};
use crate::repo::helpers::announcement_display_name;
use crate::shared::event_list::commit_list;
use crate::shared::log_dropped;
use crate::shared::panels::{close_detail, open_detail};
use crate::shared::progress::error_row;

pub struct NewPullRequestView {
    workspace: WeakEntity<AppRoot>,
    panel_id: SharedString,
    pub(super) store: Entity<RepoStore>,
    pub(super) repo_path: Option<PathBuf>,
    pub(super) branches: Vec<SharedString>,
    pub(super) fork: Option<ForkCompare>,
    pub(super) base: SharedString,
    pub(super) compare: SharedString,
    pub(super) base_select: Entity<ComboboxState<SharedString>>,
    pub(super) compare_select: Entity<ComboboxState<SharedString>>,
    subject: Entity<InputState>,
    description: Entity<InputState>,
    pub(super) merge_base: Option<String>,
    pub(super) commits: Option<Arc<Vec<signed_git::FileCommit>>>,
    pub(super) loading: bool,
    pub(super) error: Option<SharedString>,
    submitting: bool,
    pub(super) compare_generation: u64,
    pub(super) fork_generation: u64,
    active_tab: usize,
    pub(super) pane: Entity<DiffPane>,
    scroll_handle: UniformListScrollHandle,
    pub(super) tasks: Vec<gpui::Task<Result<(), anyhow::Error>>>,
    source_open: bool,
    /// The checkouts, forks, and user the source menu lists.
    checkouts: Arc<Vec<PathBuf>>,
    forks: Arc<Vec<Announcement>>,
    user: Option<PublicKey>,
    _subscriptions: Vec<Subscription>,
}

impl NewPullRequestView {
    pub fn new(
        workspace: WeakEntity<AppRoot>,
        panel_id: SharedString,
        store: Entity<RepoStore>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let checkouts = CheckoutsStore::global(cx);
        let repositories = RepoListStore::global(cx);
        let backend = Backend::global(cx);

        let subject = InputState::new(cx);
        let description = InputState::builder().multiline().build(cx);

        let pane = cx.new(DiffPane::new);
        let weak = cx.entity().downgrade();

        let base_select = cx.new(|cx| {
            ComboboxState::new(
                Combobox::new("pr-base", "Base", Vec::new()).on_change({
                    let weak = weak.clone();
                    move |value, _window, cx| {
                        if let Some(base) = value {
                            log_dropped(weak.update(cx, |this, cx| {
                                if this.base == base {
                                    return;
                                }
                                this.base = base;
                                this.reload_compare(cx);
                            }));
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
                            log_dropped(weak.update(cx, |this, cx| {
                                if this.compare == compare {
                                    return;
                                }
                                this.compare = compare;
                                this.reload_compare(cx);
                            }));
                        }
                    }
                }),
                window,
                cx,
            )
        });

        let mut subscriptions = vec![];

        subscriptions.push(cx.observe(&checkouts, |this, _, cx| {
            this.refresh_sources(cx);
        }));

        subscriptions.push(cx.observe(&repositories, |this, _, cx| {
            this.refresh_sources(cx);
        }));

        subscriptions.push(cx.observe(&store, |this, _, cx| {
            this.refresh_sources(cx);
        }));

        subscriptions.push(cx.subscribe(&backend, |this, _, event, cx| {
            if matches!(event, BackendEvent::SignerChanged) {
                this.refresh_sources(cx);
            }
        }));

        cx.defer_in(window, move |this, window, cx| {
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
            this.refresh_sources(cx);
        });

        Self {
            workspace,
            panel_id,
            store,
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
            fork_generation: 0,
            active_tab: 0,
            pane,
            scroll_handle: UniformListScrollHandle::new(),
            tasks: Vec::new(),
            source_open: false,
            checkouts: Arc::default(),
            forks: Arc::default(),
            user: None,
            _subscriptions: subscriptions,
        }
    }

    /// Reaps finished tasks, then queues another.
    pub(super) fn push_task(&mut self, task: gpui::Task<Result<(), anyhow::Error>>) {
        self.tasks.retain(|task| !task.is_ready());
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
        let workspace = self.workspace.clone();
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
            close_detail(&workspace, &panel_id, cx);
            Ok(())
        });
        self.push_task(task);
    }

    fn open_commit_diff(&mut self, commit_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo_path) = self.work_path() else {
            return;
        };
        let id = match self.store.read(cx).addr() {
            Some(addr) => SharedString::from(format!("commit:{addr}:{commit_id}")),
            None => SharedString::from(format!("commit:{}:{commit_id}", repo_path.display())),
        };
        let workspace = self.workspace.clone();
        let commit_id = commit_id.to_owned();
        open_detail(
            &workspace,
            id,
            move |window, cx| {
                cx.new(|cx| {
                    CommitDiffView::new(repo_path.clone(), commit_id.clone(), false, window, cx)
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
        let source_view = cx.entity().downgrade();
        let associated = Arc::clone(&self.checkouts);

        let active_path = (self.fork.is_none())
            .then(|| self.repo_path.clone())
            .flatten();

        let candidates = Arc::clone(&self.forks);
        let user = signed_state::Backend::global(cx).read(cx).current_user();
        let active_fork = self.fork.as_ref().map(|fork| fork.announcement.addr());
        let can_submit = self.submit_enabled(cx);

        let source_label = match &self.fork {
            Some(fork) => truncate_label(&announcement_display_name(&fork.announcement)),
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
                    .child(div().text_xs().font_medium().child("Merge Into"))
                    .child(div().w(px(160.)).child(self.base_select.clone())),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_xs().font_medium().child("Pull From"))
                    .child(div().w(px(160.)).child(self.compare_select.clone())),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_xs().font_medium().child("Source"))
                    .child(
                        Menu::new("pr-source")
                            .open(self.source_open)
                            .on_change(move |open, _, cx| {
                                log_dropped(weak_menu.update(cx, |this, cx| {
                                    this.source_open = open;
                                    cx.notify();
                                }));
                            })
                            .trigger(move |_, _| {
                                Button::new("compare-source")
                                    .label(source_label.clone())
                                    .variant(ButtonVariant::Secondary)
                                    .into_any_element()
                            })
                            .items(move |_, _| {
                                super::helpers::source_menu_items(
                                    source_view.clone(),
                                    associated.clone(),
                                    active_path.clone(),
                                    candidates.clone(),
                                    user,
                                    active_fork.clone(),
                                )
                            }),
                    ),
            )
            .when_some(refresh_fork, |this, fork| {
                this.child(
                    Button::new("refresh-fork")
                        .icon(Icon::named(IconRole::Refresh))
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
                        log_dropped(weak.update(cx, |this, cx| this.submit(window, cx)));
                    }),
            )
            .into_any_element()
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let files = self.pane.read(cx).diff().map_or(0, |diff| diff.files.len());
        let commits = self.commits.as_ref().map_or(0, |commits| commits.len());
        let weak = cx.entity().downgrade();

        Tabs::new("new-pr-tabs")
            .selected(self.active_tab)
            .tabs(move |_, _| {
                vec![
                    Tab::new("files", format!("Files ({files})")).lead(Icon::named(IconRole::File)),
                    Tab::new("commits", format!("Commits ({commits})"))
                        .lead(Icon::named(IconRole::GitCommit)),
                ]
            })
            .on_change(move |index, _, cx| {
                log_dropped(weak.update(cx, |this, cx| {
                    this.active_tab = index;
                    cx.notify();
                }));
            })
            .into_any_element()
    }

    fn render_content(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.loading {
            return Spinner::centered("new-pr-loading");
        }

        if !self.has_source() {
            return EmptyState::centered("Choose a local checkout or an announced fork to compare");
        }

        if self.commits.is_none() && self.error.is_some() {
            return EmptyState::centered("Nothing to compare");
        }

        match self.active_tab {
            0 => self
                .pane
                .clone()
                .cached(StyleRefinement::default().size_full().min_h_0())
                .into_any_element(),
            _ => self.render_commits(cx),
        }
    }

    fn render_commits(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(commits) = self.commits.as_ref() else {
            return EmptyState::centered("No commits");
        };
        if commits.is_empty() {
            return EmptyState::centered("No commits between the branches");
        }
        let commits = Arc::clone(commits);
        let view = cx.entity().downgrade();

        commit_list(
            "new-pr-commits",
            "new-pr-commits-scroll",
            commits,
            self.scroll_handle.clone(),
            |_, _, _| None,
            move |commit, window, cx| {
                let id = commit.id.clone();
                log_dropped(view.update(cx, |this, cx| {
                    this.open_commit_diff(&id, window, cx);
                }));
            },
        )
    }

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
        self.push_task(task);
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
        self.push_task(task);
    }

    fn apply_checkout(
        &mut self,
        path: String,
        info: Option<(Vec<String>, Option<String>)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.compare_generation = self.compare_generation.wrapping_add(1);
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
        let base = preferred_branch(&branches, announced);
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

    /// Rebuilds the source menu's checkouts, forks, and user when their inputs changed.
    fn refresh_sources(&mut self, cx: &mut Context<Self>) {
        let (addr, euc) = {
            let store = self.store.read(cx);
            (
                store.addr().cloned(),
                store
                    .announcement
                    .as_ref()
                    .and_then(|announcement| announcement.euc.clone()),
            )
        };

        let checkouts = CheckoutsStore::global(cx);
        let repositories = RepoListStore::global(cx);
        let backend = Backend::global(cx);
        let user = backend.read(cx).current_user();
        let checkouts = addr
            .as_ref()
            .map_or_else(Vec::new, |addr| checkouts.read(cx).associations_of(addr));

        let forks: Vec<Announcement> = addr.as_ref().map_or_else(Vec::new, |addr| {
            Announcement::forks_in(
                &repositories.read(cx).announcements,
                addr,
                euc.as_deref(),
                user,
            )
            .into_iter()
            .cloned()
            .collect()
        });

        if self.user == user
            && self.checkouts.as_slice() == checkouts.as_slice()
            && self.forks.as_slice() == forks.as_slice()
        {
            return;
        }

        self.user = user;
        self.checkouts = Arc::new(checkouts);
        self.forks = Arc::new(forks);

        cx.notify();
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
        self.push_task(task);
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
        self.compare_generation = self.compare_generation.wrapping_add(1);

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
        let base = preferred_branch(&base_branches, announced);
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
        self.base = SharedString::from(base);
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
        self.compare_generation = self.compare_generation.wrapping_add(1);
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
                        this.commits = Some(Arc::new(commits));
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
        self.push_task(task);
    }
}

impl Panel for NewPullRequestView {
    fn title(&self, _cx: &App) -> SharedString {
        SharedString::from("New pull request")
    }

    fn loading(&self, _cx: &App) -> bool {
        self.loading
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
                            .when_some(error_row(&self.error, cx), |this, error| this.child(error)),
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

/// The announced head when listed, else `main`, else the first branch.
fn preferred_branch(branches: &[String], announced: Option<String>) -> String {
    announced
        .filter(|branch| branches.contains(branch))
        .or_else(|| {
            branches
                .iter()
                .find(|branch| branch.as_str() == "main")
                .cloned()
        })
        .unwrap_or_else(|| branches[0].clone())
}
