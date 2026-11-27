use std::ops::Range;
use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, UniformListScrollHandle, WeakEntity, Window, div, uniform_list,
};
use metadata::ProfileDirectory;
use nostr::prelude::{EventId, Kind, PublicKey};
use signed_core::{GitEvent, RepoStatus};
use signed_state::RepoStore;
use ui::alert::{Alert, AlertSeverity};
use ui::avatar::Avatar;
use ui::common::{Sizable, h_flex, v_flex};
use ui::dropdown_menu::DropdownMenu;
use ui::empty_state::EmptyState;
use ui::icons::{Icon, IconRole};
use ui::layout::panel::{Panel, PanelSlot};
use ui::menu::MenuItem;
use ui::scrollbar::Scrollbar;
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
use crate::shared::labels::ShortIdExt as _;
use crate::shared::log_dropped;
use crate::shared::widgets::{StatusCounts, status_badge};

/// One pull request row, with its display strings built on rebuild.
struct PrRow {
    id: EventId,
    /// The row's element id.
    element_id: SharedString,
    status: RepoStatus,
    subject: SharedString,
    author: PublicKey,
    age: SharedString,
    short: SharedString,
}

pub struct PullRequestsView {
    workspace: WeakEntity<AppRoot>,
    store: Entity<RepoStore>,
    filter: PullRequestFilter,
    rows: Arc<Vec<PrRow>>,
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
            store,
            filter: PullRequestFilter::Open,
            rows: Arc::new(Vec::new()),
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

            StatusCounts::filter(
                roots,
                |pr| store.status_of(pr),
                |status| filter.matches(status),
            )
        };

        let rows: Vec<PrRow> = visible_prs
            .iter()
            .filter_map(|index| {
                let pr = store.pull_requests.get(*index)?;
                Some(PrRow {
                    id: pr.id,
                    element_id: SharedString::from(format!("pr:{}", pr.id.to_hex())),
                    status: store.status_of(pr),
                    subject: SharedString::from(pr.activity_subject()),
                    author: pr.pubkey,
                    age: SharedString::from(relative_time(pr.created_at)),
                    short: pr.id.to_hex().short_id(),
                })
            })
            .collect();

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
        if !filter_changed && self.rows.len() == rows.len() && self.counts == counts {
            return;
        }

        self.synced_filter = filter;
        self.rows = Arc::new(rows);
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

        log_dropped(workspace.update(cx, |root, cx| {
            root.panels().clone().update(cx, |panels, cx| {
                panels.open(
                    PanelSlot::Split,
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
                )
            })
        }));
    }

    fn open_send_patch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(addr) = self.store.read(cx).addr().cloned() else {
            return;
        };

        let id = SharedString::from(format!("send-patch:{addr}"));
        let workspace = self.workspace.clone();
        let workspace_for_build = workspace.clone();
        let store = self.store.clone();

        log_dropped(workspace.update(cx, |root, cx| {
            root.panels().clone().update(cx, |panels, cx| {
                panels.open(
                    PanelSlot::Split,
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
                )
            })
        }));
    }

    /// One pull request row of the list.
    fn render_pr(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.rows.get(ix) else {
            return div().into_any_element();
        };

        let profile = self.directory.read(cx).profile(&row.author, cx);
        let pr_id = row.id;
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
                this.open_pr(pr_id, window, cx);
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

    /// Opens this pull request's detail panel in the split stack.
    fn open_pr(&mut self, pr_id: EventId, window: &mut Window, cx: &mut Context<Self>) {
        let panels = self
            .workspace
            .read_with(cx, |root, _| root.panels().clone());

        let Ok(panels) = panels else {
            log::warn!("pull request list outlived the app shell");
            return;
        };

        PullRequestDetailView::open(
            &panels,
            &self.workspace,
            self.store.clone(),
            pr_id,
            window,
            cx,
        );
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let counts = self.counts;
        let colors = cx.theme().colors;
        let muted = colors.muted_foreground;
        let patch_weak = cx.weak_entity();

        h_flex()
            .px_2()
            .h_12()
            .w_full()
            .gap_3()
            .border_b_1()
            .border_color(colors.border)
            .bg(colors.muted)
            .child(
                Segmented::new("pr-filter", self.filter)
                    .options(vec![
                        SegmentedOption::new(PullRequestFilter::All)
                            .label(format!("All ({})", counts.total))
                            .icon(Icon::named(IconRole::GitPullRequest).color(muted)),
                        SegmentedOption::new(PullRequestFilter::Open)
                            .label(format!("Open ({})", counts.open))
                            .icon(Icon::named(IconRole::GitPullRequest).color(muted)),
                        SegmentedOption::new(PullRequestFilter::Closed)
                            .label(format!("Closed ({})", counts.closed))
                            .icon(Icon::named(IconRole::GitPullRequestClosed).color(muted)),
                        SegmentedOption::new(PullRequestFilter::Draft)
                            .label(format!("Draft ({})", counts.draft))
                            .icon(Icon::named(IconRole::GitPullRequestDraft).color(muted)),
                        SegmentedOption::new(PullRequestFilter::Merged)
                            .label(format!("Merged ({})", counts.applied))
                            .icon(Icon::named(IconRole::GitPullRequestMerged).color(muted)),
                    ])
                    .on_change(cx.processor(|this, filter: PullRequestFilter, _, cx| {
                        this.filter = filter;
                        this.rebuild(cx);
                    })),
            )
            .child(div().flex_1())
            .child(
                DropdownMenu::new("new-pr-actions")
                    .label("New")
                    .icon(Icon::named(IconRole::GitPullRequest))
                    .small()
                    .open(self.new_pr_menu_open)
                    .on_click(cx.listener(|this, _, window, cx| this.open_new(window, cx)))
                    .on_open_change(cx.processor(|this, open: bool, _, cx| {
                        this.new_pr_menu_open = open;
                        cx.notify();
                    }))
                    .items(move |_, _| {
                        vec![MenuItem::new("Send Patch").handler({
                            let weak = patch_weak.clone();
                            move |window, cx| {
                                log_dropped(weak.update(cx, |this, cx| {
                                    this.open_send_patch(window, cx);
                                }));
                            }
                        })]
                    }),
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
        let (last_error, last_warning) = {
            let store = self.store.read(cx);
            (store.last_error.clone(), store.last_warning.clone())
        };

        let dismiss_warning_store = self.store.clone();
        let dismiss_error_store = self.store.clone();

        let scroll_handle = self.scroll_handle.clone();
        let count = self.rows.len();

        v_flex()
            .size_full()
            .when_some(last_warning, |this, warning| {
                this.child(
                    Alert::new("pr-warning")
                        .severity(AlertSeverity::Warning)
                        .style(ui::alert::AlertStyle::Inline)
                        .title("Pull request warning")
                        .message(warning)
                        .on_dismiss(move |cx| {
                            dismiss_warning_store.update(cx, |store, cx| {
                                store.last_warning = None;
                                cx.notify();
                            });
                        }),
                )
            })
            .when_some(last_error, |this, error| {
                this.child(
                    Alert::new("pr-error")
                        .severity(AlertSeverity::Danger)
                        .style(ui::alert::AlertStyle::Inline)
                        .title("Pull request error")
                        .message(error)
                        .on_dismiss(move |cx| {
                            dismiss_error_store.update(cx, |store, cx| {
                                store.last_error = None;
                                cx.notify();
                            });
                        }),
                )
            })
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
                                "prl",
                                count,
                                cx.processor(|this, range: Range<usize>, _window, cx| {
                                    range.map(|ix| this.render_pr(ix, cx)).collect()
                                }),
                            )
                            .track_scroll(&scroll_handle)
                            .size_full(),
                        )
                        .child(Scrollbar::vertical("prl-scroll", scroll_handle.clone()))
                    })
                    .when(count == 0, |this| {
                        this.child(EmptyState::centered(filter.empty_message()))
                    })
                    .into_any_element(),
            )
            .into_any_element()
    }
}
