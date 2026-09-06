use std::path::PathBuf;
use std::rc::Rc;

use assets::CustomIconName;
use dock::{BasePanel, DockArea, DockPlacement, Panel, PanelEvent, panel_handle};
use gpui::prelude::*;
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, Pixels, Render,
    SharedString, Size, Task, WeakEntity, Window, div, px, relative, size,
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
use nostr::prelude::{Event, EventId, Kind, Nip34Tag};
use signed_core::{activity_subject, pull_request_patch};
use signed_git::{FileCommit, patch_commits, patch_diffs};
use signed_state::{Backend, GitStore, ProfileStore, RepoStore};
use signed_ui::{CountBadge, UserAvatar, placeholder, status_badge};
use utils::{relative_time, relative_time_secs};

use super::diff::{CommitDiffView, DiffPane};
use super::helpers::{comment_form, comments_section, pr_roots, sidebar_section};

/// Height of one commit row in the commits tab's virtual list.
const ROW_HEIGHT: f32 = 37.;

/// Detail panel of a single pull request.
pub struct PullRequestDetailView {
    focus_handle: FocusHandle,
    /// Dock area where new panels, e.g. commit diffs, are added.
    dock_area: WeakEntity<DockArea>,
    /// Repo store holding the PR, its status and comments.
    store: Entity<RepoStore>,
    /// Event id of the root PR event, kind 1618.
    /// Updates are revisions.
    pr_id: EventId,
    /// Input state of the comment textarea.
    comment_input: Entity<TextareaState>,
    /// Display name of the repository, for panels opened from here.
    repo_name: SharedString,
    /// Local clone the PR's git changes come from.
    worktree: Option<PathBuf>,
    /// Root PR's content, shown as plain text.
    description: SharedString,
    /// Tip commit of the PR, the latest update's `c` tag or the root's.
    current_commit: Option<SharedString>,
    /// Commits of the patch series, in patch order, oldest first.
    commits: Vec<FileCommit>,
    /// The patch is being parsed on a background task.
    loading: bool,
    error: Option<SharedString>,
    /// Active header tab, 0 = Discussion, 1 = Files, 2 = Commits.
    active_tab: usize,
    /// Changed-files explorer and per-file diff, like the commit and compare views.
    pane: Entity<DiffPane>,
    /// Per-row heights of the commits tab's virtual list, built when the patch series loads.
    commit_item_sizes: Rc<Vec<Size<Pixels>>>,
    /// Virtual list state of the commits tab.
    commit_scroll_handle: VirtualListScrollHandle,
    /// In-flight tasks, finished tasks are pruned on every push.
    tasks: Vec<Task<Result<(), anyhow::Error>>>,
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

        // Defer loading until the window is ready, like the commit diff view.
        cx.defer_in(window, |this, window, cx| {
            this.load(window, cx);
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
            active_tab: 0,
            pane,
            commit_item_sizes: Rc::new(Vec::new()),
            commit_scroll_handle: VirtualListScrollHandle::new(),
            tasks: Vec::new(),
        }
    }

    /// Snapshot the PR events from the store.
    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.loading = true;
        self.error = None;
        cx.notify();

        let cache = GitStore::global(cx).cache().clone();

        let (description, patch, current_commit, merge_base, clone_urls, addr, has_patch_link) = {
            let store = self.store.read(cx);

            let Some(root) = store
                .pull_requests
                .iter()
                .find(|pr| pr.id == self.pr_id && pr.kind == Kind::GitPullRequest)
            else {
                self.loading = false;
                self.error = Some("Pull request not found".into());
                cx.notify();
                return;
            };

            let update = latest_update(store.pull_requests.iter(), root);

            let tip = update
                .and_then(current_commit_of)
                .or_else(|| current_commit_of(root));

            let base = update
                .and_then(merge_base_of)
                .or_else(|| merge_base_of(root));

            let clone_urls = clone_urls_of(root).or_else(|| {
                store
                    .announcement
                    .as_ref()
                    .map(|a| a.clone.iter().map(ToString::to_string).collect())
            });

            (
                root.content.clone(),
                pull_request_patch(root, store.patches.iter()),
                tip,
                base,
                clone_urls.unwrap_or_default(),
                store.addr().clone(),
                root.tags.event_ids().next().is_some(),
            )
        };

        self.description = description.into();

        let task = cx.spawn_in(window, async move |this, cx| {
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
                let base = merge_base.clone();
                let tip = current_commit.clone();

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

            this.update_in(cx, |this, _window, cx| {
                this.loading = false;
                this.worktree = worktree;
                this.current_commit = current_commit.map(SharedString::from);
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

        self.tasks.retain(|task| !task.is_ready());
        self.tasks.push(task);
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

    /// Full-height Commits tab.
    ///
    /// Every commit of the patch series, or a status message while loading or empty.
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

    /// One row of the commits tab, id, summary, author and time.
    ///
    /// Clicking a row opens the commit's diff in the bottom dock.
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

    /// Always-visible header with a status badge and title, like the issue panel.
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

/// Open the update pull request dialog.
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

/// The `c` tag of a PR event, the commit the proposal points at.
fn current_commit_of(root: &Event) -> Option<String> {
    root.tags
        .iter()
        .find_map(|tag| match Nip34Tag::parse(tag.as_slice()) {
            Ok(Nip34Tag::CurrentCommit(commit)) => Some(commit.to_string()),
            _ => None,
        })
}

/// The `merge-base` tag of a PR event, as hex.
///
/// The most recent common ancestor with the target branch.
fn merge_base_of(event: &Event) -> Option<String> {
    event
        .tags
        .iter()
        .find_map(|tag| match Nip34Tag::parse(tag.as_slice()) {
            Ok(Nip34Tag::MergeBase(commit)) => Some(commit.to_string()),
            _ => None,
        })
}

/// The `clone` tag of a PR event.
///
/// URLs where the proposed branch can be fetched, or `None` if the PR has none.
fn clone_urls_of(event: &Event) -> Option<Vec<String>> {
    event
        .tags
        .iter()
        .find_map(|tag| match Nip34Tag::parse(tag.as_slice()) {
            Ok(Nip34Tag::Clone(urls)) => Some(urls.iter().map(ToString::to_string).collect()),
            _ => None,
        })
}

/// The `branch-name` tag of a PR event, if any.
fn branch_name_of(event: &Event) -> Option<String> {
    event
        .tags
        .iter()
        .find_map(|tag| match Nip34Tag::parse(tag.as_slice()) {
            Ok(Nip34Tag::BranchName(name)) => Some(name),
            _ => None,
        })
}

/// The latest PR update, kind 1619, revising `root`.
fn latest_update<'a>(events: impl Iterator<Item = &'a Event>, root: &Event) -> Option<&'a Event> {
    let root_hex = root.id.to_hex();
    events
        .filter(|e| e.kind == Kind::GitPullRequestUpdate)
        .filter(|e| e.pubkey == root.pubkey)
        .filter(|e| {
            e.tags
                .iter()
                .any(|t| t.kind() == "E" && t.content() == Some(root_hex.as_str()))
        })
        .max_by_key(|e| e.created_at)
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

#[cfg(test)]
mod tests {
    use nostr::prelude::{Tag, *};

    use super::*;

    const COMMIT_HEX: &str = "1111111111111111111111111111111111111111";
    const OTHER_ROOT_HEX: &str = "2222222222222222222222222222222222222222";

    fn keys() -> Keys {
        Keys::new(
            SecretKey::from_hex("0000000000000000000000000000000000000000000000000000000000000001")
                .expect("valid secret key"),
        )
    }

    /// Build a signed event with a controlled `created_at`.
    fn signed(kind: Kind, tags: Vec<Tag>, created_at: u64) -> Event {
        EventBuilder::new(kind, "")
            .tags(tags)
            .custom_created_at(Timestamp::from(created_at))
            .finalize(&keys())
            .expect("signed event")
    }

    fn pr_root() -> Event {
        signed(
            Kind::GitPullRequest,
            vec![
                Tag::parse(["c", COMMIT_HEX]).expect("valid tag"),
                Tag::parse(["branch-name", "feature/x"]).expect("valid tag"),
            ],
            100,
        )
    }

    #[test]
    fn reads_current_commit_and_branch_name() {
        let pr = pr_root();
        assert_eq!(current_commit_of(&pr).as_deref(), Some(COMMIT_HEX));
        assert_eq!(branch_name_of(&pr).as_deref(), Some("feature/x"));
    }

    #[test]
    fn returns_none_without_pr_tags() {
        let pr = signed(Kind::GitPullRequest, vec![], 100);
        assert_eq!(current_commit_of(&pr), None);
        assert_eq!(branch_name_of(&pr), None);
    }

    #[test]
    fn latest_update_picks_newest_revision_of_the_root() {
        let root = pr_root();
        let root_hex = root.id.to_hex();

        let revision = |created_at: u64| {
            signed(
                Kind::GitPullRequestUpdate,
                vec![Tag::parse(["E", &root_hex]).expect("valid tag")],
                created_at,
            )
        };
        // An update revising a different PR must be ignored even though it is newer.
        let unrelated = signed(
            Kind::GitPullRequestUpdate,
            vec![Tag::parse(["E", OTHER_ROOT_HEX]).expect("valid tag")],
            999,
        );

        let events = [unrelated, revision(200), root.clone(), revision(300)];
        let latest = latest_update(events.iter(), &root).expect("an update");

        assert_eq!(latest.created_at.as_secs(), 300);
        assert_eq!(latest.kind, Kind::GitPullRequestUpdate);
    }

    #[test]
    fn latest_update_ignores_other_authors() {
        let root = pr_root();
        let root_hex = root.id.to_hex();
        let other = Keys::new(
            SecretKey::from_hex("0000000000000000000000000000000000000000000000000000000000000002")
                .expect("valid secret key"),
        );
        let stranger = EventBuilder::new(Kind::GitPullRequestUpdate, "")
            .tags([Tag::parse(["E", &root_hex]).expect("valid tag")])
            .custom_created_at(Timestamp::from(999))
            .finalize(&other)
            .expect("signed event");

        // The tip of a PR is only mutable by its author.
        // A newer update from anyone else must not win.
        assert!(latest_update([&stranger, &root].into_iter(), &root).is_none());
    }

    #[test]
    fn latest_update_ignores_roots_without_revisions() {
        let root = pr_root();
        assert!(latest_update([&root].into_iter(), &root).is_none());
    }

    #[test]
    fn commit_meta_combines_author_and_time() {
        let commit = |author: &str, time: i64| FileCommit {
            id: COMMIT_HEX.into(),
            summary: "summary".into(),
            description: None,
            author: author.into(),
            time,
        };

        assert_eq!(commit_meta(&commit("Alice", 0)), "Alice");
        assert_eq!(commit_meta(&commit("", 0)), "");
        assert!(!commit_meta(&commit("", 1_000_000)).is_empty());
        assert!(!commit_meta(&commit("Alice", 1_000_000)).is_empty());
    }
}
