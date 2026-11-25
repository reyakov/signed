use std::path::PathBuf;
use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StyleRefinement, Styled as _, Subscription,
    UniformListScrollHandle, WeakEntity, Window, div,
};
use metadata::ProfileDirectory;
use nostr::prelude::{EventId, Kind, Url};
use signed_core::{GitEvent, PullRequest, RepoAddr};
use signed_git::{FileCommit, PatchParser};
use signed_state::{Backend, BackendEvent, Mirrors, RepoStore};
use ui::avatar::Avatar;
use ui::button::Button;
use ui::clipboard::ClipboardButton;
use ui::common::{Sizable as _, StyledExt as _, h_flex, v_flex};
use ui::empty_state::EmptyState;
use ui::icons::{Icon, IconRole};
use ui::input::InputState;
use ui::layout::panel::{Panel, PanelSlot};
use ui::layout::root::RootContext as _;
use ui::scroll_area::ScrollArea;
use ui::spinner::Spinner;
use ui::tabs::{Tab, Tabs};
use ui::theme::ActiveTheme as _;
use utils::relative_time;

use crate::app::AppRoot;
use crate::issues::helpers::{comment_form, comments_section, sidebar_section};
use crate::pull_requests::dialogs::update::UpdatePrDialog;
use crate::repo::commit_diff::{CommitDiffView, DiffPane};
use crate::shared::event_list::commit_list;
use crate::shared::log_dropped;
use crate::shared::widgets::status_badge;

const NOT_FOUND: &str = "Pull request not found";

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

pub struct PullRequestDetailView {
    workspace: WeakEntity<AppRoot>,
    store: Entity<RepoStore>,
    pr_id: EventId,
    comment_input: Entity<InputState>,
    worktree: Option<PathBuf>,
    description: SharedString,
    current_commit: Option<SharedString>,
    commits: Arc<Vec<FileCommit>>,
    loading: bool,
    error: Option<SharedString>,
    bound: Option<PrBinding>,
    load_generation: u64,
    active_tab: usize,
    pane: Entity<DiffPane>,
    commit_scroll_handle: UniformListScrollHandle,
    tasks: Vec<gpui::Task<Result<(), anyhow::Error>>>,
    directory: Entity<ProfileDirectory>,
    _subscriptions: Vec<Subscription>,
}

impl PullRequestDetailView {
    pub fn new(
        workspace: WeakEntity<AppRoot>,
        store: Entity<RepoStore>,
        pr_id: EventId,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let comment_input = InputState::builder().multiline().build(cx);
        let directory = ProfileDirectory::new(cx);

        let pane = cx.new(DiffPane::new);
        let weak = cx.entity().downgrade();

        let backend = Backend::global(cx);
        let mut subscriptions = vec![];

        subscriptions.push(cx.observe(&store, |this, _store, cx| {
            this.sync(cx);
        }));

        subscriptions.push(cx.subscribe(&backend, |_, _, event, cx| {
            if matches!(event, BackendEvent::SignerChanged) {
                cx.notify();
            }
        }));

        cx.defer(move |cx| {
            log_dropped(weak.update(cx, |this, cx| {
                this.sync(cx);
            }));
        });

        Self {
            workspace,
            store,
            pr_id,
            comment_input,
            worktree: None,
            description: SharedString::default(),
            current_commit: None,
            commits: Arc::new(Vec::new()),
            loading: true,
            error: None,
            bound: None,
            load_generation: 0,
            active_tab: 0,
            pane,
            commit_scroll_handle: UniformListScrollHandle::new(),
            tasks: Vec::new(),
            directory,
            _subscriptions: subscriptions,
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
        crate::shared::track_event_participants(
            author,
            owner,
            comments.into_iter(),
            &self.directory,
            cx,
        );
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        self.track_participants(cx);
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

    /// Reaps finished tasks, then queues another.
    fn push_task(&mut self, task: gpui::Task<Result<(), anyhow::Error>>) {
        self.tasks.retain(|task| !task.is_ready());
        self.tasks.push(task);
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
                    async move { PatchParser::patch_commits(&patch) }
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
                        let commits = repo.commit_range(&base, &tip)?;
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
        self.push_task(task);
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

    fn render_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let files = self.pane.read(cx).diff().map(|diff| diff.files.len());
        let commits = (!self.commits.is_empty()).then_some(self.commits.len());
        let weak = cx.entity().downgrade();
        let files_label = match files {
            Some(count) => format!("Files ({count})"),
            None => "Files".to_owned(),
        };
        let commits_label = match commits {
            Some(count) => format!("Commits ({count})"),
            None => "Commits".to_owned(),
        };

        Tabs::new("pr-tabs")
            .selected(self.active_tab)
            .tabs(move |_, _| {
                vec![
                    Tab::new("discussion", "Discussion"),
                    Tab::new("files", files_label.clone()),
                    Tab::new("commits", commits_label.clone()),
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

    fn render_discussion(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if self.loading {
            return Spinner::centered("pr-loading");
        }
        if let Some(error) = self.error.clone() {
            return EmptyState::centered(error);
        }
        let (author, picture, age, root_id, pubkey) = {
            let store = self.store.read(cx);
            let Some(root) = store
                .pull_requests
                .iter()
                .find(|pr| pr.id == self.pr_id && pr.kind == Kind::GitPullRequest)
            else {
                return EmptyState::centered(NOT_FOUND);
            };
            let profile = self.directory.read(cx).profile(&root.pubkey, cx);
            (
                profile.name(),
                profile.picture(),
                relative_time(root.created_at),
                root.id,
                root.pubkey,
            )
        };

        let store = self.store.read(cx);

        h_flex()
            .size_full()
            .min_h_0()
            .child(
                ScrollArea::new("pr-discussion-scroll")
                    .flex_1()
                    .min_w_0()
                    .child(
                        v_flex()
                            .p_4()
                            .gap_6()
                            .child(
                                v_flex()
                                    .gap_4()
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .text_base()
                                            .child(
                                                h_flex()
                                                    .gap_1()
                                                    .child(
                                                        Avatar::new(pubkey)
                                                            .name(author.clone())
                                                            .picture(picture)
                                                            .small(),
                                                    )
                                                    .child(author),
                                            )
                                            .child(SharedString::from("commented"))
                                            .child(
                                                div()
                                                    .text_color(cx.theme().colors.muted_foreground)
                                                    .child(SharedString::from(age)),
                                            ),
                                    )
                                    .when(!self.description.is_empty(), |this| {
                                        this.child(
                                            div().text_base().child(self.description.clone()),
                                        )
                                    }),
                            )
                            .child(comments_section(&self.store, root_id, &self.directory, cx))
                            .child(comment_form(
                                &self.store,
                                root_id,
                                &store.pull_requests,
                                &self.comment_input,
                                "pr-comment",
                                cx,
                            )),
                    ),
            )
            .child(sidebar_section(
                &self.store,
                root_id,
                &store.pull_requests,
                true,
                &self.directory,
                cx,
            ))
            .into_any_element()
    }

    fn render_files(&self, _cx: &mut Context<Self>) -> AnyElement {
        if self.loading {
            return Spinner::centered("pr-loading");
        }

        if let Some(error) = self.error.clone() {
            return EmptyState::centered(error);
        }

        self.pane
            .clone()
            .cached(StyleRefinement::default().size_full().min_h_0())
            .into_any_element()
    }

    fn render_commits(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.loading {
            return Spinner::centered("pr-loading");
        }
        if let Some(error) = self.error.clone() {
            return EmptyState::centered(error);
        }
        if self.commits.is_empty() {
            return EmptyState::centered("No commits found");
        }
        let commits = Arc::clone(&self.commits);
        let worktree = self.worktree.clone();
        let view = cx.entity().downgrade();

        commit_list(
            "pr-commits",
            "pr-commits-scroll",
            commits,
            self.commit_scroll_handle.clone(),
            |_, _, _| None,
            move |commit, window, cx| {
                if let Some(worktree) = worktree.clone() {
                    let id = commit.id.clone();
                    log_dropped(view.update(cx, |this, cx| {
                        this.open_commit_diff(worktree, &id, window, cx);
                    }));
                }
            },
        )
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
                root.activity_subject(),
                store.status_of(root),
                root.branch_name(),
                root.pubkey,
            )
        };

        let can_update = Backend::global(cx).read(cx).current_user() == Some(author);
        let weak = cx.entity().downgrade();

        v_flex()
            .flex_none()
            .px_4()
            .py_2()
            .gap_2()
            // The top right corner is the panel toolbar's.
            .child(
                h_flex().gap_2().pr_16().child(status_badge(status)).child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_sm()
                        .font_semibold()
                        .text_ellipsis()
                        .child(title),
                ),
            )
            .child(
                h_flex()
                    .gap_3()
                    .when_some(branch, |this, branch| {
                        this.child(
                            Button::new("branch")
                                .label(branch)
                                .icon(Icon::named(IconRole::GitBranch))
                                .small(),
                        )
                    })
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
                    })
                    .when_some(current_commit, |this, id| {
                        this.child(
                            h_flex()
                                .gap_1()
                                .font_family(cx.theme().mono_font)
                                .text_xs()
                                .text_color(cx.theme().colors.muted_foreground)
                                .child(id.clone())
                                .child(ClipboardButton::new("pr-commit", &id)),
                        )
                    }),
            )
            .into_any_element()
    }
}

impl Panel for PullRequestDetailView {
    fn title(&self, _cx: &App) -> SharedString {
        let hex = self.pr_id.to_hex();
        let short = hex.get(..8).unwrap_or(&hex);
        SharedString::from(format!("PR {short}"))
    }

    fn loading(&self, _cx: &App) -> bool {
        self.loading
    }

    fn toolbar(&self, _cx: &App) -> bool {
        true
    }
}

impl Render for PullRequestDetailView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("pull-request-detail")
            .size_full()
            .min_h_0()
            .child(self.render_header(cx))
            .child(self.render_tabs(cx))
            .map(|this| match self.active_tab {
                0 => this.child(self.render_discussion(cx)),
                1 => this.child(self.render_files(cx)),
                _ => this.child(self.render_commits(cx)),
            })
    }
}
