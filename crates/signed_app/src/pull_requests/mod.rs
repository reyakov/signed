use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, UniformListScrollHandle, WeakEntity, Window, div,
};
use metadata::ProfileDirectory;
use nostr::prelude::{Kind, PublicKey};
use signed_core::GitEvent;
use signed_state::RepoStore;
use std::rc::Rc;
use ui::alert::{Alert, AlertSeverity};
use ui::common::v_flex;
use ui::dropdown_menu::DropdownMenu;
use ui::icons::{Icon, IconRole};
use ui::layout::panel::Panel;
use ui::menu::MenuItem;
use ui::segmented::{Segmented, SegmentedOption};
use ui::theme::ActiveTheme as _;
use utils::relative_time;

mod detail;
mod dialogs;
mod helpers;
mod new;
mod send_patch;

pub use detail::PullRequestDetailView;
use helpers::PullRequestFilter;
pub use new::NewPullRequestView;
pub use send_patch::SendPatchView;

use crate::app::AppRoot;
use crate::issues::helpers::{StatusCounts, filter_by_status};
use crate::repo::helpers::short_id;
use crate::shared::event_list::{EventHandler, EventList, EventRow, filter_toolbar};
use crate::shared::log_dropped;
use crate::shared::panels::open_detail;

pub struct PullRequestsView {
    workspace: WeakEntity<AppRoot>,
    store: Entity<RepoStore>,
    filter: PullRequestFilter,
    visible_prs: Vec<usize>,
    counts: StatusCounts,
    synced_filter: PullRequestFilter,
    scroll_handle: UniformListScrollHandle,
    directory: Entity<ProfileDirectory>,
    new_pr_menu_open: bool,
    _subscription: Subscription,
}

impl PullRequestsView {
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
            store,
            filter: PullRequestFilter::Open,
            visible_prs: Vec::new(),
            counts: StatusCounts::default(),
            synced_filter: PullRequestFilter::Open,
            scroll_handle: UniformListScrollHandle::new(),
            directory: ProfileDirectory::new(cx),
            new_pr_menu_open: false,
            _subscription: subscription,
        }
    }

    /// Total number of pull requests across all statuses, for the sidebar label.
    pub fn total_count(&self) -> usize {
        self.counts.total
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let store = self.store.read(cx);
        let filter = self.filter;

        let (visible_prs, counts) = {
            let roots = store
                .pull_requests
                .iter()
                .filter(|pr| pr.kind == Kind::GitPullRequest);

            filter_by_status(
                roots,
                |pr| store.status_of(pr),
                |status| filter.matches(status),
            )
        };

        let mut keys: Vec<PublicKey> = store
            .pull_requests
            .iter()
            .filter(|pr| pr.kind == Kind::GitPullRequest)
            .map(|pr| pr.pubkey)
            .collect();

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
        if !filter_changed && self.visible_prs == visible_prs && self.counts == counts {
            return;
        }
        self.synced_filter = filter;
        self.visible_prs = visible_prs;
        self.counts = counts;
        cx.notify();
    }

    fn open_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(addr) = self.store.read(cx).addr().cloned() else {
            return;
        };

        let id = SharedString::from(format!("new-pr:{addr}"));
        let workspace = self.workspace.clone();
        let workspace_for_build = workspace.clone();
        let store = self.store.clone();

        open_detail(
            &workspace,
            id.clone(),
            move |window, cx| {
                cx.new(|cx| {
                    NewPullRequestView::new(
                        workspace_for_build.clone(),
                        id.clone(),
                        store.clone(),
                        window,
                        cx,
                    )
                })
            },
            window,
            cx,
        );
    }

    fn open_send_patch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(addr) = self.store.read(cx).addr().cloned() else {
            return;
        };

        let id = SharedString::from(format!("send-patch:{addr}"));
        let workspace = self.workspace.clone();
        let workspace_for_build = workspace.clone();
        let store = self.store.clone();

        open_detail(
            &workspace,
            id.clone(),
            move |window, cx| {
                cx.new(|cx| {
                    SendPatchView::new(
                        workspace_for_build.clone(),
                        id.clone(),
                        store.clone(),
                        window,
                        cx,
                    )
                })
            },
            window,
            cx,
        );
    }

    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let counts = self.counts;
        let muted = cx.theme().colors.muted_foreground;
        let weak = cx.entity().downgrade();
        let open_weak = cx.entity().downgrade();
        let menu_open_weak = cx.entity().downgrade();
        let patch_weak = cx.entity().downgrade();

        filter_toolbar(
            cx,
            [
                Segmented::new("pr-filter", self.filter)
                    .options(vec![
                        SegmentedOption::new(
                            PullRequestFilter::All,
                            format!("All ({})", counts.total),
                        )
                        .icon(Icon::named(IconRole::GitPullRequest).color(muted)),
                        SegmentedOption::new(
                            PullRequestFilter::Open,
                            format!("Open ({})", counts.open),
                        )
                        .icon(Icon::named(IconRole::GitPullRequest).color(muted)),
                        SegmentedOption::new(
                            PullRequestFilter::Closed,
                            format!("Closed ({})", counts.closed),
                        )
                        .icon(Icon::named(IconRole::GitPullRequestClosed).color(muted)),
                        SegmentedOption::new(
                            PullRequestFilter::Draft,
                            format!("Draft ({})", counts.draft),
                        )
                        .icon(Icon::named(IconRole::GitPullRequestDraft).color(muted)),
                        SegmentedOption::new(
                            PullRequestFilter::Merged,
                            format!("Merged ({})", counts.applied),
                        )
                        .icon(Icon::named(IconRole::GitPullRequestMerged).color(muted)),
                    ])
                    .on_change(move |filter, _, cx| {
                        if let Err(error) = weak.update(cx, |this, cx| {
                            this.filter = filter;
                            this.rebuild(cx);
                        }) {
                            log::warn!("failed to switch PR filter: {error}");
                        }
                    })
                    .into_any_element(),
                div().flex_1().into_any_element(),
                DropdownMenu::new("new-pr-actions")
                    .label("New")
                    .icon(Icon::named(IconRole::GitPullRequest))
                    .open(self.new_pr_menu_open)
                    .on_click(move |_, window, cx| {
                        if let Err(error) =
                            open_weak.update(cx, |this, cx| this.open_new(window, cx))
                        {
                            log::warn!("failed to open new PR: {error}");
                        }
                    })
                    .on_open_change(move |open, _, cx| {
                        log_dropped(menu_open_weak.update(cx, |this, cx| {
                            this.new_pr_menu_open = open;
                            cx.notify();
                        }));
                    })
                    .items(move |_, _| {
                        vec![MenuItem::new("Send Patch").handler({
                            let weak = patch_weak.clone();
                            move |window, cx| {
                                log_dropped(weak.update(cx, |this, cx| {
                                    this.open_send_patch(window, cx);
                                }));
                            }
                        })]
                    })
                    .into_any_element(),
            ],
        )
    }
}

impl Panel for PullRequestsView {
    fn title(&self, _cx: &App) -> SharedString {
        SharedString::from("Pull requests")
    }
}

impl Render for PullRequestsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let filter = self.filter;
        let visible = self.visible_prs.clone();
        let directory = self.directory.clone();
        let store = self.store.clone();
        let workspace = self.workspace.clone();

        let (last_error, last_warning) = {
            let store = self.store.read(cx);
            (store.last_error.clone(), store.last_warning.clone())
        };

        let dismiss_warning_store = self.store.clone();
        let dismiss_error_store = self.store.clone();

        let rows = visible
            .iter()
            .enumerate()
            .filter_map(|(ix, &pr_ix)| {
                // The store re-read per frame can shrink below a captured index.
                let pr = store.read(cx).pull_requests.get(pr_ix)?;
                let profile = directory.read(cx).profile(&pr.pubkey, cx);
                let pr_id = pr.id;
                let id_hex = pr.id.to_hex();
                let status = store.read(cx).status_of(pr);

                let on_click: EventHandler = {
                    let workspace = workspace.clone();
                    let store = store.clone();
                    Rc::new(move |_, window, cx| {
                        let id = SharedString::from(format!("pr:{pr_id}"));
                        let workspace_for_build = workspace.clone();
                        let store = store.clone();
                        open_detail(
                            &workspace,
                            id,
                            move |window, cx| {
                                cx.new(|cx| {
                                    PullRequestDetailView::new(
                                        workspace_for_build.clone(),
                                        store.clone(),
                                        pr_id,
                                        window,
                                        cx,
                                    )
                                })
                            },
                            window,
                            cx,
                        );
                    })
                };

                Some(EventRow {
                    id: ix.into(),
                    status,
                    title: pr.activity_subject().into(),
                    author: profile.name(),
                    picture: profile.picture(),
                    pubkey: pr.pubkey,
                    short_id: short_id(&id_hex),
                    age: SharedString::from(relative_time(pr.created_at)),
                    on_click,
                })
            })
            .collect();

        v_flex()
            .size_full()
            .when_some(last_warning, |this, warning| {
                this.child(
                    Alert::new("pr-warning")
                        .severity(AlertSeverity::Warning)
                        .title("Pull request warning")
                        .message(warning)
                        .on_dismiss(move |cx| {
                            dismiss_warning_store.update(cx, |store, _| store.last_warning = None);
                        }),
                )
            })
            .when_some(last_error, |this, error| {
                this.child(
                    Alert::new("pr-error")
                        .severity(AlertSeverity::Danger)
                        .title("Pull request error")
                        .message(error)
                        .on_dismiss(move |cx| {
                            dismiss_error_store.update(cx, |store, _| store.last_error = None);
                        }),
                )
            })
            .child(
                EventList::new("prl")
                    .toolbar(self.render_header(cx))
                    .scroll_handle(self.scroll_handle.clone())
                    .rows(rows)
                    .empty_message(filter.empty_message())
                    .render(cx),
            )
            .into_any_element()
    }
}
