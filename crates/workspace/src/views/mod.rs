mod commit_diff;
mod dialog_state;
pub(crate) mod discussion;
mod inbox;
mod issues;
mod pull_requests;
mod repo;
mod repo_list;
mod send_patch;
pub(crate) mod sidebar;
mod status_list;
pub(crate) mod tree;

use gpui::prelude::*;
use gpui::{AnyElement, App, div};
use gpui_component::{Sizable as _, h_flex};
pub use inbox::InboxView;
pub use repo::RepoDetailView;
pub(crate) use repo::{RepoItem, open_repo_item, open_repo_panel};
pub use repo_list::RepoListView;
pub use sidebar::SidebarPanel;
use signed_state::{ProfileStore, RepoStore};
use signed_ui::{Avatar, PixelAvatar};

pub(crate) fn tab_title(avatar: AnyElement, label: impl IntoElement) -> impl IntoElement {
    h_flex()
        .gap_1p5()
        .items_center()
        .child(div().flex_shrink_0().child(avatar))
        .child(label)
}

pub(crate) fn panel_avatar(seed: impl AsRef<str>) -> AnyElement {
    PixelAvatar::new(seed).xsmall().into_any_element()
}

pub(crate) fn repo_tab_avatar(store: &RepoStore, cx: &App) -> AnyElement {
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
        .map(|announcement| ProfileStore::global(cx).read(cx).get(&announcement.owner));

    match owner.and_then(|profile| profile.picture().map(|picture| (profile.name(), picture))) {
        Some((name, picture)) => Avatar::new(name)
            .picture(Some(picture))
            .xsmall()
            .into_any_element(),
        None => PixelAvatar::new(seed).xsmall().into_any_element(),
    }
}
