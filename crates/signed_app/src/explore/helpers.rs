use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Entity, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
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

use crate::repo::actions::open_repo_panel;
use crate::workspace::Workspace;

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

pub(super) fn render_card(
    ix: usize,
    announcement: &Announcement,
    last_activity: Option<Timestamp>,
    announcements: &[Announcement],
    directory: &Entity<ProfileDirectory>,
    workspace: &WeakEntity<Workspace>,
    cx: &mut gpui::App,
) -> AnyElement {
    let owner = directory.read(cx).profile(&announcement.owner, cx);
    let owner_name = owner.name();
    let workspace = workspace.clone();
    let announcement = announcement.clone();

    let name = announcement
        .name
        .as_deref()
        .map(SharedString::from)
        .unwrap_or_else(|| SharedString::from(announcement.id.clone()));

    let description = announcement
        .description
        .as_deref()
        .map(SharedString::from)
        .unwrap_or_else(|| SharedString::from("No description"));

    let activity = last_activity
        .map(relative_time)
        .map(|label| SharedString::from(format!("Updated {label}")))
        .unwrap_or_default();

    let fork_label: Option<SharedString> = announcement.upstream.as_ref().and_then(|upstream| {
        let addr = upstream.addr.as_ref()?;
        let name = announcements
            .iter()
            .find(|a| a.addr() == *addr)
            .map(|a| {
                a.name
                    .as_deref()
                    .map(SharedString::from)
                    .unwrap_or_else(|| SharedString::from(a.id.clone()))
            })
            .unwrap_or_else(|| SharedString::from(addr.identifier().to_owned()));
        Some(SharedString::from(format!("forked from {name}")))
    });

    v_flex()
        .id(ix)
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
                        .min_w_0()
                        .text_sm()
                        .font_semibold()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(name),
                )
                .when_some(fork_label, |this, label| {
                    this.child(
                        h_flex()
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
                .child(description),
        )
        .child(
            h_flex()
                .h_12()
                .gap_2()
                .overflow_hidden()
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Avatar::new(announcement.owner)
                                .name(owner_name.clone())
                                .picture(owner.picture())
                                .small(),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().colors.muted_foreground)
                                .whitespace_nowrap()
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
                        .child(activity),
                ),
        )
        .on_click(move |_, window, cx| {
            open_repo_panel(
                &workspace,
                &announcement.addr(),
                Some(&announcement),
                window,
                cx,
            );
        })
        .into_any_element()
}
