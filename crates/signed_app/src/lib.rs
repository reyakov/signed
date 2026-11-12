mod app;
mod explore;
mod identity;
mod inbox;
mod issues;
mod pull_requests;
mod repo;
mod shared;
mod sidebar;

pub use app::AppRoot;
pub use app::helpers::apply_appearance;

use gpui::{App, AppContext as _, Entity, Window};

pub fn root(window: &mut Window, cx: &mut App) -> Entity<AppRoot> {
    cx.new(|cx| AppRoot::new(window, cx))
}
