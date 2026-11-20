use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, UniformListScrollHandle, WeakEntity, Window, div,
};
use helpers::{IssueFilter, StatusCounts, filter_by_status};
use metadata::ProfileDirectory;
use nostr::prelude::PublicKey;
use signed_core::GitEvent;
use signed_state::RepoStore;
use ui::button::Button;
use ui::common::v_flex;
use ui::icons::{Icon, IconRole};
use ui::layout::panel::Panel;
use ui::segmented::{Segmented, SegmentedOption};
use ui::theme::ActiveTheme as _;
use utils::relative_time;

use std::rc::Rc;

use self::detail::IssueDetailView;
use crate::repo::helpers::short_id;
use crate::shared::event_list::{EventList, EventRow};
use crate::shared::panels;
use crate::workspace::Workspace;

pub mod detail;
pub(crate) mod helpers;
pub mod new_issue;

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

        crate::shared::event_list::filter_toolbar(
            cx,
            [
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
                    })
                    .into_any_element(),
                div().flex_1().into_any_element(),
                Button::new("new-issue")
                    .label("New issue")
                    .icon(Icon::named(IconRole::CirclePlus))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.open_new_issue(cx);
                    }))
                    .into_any_element(),
            ],
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
        let visible_issues = self.visible_issues.clone();
        let directory = self.directory.clone();
        let store = self.store.clone();
        let workspace = self.workspace.clone();

        let rows = visible_issues
            .iter()
            .enumerate()
            .filter_map(|(ix, &issue_ix)| {
                // The store re-read per frame can shrink below a captured index.
                let issue = store.read(cx).issues.get(issue_ix)?;
                let profile = directory.read(cx).profile(&issue.pubkey, cx);
                let issue_id = issue.id;
                let id_hex = issue.id.to_hex();
                let status = store.read(cx).status_of(issue);

                let on_click: crate::shared::event_list::EventHandler = {
                    let workspace = workspace.clone();
                    let store = store.clone();
                    Rc::new(move |_, window, cx| {
                        let id = SharedString::from(format!("issue:{issue_id}"));
                        let store = store.clone();
                        panels::open_detail(
                            &workspace,
                            id,
                            move |_, cx| {
                                cx.new(|cx| IssueDetailView::new(store.clone(), issue_id, cx))
                            },
                            window,
                            cx,
                        );
                    })
                };

                Some(EventRow {
                    id: ix.into(),
                    status,
                    title: issue.activity_subject().into(),
                    author: profile.name(),
                    picture: profile.picture(),
                    pubkey: issue.pubkey,
                    short_id: short_id(&id_hex),
                    age: SharedString::from(relative_time(issue.created_at)),
                    on_click,
                })
            })
            .collect();

        let empty_message = match filter {
            IssueFilter::All => "No issues",
            IssueFilter::Open => "No open issues",
            IssueFilter::Closed => "No closed issues",
        };

        v_flex()
            .size_full()
            .when_some(self.new_issue.clone(), |el, view| el.child(view))
            .child(
                EventList::new("issues")
                    .toolbar(self.render_header(cx))
                    .scroll_handle(self.scroll_handle.clone())
                    .rows(rows)
                    .empty_message(empty_message)
                    .render(cx),
            )
            .into_any_element()
    }
}
