use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, IntoElement, ListAlignment,
    ListSizingBehavior, ListState, ParentElement as _, Pixels, Render, ScrollHandle, SharedString,
    Styled as _, Subscription, Window, div, list, px, rems,
};
use metadata::ProfileDirectory;
use nostr::prelude::EventId;
use signed_core::GitEvent;
use signed_state::RepoStore;
use ui::avatar::Avatar;
use ui::badge::{Badge, BadgeVariant};
use ui::button::Button;
use ui::common::{Sizable as _, StyledExt as _, h_flex, v_flex};
use ui::empty_state::EmptyState;
use ui::icons::{Icon, IconRole};
use ui::input::{Input, InputState};
use ui::layout::panel::{Panel, PanelSlot, PanelStack, SplitSide};
use ui::scroll_area::ScrollArea;
use ui::spinner::Spinner;
use ui::theme::ActiveTheme as _;
use ui::tooltip::Tooltip;
use utils::relative_time;

use crate::shared::labels::ProfileDirectoryExt as _;
use crate::shared::log_dropped;
use crate::shared::widgets::status_badge;

/// The split pane showing the issue's participants and labels.
const SIDEBAR_ID: &str = "Issue details";
const SIDEBAR_WIDTH: Pixels = px(240.);

pub struct IssueDetailView {
    store: Entity<RepoStore>,
    issue_id: EventId,
    comment_input: Entity<InputState>,
    /// Profiles for the author, commenters and the repository owner.
    directory: Entity<ProfileDirectory>,
    scroll_handle: ScrollHandle,
    comments_scroll: ListState,
    _subscription: Subscription,
}

impl IssueDetailView {
    pub fn new(store: Entity<RepoStore>, issue_id: EventId, cx: &mut Context<Self>) -> Self {
        let comment_input = InputState::builder().multiline().build(cx);
        let directory = ProfileDirectory::new(cx);
        let weak = cx.weak_entity();

        let subscription = cx.observe(&store, |this, _store, cx| {
            this.track_participants(cx);
            cx.notify();
        });

        cx.defer(move |cx| {
            log_dropped(weak.update(cx, |this, cx| {
                this.track_participants(cx);
            }));
        });

        Self {
            store,
            issue_id,
            comment_input,
            directory,
            scroll_handle: ScrollHandle::new(),
            comments_scroll: ListState::new(0, ListAlignment::Top, px(48.)),
            _subscription: subscription,
        }
    }

    /// Opens the issue's detail as the main panel and its sidebar in the split pane.
    pub fn open(
        panels: &Entity<PanelStack>,
        store: Entity<RepoStore>,
        issue_id: EventId,
        window: &mut Window,
        cx: &mut App,
    ) {
        let id = SharedString::from(format!("issue:{issue_id}"));
        let sidebar_store = store.clone();

        panels.update(cx, |panels, cx| {
            panels.open(
                PanelSlot::Main,
                id,
                {
                    let store = store.clone();
                    move |_, cx| cx.new(|cx| IssueDetailView::new(store.clone(), issue_id, cx))
                },
                window,
                cx,
            );

            let sidebar = cx.new(|cx| IssueSidebarView {
                store: sidebar_store.clone(),
                issue_id,
                directory: ProfileDirectory::new(cx),
                scroll_handle: ScrollHandle::new(),
            });

            panels.insert_split_at(
                SIDEBAR_ID,
                SIDEBAR_WIDTH,
                SplitSide::Trailing,
                Arc::new(sidebar),
                cx,
            );
        });
    }

    /// Track the issue author, everyone who commented and the repository owner.
    fn track_participants(&mut self, cx: &mut Context<Self>) {
        let store = self.store.read(cx);
        let owner = store.announcement.as_ref().map(|a| a.owner);

        let author = store
            .issues
            .iter()
            .find(|issue| issue.id == self.issue_id)
            .map(|issue| issue.pubkey);

        let comments: Vec<_> = store
            .comments_of(&self.issue_id)
            .map(|comment| comment.pubkey)
            .collect();

        self.directory
            .track_participants(author, owner, comments.into_iter(), cx);
    }

    /// One comment card of the issue's discussion.
    fn render_comment(&self, ix: usize, cx: &App) -> AnyElement {
        let Some(comment) = self.store.read(cx).comments_of(&self.issue_id).nth(ix) else {
            return div().into_any_element();
        };

        let profile = self.directory.read(cx).profile(&comment.pubkey, cx);
        let author = profile.name();
        let age = relative_time(comment.created_at);
        let content = SharedString::from(comment.content.as_str());

        v_flex()
            .gap_1()
            .p_3()
            .border_1()
            .border_color(cx.theme().colors.border)
            .rounded(cx.theme().radius.control)
            .child(
                h_flex()
                    .gap_2()
                    .text_sm()
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Avatar::new(comment.pubkey)
                                    .name(author.clone())
                                    .picture(profile.picture())
                                    .small(),
                            )
                            .child(author),
                    )
                    .child(
                        div()
                            .text_color(cx.theme().colors.muted_foreground)
                            .child("commented"),
                    )
                    .child(
                        div()
                            .text_color(cx.theme().colors.muted_foreground)
                            .child(SharedString::from(age)),
                    ),
            )
            .child(div().text_sm().child(content))
            .into_any_element()
    }

    /// Posts the composer's text as a comment on this issue.
    fn submit_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let content = self.comment_input.read(cx).value().trim().to_string();
        if content.is_empty() {
            return;
        }

        let event = self
            .store
            .read(cx)
            .issues
            .iter()
            .find(|event| event.id == self.issue_id)
            .cloned();

        if let Some(event) = event {
            self.store.update(cx, |store, cx| {
                store.comment(&event, content, cx);
            });
        }

        self.comment_input.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
    }
}

impl Panel for IssueDetailView {
    fn title(&self, cx: &App) -> SharedString {
        let subject = self
            .store
            .read(cx)
            .issues
            .iter()
            .find(|issue| issue.id == self.issue_id)
            .map(|issue| issue.activity_subject());

        match subject {
            Some(subject) => SharedString::from(format!("issue: {subject}")),
            None => {
                let hex = self.issue_id.to_hex();
                let short = hex.get(..8).unwrap_or(&hex);
                SharedString::from(format!("issue: {short}"))
            }
        }
    }

    fn prefix(&self, cx: &App) -> Option<AnyElement> {
        let owner = self.store.read(cx).announcement.as_ref().map(|a| a.owner)?;
        let profile = self.directory.read(cx).profile(&owner, cx);

        Some(
            Avatar::new(owner)
                .name(profile.name())
                .picture(profile.picture())
                .small()
                .into_any_element(),
        )
    }

    fn loading(&self, cx: &App) -> bool {
        !self.store.read(cx).loaded
    }
}

impl Render for IssueDetailView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);

        let Some(issue) = store.issues.iter().find(|issue| issue.id == self.issue_id) else {
            // A not-yet-loaded store is not the same as a missing issue.
            return if store.loaded {
                EmptyState::centered("Issue not found").into_any_element()
            } else {
                Spinner::centered("issue-loading").into_any_element()
            };
        };

        let (title, author, status, age, issue_id, content, pubkey) = {
            let profile = self.directory.read(cx).profile(&issue.pubkey, cx);
            let content = if issue.content.is_empty() {
                SharedString::from("No description provided.")
            } else {
                SharedString::from(&issue.content)
            };
            (
                issue.activity_subject(),
                profile.name(),
                store.status_of(issue),
                relative_time(issue.created_at),
                issue.id,
                content,
                issue.pubkey,
            )
        };
        let picture = self.directory.read(cx).profile(&pubkey, cx).picture();

        ScrollArea::new("issue-detail-scroll")
            .handle(self.scroll_handle.clone())
            .size_full()
            .child(
                v_flex()
                    .p_4()
                    .gap_6()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(status_badge(status))
                            .child(div().flex_1().min_w_0().font_semibold().child(title)),
                    )
                    .child(
                        v_flex()
                            .gap_8()
                            .child(
                                v_flex()
                                    .gap_4()
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .text_sm()
                                            .child(
                                                h_flex()
                                                    .gap_1()
                                                    .child(
                                                        Avatar::new(pubkey)
                                                            .name(author.clone())
                                                            .picture(picture)
                                                            .small(),
                                                    )
                                                    .child(author),
                                            )
                                            .child(
                                                div()
                                                    .text_color(cx.theme().colors.muted_foreground)
                                                    .child("opened"),
                                            )
                                            .child(
                                                div()
                                                    .text_color(cx.theme().colors.muted_foreground)
                                                    .child(SharedString::from(age)),
                                            ),
                                    )
                                    .child(content),
                            )
                            .child({
                                let comments = self.store.read(cx).comments_of(&issue_id).count();

                                if self.comments_scroll.item_count() != comments {
                                    self.comments_scroll.reset(comments);
                                }

                                v_flex()
                                    .gap_4()
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .child(Icon::named(IconRole::Message))
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .font_semibold()
                                                    .child("Discussions"),
                                            )
                                            .child(Badge::new(
                                                format!("discussion-count:{issue_id}"),
                                                comments.to_string(),
                                            )),
                                    )
                                    .child(
                                        v_flex().max_h(rems(24.)).min_h_0().child(
                                            list(
                                                self.comments_scroll.clone(),
                                                cx.processor(|this, ix, _, cx| {
                                                    this.render_comment(ix, cx)
                                                }),
                                            )
                                            .with_sizing_behavior(ListSizingBehavior::Infer),
                                        ),
                                    )
                            })
                            .child(
                                v_flex()
                                    .gap_2()
                                    .child(
                                        Input::new("comment-input", self.comment_input.clone())
                                            .placeholder("Leave a comment...")
                                            .rows(4),
                                    )
                                    .child(
                                        h_flex()
                                            .justify_between()
                                            .child(
                                                h_flex()
                                                    .gap_1()
                                                    .text_xs()
                                                    .text_color(cx.theme().colors.muted_foreground)
                                                    .child(
                                                        Icon::named(IconRole::Markdown)
                                                            .size(px(12.))
                                                            .color(
                                                                cx.theme().colors.muted_foreground,
                                                            ),
                                                    )
                                                    .child("Markdown is supported"),
                                            )
                                            .child(
                                                Button::new("comment")
                                                    .label("Comment")
                                                    .tooltip(Tooltip::text("Post comment"))
                                                    .on_click(cx.listener(
                                                        |this, _, window, cx| {
                                                            this.submit_comment(window, cx)
                                                        },
                                                    )),
                                            ),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }
}

/// The issue's participant and label sidebar, shown in the split pane.
struct IssueSidebarView {
    store: Entity<RepoStore>,
    issue_id: EventId,
    directory: Entity<ProfileDirectory>,
    scroll_handle: ScrollHandle,
}

impl Panel for IssueSidebarView {
    fn title(&self, _cx: &App) -> SharedString {
        SharedString::from(SIDEBAR_ID)
    }
}

impl Render for IssueSidebarView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let root = store.issues.iter().find(|issue| issue.id == self.issue_id);

        ScrollArea::new("issue-sidebar-scroll")
            .handle(self.scroll_handle.clone())
            .size_full()
            .child(v_flex().w_full().p_4().when_some(root, |this, root| {
                let mut participants = vec![root.pubkey];
                participants.extend(store.comments_of(&root.id).map(|comment| comment.pubkey));
                participants.sort_by_key(|key| key.to_bytes());
                participants.dedup();

                let labels: Vec<SharedString> =
                    root.tags.hashtags().map(SharedString::from).collect();

                this.child(
                    v_flex()
                        .gap_4()
                        .child(
                            v_flex()
                                .gap_2()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_medium()
                                        .text_color(cx.theme().colors.muted_foreground)
                                        .child("Participants"),
                                )
                                .children(participants.iter().map(|pubkey| {
                                    let profile = self.directory.read(cx).profile(pubkey, cx);
                                    let name = profile.name();

                                    h_flex()
                                        .gap_1()
                                        .child(
                                            Avatar::new(*pubkey)
                                                .name(name.clone())
                                                .picture(profile.picture())
                                                .small(),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .whitespace_nowrap()
                                                .text_sm()
                                                .text_ellipsis()
                                                .child(name),
                                        )
                                })),
                        )
                        .child(
                            v_flex()
                                .gap_2()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_medium()
                                        .text_color(cx.theme().colors.muted_foreground)
                                        .child("Labels"),
                                )
                                .when(labels.is_empty(), |this| {
                                    this.child(
                                        div()
                                            .text_sm()
                                            .text_color(cx.theme().colors.muted_foreground)
                                            .child("None yet."),
                                    )
                                })
                                .when(!labels.is_empty(), |this| {
                                    this.child(v_flex().gap_1().items_start().children(
                                        labels.iter().map(|label| {
                                            Badge::new(
                                                (ElementId::from("label"), label.clone()),
                                                label.clone(),
                                            )
                                            .variant(BadgeVariant::Outline)
                                            .small()
                                            .flex_none()
                                        }),
                                    ))
                                }),
                        ),
                )
            }))
    }
}
