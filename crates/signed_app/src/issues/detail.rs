use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Render, ScrollHandle, SharedString, Styled as _, Subscription,
    WeakEntity, Window, div, px,
};
use metadata::ProfileDirectory;
use nostr::prelude::EventId;
use signed_core::GitEvent;
use signed_state::RepoStore;
use ui::avatar::Avatar;
use ui::badge::{Badge, BadgeVariant};
use ui::common::{Sizable as _, StyledExt as _, h_flex, v_flex};
use ui::empty_state::EmptyState;
use ui::input::InputState;
use ui::layout::panel::{Panel, PanelSlot, SplitSide};
use ui::scroll_area::ScrollArea;
use ui::spinner::Spinner;
use ui::theme::ActiveTheme as _;
use utils::relative_time;

use crate::app::AppRoot;
use crate::issues::helpers::{comment_form, comments_section};
use crate::shared::widgets::status_badge;
use crate::shared::{log_dropped, track_event_participants};

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
    _subscription: Subscription,
}

impl IssueDetailView {
    pub fn new(store: Entity<RepoStore>, issue_id: EventId, cx: &mut Context<Self>) -> Self {
        let comment_input = InputState::builder().multiline().build(cx);
        let directory = ProfileDirectory::new(cx);
        let weak = cx.entity().downgrade();

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
            _subscription: subscription,
        }
    }

    /// Opens the issue's detail as the main panel and its sidebar in the split pane.
    pub fn open(
        workspace: &WeakEntity<AppRoot>,
        store: Entity<RepoStore>,
        issue_id: EventId,
        window: &mut Window,
        cx: &mut App,
    ) {
        let id = SharedString::from(format!("issue:{issue_id}"));
        let sidebar_store = store.clone();

        log_dropped(workspace.update(cx, |root, cx| {
            root.panels().update(cx, |panels, cx| {
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
            })
        }));
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

        track_event_participants(author, owner, comments.into_iter(), &self.directory, cx);
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
                EmptyState::centered("Issue not found")
            } else {
                Spinner::centered("issue-loading")
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

        div()
            .id("issue-detail")
            .size_full()
            .child(
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
                                                            .text_color(
                                                                cx.theme().colors.muted_foreground,
                                                            )
                                                            .child("opened"),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_color(
                                                                cx.theme().colors.muted_foreground,
                                                            )
                                                            .child(SharedString::from(age)),
                                                    ),
                                            )
                                            .child(content),
                                    )
                                    .child(comments_section(
                                        &self.store,
                                        issue_id,
                                        &self.directory,
                                        cx,
                                    ))
                                    .child(comment_form(
                                        &self.store,
                                        issue_id,
                                        &store.issues,
                                        &self.comment_input,
                                        "comment",
                                        cx,
                                    )),
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

        let details = store
            .issues
            .iter()
            .find(|issue| issue.id == self.issue_id)
            .map(|root| {
                let mut participants = vec![root.pubkey];
                participants.extend(store.comments_of(&root.id).map(|comment| comment.pubkey));
                participants.sort_by_key(|key| key.to_hex());
                participants.dedup();

                let labels: Vec<String> = root.tags.hashtags().map(|tag| tag.to_string()).collect();

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
                                    .into_any_element()
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
                            .map(|this| {
                                if labels.is_empty() {
                                    this.child(
                                        div()
                                            .text_sm()
                                            .text_color(cx.theme().colors.muted_foreground)
                                            .child("None yet."),
                                    )
                                } else {
                                    this.child(v_flex().gap_1().items_start().children(
                                        labels.iter().map(|label| {
                                            Badge::new(
                                                SharedString::from(format!("label:{label}")),
                                                label.clone(),
                                            )
                                            .variant(BadgeVariant::Outline)
                                            .small()
                                            .flex_none()
                                        }),
                                    ))
                                }
                            }),
                    )
                    .into_any_element()
            })
            .unwrap_or_else(|| div().into_any_element());

        ScrollArea::new("issue-sidebar-scroll")
            .handle(self.scroll_handle.clone())
            .size_full()
            .child(v_flex().w_full().p_4().child(details))
    }
}
