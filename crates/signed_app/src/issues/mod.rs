use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, UniformListScrollHandle, WeakEntity, Window, div, px, uniform_list,
};
use helpers::{IssueFilter, StatusCounts, filter_by_status};
use metadata::ProfileDirectory;
use nostr::prelude::PublicKey;
use signed_core::GitEvent;
use signed_state::RepoStore;
use ui::avatar::Avatar;
use ui::button::Button;
use ui::common::{Sizable as _, h_flex, v_flex};
use ui::empty_state::EmptyState;
use ui::icons::{Icon, IconRole};
use ui::layout::panel::Panel;
use ui::scrollbar::Scrollbar;
use ui::segmented::{Segmented, SegmentedOption};
use ui::theme::{ActiveTheme as _, TextRole, Typeset as _};
use utils::relative_time;

use self::detail::IssueDetailView;
use crate::shared::panels;
use crate::shared::widgets::status_badge;
use crate::workspace::Workspace;

pub mod detail;
pub(crate) mod helpers;
pub mod new_issue;

const ISSUE_ROW_HEIGHT: f32 = 73.;

pub struct IssuesView {
    workspace: WeakEntity<Workspace>,
    store: Entity<RepoStore>,
    new_issue: Option<Entity<new_issue::NewIssueDialog>>,
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
        workspace: WeakEntity<Workspace>,
        store: Entity<RepoStore>,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscription = cx.observe(&store, |this, _store, cx| {
            this.rebuild(cx);
        });
        let weak = cx.entity().downgrade();
        cx.defer(move |cx| {
            crate::shared::log_dropped(weak.update(cx, |this, cx| this.rebuild(cx)));
        });

        Self {
            workspace,
            store: store.clone(),
            new_issue: None,
            filter: IssueFilter::Open,
            visible_issues: Vec::new(),
            counts: StatusCounts::default(),
            synced_filter: IssueFilter::Open,
            scroll_handle: UniformListScrollHandle::new(),
            directory: ProfileDirectory::new(cx),
            _subscription: subscription,
        }
    }

    fn open_new_issue(&mut self, cx: &mut Context<Self>) {
        self.new_issue = Some(new_issue::NewIssueDialog::new(self.store.clone(), cx));
        cx.notify();
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
        let muted = cx.theme().colors.muted_foreground;
        let weak = cx.entity().downgrade();

        h_flex()
            .px_4()
            .w_full()
            .gap_3()
            .border_b_1()
            .border_color(cx.theme().colors.border)
            .bg(cx.theme().colors.muted)
            .child(
                Segmented::new("issue-filter", self.filter)
                    .options(vec![
                        SegmentedOption::new(IssueFilter::All, format!("All ({})", counts.total))
                            .icon(Icon::named(IconRole::GitIssueDone).color(muted)),
                        SegmentedOption::new(IssueFilter::Open, format!("Open ({})", counts.open))
                            .icon(Icon::named(IconRole::GitIssueOngoing).color(muted)),
                        SegmentedOption::new(
                            IssueFilter::Closed,
                            format!("Closed ({})", counts.closed),
                        )
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
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.open_new_issue(cx);
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
        let count = self.visible_issues.len();
        let visible_issues = self.visible_issues.clone();
        let directory = self.directory.clone();
        let store = self.store.clone();
        let workspace = self.workspace.clone();

        v_flex()
            .size_full()
            .child(self.render_header(cx))
            .when_some(self.new_issue.clone(), |el, view| el.child(view))
            .child(
                v_flex()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .when(count > 0, |this| {
                        this.child(
                            uniform_list("issues", count, move |range, _, cx| {
                                range
                                    .map(|ix| {
                                        let issue_ix = visible_issues[ix];
                                        let issue = &store.read(cx).issues[issue_ix];
                                        let title = issue.activity_subject();
                                        let id_hex = issue.id.to_hex();
                                        let profile = directory.read(cx).profile(&issue.pubkey, cx);
                                        let author = profile.name();
                                        let age = relative_time(issue.created_at);
                                        let status = store.read(cx).status_of(issue);
                                        let issue_id = issue.id;
                                        let workspace = workspace.clone();
                                        let store = store.clone();

                                        h_flex()
                                            .id(ix)
                                            .w_full()
                                            .h(px(ISSUE_ROW_HEIGHT))
                                            .gap_4()
                                            .px_4()
                                            .border_b_1()
                                            .border_color(cx.theme().colors.border)
                                            .hover(|this| this.bg(cx.theme().colors.muted_hover))
                                            .on_click(move |_, window, cx| {
                                                let id =
                                                    SharedString::from(format!("issue:{issue_id}"));
                                                let store = store.clone();
                                                panels::open_detail(
                                                    &workspace,
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
                                                );
                                            })
                                            .child(status_badge(status))
                                            .child(
                                                v_flex()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .gap_1()
                                                    .child(
                                                        div()
                                                            .typeset(TextRole::Body, cx)
                                                            .text_ellipsis()
                                                            .whitespace_nowrap()
                                                            .child(title),
                                                    )
                                                    .child(
                                                        h_flex()
                                                            .gap_2()
                                                            .typeset(TextRole::Caption, cx)
                                                            .child(
                                                                h_flex()
                                                                    .gap_1()
                                                                    .child(
                                                                        Avatar::new(issue.pubkey)
                                                                            .name(author.clone())
                                                                            .picture(
                                                                                profile.picture(),
                                                                            )
                                                                            .small(),
                                                                    )
                                                                    .child(div().child(author)),
                                                            )
                                                            .child(SharedString::from("opened"))
                                                            .child(
                                                                div()
                                                                    .text_color(
                                                                        cx.theme()
                                                                            .colors
                                                                            .muted_foreground,
                                                                    )
                                                                    .child(SharedString::from(
                                                                        &id_hex[..8],
                                                                    )),
                                                            )
                                                            .child(SharedString::from(age)),
                                                    ),
                                            )
                                            .into_any_element()
                                    })
                                    .collect()
                            })
                            .track_scroll(&self.scroll_handle)
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
                                    "issues-scroll",
                                    self.scroll_handle.clone(),
                                )),
                        )
                    })
                    .when(count == 0, |this| {
                        let message = match filter {
                            IssueFilter::All => "No issues",
                            IssueFilter::Open => "No open issues",
                            IssueFilter::Closed => "No closed issues",
                        };
                        this.child(
                            v_flex()
                                .size_full()
                                .items_center()
                                .justify_center()
                                .child(EmptyState::new(message)),
                        )
                    }),
            )
            .into_any_element()
    }
}
