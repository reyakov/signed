use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, UniformListScrollHandle, WeakEntity, Window, div, px, uniform_list,
};
use metadata::ProfileDirectory;
use nostr::prelude::{Kind, PublicKey};
use signed_core::GitEvent;
use signed_state::RepoStore;
use ui::alert::{Alert, AlertSeverity};
use ui::avatar::Avatar;
use ui::common::{Sizable as _, h_flex, v_flex};
use ui::dropdown_menu::DropdownMenu;
use ui::icons::{Icon, IconRole};
use ui::layout::panel::Panel;
use ui::menu::MenuItem;
use ui::scrollbar::Scrollbar;
use ui::segmented::{Segmented, SegmentedOption};
use ui::theme::{ActiveTheme as _, TextRole, Typeset as _};
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

use crate::issues::helpers::{StatusCounts, filter_by_status};
use crate::shared::widgets::{empty_state, status_badge};
use crate::workspace::Workspace;

const ROW_HEIGHT: f32 = 73.;

pub struct PullRequestsView {
    workspace: WeakEntity<Workspace>,
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
        workspace: WeakEntity<Workspace>,
        store: Entity<RepoStore>,
        cx: &mut Context<Self>,
    ) -> Self {
        let weak = cx.entity().downgrade();
        let subscription = cx.observe(&store, |this, _store, cx| {
            this.rebuild(cx);
        });

        cx.defer(move |cx| {
            crate::shared::log_dropped(weak.update(cx, |this, cx| {
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

        crate::shared::panels::open_detail(
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

        crate::shared::panels::open_detail(
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

        h_flex()
            .px_4()
            .w_full()
            .gap_3()
            .border_b_1()
            .border_color(cx.theme().colors.border)
            .bg(cx.theme().colors.muted)
            .child(
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
                    }),
            )
            .child(div().flex_1())
            .child(
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
                        crate::shared::log_dropped(menu_open_weak.update(cx, |this, cx| {
                            this.new_pr_menu_open = open;
                            cx.notify();
                        }));
                    })
                    .items(move |_, _| {
                        vec![MenuItem::new("Send Patch").handler({
                            let weak = patch_weak.clone();
                            move |window, cx| {
                                crate::shared::log_dropped(weak.update(cx, |this, cx| {
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
        let count = self.visible_prs.len();
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

        v_flex()
            .size_full()
            .child(self.render_header(cx))
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
                v_flex()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .when(count > 0, |this| {
                        this.child(
                            uniform_list("prl", count, move |range, _, cx| {
                                range
                                    .map(|ix| {
                                        let pr_ix = visible[ix];
                                        let pr = &store.read(cx).pull_requests[pr_ix];
                                        let pr_id = pr.id;
                                        let title = pr.activity_subject();
                                        let id_hex = pr.id.to_hex();
                                        let age = relative_time(pr.created_at);
                                        let status = store.read(cx).status_of(pr);
                                        let profile = directory.read(cx).profile(&pr.pubkey, cx);
                                        let author = profile.name();
                                        let workspace = workspace.clone();
                                        let store = store.clone();

                                        h_flex()
                                            .id(ix)
                                            .w_full()
                                            .h(px(ROW_HEIGHT))
                                            .gap_4()
                                            .px_4()
                                            .border_b_1()
                                            .border_color(cx.theme().colors.border)
                                            .hover(|this| this.bg(cx.theme().colors.muted_hover))
                                            .on_click(move |_, window, cx| {
                                                let id = SharedString::from(format!("pr:{pr_id}"));
                                                let workspace_for_build = workspace.clone();
                                                let store = store.clone();
                                                crate::shared::panels::open_detail(
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
                                                                        Avatar::new(pr.pubkey)
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
                                    "prl-scroll",
                                    self.scroll_handle.clone(),
                                )),
                        )
                    })
                    .when(count == 0, |this| {
                        this.child(empty_state(filter.empty_message()))
                    }),
            )
            .into_any_element()
    }
}
