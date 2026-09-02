//! The "new pull request" panel: pick a local checkout, a base and a
//! compare branch (GitHub-style), review the diff and the commit list, then
//! publish the PR with only a title and an optional description. The patch
//! series is generated from the checkout at submit time; there is no patch
//! input.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use assets::CustomIconName;
use dock::{BasePanel, DockArea, DockPlacement, Panel, PanelEvent, panel_handle};
use gpui::prelude::*;
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, PathPromptOptions,
    Pixels, Render, SharedString, Size, Subscription, Task, WeakEntity, Window, div, px, relative,
    size,
};
use gpui_base::{Button as BaseButton, StyledExt};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::combobox::{
    Caret, Combobox, ComboboxEvent, ComboboxState, ComboboxTriggerContext,
};
use gpui_component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_component::scroll::Scrollbar;
use gpui_component::searchable_list::SearchableVec;
use gpui_component::spinner::Spinner;
use gpui_component::{
    ActiveTheme, Disableable, Icon, IconName, Sizable, VirtualListScrollHandle, h_flex, v_flex,
    v_virtual_list,
};
use signed_git::{
    format_patch_between, merge_base, worktree_commit_range_commits, worktree_commit_range_diff,
};
use signed_state::RepoStore;
use signed_ui::placeholder;

use super::commits::{COMMIT_ROW_HEIGHT, commit_row};
use super::diff::{CommitDiffView, DiffPane};

/// The "new pull request" panel of a repository.
///
/// Both branch selectors list the branches of a user-chosen local checkout;
/// the compare view (Files/Commits tabs) is built from `merge-base..compare`
/// in that checkout, and the patch series published with the PR is generated
/// from the same range at submit time.
pub struct NewPullRequestView {
    focus_handle: FocusHandle,
    /// Dock area the panel lives in; commit diffs are opened there.
    dock_area: WeakEntity<DockArea>,
    /// Store of the target repository (for the announced HEAD default).
    store: Entity<RepoStore>,
    /// Display name of the repository, for the panel title.
    repo_name: SharedString,
    /// The user's checkout: where both branches live and where the tip is
    /// pushed from.
    repo_path: Option<PathBuf>,
    /// Branches of the checkout, backing both selectors.
    branches: Vec<SharedString>,
    /// Selected base branch (the target of the PR).
    base: SharedString,
    /// Selected compare branch (the source of the PR).
    compare: SharedString,
    base_select: Entity<ComboboxState<SearchableVec<SharedString>>>,
    compare_select: Entity<ComboboxState<SearchableVec<SharedString>>>,
    /// Title input (required).
    subject: Entity<InputState>,
    /// Description input (optional).
    description: Entity<TextareaState>,
    /// Merge base of the selected branches; `None` until the compare loads.
    merge_base: Option<String>,
    /// Commits in `merge_base..compare`, newest first.
    commits: Option<Vec<signed_git::FileCommit>>,
    /// The compare is being computed.
    loading: bool,
    /// Error of the last compare or submit attempt.
    error: Option<SharedString>,
    /// A submit (patch generation + publish) is in flight.
    submitting: bool,
    /// Bumped on every branch switch; stale compare results are discarded.
    compare_generation: u64,
    /// Active tab: 0 = Files, 1 = Commits.
    active_tab: usize,
    /// The compare diff (Files tab).
    pane: Entity<DiffPane>,
    /// Virtual list state of the Commits tab.
    scroll_handle: VirtualListScrollHandle,
    item_sizes: Rc<Vec<Size<Pixels>>>,
    _subscriptions: Vec<Subscription>,
    tasks: Vec<Task<Result<(), anyhow::Error>>>,
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

        let base_select: Entity<ComboboxState<SearchableVec<SharedString>>> = cx.new(|cx| {
            ComboboxState::new(
                SearchableVec::new(Vec::<SharedString>::new()),
                Vec::new(),
                window,
                cx,
            )
            .searchable(true)
        });

        let compare_select: Entity<ComboboxState<SearchableVec<SharedString>>> = cx.new(|cx| {
            ComboboxState::new(
                SearchableVec::new(Vec::<SharedString>::new()),
                Vec::new(),
                window,
                cx,
            )
            .searchable(true)
        });

        let subscriptions = vec![
            // Re-evaluate the Create button's enabled state as the title
            // changes.
            cx.subscribe(&subject, |_this, _state, _event: &InputEvent, cx| {
                cx.notify();
            }),
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

        Self {
            focus_handle: cx.focus_handle(),
            dock_area,
            store,
            repo_name,
            repo_path: None,
            branches: Vec::new(),
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
            tasks: Vec::new(),
        }
    }

    /// Prompt for a local checkout; on success populate the branch selectors
    /// (defaults: the announced HEAD branch for the base, the checkout's
    /// current branch for the compare) and load the compare.
    fn choose_checkout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose local checkout".into()),
        });

        let task = cx.spawn_in(window, async move |this, cx| {
            // `Ok(Ok(Some(paths)))` means the user picked a folder; a
            // cancel (or a picker failure) resolves to anything else.
            let picked = match prompt.await {
                Ok(Ok(Some(mut paths))) => paths.pop(),
                _ => None,
            };
            let Some(path) = picked else {
                return Ok(());
            };
            let path = path.to_string_lossy().to_string();

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
        self.tasks.push(task);
    }

    /// Apply a picked checkout: fill the selectors and load the compare.
    fn apply_checkout(
        &mut self,
        path: String,
        info: Option<(Vec<String>, Option<String>)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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

        // Defaults: the announced HEAD branch when the checkout has it
        // (falling back to `main`, then the first branch); the checkout's
        // current branch for the compare side.
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

    /// (Re)compute `merge_base..compare` of the selected branches on a
    /// background task: the merge base, the commit list and the diff.
    fn reload_compare(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo_path) = self.repo_path.clone() else {
            return;
        };
        let base = self.base.to_string();
        let compare = self.compare.to_string();

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

        let task = cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn({
                    let repo_path = repo_path.clone();
                    let base = base.clone();
                    let compare = compare.clone();
                    async move {
                        let merge_base = merge_base(Path::new(&repo_path), &base, &compare)?
                            .ok_or_else(|| {
                                anyhow::anyhow!("{base} and {compare} share no common ancestor")
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
                // A stale result (the branches changed mid-flight) must not
                // clobber a newer compare; the newer task clears the flag.
                if generation != this.compare_generation {
                    return;
                }
                this.loading = false;
                match result {
                    Ok((merge_base, commits, diff)) => {
                        this.merge_base = Some(merge_base);
                        let count = commits.len();
                        this.item_sizes = Rc::new(vec![size(px(0.), px(COMMIT_ROW_HEIGHT)); count]);
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

    /// Publish the pull request: generate the patch series from the checkout
    /// on a background task, hand it to the store, and close the panel once
    /// the publish is underway (errors surface in the pull request list).
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.submitting || self.loading {
            return;
        }
        let Some(repo_path) = self.repo_path.clone() else {
            return;
        };
        let Some(merge_base) = self.merge_base.clone() else {
            return;
        };
        let subject = self.subject.read(cx).value().to_string();
        let description = self.description.read(cx).value().to_string();
        let branch_name = self.compare.to_string();
        let store = self.store.clone();
        let dock_area = self.dock_area.clone();
        let entity = cx.entity().clone();

        self.submitting = true;
        self.error = None;
        cx.notify();

        let task = cx.spawn_in(window, async move |this, cx| {
            // Regenerate the series at submit time so the published patch
            // covers the current tip of the compare branch.
            let patch = cx
                .background_spawn({
                    let repo_path = repo_path.clone();
                    let merge_base = merge_base.clone();
                    let branch_name = branch_name.clone();
                    async move {
                        format_patch_between(Path::new(&repo_path), &merge_base, &branch_name)
                    }
                })
                .await;

            let patch = match patch {
                Ok(patch) if !patch.is_empty() => patch,
                Ok(_) => {
                    this.update_in(cx, |this, _window, cx| {
                        this.submitting = false;
                        this.error = Some("No commits between the branches to propose".into());
                        cx.notify();
                    })?;
                    return Ok(());
                }
                Err(error) => {
                    this.update_in(cx, |this, _window, cx| {
                        this.submitting = false;
                        this.error = Some(format!("Failed to generate the patch: {error}").into());
                        cx.notify();
                    })?;
                    return Ok(());
                }
            };

            this.update_in(cx, |this, window, cx| {
                this.submitting = false;
                store.update(cx, |store, cx| {
                    store.open_pull_request(
                        (!subject.is_empty()).then_some(subject),
                        description,
                        Some(branch_name),
                        patch,
                        false,
                        Some(merge_base),
                        Some(repo_path),
                        cx,
                    );
                });
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
        self.tasks.push(task);
    }

    /// Open the diff of `commit_id` (from the Commits tab) in a new panel.
    fn open_commit_diff(&mut self, commit_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo_path) = self.repo_path.clone() else {
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
            dock_area.add_panel_view(panel_handle(panel), DockPlacement::Center, None, window, cx);
        });
    }

    /// The compare bar: base/compare selectors, the checkout chooser and the
    /// Create button.
    fn render_compare_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let has_checkout = self.repo_path.is_some();
        let checkout = self.repo_path.clone();
        let can_submit = has_checkout
            && !self.loading
            && !self.submitting
            && self.merge_base.is_some()
            && self
                .commits
                .as_ref()
                .is_some_and(|commits| !commits.is_empty())
            && !self.subject.read(cx).value().is_empty();

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
                                .disabled(!has_checkout)
                                .bg(cx.theme().muted)
                                .rounded(cx.theme().radius)
                                .render_trigger(|ctx, _window, cx| {
                                    render_ref_trigger(ctx, CustomIconName::GitBranch, cx)
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
                                .disabled(!has_checkout)
                                .bg(cx.theme().muted)
                                .rounded(cx.theme().radius)
                                .render_trigger(|ctx, _window, cx| {
                                    render_ref_trigger(ctx, CustomIconName::GitBranch, cx)
                                }),
                        ),
                    ),
            )
            .child(
                Button::new("choose-checkout")
                    .icon(IconName::Folder)
                    .ghost()
                    .tooltip(checkout.as_ref().map_or_else(
                        || "Choose a local checkout".into(),
                        |path| path.display().to_string(),
                    ))
                    .on_click(cx.listener(|this, _event, window, cx| {
                        this.choose_checkout(window, cx);
                    })),
            )
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

    /// The title and description inputs.
    fn render_inputs(&self, _cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .px_4()
            .w_full()
            .gap_2()
            .child(Input::new(&self.subject))
            .child(Textarea::new(&self.description).h_24())
            .into_any_element()
    }

    /// The Files/Commits tab bar, mirroring the repository panel's.
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
                    .child(count_badge(files, cx))
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
                    .child(count_badge(commits, cx))
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

    /// The active tab's body.
    fn render_content(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.loading {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .child(Spinner::new().small())
                .into_any_element();
        }
        if self.repo_path.is_none() {
            return placeholder("Choose a local checkout to compare branches", cx);
        }
        if self.commits.is_none() && self.error.is_some() {
            return placeholder("Nothing to compare", cx);
        }
        match self.active_tab {
            0 => self.pane.clone().into_any_element(),
            _ => self.render_commits_tab(cx),
        }
    }

    /// The Commits tab: `merge_base..compare` in a virtual list; clicking a
    /// row opens the commit's diff in a new panel.
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

/// The count badge of a tab, styled like the repository panel's.
fn count_badge(count: usize, cx: &App) -> impl IntoElement {
    h_flex()
        .justify_center()
        .px_1()
        .py_0p5()
        .min_w_4()
        .text_size(px(8.))
        .bg(cx.theme().muted)
        .text_color(cx.theme().muted_foreground)
        .rounded(cx.theme().radius)
        .line_height(relative(1.))
        .child(SharedString::from(count.to_string()))
}

/// The trigger of a branch selector: icon + current selection (or
/// placeholder) + caret. `Combobox` replaces its default trigger entirely.
fn render_ref_trigger(
    ctx: &ComboboxTriggerContext<SearchableVec<SharedString>>,
    icon: CustomIconName,
    cx: &App,
) -> AnyElement {
    let muted = cx.theme().muted_foreground;

    h_flex()
        .w_full()
        .min_w_0()
        .gap_1()
        .items_center()
        .child(Icon::new(icon).small().flex_shrink_0())
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .when(ctx.selection().is_empty(), |this| this.text_color(muted))
                .child(
                    ctx.selection()
                        .first()
                        .map(|(_, item)| item.clone())
                        .or_else(|| ctx.placeholder().cloned())
                        .unwrap_or_default(),
                ),
        )
        .child(Caret::new(ctx.size()).text_color(muted))
        .into_any_element()
}

/// Open the "new pull request" panel in the center dock.
pub(super) fn open_new_pull_panel(
    dock_area: WeakEntity<DockArea>,
    store: Entity<RepoStore>,
    window: &mut Window,
    cx: &mut App,
) {
    let panel = cx.new(|cx| NewPullRequestView::new(dock_area.clone(), store, window, cx));

    let _ = dock_area.update(cx, |dock_area, cx| {
        dock_area.add_panel_view(panel_handle(panel), DockPlacement::Center, None, window, cx);
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
