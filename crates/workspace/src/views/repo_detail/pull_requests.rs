use std::rc::Rc;

use assets::CustomIconName;
use dock::{BasePanel, DockArea, DockPlacement, Panel, PanelEvent, panel_handle};
use gpui::prelude::*;
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, Pixels, Render,
    SharedString, Size, WeakEntity, Window, div, px, relative, size,
};
use gpui_base::Button as BaseButton;
use gpui_component::avatar::Avatar;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::dialog::{DialogDescription, DialogFooter, DialogHeader, DialogTitle};
use gpui_component::form::{field, v_form};
use gpui_component::input::{Input, InputState, Textarea, TextareaState};
use gpui_component::scroll::Scrollbar;
use gpui_component::{
    ActiveTheme, Icon, Sizable, VirtualListScrollHandle, WindowExt, h_flex, v_flex, v_virtual_list,
};
use nostr::prelude::{EventId, Kind};
use signed_core::{RepoStatus, activity_subject};
use signed_state::{ProfileStore, RepoStore};
use utils::relative_time;

use super::helpers::{placeholder, status_badge};
use super::pull_request_detail::PullRequestDetailView;
use crate::image_cache::{MAX_IMAGES, image_cache};

/// Height of one pull request row in the virtual list; same layout as an
/// issue row.
const PR_ROW_HEIGHT: f32 = 73.;

/// Status filter of the pull request list, chosen via the header's filter
/// buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PullRequestFilter {
    /// Every pull request, regardless of status.
    All,
    /// Pull requests whose resolved status is [`RepoStatus::Open`].
    Open,
    /// Pull requests whose resolved status is [`RepoStatus::Closed`].
    Closed,
    /// Pull requests whose resolved status is [`RepoStatus::Draft`].
    Draft,
    /// Pull requests whose resolved status is [`RepoStatus::Applied`]
    /// (i.e. merged).
    Merged,
}

impl PullRequestFilter {
    /// Whether a pull request with `status` is included by this filter.
    fn matches(self, status: RepoStatus) -> bool {
        match self {
            Self::All => true,
            Self::Open => status == RepoStatus::Open,
            Self::Closed => status == RepoStatus::Closed,
            Self::Draft => status == RepoStatus::Draft,
            Self::Merged => status == RepoStatus::Applied,
        }
    }
}

pub struct PullRequestsView {
    focus_handle: FocusHandle,
    /// Dock area the detail panels are added to.
    dock_area: WeakEntity<DockArea>,
    /// Repo store holding the pull requests and their statuses.
    store: Entity<RepoStore>,
    /// Display name of the repository, for the panel title.
    repo_name: SharedString,
    /// Filter selected in the header filter buttons.
    filter: PullRequestFilter,
    /// Per-row heights of the virtual list.
    item_sizes: Rc<Vec<Size<Pixels>>>,
    /// Number of rows [`Self::item_sizes`] was built for (the filtered
    /// pull request count); rebuilt on change.
    pr_len: usize,
    /// Indices into the store's `pull_requests` matching [`Self::filter`]
    /// (root PR events only; updates are revisions of the root); the
    /// virtual list renders this slice. Rebuilt only when the store
    /// version or the filter changes, keyed by [`Self::cache_key`].
    visible_prs: Vec<usize>,
    /// Header counts `(total, open, closed, draft, merged)` of the root
    /// pull requests only (revisions are not separate PRs), rebuilt with
    /// [`Self::visible_prs`].
    counts: (usize, usize, usize, usize, usize),
    /// Store version and filter the cached rows/counts were built from.
    cache_key: Option<(u64, PullRequestFilter)>,
    /// Virtual list state of the pull requests list.
    scroll_handle: VirtualListScrollHandle,
}

impl PullRequestsView {
    pub fn new(
        dock_area: WeakEntity<DockArea>,
        store: Entity<RepoStore>,
        repo_name: SharedString,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            dock_area,
            store,
            repo_name,
            filter: PullRequestFilter::Open,
            item_sizes: Rc::new(Vec::new()),
            pr_len: 0,
            visible_prs: Vec::new(),
            counts: (0, 0, 0, 0, 0),
            cache_key: None,
            scroll_handle: VirtualListScrollHandle::new(),
        }
    }

    /// Open the detail panel of `pr_id` at the bottom of the dock area.
    fn open_pull_request_detail(
        &mut self,
        pr_id: EventId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(dock_area) = self.dock_area.upgrade() else {
            return;
        };

        let panel = cx.new(|cx| {
            PullRequestDetailView::new(
                self.dock_area.clone(),
                self.store.clone(),
                pr_id,
                window,
                cx,
            )
        });

        dock_area.update(cx, |dock_area, cx| {
            dock_area.add_panel_view(panel_handle(panel), DockPlacement::Center, None, window, cx);
        });
    }

    /// Render one row of the pull request list; `ix` is the row index and
    /// `pr_ix` the index of the pull request in the store's `pull_requests`.
    fn render_row(&self, ix: usize, pr_ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let pr = &self.store.read(cx).pull_requests[pr_ix];
        let pr_id = pr.id;
        let title = activity_subject(pr);
        let id_hex = pr.id.to_hex();

        let age = relative_time(pr.created_at);
        let status = self.store.read(cx).status_of(pr);

        let profile = ProfileStore::global(cx).read(cx).get(&pr.pubkey);
        let author = profile.name();
        let picture = profile.picture();

        h_flex()
            .id(ix)
            .w_full()
            .gap_4()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .items_start()
            .on_click(cx.listener(move |this, _event, window, cx| {
                this.open_pull_request_detail(pr_id, window, cx);
            }))
            .child(status_badge(status, cx))
            .child(
                v_flex()
                    .flex_1()
                    .child(
                        div()
                            .h_8()
                            .min_w_0()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .line_clamp(1)
                            .text_sm()
                            .child(title),
                    )
                    .child(
                        h_flex()
                            .h_6()
                            .gap_2()
                            .text_xs()
                            .child(
                                h_flex()
                                    .gap_1()
                                    .child(
                                        Avatar::new()
                                            .name(author.clone())
                                            .when_some(picture, |this, url| this.src(url))
                                            .rounded(cx.theme().radius)
                                            .small(),
                                    )
                                    .child(div().child(author)),
                            )
                            .child(SharedString::from("opened"))
                            .child(
                                div()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(SharedString::from(&id_hex[..8])),
                            )
                            .child(SharedString::from(age)),
                    ),
            )
            .hover(|this| this.bg(cx.theme().list_hover))
            .into_any_element()
    }

    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        // Counts of the last list rebuild (`render` rebuilds first when the
        // store version or filter changed, so this is never stale).
        let (total, open, closed, draft, merged) = self.counts;

        h_flex()
            .px_4()
            .w_full()
            .gap_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().muted.opacity(0.5))
            .child(
                h_flex()
                    .h_12()
                    .gap_2()
                    .child(
                        BaseButton::new("all")
                            .flex()
                            .items_center()
                            .h_7()
                            .px_2()
                            .gap_1()
                            .child(Icon::new(CustomIconName::GitPullRequest))
                            .child(div().text_sm().child("All"))
                            .child(
                                h_flex()
                                    .justify_center()
                                    .ml_2()
                                    .px_1()
                                    .py_0p5()
                                    .min_w_4()
                                    .text_size(px(8.))
                                    .bg(cx.theme().muted)
                                    .text_color(cx.theme().muted_foreground)
                                    .rounded(cx.theme().radius)
                                    .line_height(relative(1.))
                                    .child(SharedString::from(total.to_string())),
                            )
                            .text_color(cx.theme().button_foreground)
                            .rounded(cx.theme().radius)
                            .hover(|this| this.bg(cx.theme().button_hover))
                            .active(|this| this.bg(cx.theme().button_active))
                            .selected(self.filter == PullRequestFilter::All)
                            .when(self.filter == PullRequestFilter::All, |this| {
                                this.bg(cx.theme().button_active)
                            })
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.filter = PullRequestFilter::All;
                                cx.notify();
                            })),
                    )
                    .child(
                        BaseButton::new("open")
                            .flex()
                            .items_center()
                            .h_7()
                            .px_2()
                            .gap_1()
                            .child(Icon::new(CustomIconName::GitPullRequest))
                            .child(div().text_sm().child("Open"))
                            .child(
                                h_flex()
                                    .justify_center()
                                    .ml_2()
                                    .px_1()
                                    .py_0p5()
                                    .min_w_4()
                                    .text_size(px(8.))
                                    .bg(cx.theme().muted)
                                    .text_color(cx.theme().muted_foreground)
                                    .rounded(cx.theme().radius)
                                    .line_height(relative(1.))
                                    .child(SharedString::from(open.to_string())),
                            )
                            .text_color(cx.theme().button_foreground)
                            .rounded(cx.theme().radius)
                            .hover(|this| this.bg(cx.theme().button_hover))
                            .active(|this| this.bg(cx.theme().button_active))
                            .selected(self.filter == PullRequestFilter::Open)
                            .when(self.filter == PullRequestFilter::Open, |this| {
                                this.bg(cx.theme().button_active)
                            })
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.filter = PullRequestFilter::Open;
                                cx.notify();
                            })),
                    )
                    .child(
                        BaseButton::new("closed")
                            .flex()
                            .items_center()
                            .h_7()
                            .px_2()
                            .gap_1()
                            .child(Icon::new(CustomIconName::GitPullRequestClosed))
                            .child(div().text_sm().child("Closed"))
                            .child(
                                h_flex()
                                    .justify_center()
                                    .ml_2()
                                    .px_1()
                                    .py_0p5()
                                    .min_w_4()
                                    .text_size(px(8.))
                                    .bg(cx.theme().muted)
                                    .text_color(cx.theme().muted_foreground)
                                    .rounded(cx.theme().radius)
                                    .line_height(relative(1.))
                                    .child(SharedString::from(closed.to_string())),
                            )
                            .text_color(cx.theme().button_foreground)
                            .rounded(cx.theme().radius)
                            .hover(|this| this.bg(cx.theme().button_hover))
                            .active(|this| this.bg(cx.theme().button_active))
                            .selected(self.filter == PullRequestFilter::Closed)
                            .when(self.filter == PullRequestFilter::Closed, |this| {
                                this.bg(cx.theme().button_active)
                            })
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.filter = PullRequestFilter::Closed;
                                cx.notify();
                            })),
                    )
                    .child(
                        BaseButton::new("draft")
                            .flex()
                            .items_center()
                            .h_7()
                            .px_2()
                            .gap_1()
                            .child(Icon::new(CustomIconName::GitPullRequestDraft))
                            .child(div().text_sm().child("Draft"))
                            .child(
                                h_flex()
                                    .justify_center()
                                    .ml_2()
                                    .px_1()
                                    .py_0p5()
                                    .min_w_4()
                                    .text_size(px(8.))
                                    .bg(cx.theme().muted)
                                    .text_color(cx.theme().muted_foreground)
                                    .rounded(cx.theme().radius)
                                    .line_height(relative(1.))
                                    .child(SharedString::from(draft.to_string())),
                            )
                            .text_color(cx.theme().button_foreground)
                            .rounded(cx.theme().radius)
                            .hover(|this| this.bg(cx.theme().button_hover))
                            .active(|this| this.bg(cx.theme().button_active))
                            .selected(self.filter == PullRequestFilter::Draft)
                            .when(self.filter == PullRequestFilter::Draft, |this| {
                                this.bg(cx.theme().button_active)
                            })
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.filter = PullRequestFilter::Draft;
                                cx.notify();
                            })),
                    )
                    .child(
                        BaseButton::new("merged")
                            .flex()
                            .items_center()
                            .h_7()
                            .px_2()
                            .gap_1()
                            .child(Icon::new(CustomIconName::GitPullRequestMerged))
                            .child(div().text_sm().child("Merged"))
                            .child(
                                h_flex()
                                    .justify_center()
                                    .ml_2()
                                    .px_1()
                                    .py_0p5()
                                    .min_w_4()
                                    .text_size(px(8.))
                                    .bg(cx.theme().muted)
                                    .text_color(cx.theme().muted_foreground)
                                    .rounded(cx.theme().radius)
                                    .line_height(relative(1.))
                                    .child(SharedString::from(merged.to_string())),
                            )
                            .text_color(cx.theme().button_foreground)
                            .rounded(cx.theme().radius)
                            .hover(|this| this.bg(cx.theme().button_hover))
                            .active(|this| this.bg(cx.theme().button_active))
                            .selected(self.filter == PullRequestFilter::Merged)
                            .when(self.filter == PullRequestFilter::Merged, |this| {
                                this.bg(cx.theme().button_active)
                            })
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.filter = PullRequestFilter::Merged;
                                cx.notify();
                            })),
                    ),
            )
            .child(div().flex_1())
            .child(
                BaseButton::new("new-pr")
                    .flex()
                    .items_center()
                    .h_7()
                    .px_2()
                    .gap_1()
                    .child(Icon::new(CustomIconName::CirclePlus))
                    .child(div().text_sm().child("New pull request"))
                    .text_color(cx.theme().button_primary_foreground)
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().button_primary)
                    .hover(|this| this.bg(cx.theme().button_primary_hover))
                    .active(|this| this.bg(cx.theme().button_primary_active))
                    .on_click(cx.listener(|this, _event, window, cx| {
                        open_new_pull_request_dialog(this.store.clone(), window, cx);
                    })),
            )
            .into_any_element()
    }
}

/// Open the "new pull request" dialog: a title, an optional description and
/// a patch input that submit through [`RepoStore::open_pull_request`] when
/// confirmed.
pub(super) fn open_new_pull_request_dialog(
    store: Entity<RepoStore>,
    window: &mut Window,
    cx: &mut App,
) {
    let subject = cx.new(|cx| InputState::new(window, cx).placeholder("Pull request title"));
    let description =
        cx.new(|cx| TextareaState::new(window, cx).placeholder("Describe the change..."));
    let patch = cx
        .new(|cx| TextareaState::new(window, cx).placeholder("Paste `git format-patch` output..."));

    window.open_dialog(cx, move |dialog, _window, _cx| {
        let subject = subject.clone();
        let description = description.clone();
        let patch = patch.clone();
        let store = store.clone();

        dialog
            .width(px(520.))
            .margin_top(px(50.))
            .content(move |body, _window, _cx| {
                body.child(
                    DialogHeader::new()
                        .child(DialogTitle::new().child("New pull request"))
                        .child(
                            DialogDescription::new()
                                .child("Propose a change with the output of `git format-patch`."),
                        ),
                )
                .child(
                    v_form()
                        .child(
                            field()
                                .label("Title")
                                .required(true)
                                .child(Input::new(&subject)),
                        )
                        .child(
                            field()
                                .label("Description")
                                .child(Textarea::new(&description).h(px(96.))),
                        )
                        .child(
                            field()
                                .label("Patch")
                                .child(Textarea::new(&patch).h(px(160.))),
                        ),
                )
                .child(
                    DialogFooter::new().justify_end().child(
                        Button::new("submit")
                            .primary()
                            .label("Create pull request")
                            .tooltip("Create pull request")
                            .on_click({
                                let subject = subject.clone();
                                let description = description.clone();
                                let patch = patch.clone();
                                let store = store.clone();

                                move |_event, window, cx| {
                                    let subject = subject.read(cx).value().to_string();
                                    let description = description.read(cx).value().to_string();
                                    let patch = patch.read(cx).value().to_string();
                                    let subject = (!subject.is_empty()).then_some(subject);

                                    store.update(cx, |store, cx| {
                                        store.open_pull_request(subject, description, patch, cx);
                                    });

                                    window.close_dialog(cx);
                                }
                            }),
                    ),
                )
            })
    });
}

impl BasePanel for PullRequestsView {
    fn panel_name(&self) -> &'static str {
        "pull-requests"
    }
}

impl Panel for PullRequestsView {
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().child(SharedString::from(format!(
            "{}/pull-requests",
            self.repo_name
        )))
    }
}

impl EventEmitter<PanelEvent> for PullRequestsView {}

impl Focusable for PullRequestsView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PullRequestsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let filter = self.filter;

        // Rebuild the filtered rows and header counts only when the store
        // refreshed or the filter changed; other renders reuse the cache.
        let version = self.store.read(cx).version();
        if self.cache_key != Some((version, filter)) {
            let store = self.store.read(cx);
            let mut counts = (0usize, 0usize, 0usize, 0usize, 0usize);
            self.visible_prs = store
                .pull_requests
                .iter()
                .enumerate()
                .filter_map(|(ix, pr)| {
                    // Kind-30620 patches are revisions of a root PR (NIP-34),
                    // not separate pull requests: count only root events, or
                    // the header counts inflate with every revision (which
                    // also default to `Open` in `status_of`).
                    if pr.kind != Kind::GitPullRequest {
                        return None;
                    }
                    let status = store.status_of(pr);
                    counts.0 += 1;
                    match status {
                        RepoStatus::Open => counts.1 += 1,
                        RepoStatus::Closed => counts.2 += 1,
                        RepoStatus::Draft => counts.3 += 1,
                        RepoStatus::Applied => counts.4 += 1,
                    }
                    filter.matches(status).then_some(ix)
                })
                .collect();
            self.counts = counts;
            self.cache_key = Some((version, filter));
        }

        let count = self.visible_prs.len();

        // The virtual list's item count comes from `item_sizes`; rebuild it
        // whenever the filtered pull request count changes.
        if count != self.pr_len {
            self.pr_len = count;
            self.item_sizes = Rc::new(vec![size(px(0.), px(PR_ROW_HEIGHT)); count]);
        }

        let sizes = self.item_sizes.clone();
        let scroll_handle = self.scroll_handle.clone();
        let view = cx.entity().clone();

        v_flex()
            .size_full()
            .image_cache(image_cache("pull-requests", MAX_IMAGES))
            .child(self.render_header(cx))
            .child(
                v_flex()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .when(count > 0, |this| {
                        this.child(
                            v_virtual_list(view, "prl", sizes, move |this, range, _window, cx| {
                                range
                                    .map(|ix| {
                                        let pr_ix = this.visible_prs[ix];
                                        this.render_row(ix, pr_ix, cx)
                                    })
                                    .collect()
                            })
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
                    })
                    .when(count == 0, |this| {
                        let message = match filter {
                            PullRequestFilter::All => "No pull requests",
                            PullRequestFilter::Open => "No open pull requests",
                            PullRequestFilter::Closed => "No closed pull requests",
                            PullRequestFilter::Draft => "No draft pull requests",
                            PullRequestFilter::Merged => "No merged pull requests",
                        };
                        this.child(placeholder(message, cx))
                    }),
            )
            .into_any_element()
    }
}
