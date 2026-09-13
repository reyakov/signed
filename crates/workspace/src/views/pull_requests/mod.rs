use std::rc::Rc;

use assets::CustomIconName;
use dock::{BasePanel, DockArea, Panel, PanelEvent, add_center_panel, panel_handle};
use gpui::prelude::*;
use gpui::{
    AnyElement, App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, Pixels,
    Render, SharedString, Size, Subscription, WeakEntity, Window, div, px, size,
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

pub(super) mod detail;
pub(super) mod new;

use self::detail::PullRequestDetailView;
use self::new::open_new_pull_panel;
use super::send_patch::open_send_patch_panel;
use crate::views::repo::RepoAction;

const ROW_HEIGHT: f32 = 73.;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PullRequestFilter {
    All,
    Open,
    Closed,
    Draft,
    Merged,
}

impl PullRequestFilter {
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
    dock_area: WeakEntity<DockArea>,
    store: Entity<RepoStore>,
    repo_name: SharedString,
    filter: PullRequestFilter,
    item_sizes: Rc<Vec<Size<Pixels>>>,
    /// Indices into the store's `pull_requests` matching [`Self::filter`].
    visible_prs: Vec<usize>,
    /// Header counts `(total, open, closed, draft, merged)`.
    counts: (usize, usize, usize, usize, usize),
    // A filter change notifies even when the visible rows are unchanged,
    // e.g. switching between two empty filters.
    synced_filter: PullRequestFilter,
    scroll_handle: VirtualListScrollHandle,
    _subscription: Subscription,
}

impl PullRequestsView {
    pub fn new(
        dock_area: WeakEntity<DockArea>,
        store: Entity<RepoStore>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let repo_name = store.read(cx).name();

        let subscription = cx.observe(&store, |this, _store, cx| {
            this.rebuild(cx);
        });

        cx.defer_in(window, |this, _window, cx| {
            this.rebuild(cx);
        });

        Self {
            focus_handle: cx.focus_handle(),
            dock_area,
            store,
            repo_name,
            filter: PullRequestFilter::Open,
            item_sizes: Rc::new(Vec::new()),
            visible_prs: Vec::new(),
            counts: (0, 0, 0, 0, 0),
            synced_filter: PullRequestFilter::Open,
            scroll_handle: VirtualListScrollHandle::new(),
            _subscription: subscription,
        }
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let filter = self.filter;

        let (visible_prs, counts) = {
            let store = self.store.read(cx);
            let mut counts = (0usize, 0usize, 0usize, 0usize, 0usize);

            let visible_prs: Vec<usize> = store
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

            (visible_prs, counts)
        };

        let filter_changed = self.synced_filter != filter;

        if !filter_changed && self.visible_prs == visible_prs && self.counts == counts {
            return;
        }

        self.synced_filter = filter;
        self.item_sizes = Rc::new(vec![size(px(0.), px(ROW_HEIGHT)); visible_prs.len()]);
        self.visible_prs = visible_prs;
        self.counts = counts;

        cx.notify();
    }

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
                                this.rebuild(cx);
                            })),
                    )
                    .child(
                        SegmentButton::new("open", "Open")
                            .icon(Icon::new(CustomIconName::GitPullRequest))
                            .count(open)
                            .selected(self.filter == PullRequestFilter::Open)
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.filter = PullRequestFilter::Open;
                                this.rebuild(cx);
                            })),
                    )
                    .child(
                        SegmentButton::new("closed", "Closed")
                            .icon(Icon::new(CustomIconName::GitPullRequestClosed))
                            .count(closed)
                            .selected(self.filter == PullRequestFilter::Closed)
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.filter = PullRequestFilter::Closed;
                                this.rebuild(cx);
                            })),
                    )
                    .child(
                        SegmentButton::new("draft", "Draft")
                            .icon(Icon::new(CustomIconName::GitPullRequestDraft))
                            .count(draft)
                            .selected(self.filter == PullRequestFilter::Draft)
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.filter = PullRequestFilter::Draft;
                                this.rebuild(cx);
                            })),
                    )
                    .child(
                        SegmentButton::new("merged", "Merged")
                            .icon(Icon::new(CustomIconName::GitPullRequestMerged))
                            .count(merged)
                            .selected(self.filter == PullRequestFilter::Merged)
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.filter = PullRequestFilter::Merged;
                                this.rebuild(cx);
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
        let count = self.visible_prs.len();
        let sizes = self.item_sizes.clone();
        let scroll_handle = self.scroll_handle.clone();
        let view = cx.entity().clone();

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
