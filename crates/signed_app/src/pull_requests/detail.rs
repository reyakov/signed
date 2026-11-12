use std::path::PathBuf;

use assets::CustomIconName;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, UniformListScrollHandle,
    WeakEntity, Window, div, uniform_list,
};
use gpui_native_kit::avatar::Avatar;
use gpui_native_kit::button::Button;
use gpui_native_kit::clipboard::ClipboardButton;
use gpui_native_kit::common::{Sizable as _, h_flex, v_flex};
use gpui_native_kit::data::tabs::{Tab, Tabs};
use gpui_native_kit::icons::Icon;
use gpui_native_kit::input::InputState;
use gpui_native_kit::layout::dock::{DockState, Panel};
use gpui_native_kit::scroll_area::ScrollArea;
use gpui_native_kit::scrollbar::Scrollbar;
use gpui_native_kit::spinner::Spinner;
use gpui_native_kit::theme::ActiveTheme as _;
use metadata::ProfileDirectory;
use nostr::prelude::{EventId, Kind, PublicKey, Url};
use signed_core::{GitEvent, PullRequest, RepoAddr};
use signed_git::{FileCommit, PatchParser};
use signed_state::{Backend, Mirrors, RepoStore};
use utils::relative_time;

use super::dialogs;
use crate::issues::helpers::{comment_form, comments_section, pr_roots, sidebar_section};
use crate::repo::commit_diff::{CommitDiffView, DiffPane};
use crate::repo::helpers::commit_row;
use crate::shared::widgets::{empty_state, repo_tab_avatar, status_badge};

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
    dock: WeakEntity<DockState>,
    store: Entity<RepoStore>,
    pr_id: EventId,
    comment_input: Entity<InputState>,
    repo_name: SharedString,
    worktree: Option<PathBuf>,
    description: SharedString,
    current_commit: Option<SharedString>,
    commits: Vec<FileCommit>,
    loading: bool,
    error: Option<SharedString>,
    bound: Option<PrBinding>,
    load_generation: u64,
    active_tab: usize,
    pane: Entity<DiffPane>,
    commit_scroll_handle: UniformListScrollHandle,
    tasks: Vec<gpui::Task<Result<(), anyhow::Error>>>,
    directory: Entity<ProfileDirectory>,
    _subscription: Subscription,
}

impl PullRequestDetailView {
    pub fn new(
        dock: WeakEntity<DockState>,
        store: Entity<RepoStore>,
        pr_id: EventId,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let pane = cx.new(DiffPane::new);
        let comment_input = InputState::builder().multiline().build(cx);
        let weak = cx.entity().downgrade();
        let repo_name = store.read(cx).name();

        let subscription = cx.observe(&store, |this, _store, cx| {
            this.sync(cx);
        });

        cx.defer(move |cx| {
            crate::shared::log_dropped(weak.update(cx, |this, cx| this.sync(cx)));
        });

        Self {
            dock,
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
            commit_scroll_handle: UniformListScrollHandle::new(),
            tasks: Vec::new(),
            directory: ProfileDirectory::new(cx),
            _subscription: subscription,
        }
    }

    fn track_participants(&mut self, cx: &mut Context<Self>) {
        let store = self.store.read(cx);
        let mut keys: Vec<PublicKey> = store
            .pull_requests
            .iter()
            .find(|pr| pr.id == self.pr_id && pr.kind == Kind::GitPullRequest)
            .map(|root| vec![root.pubkey])
            .unwrap_or_default();
        keys.extend(store.comments_of(&self.pr_id).map(|comment| comment.pubkey));
        if let Some(owner) = store.announcement.as_ref().map(|a| a.owner) {
            keys.push(owner);
        }
        self.directory
            .update(cx, |directory, cx| directory.track(keys, cx));
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
                this.commits = commits;
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
                        worktree.clone(),
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
                crate::shared::log_dropped(weak.update(cx, |this, cx| {
                    this.active_tab = index;
                    cx.notify();
                }));
            })
            .into_any_element()
    }

    fn render_discussion(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if self.loading {
            return centered_spinner();
        }
        if let Some(error) = self.error.clone() {
            return empty_state(error);
        }
        let (author, picture, age, root_id, pubkey) = {
            let store = self.store.read(cx);
            let Some(root) = store
                .pull_requests
                .iter()
                .find(|pr| pr.id == self.pr_id && pr.kind == Kind::GitPullRequest)
            else {
                return empty_state(NOT_FOUND);
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
                                            .text_sm()
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
                                        this.child(div().text_sm().child(self.description.clone()))
                                    }),
                            )
                            .child(comments_section(&self.store, root_id, &self.directory, cx))
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
            .child(sidebar_section(
                &self.store,
                root_id,
                pr_roots,
                true,
                &self.directory,
                cx,
            ))
            .into_any_element()
    }

    fn render_files(&self, _cx: &mut Context<Self>) -> AnyElement {
        if self.loading {
            return centered_spinner();
        }
        if let Some(error) = self.error.clone() {
            return empty_state(error);
        }
        self.pane.clone().into_any_element()
    }

    fn render_commits(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.loading {
            return centered_spinner();
        }
        if let Some(error) = self.error.clone() {
            return empty_state(error);
        }
        if self.commits.is_empty() {
            return empty_state("No commits found");
        }
        let commits = self.commits.clone();
        let worktree = self.worktree.clone();
        let view = cx.entity().downgrade();

        v_flex()
            .relative()
            .flex_1()
            .min_h_0()
            .child(
                uniform_list("pr-commits", commits.len(), move |range, _, cx| {
                    range
                        .map(|ix| {
                            let commit = commits[ix].clone();
                            let id = commit.id.clone();
                            let worktree = worktree.clone();
                            let view = view.clone();
                            commit_row(
                                ix,
                                &commit,
                                move |window, cx| {
                                    if let Some(worktree) = worktree.clone() {
                                        crate::shared::log_dropped(view.update(cx, |this, cx| {
                                            this.open_commit_diff(worktree, &id, window, cx);
                                        }));
                                    }
                                },
                                cx,
                            )
                        })
                        .collect()
                })
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
                    .child(Scrollbar::vertical(
                        "pr-commits-scroll",
                        self.commit_scroll_handle.clone(),
                    )),
            )
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
            .child(
                h_flex().gap_2().child(status_badge(status)).child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
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
                                .icon(Icon::named(CustomIconName::GitBranch))
                                .small(),
                        )
                    })
                    .when(can_update, |this| {
                        this.child(Button::new("update-pr").label("Update").small().on_click(
                            move |_, _, cx| {
                                crate::shared::log_dropped(weak.update(cx, |this, cx| {
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
                                        dialogs::update::open(this.store.clone(), root, cx);
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
    fn title(&self, _cx: &App) -> Option<SharedString> {
        let hex = self.pr_id.to_hex();
        Some(super::helpers::pr_label(&self.repo_name, &hex[..8]))
    }

    fn lead(&self, cx: &App) -> Option<AnyElement> {
        Some(repo_tab_avatar(self.store.read(cx), &self.directory, cx))
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

fn centered_spinner() -> AnyElement {
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .child(Spinner::new("pr-loading"))
        .into_any_element()
}
