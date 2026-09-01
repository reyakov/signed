use gpui::prelude::*;
use gpui::{AnyElement, App, div};
use gpui_component::{ActiveTheme, v_flex};

/// A centered muted placeholder message, filling its parent.
pub fn placeholder(message: &str, cx: &App) -> AnyElement {
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .p_4()
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(message.to_string()),
        )
        .into_any_element()
}
