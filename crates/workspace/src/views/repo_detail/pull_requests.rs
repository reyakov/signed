use std::rc::Rc;

use assets::CustomIconName;
use dock::{BasePanel, DockArea, Panel, PanelEvent, add_center_panel, panel_handle};
use gpui::prelude::*;
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, Pixels, Render,
    SharedString, Size, WeakEntity, Window, div, px, size,
};
use gpui_base::Button as BaseButton;
use gpui_component::alert::Alert;
use gpui_component::scroll::Scrollbar;
use gpui_component::{
    ActiveTheme, Icon, IconName, VirtualListScrollHandle, h_flex, v_flex, v_virtual_list,
};
use nostr::prelude::{EventId, Kind};
use signed_core::{RepoStatus, activity_subject};
use signed_state::{ProfileStore, RepoStore};
use signed_ui::{DropdownButton, SegmentButton, UserAvatar, placeholder, status_badge};
use utils::relative_time;

use super::RepoAction;
use super::new_pull_request::open_new_pull_panel;
use super::pull_request_detail::PullRequestDetailView;
use super::send_patch::open_send_patch_panel;

/// Height of one pull request row in the virtual list.
const ROW_HEIGHT: f32 = 73.;

/// Status filter of the pull request list, chosen via the header's filter buttons.
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
    /// Pull requests whose resolved status is [`RepoStatus::Applied`].
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
    /// The filtered pull request count [`Self::item_sizes`] was built for.
    pr_len: usize,
    /// Indices into the store's `pull_requests` matching [`Self::filter`].
    visible_prs: Vec<usize>,
    /// Header counts `(total, open, closed, draft, merged)`.
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
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let repo_name = store.read(cx).name();

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

    /// Open the detail panel of `pr_id` in the dock area.
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
            add_center_panel(dock_area, panel_handle(panel), window, cx);
        });
    }

    /// Render one row of the pull request list.
    ///
    /// `ix` is the row index, `pr_ix` the index in the store's `pull_requests`.
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
                                    .child(UserAvatar::new(author.clone()).picture(picture))
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
        // Counts of the last list rebuild.
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
                    .gap_1()
                    .child(
                        SegmentButton::new("all", "All")
                            .icon(Icon::new(CustomIconName::GitPullRequest))
                            .count(total)
                            .selected(self.filter == PullRequestFilter::All)
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.filter = PullRequestFilter::All;
                                cx.notify();
                            })),
                    )
                    .child(
                        SegmentButton::new("open", "Open")
                            .icon(Icon::new(CustomIconName::GitPullRequest))
                            .count(open)
                            .selected(self.filter == PullRequestFilter::Open)
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.filter = PullRequestFilter::Open;
                                cx.notify();
                            })),
                    )
                    .child(
                        SegmentButton::new("closed", "Closed")
                            .icon(Icon::new(CustomIconName::GitPullRequestClosed))
                            .count(closed)
                            .selected(self.filter == PullRequestFilter::Closed)
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.filter = PullRequestFilter::Closed;
                                cx.notify();
                            })),
                    )
                    .child(
                        SegmentButton::new("draft", "Draft")
                            .icon(Icon::new(CustomIconName::GitPullRequestDraft))
                            .count(draft)
                            .selected(self.filter == PullRequestFilter::Draft)
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.filter = PullRequestFilter::Draft;
                                cx.notify();
                            })),
                    )
                    .child(
                        SegmentButton::new("merged", "Merged")
                            .icon(Icon::new(CustomIconName::GitPullRequestMerged))
                            .count(merged)
                            .selected(self.filter == PullRequestFilter::Merged)
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.filter = PullRequestFilter::Merged;
                                cx.notify();
                            })),
                    ),
            )
            .child(div().flex_1())
            .child(
                h_flex().items_center().child(
                    DropdownButton::new("new-pr-actions")
                        .action(
                            BaseButton::new("new-pr")
                                .child(
                                    h_flex()
                                        .h_8()
                                        .px_2()
                                        .gap_1()
                                        .rounded(cx.theme().radius)
                                        .bg(cx.theme().primary)
                                        .hover(|this| this.bg(cx.theme().primary_hover))
                                        .text_sm()
                                        .text_color(cx.theme().primary_foreground)
                                        .child(Icon::new(IconName::Plus))
                                        .child("New"),
                                )
                                .on_click(cx.listener(|this, _event, window, cx| {
                                    open_new_pull_panel(
                                        this.dock_area.clone(),
                                        this.store.clone(),
                                        window,
                                        cx,
                                    );
                                })),
                        )
                        .dropdown_menu(|menu, _, _| {
                            menu.menu_element(Box::new(RepoAction::SendPatch), |_, _| {
                                h_flex()
                                    .gap_2()
                                    .text_sm()
                                    .child(Icon::new(IconName::File))
                                    .child("Send Patch")
                            })
                        }),
                ),
            )
            .into_any_element()
    }
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

        // Rows and counts are rebuilt only when the store refreshed or filter changed.
        let version = self.store.read(cx).version();

        if self.cache_key != Some((version, filter)) {
            let store = self.store.read(cx);
            let mut counts = (0usize, 0usize, 0usize, 0usize, 0usize);
            self.visible_prs = store
                .pull_requests
                .iter()
                .enumerate()
                .filter_map(|(ix, pr)| {
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

        // The virtual list's item count comes from `item_sizes`.
        // Rebuild it whenever the filtered pull request count changes.
        if count != self.pr_len {
            self.pr_len = count;
            self.item_sizes = Rc::new(vec![size(px(0.), px(ROW_HEIGHT)); count]);
        }

        let sizes = self.item_sizes.clone();
        let scroll_handle = self.scroll_handle.clone();
        let view = cx.entity().clone();

        // Non-fatal warnings and errors of the last action, like creating or updating a PR.
        // Shown as dismissible banners above the list.
        let (last_error, last_warning) = {
            let store = self.store.read(cx);
            (store.last_error.clone(), store.last_warning.clone())
        };

        v_flex()
            .size_full()
            .image_cache(gpui::retain_all("pull-requests"))
            .on_action(cx.listener(|this, action: &RepoAction, window, cx| {
                if action == &RepoAction::SendPatch {
                    open_send_patch_panel(this.dock_area.clone(), this.store.clone(), window, cx);
                }
            }))
            .child(self.render_header(cx))
            .when_some(last_warning, |this, warning| {
                this.child(Alert::warning("pr-warning", warning).banner().on_close({
                    let store = self.store.clone();
                    move |_event, _window, cx| {
                        store.update(cx, |store, _| store.last_warning = None);
                    }
                }))
            })
            .when_some(last_error, |this, error| {
                this.child(Alert::error("pr-error", error).banner().on_close({
                    let store = self.store.clone();
                    move |_event, _window, cx| {
                        store.update(cx, |store, _| store.last_error = None);
                    }
                }))
            })
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
