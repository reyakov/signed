use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, UniformListScrollHandle, WeakEntity, Window, div, uniform_list,
};
use metadata::ProfileDirectory;
use nostr::prelude::{Kind, PublicKey};
use signed_core::GitEvent;
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
use crate::issues::helpers::{StatusCounts, filter_by_status};
use crate::repo::helpers::short_id;
use crate::shared::log_dropped;
use crate::shared::widgets::status_badge;

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

    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let counts = self.counts;
        let colors = cx.theme().colors;
        let muted = colors.muted_foreground;
        let weak = cx.entity().downgrade();
        let open_weak = cx.entity().downgrade();
        let menu_open_weak = cx.entity().downgrade();
        let patch_weak = cx.entity().downgrade();

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
                    .on_change(move |filter, _, cx| {
                        if let Err(error) = weak.update(cx, |this, cx| {
                            this.filter = filter;
                            this.rebuild(cx);
                        }) {
                            log::warn!("failed to switch PR filter: {error}");
                        }
                    }),
            )
            .child(div().flex_1())
            .child(
                DropdownMenu::new("new-pr-actions")
                    .label("New")
                    .icon(Icon::named(IconRole::GitPullRequest))
                    .small()
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
                    }),
            )
            .into_any_element()
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

        let scroll_handle = self.scroll_handle.clone();
        let count = visible.len();

        v_flex()
            .size_full()
            .when_some(last_warning, |this, warning| {
                this.child(
                    Alert::new("pr-warning")
                        .severity(AlertSeverity::Warning)
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
                            uniform_list("prl", count, move |range, _, cx| {
                                range
                                    .map(|ix| {
                                        // The store re-read per frame can shrink below a captured index.
                                        let Some(pr) = visible.get(ix).and_then(|pr_ix| {
                                            store.read(cx).pull_requests.get(*pr_ix)
                                        }) else {
                                            return div().into_any_element();
                                        };
                                        let profile = directory.read(cx).profile(&pr.pubkey, cx);
                                        let pr_id = pr.id;
                                        let id_hex = pr.id.to_hex();
                                        let status = store.read(cx).status_of(pr);
                                        let workspace = workspace.clone();
                                        let store = store.clone();

                                        h_flex()
                                            .id(SharedString::from(format!("pr:{id_hex}")))
                                            .w_full()
                                            .h_16()
                                            .gap_4()
                                            .px_2()
                                            .border_b_1()
                                            .border_color(cx.theme().colors.border)
                                            .hover(|this| this.bg(cx.theme().colors.muted_hover))
                                            .on_click(move |_, window, cx| {
                                                let id = SharedString::from(format!("pr:{pr_id}"));
                                                let workspace_for_build = workspace.clone();
                                                let store = store.clone();

                                                log_dropped(workspace.update(cx, |root, cx| {
                                                    root.panels().clone().update(
                                                        cx,
                                                        |panels, cx| {
                                                            panels.open(
                                                                PanelSlot::Split,
                                                                id,
                                                                move |window, cx| {
                                                                    cx.new(|cx| {
                                                                        PullRequestDetailView::new(
                                                                            workspace_for_build
                                                                                .clone(),
                                                                            store.clone(),
                                                                            pr_id,
                                                                            window,
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
                                            .child(status_badge(status))
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
                                                            .child(pr.activity_subject()),
                                                    )
                                                    .child(
                                                        h_flex()
                                                            .gap_2()
                                                            .text_xs()
                                                            .child(
                                                                h_flex()
                                                                    .gap_1()
                                                                    .child(
                                                                        Avatar::new(pr.pubkey)
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
                                                            .child(relative_time(pr.created_at)),
                                                    ),
                                            )
                                            .into_any_element()
                                    })
                                    .collect()
                            })
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
