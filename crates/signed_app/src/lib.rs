mod app;
mod dialogs;
mod panels;
mod views;

pub use app::{AppRoot, apply_appearance};

use gpui::{App, AppContext as _, Entity, Window};

pub fn root(window: &mut Window, cx: &mut App) -> Entity<AppRoot> {
    cx.new(|cx| AppRoot::new(window, cx))
}
