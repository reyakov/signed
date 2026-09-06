use gpui::prelude::*;
use gpui::{App, ClipboardItem, Div, ElementId, SharedString, div};
use gpui_component::clipboard::Clipboard;
use gpui_component::menu::PopupMenuItem;
use gpui_component::{ActiveTheme, StyledExt, h_flex};

/// A muted command row with a copy button.
pub fn copy_row<E, T>(copy_id: E, command: T, cx: &App) -> Div
where
    E: Into<ElementId>,
    T: Into<SharedString>,
{
    let command = command.into();

    h_flex()
        .h_8()
        .w_full()
        .px_2()
        .gap_2()
        .items_center()
        .bg(cx.theme().muted)
        .rounded(cx.theme().radius)
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_ellipsis()
                .text_xs()
                .child(command.clone()),
        )
        .child(
            Clipboard::new(copy_id)
                .tooltip("Copy")
                .value(command.clone()),
        )
}

/// One row of a copy menu, with a small title above the compact label.
pub fn menu_copy_row(
    id: &'static str,
    title: &'static str,
    label: String,
    copy: String,
) -> PopupMenuItem {
    let row_copy = copy.clone();
    PopupMenuItem::element(move |_window, _cx| {
        let button_copy = copy.clone();
        h_flex()
            .flex_1()
            .gap_2()
            .items_end()
            .child(
                h_flex()
                    .flex_1()
                    .gap_1()
                    .text_xs()
                    .child(div().flex_shrink_0().w_20().font_semibold().child(title))
                    .child(div().flex_1().text_ellipsis().child(label.clone())),
            )
            .child(Clipboard::new(id).tooltip("Copy").value(button_copy))
    })
    .on_click(move |_, _, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string(row_copy.clone()));
    })
}
