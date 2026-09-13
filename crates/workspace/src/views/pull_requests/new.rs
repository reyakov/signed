use std::path::{Path, PathBuf};
use std::rc::Rc;

use assets::CustomIconName;
use dock::{BasePanel, DockArea, Panel, PanelEvent, add_center_panel, panel_handle};
use gpui::prelude::*;
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, PathPromptOptions,
    Pixels, Render, SharedString, Size, Subscription, WeakEntity, Window, div, px, relative, size,
};
use gpui_base::{Button as BaseButton, StyledExt};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::combobox::{Combobox, ComboboxEvent, ComboboxState};
use gpui_component::input::{Input, InputState, Textarea, TextareaState};
use gpui_component::menu::{DropdownMenu, PopupMenu, PopupMenuItem};
use gpui_component::scroll::Scrollbar;
use gpui_component::searchable_list::SearchableVec;
use gpui_component::spinner::Spinner;
use gpui_component::{
    ActiveTheme, Disableable, Icon, IconName, Sizable, VirtualListScrollHandle, h_flex, v_flex,
    v_virtual_list,
};
use nostr::prelude::*;
use signed_core::{Announcement, RepoAddr, fork_candidates};
use signed_git::{
    delete_refs_with_prefix, fetch_repo_refs, fork_namespace, merge_base, refs_with_prefix,
    worktree_commit_range_commits, worktree_commit_range_diff,
};
use signed_state::{Backend, CheckoutsStore, GitStore, RepoListStore, RepoStore};
use signed_ui::{CountBadge, placeholder, ref_selector_trigger};

use crate::views::commit_diff::{COMMIT_ROW_HEIGHT, CommitDiffView, DiffPane, commit_row};

pub struct NewPullRequestView {
    focus_handle: FocusHandle,
    dock_area: WeakEntity<DockArea>,
    store: Entity<RepoStore>,
    repo_name: SharedString,
    /// The user's local checkout.
    repo_path: Option<PathBuf>,
    /// Backs both selectors in checkout mode.
    branches: Vec<SharedString>,
    fork: Option<ForkCompare>,
    /// Selected base branch, the PR target, stored as a short name.
    base: SharedString,
    /// Selected compare branch, the PR source, stored as a short name.
    compare: SharedString,
    base_select: Entity<ComboboxState<SearchableVec<SharedString>>>,
    compare_select: Entity<ComboboxState<SearchableVec<SharedString>>>,
    subject: Entity<InputState>,
    description: Entity<TextareaState>,
    /// Merge base of the selected branches, `None` until the compare loads.
    merge_base: Option<String>,
    /// Commits in `merge_base..compare`, newest first.
    commits: Option<Vec<signed_git::FileCommit>>,
    loading: bool,
    error: Option<SharedString>,
    /// A submit, patch generation and publish, is in flight.
    submitting: bool,
    /// Bumped on every branch switch, stale compare results are discarded.
    compare_generation: u64,
    /// 0 = Files, 1 = Commits.
    active_tab: usize,
    /// The compare diff, the Files tab body.
    pane: Entity<DiffPane>,
    scroll_handle: VirtualListScrollHandle,
    item_sizes: Rc<Vec<Size<Pixels>>>,
    _subscriptions: Vec<Subscription>,
}

struct ForkCompare {
    /// Fork announcement the compare branch is imported from.
    announcement: Announcement,
    /// Import namespace of the form `<owner-hex>/<sanitized-id>`.
    namespace: String,
    /// Path of the target repository's GitCache mirror.
    mirror_path: PathBuf,
}

impl ForkCompare {
    fn base_ref(name: &str) -> String {
        format!("refs/remotes/origin/{name}")
    }

    fn compare_ref(&self, name: &str) -> String {
        format!("refs/fork/{}/{}", self.namespace, name)
    }
}

fn fork_display_name(announcement: &Announcement) -> SharedString {
    announcement
        .name
        .as_deref()
        .map(SharedString::from)
        .unwrap_or_else(|| SharedString::from(announcement.id.clone()))
}

fn shorten_owner(owner: &PublicKey) -> String {
    let hex = owner.to_hex();
    hex.chars().take(10).collect()
}

/// Truncate a label for the fixed-width controls of the compare bar.
fn truncate_label(label: &str) -> SharedString {
    const MAX: usize = 18;
    let mut chars = label.chars();
    let (prefix, rest) = (chars.by_ref().take(MAX).collect::<String>(), chars.next());
    let label = if rest.is_some() {
        format!("{}…", &prefix[..prefix.len().saturating_sub(1)])
    } else {
        prefix
    };
    SharedString::from(label)
}

fn checkout_source_item(
    view: WeakEntity<NewPullRequestView>,
    path: PathBuf,
    active: bool,
) -> PopupMenuItem {
    let subtitle = path.display().to_string();
    let title = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| subtitle.clone());
    PopupMenuItem::element(move |_window, cx| {
        source_row(
            IconName::Folder,
            truncate_label(&title),
            truncate_label(&subtitle),
            cx,
        )
    })
    .checked(active)
    .on_click(move |_event, window, cx| {
        if let Some(view) = view.upgrade() {
            view.update(cx, |this, cx| {
                this.apply_folder_path(path.clone(), window, cx)
            });
        }
    })
}

fn choose_folder_source_item(view: WeakEntity<NewPullRequestView>) -> PopupMenuItem {
    PopupMenuItem::element(move |_window, cx| {
        source_row(
            IconName::FolderOpen,
            "Choose another folder…",
            "Pick any local checkout",
            cx,
        )
    })
    .on_click(move |_event, window, cx| {
        if let Some(view) = view.upgrade() {
            view.update(cx, |this, cx| this.choose_checkout(window, cx));
        }
    })
}

fn fork_source_item(
    view: WeakEntity<NewPullRequestView>,
    announcement: Announcement,
    subtitle: SharedString,
    _active: bool,
) -> PopupMenuItem {
    let title = truncate_label(&fork_display_name(&announcement));

    PopupMenuItem::element(move |_window, cx| {
        source_row(
            CustomIconName::GitBranch,
            title.clone(),
            truncate_label(&subtitle),
            cx,
        )
    })
    .on_click(move |_event, window, cx| {
        if let Some(view) = view.upgrade() {
            view.update(cx, |this, cx| {
                this.choose_fork(announcement.clone(), window, cx)
            });
        }
    })
}

fn source_row<T>(icon: impl Into<Icon>, title: T, subtitle: T, cx: &App) -> AnyElement
where
    T: Into<SharedString>,
{
    let title = title.into();
    let subtitle = subtitle.into();

    h_flex()
        .gap_2()
        .w_full()
        .min_w_0()
        .items_center()
        .child(Icon::new(icon).small().flex_shrink_0())
        .child(
            v_flex()
                .min_w_0()
                .flex_1()
                .child(
                    div()
                        .w_full()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_sm()
                        .line_height(relative(1.25))
                        .child(title),
                )
                .child(
                    div()
                        .w_full()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .line_height(relative(1.25))
                        .child(subtitle),
                ),
        )
        .into_any_element()
}

impl NewPullRequestView {
    pub fn new(
        dock_area: WeakEntity<DockArea>,
        store: Entity<RepoStore>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let repo_name = store.read(cx).name();
        let pane = cx.new(DiffPane::new);
        let subject = cx.new(|cx| InputState::new(window, cx).placeholder("Title"));
        let description = cx.new(|cx| TextareaState::new(window, cx).placeholder("Describe..."));

        let base_select = cx.new(|cx| {
            ComboboxState::new(
                SearchableVec::new(Vec::<SharedString>::new()),
                Vec::new(),
                window,
                cx,
            )
            .searchable(true)
        });

        let compare_select = cx.new(|cx| {
            ComboboxState::new(
                SearchableVec::new(Vec::<SharedString>::new()),
                Vec::new(),
                window,
                cx,
            )
            .searchable(true)
        });

        let subscriptions = vec![
            cx.subscribe_in(&base_select, window, |this, _state, event, window, cx| {
                if let ComboboxEvent::Change(values) = event
                    && let Some(name) = values.first()
                {
                    this.base = name.clone();
                    this.reload_compare(window, cx);
                }
            }),
            cx.subscribe_in(
                &compare_select,
                window,
                |this, _state, event, window, cx| {
                    if let ComboboxEvent::Change(values) = event
                        && let Some(name) = values.first()
                    {
                        this.compare = name.clone();
                        this.reload_compare(window, cx);
                    }
                },
            ),
        ];

        cx.defer_in(window, |this, window, cx| {
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

        Self {
            focus_handle: cx.focus_handle(),
            dock_area,
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
            scroll_handle: VirtualListScrollHandle::new(),
            item_sizes: Rc::new(Vec::new()),
            _subscriptions: subscriptions,
        }
    }

    fn has_source(&self) -> bool {
        self.repo_path.is_some() || self.fork.is_some()
    }

    /// The path git ops run against.
    ///
    /// The target's mirror in fork mode, the user's checkout otherwise.
    fn work_path(&self) -> Option<PathBuf> {
        match &self.fork {
            Some(fork) => Some(fork.mirror_path.clone()),
            None => self.repo_path.clone(),
        }
    }

    /// The full ref the selected base branch resolves to.
    /// The mirror's remote-tracking ref in fork mode.
    ///
    /// The plain branch name in checkout mode, git resolves it through `refs/heads`.
    fn base_ref(&self) -> String {
        match &self.fork {
            Some(_) => ForkCompare::base_ref(&self.base),
            None => self.base.to_string(),
        }
    }

    /// The full ref the selected compare branch resolves to.
    /// The imported `refs/fork/<namespace>` ref in fork mode.
    ///
    /// The plain branch name in checkout mode.
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
                // `Ok(Ok(Some(paths)))` means the user picked a folder.
                // A cancel or picker failure resolves to anything else.
                let picked = match prompt.await {
                    Ok(Ok(Some(mut paths))) => paths.pop(),
                    _ => None,
                };

                let Some(path) = picked else {
                    return Ok(());
                };

                this.update_in(cx, |this, window, cx| {
                    this.apply_folder_path(path, window, cx);
                })?;

                Ok(())
            });
        task.detach();
    }

    /// Branches and the current branch are read off the UI thread, then applied.
    fn apply_folder_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let path = path.to_string_lossy().to_string();

        let task: gpui::Task<Result<(), anyhow::Error>> =
            cx.spawn_in(window, async move |this, cx| {
                // Branches and the current branch are read off the UI thread.
                let info = cx
                    .background_spawn({
                        let path = path.clone();
                        async move {
                            let repo = gix::open(Path::new(&path)).ok()?;
                            let branches =
                                signed_git::worktree_branches(Path::new(&path)).unwrap_or_default();
                            let current = signed_git::current_branch(&repo).ok().flatten();
                            Some((branches, current))
                        }
                    })
                    .await;

                this.update_in(cx, |this, window, cx| {
                    this.apply_checkout(path, info, window, cx);
                })?;

                Ok(())
            });
        task.detach();
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

        // Defaults, the announced HEAD branch when the checkout has it.
        // Falling back to `main`, then the first branch.
        // The checkout's current branch is the compare side default.
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

        // Remember this folder as a checkout of the target repository.
        // The next panel pre-fills it.
        if let Some(addr) = self.store.read(cx).addr().cloned() {
            let checkout_store = CheckoutsStore::global(cx);
            checkout_store.update(cx, |store, cx| {
                store.record(PathBuf::from(&path), addr, cx);
            });
        }

        let branches = self.branches.clone();
        let base = SharedString::from(base.clone());
        let compare = SharedString::from(compare.clone());

        self.base = base.clone();
        self.compare = compare.clone();

        self.base_select.update(cx, |state, cx| {
            state.set_items(SearchableVec::from(branches.clone()), window, cx);
            state.set_selected_values(&[base], window, cx);
        });

        self.compare_select.update(cx, |state, cx| {
            state.set_items(SearchableVec::from(branches), window, cx);
            state.set_selected_values(&[compare], window, cx);
        });

        self.reload_compare(window, cx);
    }

    /// Used to find fork candidates. `None` while the repository is not announced.
    fn base_repo(&self, cx: &App) -> Option<(RepoAddr, Option<String>)> {
        let store = self.store.read(cx);
        let addr = store.addr()?.clone();
        let euc = store.announcement.as_ref().and_then(|a| a.euc.clone());
        Some((addr, euc))
    }

    /// Announced forks of the target repository a compare can use, own first.
    ///
    /// Re-read whenever the picker opens.
    fn fork_candidates(&self, cx: &App) -> Vec<Announcement> {
        let Some((base, euc)) = self.base_repo(cx) else {
            return Vec::new();
        };
        let user = Backend::global(cx).read(cx).current_user();
        let announcements = RepoListStore::global(cx).read(cx).announcements.clone();
        fork_candidates(&announcements, &base, euc.as_deref(), user)
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
        let refresh = self
            .fork
            .as_ref()
            .is_some_and(|fork| fork.announcement.addr() == announcement.addr());

        let Some((base, _euc)) = self.base_repo(cx) else {
            return;
        };
        let cache = GitStore::global(cx).cache().clone();
        let mirror_path = cache.repo_path(&base);
        let namespace = fork_namespace(&announcement);
        let clone_urls = announcement.clone.clone();

        let base_clone_urls: Vec<Url> = self
            .store
            .read(cx)
            .announcement
            .as_ref()
            .map(|a| a.clone.clone())
            .unwrap_or_default();

        // Keep the current compare and base when the fork is already applied.
        // apply_fork drops them when the branch no longer exists.
        let keep_compare = refresh.then(|| self.compare.clone());
        let keep_base = refresh.then(|| self.base.clone());

        // The fork applied when the fetch started.
        // A source switch mid-flight must not let the stale result clobber the newer state.
        let expected_fork = self.fork.as_ref().map(|fork| fork.announcement.addr());

        self.loading = true;
        self.error = None;
        cx.notify();

        let task: gpui::Task<Result<(), anyhow::Error>> =
            cx.spawn_in(window, async move |this, cx| {
                // The fork and base must share history for a merge-base to exist.
                // The target's mirror is the object store both sides land in.
                // `ensure_clone` fetches `origin` when the mirror already exists.
                let result = cx
                    .background_spawn({
                        let cache = cache.clone();
                        let base = base.clone();
                        let base_clone_urls = base_clone_urls.clone();
                        let namespace = namespace.clone();
                        let clone_urls = clone_urls.clone();
                        let mirror_path = mirror_path.clone();
                        async move {
                            cache.ensure_clone(&base, &base_clone_urls)?;

                            // Prune stale imports of any fork.
                            // Then import this fork's heads under its namespace.
                            delete_refs_with_prefix(&mirror_path, "refs/fork")?;

                            fetch_repo_refs(
                                &mirror_path,
                                &clone_urls,
                                &format!("+refs/heads/*:refs/fork/{namespace}/*"),
                            )?;

                            // Both branch lists are short names, sorted like the checkout's.
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
                                refs_with_prefix(&mirror_path, "refs/remotes/origin")?,
                                "refs/remotes/origin",
                            );

                            let compare_branches = strip(
                                refs_with_prefix(&mirror_path, &format!("refs/fork/{namespace}"))?,
                                &format!("refs/fork/{namespace}"),
                            );

                            Ok::<_, anyhow::Error>((base_branches, compare_branches))
                        }
                    })
                    .await;

                this.update_in(cx, |this, window, cx| {
                    // A source switch mid-flight discards the stale result.
                    // E.g. the user picked a folder while the fork was fetching.
                    let applied = this.fork.as_ref().map(|fork| fork.announcement.addr());
                    if applied != expected_fork {
                        this.loading = false;
                        cx.notify();
                        return;
                    }

                    this.apply_fork(
                        announcement,
                        mirror_path,
                        namespace,
                        result,
                        keep_base,
                        keep_compare,
                        window,
                        cx,
                    );
                })?;

                Ok(())
            });
        task.detach();
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_fork(
        &mut self,
        announcement: Announcement,
        mirror_path: PathBuf,
        namespace: String,
        result: Result<(Vec<String>, Vec<String>), anyhow::Error>,
        keep_base: Option<SharedString>,
        keep_compare: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.loading = false;

        let (base_branches, compare_branches) = match result {
            Ok(branches) => branches,
            Err(error) => {
                // Keep the previous source, if any.
                // The error shows inline next to the compare bar.
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
            self.error =
                Some("Could not list the target repository's branches; try again later".into());
            cx.notify();
            return;
        }

        let base_branches: Vec<SharedString> =
            base_branches.into_iter().map(SharedString::from).collect();

        let compare_branches: Vec<SharedString> = compare_branches
            .into_iter()
            .map(SharedString::from)
            .collect();

        // Base defaults to the announced HEAD branch when the mirror has it.
        // Otherwise `main`, then the first branch.
        // The fork's `main` is the compare default, else the first branch.
        // A refresh keeps the previous selection when the branch still exists.
        let announced = self.store.read(cx).head.clone();
        let contains =
            |name: &str, list: &[SharedString]| list.iter().any(|branch| branch.as_ref() == name);

        let keep_base = keep_base.filter(|name| contains(name, &base_branches));
        let keep_compare = keep_compare.filter(|name| contains(name, &compare_branches));

        let base = keep_base
            .or_else(|| {
                announced
                    .as_ref()
                    .filter(|branch| contains(branch, &base_branches))
                    .map(SharedString::from)
            })
            .or_else(|| {
                base_branches
                    .iter()
                    .find(|branch| branch.as_ref() == "main")
                    .cloned()
            })
            .unwrap_or_else(|| base_branches[0].clone());

        let compare = keep_compare
            .or_else(|| {
                compare_branches
                    .iter()
                    .find(|branch| branch.as_ref() == "main")
                    .cloned()
            })
            .unwrap_or_else(|| compare_branches[0].clone());

        self.fork = Some(ForkCompare {
            announcement,
            namespace,
            mirror_path,
        });

        self.error = None;
        self.base = base.clone();
        self.compare = compare.clone();

        self.base_select.update(cx, |state, cx| {
            state.set_items(SearchableVec::from(base_branches), window, cx);
            state.set_selected_values(&[base], window, cx);
        });

        self.compare_select.update(cx, |state, cx| {
            state.set_items(SearchableVec::from(compare_branches), window, cx);
            state.set_selected_values(&[compare], window, cx);
        });

        self.reload_compare(window, cx);
    }

    fn reload_compare(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo_path) = self.work_path() else {
            return;
        };

        let base = self.base_ref();
        let compare = self.compare_ref();

        // Short names for the error copy, the full refs go to git.
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

        let task: gpui::Task<Result<(), anyhow::Error>> =
            cx.spawn_in(window, async move |this, cx| {
                let result = cx
                    .background_spawn({
                        let repo_path = repo_path.clone();
                        let base = base.clone();
                        let compare = compare.clone();
                        let base_name = base_name.clone();
                        let compare_name = compare_name.clone();
                        async move {
                            let merge_base = merge_base(Path::new(&repo_path), &base, &compare)?
                                .ok_or_else(|| {
                                    anyhow::anyhow!(
                                        "{base_name} and {compare_name} share no common ancestor"
                                    )
                                })?;
                            let commits = worktree_commit_range_commits(
                                Path::new(&repo_path),
                                &merge_base,
                                &compare,
                            )?;
                            let diff = worktree_commit_range_diff(
                                Path::new(&repo_path),
                                &merge_base,
                                &compare,
                            )?;
                            Ok::<_, anyhow::Error>((merge_base, commits, diff))
                        }
                    })
                    .await;

                this.update_in(cx, |this, _window, cx| {
                    // A stale result, branches changed mid-flight, must not clobber a newer compare.
                    if generation != this.compare_generation {
                        return;
                    }
                    this.loading = false;

                    match result {
                        Ok((merge_base, commits, diff)) => {
                            this.merge_base = Some(merge_base);
                            let count = commits.len();
                            this.item_sizes =
                                Rc::new(vec![size(px(0.), px(COMMIT_ROW_HEIGHT)); count]);
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

        task.detach();
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

        // The published `branch-name` is the compare branch's short name.
        let branch_name = self.compare.to_string();

        // The patch comes from the compare ref.
        // Plain branch name in checkout mode, imported `refs/fork/…` ref in fork mode.
        let compare_ref = self.compare_ref();
        let store = self.store.clone();
        let dock_area = self.dock_area.clone();
        let entity = cx.entity().clone();

        self.submitting = true;
        self.error = None;
        cx.notify();

        let task: gpui::Task<Result<(), anyhow::Error>> =
            cx.spawn_in(window, async move |this, cx| {
                // Regenerate the series at submit time.
                // The published patch covers the current tip of the compare branch.
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
                    this.update_in(cx, |this, _window, cx| {
                        this.submitting = false;
                        this.error = Some(error.to_string().into());
                        cx.notify();
                    })?;
                    return Ok(());
                }

                this.update_in(cx, |this, window, cx| {
                    this.submitting = false;

                    // Close the panel once the publish is underway.
                    cx.defer_in(window, {
                        let dock_area = dock_area.clone();
                        let entity = entity.clone();
                        move |_, window, cx| {
                            if let Some(dock_area) = dock_area.upgrade() {
                                dock_area.update(cx, |dock, cx| {
                                    dock.remove_panel(entity, window, cx);
                                });
                            }
                        }
                    });

                    cx.notify();
                })?;

                Ok(())
            });

        task.detach();
    }

    fn open_commit_diff(&mut self, commit_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo_path) = self.work_path() else {
            return;
        };
        let Some(dock_area) = self.dock_area.upgrade() else {
            return;
        };

        let panel = cx.new(|cx| {
            CommitDiffView::new(
                repo_path,
                self.repo_name.clone(),
                commit_id.into(),
                window,
                cx,
            )
        });

        dock_area.update(cx, |dock_area, cx| {
            add_center_panel(dock_area, panel_handle(panel), window, cx);
        });
    }

    fn render_compare_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let has_source = self.has_source();
        let can_submit = has_source
            && !self.loading
            && !self.submitting
            && self.merge_base.is_some()
            && self
                .commits
                .as_ref()
                .is_some_and(|commits| !commits.is_empty())
            && !self.subject.read(cx).value().is_empty();

        // Source-picker data snapshotted when the menu is built.
        // Each open rebuilds the items from the live announcements.
        let source_menu = self.source_menu(cx);
        let source_label = self.source_trigger();
        let source_tooltip = match &self.fork {
            Some(fork) => {
                format!(
                    "Comparing against {}",
                    fork_display_name(&fork.announcement)
                )
            }
            None => self.repo_path.as_ref().map_or_else(
                || "Choose a compare source".into(),
                |p| p.display().to_string(),
            ),
        };

        let refresh_fork = self.fork.as_ref().map(|fork| fork.announcement.clone());

        h_flex()
            .px_4()
            .h_16()
            .w_full()
            .gap_2()
            .items_end()
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(cx.theme().muted_foreground)
                            .child("Merge Into"),
                    )
                    .child(
                        div().w(px(140.)).child(
                            Combobox::new(&self.base_select)
                                .placeholder("branch")
                                .appearance(false)
                                .menu_width(px(220.))
                                .disabled(!has_source)
                                .bg(cx.theme().muted)
                                .rounded(cx.theme().radius)
                                .render_trigger(|ctx, _window, cx| {
                                    ref_selector_trigger(ctx, CustomIconName::GitBranch, cx)
                                }),
                        ),
                    ),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(cx.theme().muted_foreground)
                            .child("Pull From"),
                    )
                    .child(
                        div().w(px(140.)).child(
                            Combobox::new(&self.compare_select)
                                .placeholder("branch")
                                .appearance(false)
                                .menu_width(px(220.))
                                .disabled(!has_source)
                                .bg(cx.theme().muted)
                                .rounded(cx.theme().radius)
                                .render_trigger(|ctx, _window, cx| {
                                    ref_selector_trigger(ctx, CustomIconName::GitBranch, cx)
                                }),
                        ),
                    ),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(cx.theme().muted_foreground)
                            .child("Source"),
                    )
                    .child(
                        Button::new("compare-source")
                            .ghost()
                            .w(px(190.))
                            .child(div().text_sm().child(source_label))
                            .dropdown_caret(true)
                            .tooltip(source_tooltip)
                            .dropdown_menu(source_menu),
                    ),
            )
            .when_some(refresh_fork, |this, fork| {
                this.child(
                    v_flex().gap_1().child(div()).child(
                        Button::new("refresh-fork")
                            .icon(CustomIconName::Refresh)
                            .ghost()
                            .tooltip("Re-fetch the fork")
                            .on_click(cx.listener(move |this, _event, window, cx| {
                                this.choose_fork(fork.clone(), window, cx);
                            })),
                    ),
                )
            })
            .child(div().flex_1())
            .child(
                Button::new("create-pr")
                    .primary()
                    .icon(IconName::Plus)
                    .loading(self.submitting)
                    .disabled(!can_submit)
                    .on_click(cx.listener(|this, _event, window, cx| {
                        this.submit(window, cx);
                    })),
            )
            .into_any_element()
    }

    fn source_trigger(&self) -> SharedString {
        match &self.fork {
            Some(fork) => truncate_label(&fork_display_name(&fork.announcement)),
            None => self.repo_path.as_ref().map_or_else(
                || SharedString::from("No source"),
                |path| truncate_label(&path.display().to_string()),
            ),
        }
    }

    fn source_menu(
        &self,
        cx: &Context<Self>,
    ) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
        let view = cx.entity().downgrade();
        let associated = self
            .store
            .read(cx)
            .addr()
            .map(|addr| CheckoutsStore::global(cx).read(cx).associations_of(addr))
            .unwrap_or_default();

        let active_path = (self.fork.is_none())
            .then(|| self.repo_path.clone())
            .flatten();

        let candidates = self.fork_candidates(cx);
        let user = Backend::global(cx).read(cx).current_user();
        let active_fork = self.fork.as_ref().map(|fork| fork.announcement.addr());

        move |mut menu, _window, _cx| {
            for path in &associated {
                menu = menu.item(checkout_source_item(
                    view.clone(),
                    path.clone(),
                    active_path.as_ref() == Some(path),
                ));
            }

            menu = menu.item(choose_folder_source_item(view.clone()));
            menu = menu.item(PopupMenuItem::separator());

            if candidates.is_empty() {
                menu = menu.item(PopupMenuItem::label(
                    "No announced forks of this repository",
                ));
            } else {
                for candidate in candidates.iter() {
                    let subtitle: SharedString = if Some(candidate.owner) == user {
                        "Your fork".into()
                    } else {
                        SharedString::from(format!("by {}", shorten_owner(&candidate.owner)))
                    };
                    menu = menu.item(fork_source_item(
                        view.clone(),
                        candidate.clone(),
                        subtitle,
                        active_fork == Some(candidate.addr()),
                    ));
                }
            }

            menu
        }
    }

    fn render_inputs(&self, _cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .px_4()
            .w_full()
            .gap_2()
            .child(Input::new(&self.subject))
            .child(Textarea::new(&self.description).h_24())
            .into_any_element()
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let files = self.pane.read(cx).diff().map_or(0, |diff| diff.files.len());
        let commits = self.commits.as_ref().map_or(0, |commits| commits.len());

        h_flex()
            .px_4()
            .pb_4()
            .w_full()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(cx.theme().border)
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
                    .child(CountBadge::new(files))
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
                    .child(CountBadge::new(commits))
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
            .into_any_element()
    }

    fn render_content(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.loading {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .child(Spinner::new().small())
                .into_any_element();
        }
        if !self.has_source() {
            return placeholder(
                "Choose a local checkout or an announced fork to compare",
                cx,
            );
        }
        if self.commits.is_none() && self.error.is_some() {
            return placeholder("Nothing to compare", cx);
        }
        match self.active_tab {
            0 => self.pane.clone().into_any_element(),
            _ => self.render_commits_tab(cx),
        }
    }

    fn render_commits_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(commits) = self.commits.as_ref() else {
            return placeholder("No commits", cx);
        };
        if commits.is_empty() {
            return placeholder("No commits between the branches", cx);
        }

        let view = cx.entity().clone();
        let sizes = self.item_sizes.clone();
        let scroll_handle = self.scroll_handle.clone();

        v_flex()
            .relative()
            .flex_1()
            .w_full()
            .min_h_0()
            .child(
                v_virtual_list(
                    view,
                    "pr-commits",
                    sizes,
                    move |this, range, _window, cx| {
                        let commits = this.commits.as_deref().unwrap_or(&[]);
                        let view = cx.entity().downgrade();
                        range
                            .map(|ix| {
                                let id = commits[ix].id.clone();
                                let view = view.clone();
                                commit_row(
                                    ix,
                                    &commits[ix],
                                    move |window, cx| {
                                        if let Some(view) = view.upgrade() {
                                            view.update(cx, |this, cx| {
                                                this.open_commit_diff(&id, window, cx)
                                            });
                                        }
                                    },
                                    cx,
                                )
                            })
                            .collect()
                    },
                )
                .track_scroll(&scroll_handle)
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .child(Scrollbar::vertical(&scroll_handle)),
            )
            .into_any_element()
    }
}

pub(crate) fn open_new_pull_panel(
    dock_area: WeakEntity<DockArea>,
    store: Entity<RepoStore>,
    window: &mut Window,
    cx: &mut App,
) {
    let panel = cx.new(|cx| NewPullRequestView::new(dock_area.clone(), store, window, cx));

    let _ = dock_area.update(cx, |dock_area, cx| {
        add_center_panel(dock_area, panel_handle(panel), window, cx);
    });
}

impl BasePanel for NewPullRequestView {
    fn panel_name(&self) -> &'static str {
        "new-pull-request"
    }
}

impl Panel for NewPullRequestView {
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().child(SharedString::from(format!(
            "{}/new-pull-request",
            self.repo_name
        )))
    }
}

impl EventEmitter<PanelEvent> for NewPullRequestView {}

impl Focusable for NewPullRequestView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for NewPullRequestView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("new-pr")
            .size_full()
            .child(
                v_flex()
                    .gap_4()
                    .child(self.render_compare_bar(cx))
                    .child(self.render_inputs(cx))
                    .when_some(self.error.clone(), |this, error| {
                        this.child(
                            h_flex()
                                .px_4()
                                .py_1()
                                .w_full()
                                .text_xs()
                                .text_color(cx.theme().danger)
                                .child(error),
                        )
                    })
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
