use std::path::PathBuf;
use std::rc::Rc;

use assets::CustomIconName;
use dock::{BasePanel, DockArea, DockPlacement, Panel, PanelEvent, panel_handle};
use gpui::prelude::*;
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, Pixels, Render,
    SharedString, Size, Subscription, WeakEntity, Window, div, px, relative, size,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::clipboard::Clipboard;
use gpui_component::dialog::{DialogDescription, DialogFooter, DialogHeader, DialogTitle};
use gpui_component::form::{field, v_form};
use gpui_component::input::{Textarea, TextareaState};
use gpui_component::scroll::{ScrollableElement, Scrollbar};
use gpui_component::spinner::Spinner;
use gpui_component::tab::{Tab, TabBar};
use gpui_component::{
    ActiveTheme, Sizable, StyledExt, VirtualListScrollHandle, WindowExt, h_flex, v_flex,
    v_virtual_list,
};
use nostr::prelude::{Event, EventId, Kind, Url};
use signed_core::{
    RepoAddr, activity_subject, branch_name_of, clone_urls_of, current_commit_of, latest_update,
    merge_base_of, pull_request_patch,
};
use signed_git::{FileCommit, patch_commits, patch_diffs};
use signed_state::{Backend, GitStore, ProfileStore, RepoStore};
use signed_ui::{CountBadge, UserAvatar, placeholder, status_badge};
use utils::{relative_time, relative_time_secs};

use crate::views::commit_diff::{CommitDiffView, DiffPane};
use crate::views::discussion::{comment_form, comments_section, pr_roots, sidebar_section};

const ROW_HEIGHT: f32 = 37.;

/// Shown once the store's first pass is applied and the root PR is still absent.
const NOT_FOUND: &str = "Pull request not found";

/// A store refresh re-binds the panel, and reloads only when these change.
#[derive(Clone, PartialEq, Eq)]
struct PrBinding {
    description: String,
    patch: String,
    tip: Option<String>,
    base: Option<String>,
    clone_urls: Vec<Url>,
    addr: RepoAddr,
    has_patch_link: bool,
}

/// Detail panel of a single pull request.
pub struct PullRequestDetailView {
    focus_handle: FocusHandle,
    dock_area: WeakEntity<DockArea>,
    store: Entity<RepoStore>,
    /// Event id of the root PR event, kind 1618. Updates are revisions.
    pr_id: EventId,
    comment_input: Entity<TextareaState>,
    repo_name: SharedString,
    /// Local clone the PR's git changes come from.
    worktree: Option<PathBuf>,
    description: SharedString,
    /// Tip commit of the PR, from the latest update's `c` tag or the root.
    current_commit: Option<SharedString>,
    /// Commits of the patch series, in patch order, oldest first.
    commits: Vec<FileCommit>,
    /// The patch is being parsed on a background task.
    loading: bool,
    error: Option<SharedString>,
    /// Root PR inputs the in-flight diff load was started for.
    bound: Option<PrBinding>,
    /// Generation of the in-flight diff load. Stale results are discarded.
    load_generation: u64,
    /// 0 = Discussion, 1 = Files, 2 = Commits.
    active_tab: usize,
    pane: Entity<DiffPane>,
    commit_item_sizes: Rc<Vec<Size<Pixels>>>,
    commit_scroll_handle: VirtualListScrollHandle,
    /// The dock caches item panels, so without this observer a panel opened
    /// before the store loaded would stay on its placeholder.
    _subscription: Subscription,
}

impl PullRequestDetailView {
    pub fn new(
        dock_area: WeakEntity<DockArea>,
        store: Entity<RepoStore>,
        pr_id: EventId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let repo_name = store.read(cx).name();
        let pane = cx.new(DiffPane::new);

        let comment_input =
            cx.new(|cx| TextareaState::new(window, cx).placeholder("Leave a comment..."));

        let subscription = cx.observe(&store, |this, _store, cx| this.sync(cx));

        // Defer loading until the window is ready, like the commit diff view.
        cx.defer_in(window, |this, _window, cx| {
            this.sync(cx);
        });

        Self {
            focus_handle: cx.focus_handle(),
            dock_area,
            store,
            pr_id,
            comment_input,
            repo_name,
            worktree: None,
            description: SharedString::default(),
            current_commit: None,
            commits: Vec::new(),
            loading: true,
            error: None,
            bound: None,
            load_generation: 0,
            active_tab: 0,
            pane,
            commit_item_sizes: Rc::new(Vec::new()),
            commit_scroll_handle: VirtualListScrollHandle::new(),
            _subscription: subscription,
        }
    }

    /// Snapshot the root PR from the store and reload the diff when it changed.
    ///
    /// Re-runs on construction and on every store refresh. Item panels are
    /// cached by the dock, so this is the only way a panel opened before the
    /// store's first pass learns about its PR.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let loaded = self.store.read(cx).loaded;

        let binding = {
            let store = self.store.read(cx);

            store.addr().and_then(|addr| {
                store
                    .pull_requests
                    .iter()
                    .find(|pr| pr.id == self.pr_id && pr.kind == Kind::GitPullRequest)
                    .map(|root| {
                        let update = latest_update(store.pull_requests.iter(), root);

                        let tip = update
                            .and_then(current_commit_of)
                            .or_else(|| current_commit_of(root));

                        let base = update
                            .and_then(merge_base_of)
                            .or_else(|| merge_base_of(root));

                        let clone_urls = clone_urls_of(root)
                            .or_else(|| store.announcement.as_ref().map(|a| a.clone.clone()))
                            .unwrap_or_default();

                        PrBinding {
                            description: root.content.clone(),
                            patch: pull_request_patch(root, store.patches.iter()),
                            tip,
                            base,
                            clone_urls,
                            addr: addr.clone(),
                            has_patch_link: root.tags.event_ids().next().is_some(),
                        }
                    })
            })
        };

        let Some(binding) = binding else {
            self.sync_missing(loaded, cx);
            return;
        };

        if self.bound.as_ref() == Some(&binding) {
            return;
        }

        self.bound = Some(binding.clone());
        self.load_diff(binding, cx);
    }

    /// The store does not hold the root PR yet, or at all.
    ///
    /// Loading until the first pass is applied, not found afterwards.
    fn sync_missing(&mut self, loaded: bool, cx: &mut Context<Self>) {
        self.bound = None;

        if !loaded {
            if !self.loading || self.error.is_some() {
                self.loading = true;
                self.error = None;
                cx.notify();
            }
            return;
        }

        if self.error.as_deref() != Some(NOT_FOUND) {
            self.loading = false;
            self.error = Some(NOT_FOUND.into());
            cx.notify();
        }
    }

    /// Load the bound PR's changed files and commits.
    ///
    /// Nostr-backed pull requests parse the patch series, git-backed ones fetch
    /// the clone and diff the `merge-base..tip` range.
    fn load_diff(&mut self, binding: PrBinding, cx: &mut Context<Self>) {
        self.loading = true;
        self.error = None;
        self.description = binding.description.clone().into();
        self.current_commit = binding.tip.clone().map(SharedString::from);
        cx.notify();

        let cache = GitStore::global(cx).cache().clone();

        self.load_generation = self.load_generation.wrapping_add(1);
        let generation = self.load_generation;

        let PrBinding {
            patch,
            tip,
            base,
            clone_urls,
            addr,
            has_patch_link,
            ..
        } = binding;

        let task: gpui::Task<Result<(), anyhow::Error>> = cx.spawn(async move |this, cx| {
            let nostr_diff = cx
                .background_spawn({
                    let patch = patch.clone();
                    async move { patch_diffs(&patch) }
                })
                .await;

            let nostr_commits = cx
                .background_spawn({
                    let patch = patch.clone();
                    async move { patch_commits(&patch) }
                })
                .await;

            // PRs without patch events, e.g. published by ngit, carry their changes in git.
            // Fetch the clone and diff the `merge-base..tip` range.
            let use_nostr = match &nostr_diff {
                Ok(diff) => has_patch_link || !diff.files.is_empty(),
                Err(_) => true,
            };

            let git = if use_nostr {
                None
            } else {
                let cache = cache.clone();
                let addr = addr.clone();
                let clone_urls = clone_urls.clone();
                let base = base.clone();
                let tip = tip.clone();

                Some(
                    cx.background_spawn(async move {
                        let repo = cache.ensure_clone(&addr, &clone_urls)?;

                        let workdir = repo
                            .workdir()
                            .ok_or_else(|| anyhow::anyhow!("repository has no worktree"))?
                            .to_path_buf();

                        let tip =
                            tip.ok_or_else(|| anyhow::anyhow!("pull request has no tip commit"))?;

                        let base = match base {
                            Some(base) => base,
                            // No `merge-base` tag. Use the merge base of the tip and the default branch.
                            None => {
                                let head = repo
                                    .head_id()
                                    .map_err(|_| anyhow::anyhow!("repository has no HEAD"))?;
                                let tip_id = repo.rev_parse_single(tip.as_bytes())?;
                                repo.merge_base(tip_id, head)?.to_string()
                            }
                        };

                        let diff = signed_git::worktree_commit_range_diff(&workdir, &base, &tip)?;
                        let commits =
                            signed_git::worktree_commit_range_commits(&workdir, &base, &tip)?;

                        Ok::<_, anyhow::Error>((diff, commits, workdir))
                    })
                    .await,
                )
            };

            let (diff, commits, worktree) = match git {
                Some(Ok((diff, commits, worktree))) => (Ok(diff), commits, Some(worktree)),
                Some(Err(error)) => (Err(error), Vec::new(), None),
                None => (nostr_diff, nostr_commits, None),
            };

            this.update(cx, |this, cx| {
                // A newer binding superseded this load.
                if this.load_generation != generation {
                    return;
                }

                this.loading = false;
                this.worktree = worktree;
                this.commit_item_sizes = Rc::new(vec![size(px(0.), px(ROW_HEIGHT)); commits.len()]);
                this.commits = commits;

                match diff {
                    Ok(diff) => {
                        this.pane.update(cx, |pane, cx| pane.set_diff(diff, cx));
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

    /// Open the diff of `commit_id` in the bottom dock of the area.
    fn open_commit_diff(
        &mut self,
        worktree: PathBuf,
        commit_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(dock_area) = self.dock_area.upgrade() else {
            return;
        };

        let panel = cx.new(|cx| {
            CommitDiffView::new(
                worktree,
                self.repo_name.clone(),
                commit_id.into(),
                window,
                cx,
            )
        });

        dock_area.update(cx, |dock_area, cx| {
            dock_area.add_panel_view(panel_handle(panel), DockPlacement::Bottom, None, window, cx);
        });
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let active = self.active_tab;
        let files_count = self.pane.read(cx).diff().map(|diff| diff.files.len());
        let commits_count = if self.commits.is_empty() {
            None
        } else {
            Some(self.commits.len())
        };

        TabBar::new("pr-tabs")
            .underline()
            .small()
            .px_4()
            .w_full()
            .selected_index(active)
            .on_click(cx.listener(|this, index, _window, cx| {
                this.active_tab = *index;
                cx.notify();
            }))
            .child(Tab::new().label("Discussion"))
            .child(
                Tab::new()
                    .label("Files")
                    .when_some(files_count, |this, count| {
                        this.suffix(CountBadge::new(count))
                    }),
            )
            .child(
                Tab::new()
                    .label("Commits")
                    .when_some(commits_count, |this, count| {
                        this.suffix(CountBadge::new(count))
                    }),
            )
            .into_any_element()
    }

    fn render_discussion(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if self.loading {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .child(Spinner::new().small())
                .into_any_element();
        }

        if let Some(error) = self.error.clone() {
            return placeholder(&error, cx);
        }

        let (author, picture, age, root_id) = {
            let store = self.store.read(cx);
            let Some(root) = store
                .pull_requests
                .iter()
                .find(|pr| pr.id == self.pr_id && pr.kind == Kind::GitPullRequest)
            else {
                return placeholder("Pull request not found", cx);
            };
            let profile = ProfileStore::global(cx).read(cx).get(&root.pubkey);
            (
                profile.name(),
                profile.picture(),
                relative_time(root.created_at),
                root.id,
            )
        };

        h_flex()
            .flex_1()
            .w_full()
            .min_h_0()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .p_4()
                    .gap_6()
                    .overflow_y_scrollbar()
                    .child(
                        v_flex()
                            .px_4()
                            .gap_8()
                            .child(
                                v_flex()
                                    .gap_4()
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .text_sm()
                                            .child(
                                                h_flex()
                                                    .gap_1()
                                                    .child(
                                                        UserAvatar::new(author.clone())
                                                            .picture(picture),
                                                    )
                                                    .child(author),
                                            )
                                            .child(SharedString::from("commented"))
                                            .child(
                                                div()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(SharedString::from(age)),
                                            ),
                                    )
                                    .when(!self.description.is_empty(), |this| {
                                        this.child(div().text_sm().child(self.description.clone()))
                                    }),
                            )
                            .child(comments_section(&self.store, root_id, cx))
                            .child(comment_form(
                                &self.store,
                                root_id,
                                pr_roots,
                                &self.comment_input,
                                "pr-comment",
                                cx,
                            )),
                    ),
            )
            .child(sidebar_section(&self.store, root_id, pr_roots, true, cx))
            .into_any_element()
    }

    fn render_files_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.loading {
            return v_flex()
                .flex_1()
                .w_full()
                .min_h_0()
                .items_center()
                .justify_center()
                .child(Spinner::new().small())
                .into_any_element();
        }

        if let Some(error) = self.error.clone() {
            return placeholder(&error, cx);
        }

        h_flex()
            .flex_1()
            .w_full()
            .min_h_0()
            .overflow_hidden()
            .child(self.pane.clone())
            .into_any_element()
    }

    fn render_commits_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.loading {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .child(Spinner::new().small())
                .into_any_element();
        }
        if let Some(error) = self.error.clone() {
            return placeholder(&error, cx);
        }
        if self.commits.is_empty() {
            return placeholder("No commits found", cx);
        }

        v_flex()
            .relative()
            .flex_1()
            .w_full()
            .min_h_0()
            .child(
                v_virtual_list(
                    cx.entity().clone(),
                    "pr-commits",
                    self.commit_item_sizes.clone(),
                    move |this, range, _window, cx| {
                        range
                            .map(|ix| this.render_commit_row(ix, &this.commits[ix], cx))
                            .collect()
                    },
                )
                .track_scroll(&self.commit_scroll_handle)
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .child(Scrollbar::vertical(&self.commit_scroll_handle)),
            )
            .into_any_element()
    }

    fn render_commit_row(
        &self,
        ix: usize,
        commit: &FileCommit,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let meta = commit_meta(commit);
        let id = commit.id.clone();

        h_flex()
            .id(ix)
            .px_4()
            .h(px(ROW_HEIGHT))
            .gap_2()
            .items_center()
            .text_sm()
            .border_b(px(1.))
            .border_color(cx.theme().border)
            .hover(|this| this.bg(cx.theme().list_hover))
            .child(
                div()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(commit.id.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(commit.summary.clone()),
            )
            .when(!meta.is_empty(), |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(meta)),
                )
            })
            // Commits parsed from the nostr patch set may not exist in any local clone.
            // Only git-backed PRs open a diff viewer.
            .when_some(self.worktree.clone(), |this, worktree| {
                this.on_click(cx.listener(move |this, _event, window, cx| {
                    this.open_commit_diff(worktree.clone(), &id, window, cx);
                }))
            })
            .into_any_element()
    }

    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let current_commit = self.current_commit.clone();
        let (title, status, branch, author) = {
            let store = self.store.read(cx);
            let Some(root) = store
                .pull_requests
                .iter()
                .find(|pr| pr.id == self.pr_id && pr.kind == Kind::GitPullRequest)
            else {
                return div().into_any_element();
            };
            (
                activity_subject(root),
                store.status_of(root),
                branch_name_of(root),
                root.pubkey,
            )
        };

        // Only the PR author may publish revisions, NIP-34 kind 1619.
        let backend = Backend::global(cx);
        let can_update = backend.read(cx).current_user() == Some(author);

        v_flex()
            .px_4()
            .mb_4()
            .child(
                h_flex()
                    .w_full()
                    .min_h_16()
                    .gap_2()
                    .child(status_badge(status, cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_semibold()
                            .line_height(relative(1.2))
                            .child(title),
                    ),
            )
            .child(
                h_flex()
                    .gap_3()
                    .when_some(branch, |this, branch| {
                        this.child(
                            Button::new("branch")
                                .ghost()
                                .small()
                                .icon(CustomIconName::GitBranch)
                                .label(branch),
                        )
                    })
                    .when(can_update, |this| {
                        this.child(
                            Button::new("update-pr")
                                .ghost()
                                .small()
                                .icon(CustomIconName::GitPullRequest)
                                .label("Update")
                                .tooltip("Publish a new revision of this pull request")
                                .on_click(cx.listener({
                                    let store = self.store.clone();
                                    let pr_id = self.pr_id;
                                    move |_this, _event, window, cx| {
                                        let root = store
                                            .read(cx)
                                            .pull_requests
                                            .iter()
                                            .find(|pr| {
                                                pr.id == pr_id && pr.kind == Kind::GitPullRequest
                                            })
                                            .cloned();
                                        if let Some(root) = root {
                                            open_update_pull_request_dialog(
                                                store.clone(),
                                                root,
                                                window,
                                                cx,
                                            );
                                        }
                                    }
                                })),
                        )
                    })
                    .when_some(current_commit, |this, id| {
                        this.child(
                            h_flex()
                                .gap_1()
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(id.clone())
                                .child(Clipboard::new("pr-commit").value(&id)),
                        )
                    }),
            )
            .into_any_element()
    }
}

fn open_update_pull_request_dialog(
    store: Entity<RepoStore>,
    root: Event,
    window: &mut Window,
    cx: &mut App,
) {
    let patch = cx.new(|cx| {
        TextareaState::new(window, cx).placeholder("Paste the updated `git format-patch` output...")
    });
    // Both the dialog body and submit button capture the root event.
    // Share it instead of cloning into each closure.
    let root = Rc::new(root);

    window.open_dialog(cx, move |dialog, _window, _cx| {
        let store = store.clone();
        let patch = patch.clone();
        let root = root.clone();

        dialog
            .width(px(520.))
            .margin_top(px(50.))
            .content(move |body, _window, _cx| {
                body.child(
                    DialogHeader::new()
                        .child(DialogTitle::new().child("Update pull request"))
                        .child(DialogDescription::new().child(
                            "Publish a new revision with the output of `git format-patch`.",
                        )),
                )
                .child(
                    v_form().child(
                        field()
                            .label("Patch")
                            .child(Textarea::new(&patch).h(px(160.))),
                    ),
                )
                .child(
                    DialogFooter::new().justify_end().child(
                        Button::new("submit")
                            .primary()
                            .label("Update pull request")
                            .tooltip("Update pull request")
                            .on_click({
                                let store = store.clone();
                                let patch = patch.clone();
                                let root = root.clone();
                                move |_event, window, cx| {
                                    let patch = patch.read(cx).value().to_string();
                                    store.update(cx, |store, cx| {
                                        store.update_pull_request(&root, patch, cx);
                                    });
                                    window.close_dialog(cx);
                                }
                            }),
                    ),
                )
            })
    });
}

/// One-line commit metadata for the commits list.
///
/// Author and relative time, whichever is available.
fn commit_meta(commit: &FileCommit) -> String {
    let author = commit.author.trim();
    let time = commit.time > 0;
    match (author.is_empty(), time) {
        (false, true) => format!("{author} · {}", relative_time_secs(commit.time)),
        (false, false) => author.to_string(),
        (true, true) => relative_time_secs(commit.time),
        (true, false) => String::new(),
    }
}

impl BasePanel for PullRequestDetailView {
    fn panel_name(&self) -> &'static str {
        "pull-request-detail"
    }
}

impl Panel for PullRequestDetailView {
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let hex = self.pr_id.to_hex();
        let id = SharedString::from(&hex[..8]);
        let title = if self.repo_name.is_empty() {
            id
        } else {
            SharedString::from(format!("{}/{}", self.repo_name, id))
        };
        div().text_sm().child(title)
    }
}

impl EventEmitter<PanelEvent> for PullRequestDetailView {}

impl Focusable for PullRequestDetailView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PullRequestDetailView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .image_cache(gpui::retain_all("pull-request-detail"))
            .id("pull-request-detail")
            .size_full()
            .min_h_0()
            .child(self.render_header(cx))
            .child(self.render_tabs(cx))
            .map(|this| match self.active_tab {
                0 => this.child(self.render_discussion(cx)),
                1 => this.child(self.render_files_tab(cx)),
                _ => this.child(self.render_commits_tab(cx)),
            })
    }
}
