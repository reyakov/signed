use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, UniformListScrollHandle, WeakEntity, Window, div, uniform_list,
};
use helpers::{IssueFilter, StatusCounts, filter_by_status};
use metadata::ProfileDirectory;
use nostr::prelude::PublicKey;
use signed_core::GitEvent;
use signed_state::RepoStore;
use ui::avatar::Avatar;
use ui::button::Button;
use ui::common::{Sizable, h_flex, v_flex};
use ui::empty_state::EmptyState;
use ui::icons::{Icon, IconRole};
use ui::layout::panel::{Panel, PanelSlot};
use ui::scrollbar::Scrollbar;
use ui::segmented::{Segmented, SegmentedOption};
use ui::theme::ActiveTheme as _;
use utils::relative_time;

use self::detail::IssueDetailView;
use crate::app::AppRoot;
use crate::issues::new_issue::NewIssueView;
use crate::repo::helpers::short_id;
use crate::shared::log_dropped;
use crate::shared::widgets::status_badge;

pub mod detail;
pub(crate) mod helpers;
pub mod new_issue;

pub struct IssuesView {
    workspace: WeakEntity<AppRoot>,
    store: Entity<RepoStore>,
    filter: IssueFilter,
    visible_issues: Vec<usize>,
    counts: StatusCounts,
    // A filter change notifies even when the visible rows are unchanged.
    synced_filter: IssueFilter,
    scroll_handle: UniformListScrollHandle,
    /// Profiles for issue authors and the repository owner.
    directory: Entity<ProfileDirectory>,
    _subscription: Subscription,
}

impl IssuesView {
    pub fn new(
        workspace: WeakEntity<AppRoot>,
        store: Entity<RepoStore>,
        cx: &mut Context<Self>,
    ) -> Self {
        let weak = cx.entity().downgrade();
        let subscription = cx.observe(&store, |this, _store, cx| {
            this.rebuild(cx);
        });

        cx.defer(move |cx| {
            log_dropped(weak.update(cx, |this, cx| {
                this.rebuild(cx);
            }));
        });

        Self {
            workspace,
            store: store.clone(),
            filter: IssueFilter::Open,
            visible_issues: Vec::new(),
            counts: StatusCounts::default(),
            synced_filter: IssueFilter::Open,
            scroll_handle: UniformListScrollHandle::new(),
            directory: ProfileDirectory::new(cx),
            _subscription: subscription,
        }
    }

    fn open_new_issue(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(addr) = self.store.read(cx).addr().cloned() else {
            return;
        };

        let id = SharedString::from(format!("new-issue:{addr}"));
        let workspace = self.workspace.clone();
        let workspace_for_build = workspace.clone();
        let store = self.store.clone();

        log_dropped(workspace.update(cx, |root, cx| {
            root.panels().clone().update(cx, |panels, cx| {
                panels.open(
                    PanelSlot::Split,
                    id.clone(),
                    move |_, cx| {
                        cx.new(|cx| {
                            NewIssueView::new(
                                workspace_for_build.clone(),
                                id.clone(),
                                store.clone(),
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

    /// Total number of issues across all statuses, for the sidebar label.
    pub fn total_count(&self) -> usize {
        self.counts.total
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let filter = self.filter;

        let (visible_issues, counts) = {
            let store = self.store.read(cx);
            filter_by_status(
                &store.issues,
                |issue| store.status_of(issue),
                |status| filter.matches(status),
            )
        };

        let store = self.store.read(cx);
        let mut keys: Vec<PublicKey> = store.issues.iter().map(|issue| issue.pubkey).collect();

        if let Some(owner) = store
            .announcement
            .as_ref()
            .map(|announcement| announcement.owner)
        {
            keys.push(owner);
        }

        self.directory.update(cx, |directory, cx| {
            directory.track(keys, cx);
        });

        let filter_changed = self.synced_filter != filter;
        let visible_issues_changed = self.visible_issues != visible_issues;
        let counts_changed = self.counts != counts;

        if !filter_changed && !visible_issues_changed && !counts_changed {
            return;
        }

        self.synced_filter = filter;
        self.visible_issues = visible_issues;
        self.counts = counts;

        cx.notify();
    }

    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let counts = self.counts;
        let colors = cx.theme().colors;
        let muted = colors.muted_foreground;
        let weak = cx.entity().downgrade();

        h_flex()
            .px_2()
            .h_12()
            .w_full()
            .gap_3()
            .border_b_1()
            .border_color(colors.border)
            .bg(colors.muted)
            .child(
                Segmented::new("issue-filter", self.filter)
                    .options(vec![
                        SegmentedOption::new(IssueFilter::All)
                            .label(format!("All ({})", counts.total))
                            .icon(Icon::named(IconRole::GitIssueDone).color(muted)),
                        SegmentedOption::new(IssueFilter::Open)
                            .label(format!("Open ({})", counts.open))
                            .icon(Icon::named(IconRole::GitIssueOngoing).color(muted)),
                        SegmentedOption::new(IssueFilter::Closed)
                            .label(format!("Closed ({})", counts.closed))
                            .icon(Icon::named(IconRole::GitIssueClosed).color(muted)),
                    ])
                    .on_change(move |filter, _, cx| {
                        if let Err(error) = weak.update(cx, |this, cx| {
                            this.filter = filter;
                            this.rebuild(cx);
                        }) {
                            log::warn!("failed to switch issue filter: {error}");
                        }
                    }),
            )
            .child(div().flex_1())
            .child(
                Button::new("new-issue")
                    .label("New issue")
                    .icon(Icon::named(IconRole::CirclePlus))
                    .small()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_new_issue(window, cx);
                    })),
            )
            .into_any_element()
    }
}

impl Panel for IssuesView {
    fn title(&self, _cx: &App) -> SharedString {
        SharedString::from("Issues")
    }
}

impl Render for IssuesView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let filter = self.filter;
        let visible_issues = self.visible_issues.clone();
        let directory = self.directory.clone();
        let store = self.store.clone();
        let workspace = self.workspace.clone();
        let scroll_handle = self.scroll_handle.clone();
        let count = visible_issues.len();

        let empty_message = match filter {
            IssueFilter::All => "No issues",
            IssueFilter::Open => "No open issues",
            IssueFilter::Closed => "No closed issues",
        };

        v_flex()
            .size_full()
            .child(
                v_flex()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(self.render_header(cx))
                    .when(count > 0, |this| {
                        this.child(
                            uniform_list("issues", count, move |range, _, cx| {
                                range
                                    .map(|ix| {
                                        // The store re-read per frame can shrink below a captured index.
                                        let Some(issue) =
                                            visible_issues.get(ix).and_then(|issue_ix| {
                                                store.read(cx).issues.get(*issue_ix)
                                            })
                                        else {
                                            return div().into_any_element();
                                        };
                                        let profile = directory.read(cx).profile(&issue.pubkey, cx);
                                        let issue_id = issue.id;
                                        let id_hex = issue.id.to_hex();
                                        let status = store.read(cx).status_of(issue);
                                        let store = store.clone();
                                        let workspace = workspace.clone();

                                        h_flex()
                                            .id(SharedString::from(format!("issue:{id_hex}")))
                                            .w_full()
                                            .h_16()
                                            .px_2()
                                            .border_b_1()
                                            .border_color(cx.theme().colors.border)
                                            .hover(|this| this.bg(cx.theme().colors.muted_hover))
                                            .on_click(move |_, window, cx| {
                                                let id =
                                                    SharedString::from(format!("issue:{issue_id}"));
                                                let store = store.clone();
                                                log_dropped(workspace.update(cx, |root, cx| {
                                                    root.panels().clone().update(
                                                        cx,
                                                        |panels, cx| {
                                                            panels.open(
                                                                PanelSlot::Main,
                                                                id,
                                                                move |_, cx| {
                                                                    cx.new(|cx| {
                                                                        IssueDetailView::new(
                                                                            store.clone(),
                                                                            issue_id,
                                                                            cx,
                                                                        )
                                                                    })
                                                                },
                                                                window,
                                                                cx,
                                                            )
                                                        },
                                                    )
                                                }));
                                            })
                                            .child(
                                                h_flex()
                                                    .w_full()
                                                    .items_start()
                                                    .gap_2()
                                                    .child(h_flex().w_24().child(status_badge(status)))
                                                    .child(
                                                        v_flex()
                                                            .flex_1()
                                                            .min_w_0()
                                                            .gap_1()
                                                            .child(
                                                                div()
                                                                    .text_sm()
                                                                    .text_ellipsis()
                                                                    .whitespace_nowrap()
                                                                    .child(issue.activity_subject()),
                                                            )
                                                            .child(
                                                                h_flex()
                                                                    .gap_2()
                                                                    .text_xs()
                                                                    .child(
                                                                        h_flex()
                                                                            .gap_1()
                                                                            .child(
                                                                                Avatar::new(issue.pubkey)
                                                                                    .name(profile.name())
                                                                                    .picture(
                                                                                        profile.picture(),
                                                                                    )
                                                                                    .small(),
                                                                            )
                                                                            .child(
                                                                                div().child(profile.name()),
                                                                            ),
                                                                    )
                                                                    .child("opened")
                                                                    .child(
                                                                        div()
                                                                            .text_color(
                                                                                cx.theme()
                                                                                    .colors
                                                                                    .muted_foreground,
                                                                            )
                                                                            .child(short_id(&id_hex)),
                                                                    )
                                                                    .child(relative_time(issue.created_at)),
                                                            ),
                                                    ),
                                            )
                                            .into_any_element()
                                    })
                                    .collect()
                            })
                            .track_scroll(&scroll_handle)
                            .size_full(),
                        )
                        .child(Scrollbar::vertical("issues-scroll", scroll_handle.clone()))
                    })
                    .when(count == 0, |this| {
                        this.child(EmptyState::centered(empty_message))
                    })
                    .into_any_element(),
            )
            .into_any_element()
    }
}
