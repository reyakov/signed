use std::ops::Range;
use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, UniformListScrollHandle, WeakEntity, Window, div, uniform_list,
};
use helpers::IssueFilter;
use metadata::ProfileDirectory;
use nostr::prelude::{EventId, PublicKey};
use signed_core::{GitEvent, RepoStatus};
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
use crate::shared::labels::ShortIdExt as _;
use crate::shared::log_dropped;
use crate::shared::widgets::{StatusCounts, status_badge};

pub mod detail;
pub(crate) mod helpers;
pub mod new_issue;

/// One issue row, with its display strings built on rebuild.
struct IssueRow {
    id: EventId,
    /// The row's element id.
    element_id: SharedString,
    status: RepoStatus,
    subject: SharedString,
    author: PublicKey,
    age: SharedString,
    short: SharedString,
}

pub struct IssuesView {
    workspace: WeakEntity<AppRoot>,
    store: Entity<RepoStore>,
    filter: IssueFilter,
    rows: Arc<Vec<IssueRow>>,
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
        let weak = cx.weak_entity();
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
            rows: Arc::new(Vec::new()),
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
            StatusCounts::filter(
                &store.issues,
                |issue| store.status_of(issue),
                |status| filter.matches(status),
            )
        };

        let (keys, rows) = {
            let store = self.store.read(cx);
            let mut keys: Vec<PublicKey> = store.issues.iter().map(|issue| issue.pubkey).collect();

            if let Some(owner) = store
                .announcement
                .as_ref()
                .map(|announcement| announcement.owner)
            {
                keys.push(owner);
            }

            let rows: Vec<IssueRow> = visible_issues
                .iter()
                .filter_map(|index| {
                    let issue = store.issues.get(*index)?;
                    Some(IssueRow {
                        id: issue.id,
                        element_id: SharedString::from(format!("issue:{}", issue.id.to_hex())),
                        status: store.status_of(issue),
                        subject: SharedString::from(issue.activity_subject()),
                        author: issue.pubkey,
                        age: SharedString::from(relative_time(issue.created_at)),
                        short: issue.id.to_hex().short_id(),
                    })
                })
                .collect();

            (keys, rows)
        };

        self.directory.update(cx, |directory, cx| {
            directory.track(keys, cx);
        });

        let filter_changed = self.synced_filter != filter;
        let counts_changed = self.counts != counts;
        let rows_changed = self.rows.len() != rows.len();

        if !filter_changed && !counts_changed && !rows_changed {
            return;
        }

        self.synced_filter = filter;
        self.rows = Arc::new(rows);
        self.counts = counts;

        cx.notify();
    }

    /// One issue row of the list.
    fn render_issue(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.rows.get(ix) else {
            return div().into_any_element();
        };

        let profile = self.directory.read(cx).profile(&row.author, cx);
        let issue_id = row.id;
        let status = row.status;

        h_flex()
            .id(row.element_id.clone())
            .w_full()
            .h_16()
            .px_2()
            .border_b_1()
            .border_color(cx.theme().colors.border)
            .hover(|this| this.bg(cx.theme().colors.muted_hover))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_issue(issue_id, window, cx);
            }))
            .child(
                h_flex()
                    .w_full()
                    .items_start()
                    .gap_2()
                    .child(h_flex().w_20().child(status_badge(status)))
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
                                    .child(row.subject.clone()),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .text_xs()
                                    .child(
                                        h_flex()
                                            .min_w_0()
                                            .gap_1()
                                            .child(
                                                Avatar::new(row.author)
                                                    .name(profile.name())
                                                    .picture(profile.picture())
                                                    .small(),
                                            )
                                            .child(
                                                div().min_w_0().truncate().child(profile.name()),
                                            ),
                                    )
                                    .child("opened")
                                    .child(
                                        div()
                                            .text_color(cx.theme().colors.muted_foreground)
                                            .child(row.short.clone()),
                                    )
                                    .child(row.age.clone()),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// Opens this issue's detail panel in the split stack.
    fn open_issue(&mut self, issue_id: EventId, window: &mut Window, cx: &mut Context<Self>) {
        let panels = self
            .workspace
            .read_with(cx, |root, _| root.panels().clone());
        let Ok(panels) = panels else {
            log::warn!("issue list outlived the app shell");
            return;
        };

        IssueDetailView::open(&panels, self.store.clone(), issue_id, window, cx);
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let counts = self.counts;
        let colors = cx.theme().colors;
        let muted = colors.muted_foreground;

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
                    .on_change(cx.processor(|this, filter: IssueFilter, _, cx| {
                        this.filter = filter;
                        this.rebuild(cx);
                    })),
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
        let scroll_handle = self.scroll_handle.clone();
        let count = self.rows.len();

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
                            uniform_list(
                                "issues",
                                count,
                                cx.processor(|this, range: Range<usize>, _, cx| {
                                    range.map(|ix| this.render_issue(ix, cx)).collect()
                                }),
                            )
                            .track_scroll(&scroll_handle)
                            .size_full(),
                        )
                        .child(Scrollbar::vertical("issues-scroll", scroll_handle.clone()))
                    })
                    .when(count == 0, |this| {
                        this.child(EmptyState::centered(empty_message))
                    }),
            )
            .into_any_element()
    }
}
