use assets::CustomIconName;
use gpui::{
    AnyElement, App, Entity, IntoElement, ParentElement as _, SharedString, Styled as _, div,
};
use gpui_native_kit::avatar::Avatar;
use gpui_native_kit::badge::{Badge, BadgeVariant};
use gpui_native_kit::common::Sizable as _;
use gpui_native_kit::empty_state::EmptyState;
use gpui_native_kit::icons::Icon;
use gpui_native_kit::theme::ActiveTheme as _;
use gpui_native_kit::tooltip::Tooltip;
use metadata::ProfileDirectory;
use signed_core::RepoStatus;
use signed_state::RepoStore;

/// The status pill shown on issue and pull-request rows.
pub fn status_badge(status: RepoStatus) -> Badge {
    let (label, variant, icon) = match status {
        RepoStatus::Open => ("Open", BadgeVariant::Neutral, CustomIconName::GitIssueOpen),
        RepoStatus::Closed => (
            "Closed",
            BadgeVariant::Danger,
            CustomIconName::GitIssueClosed,
        ),
        RepoStatus::Draft => (
            "Draft",
            BadgeVariant::Warning,
            CustomIconName::GitIssueOngoing,
        ),
        RepoStatus::Applied => (
            "Applied",
            BadgeVariant::Success,
            CustomIconName::GitIssueDone,
        ),
    };

    Badge::new(format!("status-{label}"), label)
        .variant(variant)
        .icon(Icon::named(icon))
        .tooltip(Tooltip::text(label))
}

/// A centered empty-state message filling the available space.
pub fn empty_state(message: impl Into<SharedString>) -> AnyElement {
    let message = message.into();
    gpui_native_kit::common::v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .child(EmptyState::new(message))
        .into_any_element()
}

/// The profile avatar shown on a repository's panel tab.
pub fn repo_tab_avatar(
    store: &RepoStore,
    directory: &Entity<ProfileDirectory>,
    cx: &App,
) -> AnyElement {
    let seed = store
        .announcement
        .as_ref()
        .map(|announcement| format!("{}:{}", announcement.owner, announcement.id))
        .or_else(|| store.addr().map(|addr| addr.to_string()))
        .or_else(|| {
            store
                .path
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned())
        })
        .unwrap_or_default();

    let owner = store
        .announcement
        .as_ref()
        .map(|announcement| directory.read(cx).profile(&announcement.owner, cx));

    let (name, picture) = owner
        .map(|profile| (Some(profile.name()), profile.picture()))
        .unwrap_or((None, None));

    let avatar = match name {
        Some(name) => Avatar::new(seed).name(name),
        None => Avatar::new(seed),
    };

    avatar.picture(picture).small().into_any_element()
}

/// A one-line muted label used by section headers.
pub fn section_label(text: impl Into<SharedString>, cx: &App) -> AnyElement {
    div()
        .text_xs()
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(cx.theme().colors.muted_foreground)
        .child(text.into())
        .into_any_element()
}
