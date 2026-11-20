use gpui::{
    AnyElement, App, ElementId, IntoElement, ParentElement as _, SharedString, Styled as _, div,
};
use signed_core::RepoStatus;
use ui::badge::{Badge, BadgeVariant};
use ui::clipboard::ClipboardButton;
use ui::common::{Sizable as _, StyledExt as _, h_flex, v_flex};
use ui::empty_state::EmptyState;
use ui::icons::{Icon, IconRole};
use ui::scrollbar::{Scrollbar, ScrollbarHandle};
use ui::spinner::Spinner;
use ui::theme::ActiveTheme as _;
use ui::tooltip::Tooltip;

/// The status pill shown on issue and pull-request rows.
pub fn status_badge(status: RepoStatus) -> Badge {
    let (label, variant, icon) = match status {
        RepoStatus::Open => ("Open", BadgeVariant::Neutral, IconRole::GitIssueOngoing),
        RepoStatus::Closed => ("Closed", BadgeVariant::Danger, IconRole::GitIssueClosed),
        RepoStatus::Draft => ("Draft", BadgeVariant::Warning, IconRole::GitIssueOngoing),
        RepoStatus::Applied => ("Applied", BadgeVariant::Success, IconRole::GitIssueDone),
    };

    Badge::new(format!("status-{label}"), label)
        .variant(variant)
        .icon(Icon::named(icon))
        .tooltip(Tooltip::text(label))
}

/// A centered empty-state message filling the available space.
pub fn empty_state(message: impl Into<SharedString>) -> AnyElement {
    let message = message.into();
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .child(EmptyState::new(message))
        .into_any_element()
}

/// A centered spinner filling the available space.
pub fn centered_spinner(id: impl Into<ElementId>) -> AnyElement {
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .child(Spinner::new(id))
        .into_any_element()
}

/// The vertical scrollbar pinned over a uniform list's full extent.
pub fn scroll_overlay<I, H>(id: I, handle: H) -> AnyElement
where
    I: Into<ElementId>,
    H: ScrollbarHandle,
{
    div()
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .bottom_0()
        .child(Scrollbar::vertical(id, handle))
        .into_any_element()
}

/// One copyable clone command with its text visible next to the copy icon.
pub fn clone_row(id: SharedString, command: SharedString, cx: &App) -> AnyElement {
    h_flex()
        .h_8()
        .px_2()
        .gap_2()
        .min_w_0()
        .bg(cx.theme().colors.secondary)
        .rounded(cx.theme().radius.control)
        .text_color(cx.theme().colors.secondary_foreground)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .font_family(cx.theme().mono_font)
                .text_xs()
                .child(command.clone()),
        )
        .child(ClipboardButton::new(id, command).xsmall())
        .into_any_element()
}

/// A one-line muted label used by section headers.
pub fn section_label(text: impl Into<SharedString>, cx: &App) -> AnyElement {
    div()
        .text_xs()
        .font_medium()
        .text_color(cx.theme().colors.muted_foreground)
        .child(text.into())
        .into_any_element()
}
