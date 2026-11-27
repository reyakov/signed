use gpui::prelude::FluentBuilder as _;
use gpui::{
    Entity, InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce, SharedString,
    StatefulInteractiveElement as _, Styled as _, WeakEntity, div, px,
};
use metadata::ProfileDirectory;
use signed_core::Announcement;
use signed_state::{RepoListStore, Timestamp};
use ui::avatar::Avatar;
use ui::common::{Sizable as _, StyledExt as _, h_flex, v_flex};
use ui::icons::{Icon, IconRole};
use ui::theme::ActiveTheme as _;
use utils::relative_time;

use crate::app::AppRoot;
use crate::shared::labels::{AnnouncementExt as _, AnnouncementsExt as _};
use crate::shared::log_dropped;

const RECENT_COUNT: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum RepoFilter {
    #[default]
    All,
    Recent,
}

impl RepoFilter {
    /// Indices into the store's `announcements` this filter includes, in display order.
    pub(super) fn visible(self, store: &RepoListStore, query: &str) -> Vec<usize> {
        let announcements = &store.announcements;
        let mut indices: Vec<usize> = (0..announcements.len()).collect();
        let query = query.trim().to_lowercase();

        if !query.is_empty() {
            indices.retain(|&ix| {
                let announcement = &announcements[ix];
                let name = announcement.name.as_deref().unwrap_or(&announcement.id);
                name.to_lowercase().contains(&query)
            });
        }

        match self {
            Self::All => {}
            Self::Recent => indices.truncate(RECENT_COUNT),
        }

        indices
    }
}

/// One explore card's display strings, built when the row list is rebuilt.
#[derive(Clone)]
pub(super) struct CardData {
    /// Index into the store's `announcements`.
    pub(super) ix: usize,
    /// The announcement's element id.
    pub(super) id: SharedString,
    pub(super) name: SharedString,
    pub(super) description: SharedString,
    pub(super) activity: SharedString,
    pub(super) fork_label: Option<SharedString>,
}

impl CardData {
    pub(super) fn new(
        ix: usize,
        announcement: &Announcement,
        announcements: &[Announcement],
        last_activity: Option<Timestamp>,
    ) -> Self {
        let activity = last_activity
            .map(relative_time)
            .map(|label| SharedString::from(format!("Updated {label}")))
            .unwrap_or_default();

        let fork_label = announcement.upstream.as_ref().and_then(|upstream| {
            let addr = upstream.addr.as_ref()?;
            let name = announcements.upstream_display_name(addr);
            Some(SharedString::from(format!("forked from {name}")))
        });

        Self {
            ix,
            id: SharedString::from(format!("repo-card:{}", announcement.id)),
            name: announcement.display_name(),
            description: announcement
                .description
                .as_deref()
                .map(SharedString::from)
                .unwrap_or_else(|| SharedString::from("No description")),
            activity,
            fork_label,
        }
    }
}

/// One repository card in the explore grid.
#[derive(IntoElement)]
pub(super) struct RepoCard {
    data: CardData,
    announcement: Announcement,
    directory: Entity<ProfileDirectory>,
    workspace: WeakEntity<AppRoot>,
}

impl RepoCard {
    pub(super) fn new(
        data: &CardData,
        announcement: &Announcement,
        directory: &Entity<ProfileDirectory>,
        workspace: &WeakEntity<AppRoot>,
    ) -> Self {
        Self {
            data: data.clone(),
            announcement: announcement.clone(),
            directory: directory.clone(),
            workspace: workspace.clone(),
        }
    }
}

impl RenderOnce for RepoCard {
    fn render(self, _window: &mut gpui::Window, cx: &mut gpui::App) -> impl IntoElement {
        let Self {
            data,
            announcement,
            directory,
            workspace,
        } = self;

        let CardData {
            id,
            name,
            description,
            activity,
            fork_label,
            ..
        } = &data;

        let owner = directory.read(cx).profile(&announcement.owner, cx);
        let owner_name = owner.name();

        v_flex()
            .id(id.clone())
            .flex_1()
            .h_full()
            .min_w_0()
            .px_3()
            .rounded(cx.theme().radius.control)
            .border_1()
            .border_color(cx.theme().colors.border)
            .hover(|this| this.bg(cx.theme().colors.muted_hover))
            .child(
                h_flex()
                    .h_10()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .font_semibold()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(name.clone()),
                    )
                    .when_some(fork_label.clone(), |this, label| {
                        this.child(
                            h_flex()
                                .flex_shrink_0()
                                .gap_1()
                                .text_xs()
                                .text_color(cx.theme().colors.muted_foreground)
                                .whitespace_nowrap()
                                .child(
                                    Icon::named(IconRole::GitBranch)
                                        .size(px(12.))
                                        .color(cx.theme().colors.muted_foreground),
                                )
                                .child(label),
                        )
                    }),
            )
            .child(
                div()
                    .h_16()
                    .min_w_0()
                    .text_xs()
                    .text_color(cx.theme().colors.muted_foreground)
                    .line_clamp(2)
                    .child(description.clone()),
            )
            .child(
                h_flex()
                    .h_12()
                    .gap_2()
                    .overflow_hidden()
                    .child(
                        h_flex()
                            .min_w_0()
                            .gap_2()
                            .child(
                                Avatar::new(announcement.owner)
                                    .name(owner_name.clone())
                                    .picture(owner.picture())
                                    .small(),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .text_color(cx.theme().colors.muted_foreground)
                                    .child(owner_name),
                            ),
                    )
                    .child(
                        h_flex()
                            .flex_1()
                            .justify_end()
                            .text_xs()
                            .text_color(cx.theme().colors.muted_foreground)
                            .whitespace_nowrap()
                            .child(activity.clone()),
                    ),
            )
            .on_click(move |_, window, cx| {
                let result = workspace.update(cx, |root, cx| {
                    root.open_repo_panel(&announcement.addr(), Some(&announcement), window, cx);
                });
                log_dropped(result);
            })
    }
}
