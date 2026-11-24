mod app;
mod explore;
mod identity;
#[allow(dead_code)] // temporarily unlinked while the shell migrates
mod inbox;
mod issues;
mod pull_requests;
mod repo;
mod settings;
mod shared;
mod titlebar;

pub use app::AppRoot;
pub use app::helpers::apply_appearance_mode;
use gpui::{App, AppContext as _, Entity, Window};
use ui::layout::root::Root;

pub fn root(window: &mut Window, cx: &mut App) -> Entity<Root> {
    let root = cx.new(|cx| Root::new(window, cx));
    let content: Entity<AppRoot> = cx.new(|cx| AppRoot::new(window, cx));

    root.update(cx, |root, cx| root.set_content(content, cx));
    root
}
