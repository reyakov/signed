mod views;
mod workspace;

use gpui::{App, AppContext, Entity, Window};
use gpui_component::Root;
pub use views::{RepoListView, SidebarPanel};
pub use workspace::Workspace;

/// Build the root view tree.
pub fn root(window: &mut Window, cx: &mut App) -> Entity<Root> {
    let view = cx.new(|cx| Workspace::new(window, cx));
    cx.new(|cx| Root::new(view, window, cx))
}
