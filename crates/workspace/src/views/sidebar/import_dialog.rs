use gpui::{App, Window, px};
use gpui_component::WindowExt;

pub fn open(window: &mut Window, cx: &mut App) {
    window.open_dialog(cx, move |dialog, _window, _cx| {
        dialog.title("Import identity").width(px(400.))
    });
}
