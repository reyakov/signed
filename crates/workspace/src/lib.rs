mod views;
mod workspace;

pub mod image_cache;

use gpui::{App, AppContext, Entity, Window};
use gpui_component::Root;
pub use views::{RepoListView, SidebarPanel};
pub use workspace::Workspace;

/// Build the root view tree. Requires `signed_state::init` and
/// `gpui_component::init` to have been called first.
pub fn root(window: &mut Window, cx: &mut App) -> Entity<Root> {
    let view = cx.new(|cx| Workspace::new(window, cx));
    cx.new(|cx| Root::new(view, window, cx))
}
