use std::path::PathBuf;
use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, WeakEntity, Window, div, px,
};
use metadata::ProfileDirectory;
use nostr::prelude::{EventId, Kind, Url};
use signed_core::{GitEvent, PullRequest, RepoAddr};
use signed_git::{FileCommit, PatchParser};
use signed_state::{Backend, Mirrors, RepoStore};
use ui::avatar::Avatar;
use ui::badge::{Badge, BadgeVariant, GroupedBadge};
use ui::button::Button;
use ui::common::{Sizable as _, StyledExt as _, h_flex, v_flex};
use ui::empty_state::EmptyState;
use ui::icons::{Icon, IconRole};
use ui::input::InputState;
use ui::layout::panel::{Panel, PanelSlot, SplitSide};
use ui::layout::root::RootContext as _;
use ui::scroll_area::ScrollArea;
use ui::spinner::Spinner;
use ui::theme::ActiveTheme as _;
use utils::relative_time;

use crate::app::AppRoot;
use crate::issues::helpers::{comment_form, comments_section};
use crate::pull_requests::dialogs::update::UpdatePrDialog;
use crate::repo::commit_diff::{CommitDiffView, DiffPane};
use crate::repo::helpers::short_id;
use crate::shared::widgets::status_badge;
use crate::shared::{log_dropped, track_event_participants};

const NOT_FOUND: &str = "Pull request not found";
const SIDEBAR_ID: &str = "Discussion";
const SIDEBAR_WIDTH: Pixels = px(320.);

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

struct CommitRow {
    commit: FileCommit,
    insertions: usize,
    deletions: usize,
}

pub struct PullRequestDetailView {
    workspace: WeakEntity<AppRoot>,
    store: Entity<RepoStore>,
    pr_id: EventId,
    directory: Entity<ProfileDirectory>,
    worktree: Option<PathBuf>,
    description: SharedString,
    current_commit: Option<SharedString>,
    commits: Arc<Vec<CommitRow>>,
    loading: bool,
    error: Option<SharedString>,
    bound: Option<PrBinding>,
    load_generation: u64,
    pane: Entity<DiffPane>,
    scroll_handle: ScrollHandle,
    tasks: Vec<gpui::Task<Result<(), anyhow::Error>>>,
}

impl PullRequestDetailView {
    fn new(
        workspace: WeakEntity<AppRoot>,
        store: Entity<RepoStore>,
        pr_id: EventId,
        directory: Entity<ProfileDirectory>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let pane = cx.new(DiffPane::new);

        cx.defer_in(window, move |this, _window, cx| {
            this.sync(cx);
        });

        Self {
            workspace,
            store,
            pr_id,
            directory,
            worktree: None,
            description: SharedString::default(),
            current_commit: None,
            commits: Arc::new(Vec::new()),
            loading: true,
            error: None,
            bound: None,
            load_generation: 0,
            pane,
            scroll_handle: ScrollHandle::new(),
            tasks: Vec::new(),
        }
    }

    /// Opens the pull request in the main panel and its discussion in the split pane.
    pub fn open(
        workspace: &WeakEntity<AppRoot>,
        store: Entity<RepoStore>,
        pr_id: EventId,
        window: &mut Window,
        cx: &mut App,
    ) {
        let id = SharedString::from(format!("pr:{pr_id}"));
        let workspace_for_build = workspace.clone();
        let detail_store = store.clone();
        let directory = ProfileDirectory::new(cx);

        log_dropped(workspace.update(cx, |root, cx| {
            root.panels().update(cx, |panels, cx| {
                panels.open(
                    PanelSlot::Main,
                    id,
                    {
                        let directory = directory.clone();
                        move |window, cx| {
                            cx.new(|cx| {
                                Self::new(
                                    workspace_for_build.clone(),
                                    detail_store.clone(),
                                    pr_id,
                                    directory.clone(),
                                    window,
                                    cx,
                                )
                            })
                        }
                    },
                    window,
                    cx,
                );

                let sidebar = cx.new(|cx| {
                    PullRequestSidebarView::new(store.clone(), pr_id, directory.clone(), cx)
                });

                panels.insert_split_at(
                    SIDEBAR_ID,
                    SIDEBAR_WIDTH,
                    SplitSide::Trailing,
                    Arc::new(sidebar),
                    cx,
                );
            })
        }));
    }

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
                        let update = PullRequest::latest_update(store.pull_requests.iter(), root);
                        let tip = update
                            .and_then(GitEvent::current_commit)
                            .or_else(|| root.current_commit());

                        let base = update
                            .and_then(GitEvent::merge_base)
                            .or_else(|| root.merge_base());

                        let clone_urls = root
                            .clone_urls()
                            .or_else(|| store.announcement.as_ref().map(|a| a.clone.clone()))
                            .unwrap_or_default();

                        PrBinding {
                            description: root.content.clone(),
                            patch: PullRequest::new(root).patch(store.patches.iter()),
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
            self.bound = None;
            if !loaded {
                self.loading = true;
                self.error = None;
            } else if self.error.as_deref() != Some(NOT_FOUND) {
                self.loading = false;
                self.error = Some(NOT_FOUND.into());
            }
            cx.notify();
            return;
        };

        if self.bound.as_ref() == Some(&binding) {
            return;
        }

        self.bound = Some(binding.clone());
        self.load_diff(binding, cx);
    }

    fn load_diff(&mut self, binding: PrBinding, cx: &mut Context<Self>) {
        self.loading = true;
        self.error = None;
        self.description = binding.description.clone().into();
        self.current_commit = binding.tip.clone().map(SharedString::from);
        cx.notify();

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
                    async move { PatchParser::patch_diffs(&patch) }
                })
                .await;

            let nostr_commits = cx
                .background_spawn({
                    let patch = patch.clone();
                    async move {
                        PatchParser::patch_commits_with_stats(&patch)
                            .into_iter()
                            .map(|(commit, insertions, deletions)| CommitRow {
                                commit,
                                insertions,
                                deletions,
                            })
                            .collect::<Vec<_>>()
                    }
                })
                .await;

            let use_nostr = match &nostr_diff {
                Ok(diff) => has_patch_link || !diff.files.is_empty(),
                Err(_) => true,
            };

            let git = if use_nostr {
                None
            } else {
                let addr = addr.clone();
                let clone_urls = clone_urls.clone();
                let base = base.clone();
                let tip = tip.clone();
                Some(
                    cx.background_spawn(async move {
                        let repo = Mirrors::ensure(&addr, &clone_urls)?;
                        let gix_repo = repo.inner();

                        let workdir = gix_repo
                            .workdir()
                            .ok_or_else(|| anyhow::anyhow!("repository has no worktree"))?
                            .to_path_buf();

                        let tip =
                            tip.ok_or_else(|| anyhow::anyhow!("pull request has no tip commit"))?;

                        let base = match base {
                            Some(base) => base,
                            None => {
                                let head = gix_repo
                                    .head_id()
                                    .map_err(|_| anyhow::anyhow!("repository has no HEAD"))?;
                                let tip_id = gix_repo.rev_parse_single(tip.as_bytes())?;
                                gix_repo.merge_base(tip_id, head)?.to_string()
                            }
                        };

                        let diff = repo.range_diff(&base, &tip)?;
                        let commits = repo
                            .commit_range(&base, &tip)?
                            .into_iter()
                            .map(|commit| {
                                let (insertions, deletions) = match repo.commit_diff(&commit.id) {
                                    Ok(diff) => (
                                        diff.files.iter().map(|file| file.insertions).sum(),
                                        diff.files.iter().map(|file| file.deletions).sum(),
                                    ),
                                    Err(error) => {
                                        log::warn!(
                                            "failed to read stats for commit {}: {error}",
                                            commit.id
                                        );
                                        (0, 0)
                                    }
                                };
                                CommitRow {
                                    insertions,
                                    deletions,
                                    commit,
                                }
                            })
                            .collect::<Vec<_>>();

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
                if this.load_generation != generation {
                    return;
                }
                this.loading = false;
                this.worktree = worktree;
                this.commits = Arc::new(commits);

                match diff {
                    Ok(diff) => this.pane.update(cx, |pane, cx| pane.set_diff(diff, cx)),
                    Err(error) => this.error = Some(error.to_string().into()),
                }

                cx.notify();
            })?;

            Ok(())
        });

        self.tasks.push(task);
    }

    fn open_commit_diff(
        &mut self,
        worktree: PathBuf,
        commit_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = match self.store.read(cx).addr() {
            Some(addr) => SharedString::from(format!("commit:{addr}:{commit_id}")),
            None => SharedString::from(format!("commit:{}:{commit_id}", worktree.display())),
        };

        let workspace = self.workspace.clone();
        let commit_id = commit_id.to_owned();

        log_dropped(workspace.update(cx, |root, cx| {
            root.panels().clone().update(cx, |panels, cx| {
                panels.open(
                    PanelSlot::Split,
                    id,
                    move |window, cx| {
                        cx.new(|cx| {
                            CommitDiffView::new(
                                worktree.clone(),
                                commit_id.clone(),
                                false,
                                window,
                                cx,
                            )
                        })
                    },
                    window,
                    cx,
                )
            })
        }));
    }

    fn render_body(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.bound.is_none() {
            return div().into_any_element();
        }

        let description = if self.description.is_empty() {
            SharedString::from("No description provided.")
        } else {
            self.description.clone()
        };

        div()
            .flex_none()
            .border_b_1()
            .border_color(cx.theme().colors.border)
            .p_2()
            .text_sm()
            .child(description)
            .into_any_element()
    }

    fn render_commits(&self, cx: &mut Context<Self>) -> AnyElement {
        let count = self.commits.len();
        let worktree = self.worktree.clone();
        let view = cx.entity().downgrade();

        v_flex()
            .w_full()
            .flex_none()
            .child(
                h_flex()
                    .flex_none()
                    .h_10()
                    .px_2()
                    .gap_2()
                    .border_b_1()
                    .border_color(cx.theme().colors.border)
                    .child(Icon::named(IconRole::GitCommit))
                    .child(div().text_sm().font_semibold().child("Commits"))
                    .child(Badge::new("pr-commits-count", count.to_string())),
            )
            .when(count == 0, |this| {
                this.child(
                    div()
                        .p_2()
                        .text_sm()
                        .text_color(cx.theme().colors.muted_foreground)
                        .child("No commits found"),
                )
            })
            .children(self.commits.iter().map(|row| {
                let commit = &row.commit;
                let row_id = commit.id.clone();
                let worktree = worktree.clone();
                let view = view.clone();

                h_flex()
                    .id(commit.id.clone())
                    .px_2()
                    .h_12()
                    .w_full()
                    .gap_3()
                    .border_b_1()
                    .border_color(cx.theme().colors.border)
                    .hover(|this| this.bg(cx.theme().colors.muted_hover))
                    .child(
                        div()
                            .w_20()
                            .flex_none()
                            .text_sm()
                            .text_color(cx.theme().colors.muted_foreground)
                            .child(short_id(&commit.id)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(commit.summary.clone()),
                    )
                    .child(GroupedBadge::new(
                        format!("pr-commit-stats:{}", commit.id),
                        row.insertions,
                        row.deletions,
                    ))
                    .on_click(move |_, window, cx| {
                        if let Some(worktree) = worktree.clone() {
                            log_dropped(view.update(cx, |this, cx| {
                                this.open_commit_diff(worktree, &row_id, window, cx);
                            }));
                        }
                    })
                    .into_any_element()
            }))
            .into_any_element()
    }

    fn render_changes(&self, cx: &App) -> AnyElement {
        self.pane.read(cx).render_flow(self.pane.downgrade(), cx)
    }

    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let (title, status, branch, author, created_at, target_branch) = {
            let store = self.store.read(cx);
            let Some(root) = store
                .pull_requests
                .iter()
                .find(|pr| pr.id == self.pr_id && pr.kind == Kind::GitPullRequest)
            else {
                return div().into_any_element();
            };
            (
                root.activity_subject(),
                store.status_of(root),
                root.branch_name(),
                root.pubkey,
                root.created_at,
                store.head.clone(),
            )
        };

        let profile = self.directory.read(cx).profile(&author, cx);
        let author_name = profile.name();
        let author_picture = profile.picture();

        let backend = Backend::global(cx);
        let can_update = backend.read(cx).current_user() == Some(author);
        let weak = cx.entity().downgrade();
        let muted = cx.theme().colors.muted_foreground;

        v_flex()
            .flex_none()
            .p_2()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .font_semibold()
                    .text_ellipsis()
                    .child(title),
            )
            .child(
                h_flex()
                    .gap_2()
                    .text_sm()
                    .child(status_badge(status))
                    .child(div().text_color(muted).child("opened by"))
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Avatar::new(author)
                                    .name(author_name.clone())
                                    .picture(author_picture)
                                    .small(),
                            )
                            .child(author_name),
                    )
                    .child(
                        div()
                            .text_color(muted)
                            .child(SharedString::from(relative_time(created_at))),
                    )
                    .when_some(target_branch, |this, target| {
                        this.child(div().text_color(muted).child("targeting"))
                            .child(
                                Badge::new(format!("target-branch:{target}"), target)
                                    .variant(BadgeVariant::Outline)
                                    .small()
                                    .flex_none(),
                            )
                    })
                    .when_some(branch, |this, branch| {
                        this.child(div().text_color(muted).child("from")).child(
                            Badge::new(format!("source-branch:{branch}"), branch)
                                .variant(BadgeVariant::Outline)
                                .small()
                                .flex_none(),
                        )
                    })
                    .child(div().flex_1())
                    .when(can_update, |this| {
                        this.child(Button::new("update-pr").label("Update").small().on_click(
                            move |_, _, cx| {
                                log_dropped(weak.update(cx, |this, cx| {
                                    let root = this
                                        .store
                                        .read(cx)
                                        .pull_requests
                                        .iter()
                                        .find(|pr| {
                                            pr.id == this.pr_id && pr.kind == Kind::GitPullRequest
                                        })
                                        .cloned();

                                    if let Some(root) = root {
                                        let dialog =
                                            UpdatePrDialog::new(this.store.clone(), root, cx);
                                        cx.open_dialog(dialog);
                                    }
                                }));
                            },
                        ))
                    }),
            )
            .into_any_element()
    }
}

/// The pull request discussion, shown in the split pane.
struct PullRequestSidebarView {
    store: Entity<RepoStore>,
    pr_id: EventId,
    comment_input: Entity<InputState>,
    directory: Entity<ProfileDirectory>,
    scroll_handle: ScrollHandle,
    _subscription: Subscription,
}

impl PullRequestSidebarView {
    fn new(
        store: Entity<RepoStore>,
        pr_id: EventId,
        directory: Entity<ProfileDirectory>,
        cx: &mut Context<Self>,
    ) -> Self {
        let comment_input = InputState::builder().multiline().build(cx);
        let weak = cx.entity().downgrade();

        let subscription = cx.observe(&store, |this, _store, cx| {
            this.track_participants(cx);
            cx.notify();
        });

        cx.defer(move |cx| {
            log_dropped(weak.update(cx, |this, cx| {
                this.track_participants(cx);
            }));
        });

        Self {
            store,
            pr_id,
            comment_input,
            directory,
            scroll_handle: ScrollHandle::new(),
            _subscription: subscription,
        }
    }

    fn track_participants(&mut self, cx: &mut Context<Self>) {
        let store = self.store.read(cx);

        let author = store
            .pull_requests
            .iter()
            .find(|pr| pr.id == self.pr_id && pr.kind == Kind::GitPullRequest)
            .map(|root| root.pubkey);
        let owner = store.announcement.as_ref().map(|a| a.owner);

        let comments: Vec<_> = store
            .comments_of(&self.pr_id)
            .map(|comment| comment.pubkey)
            .collect();

        track_event_participants(author, owner, comments.into_iter(), &self.directory, cx);
    }
}

impl Panel for PullRequestSidebarView {
    fn title(&self, _cx: &App) -> SharedString {
        SharedString::from(SIDEBAR_ID)
    }
}

impl Render for PullRequestSidebarView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);

        let Some(root) = store
            .pull_requests
            .iter()
            .find(|pr| pr.id == self.pr_id && pr.kind == Kind::GitPullRequest)
        else {
            return if store.loaded {
                EmptyState::centered(NOT_FOUND)
            } else {
                Spinner::centered("pr-loading")
            };
        };
        let root_id = root.id;

        let discussion = ScrollArea::new("pr-discussion-scroll")
            .handle(self.scroll_handle.clone())
            .flex_1()
            .min_h_0()
            .child(v_flex().p_2().child(comments_section(
                &self.store,
                root_id,
                &self.directory,
                cx,
            )))
            .into_any_element();

        v_flex()
            .size_full()
            .min_h_0()
            .child(discussion)
            .child(div().flex_none().p_2().child(comment_form(
                &self.store,
                root_id,
                &store.pull_requests,
                &self.comment_input,
                "pr-comment",
                cx,
            )))
            .into_any_element()
    }
}

impl Panel for PullRequestDetailView {
    fn title(&self, cx: &App) -> SharedString {
        let store = self.store.read(cx);
        match store
            .pull_requests
            .iter()
            .find(|pr| pr.id == self.pr_id && pr.kind == Kind::GitPullRequest)
        {
            Some(root) => SharedString::from(root.activity_subject()),
            None => {
                let hex = self.pr_id.to_hex();
                let short = hex.get(..8).unwrap_or(&hex);
                SharedString::from(format!("PR {short}"))
            }
        }
    }

    fn prefix(&self, cx: &App) -> Option<AnyElement> {
        let owner = self.store.read(cx).announcement.as_ref().map(|a| a.owner)?;
        let profile = self.directory.read(cx).profile(&owner, cx);

        Some(
            Avatar::new(owner)
                .name(profile.name())
                .picture(profile.picture())
                .small()
                .into_any_element(),
        )
    }

    fn loading(&self, _cx: &App) -> bool {
        self.loading
    }
}

impl Render for PullRequestDetailView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.loading {
            return v_flex()
                .size_full()
                .child(Spinner::centered("pr-loading"))
                .into_any_element();
        }

        if let Some(error) = self.error.clone() {
            return v_flex()
                .size_full()
                .child(EmptyState::centered(error))
                .into_any_element();
        }

        ScrollArea::new("pr-detail-scroll")
            .handle(self.scroll_handle.clone())
            .size_full()
            .child(
                v_flex()
                    .id("pull-request-detail")
                    .w_full()
                    .child(self.render_header(cx))
                    .child(self.render_body(cx))
                    .child(self.render_commits(cx))
                    .child(self.render_changes(cx)),
            )
            .into_any_element()
    }
}
