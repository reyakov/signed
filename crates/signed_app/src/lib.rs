mod app;
mod explore;
mod identity;
mod inbox;
mod issues;
mod pull_requests;
mod repo;
mod settings;
mod shared;
mod sidebar;

pub use app::AppRoot;
pub use app::helpers::{apply_appearance, apply_appearance_mode};

use gpui::{App, AppContext as _, Entity, Window};

pub fn root(window: &mut Window, cx: &mut App) -> Entity<AppRoot> {
    cx.new(|cx| AppRoot::new(window, cx))
}
