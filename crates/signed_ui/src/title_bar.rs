use gpui::{
    App, Div, InteractiveElement as _, MouseButton, Stateful, StatefulInteractiveElement as _,
    Window, WindowControlArea,
};

/// State used to move the window when the title bar area is dragged.
struct WindowDragState {
    should_move: bool,
}

/// Make an element behave like a window title bar: dragging it moves the
/// window, and double-clicking zooms the window (or performs the platform's
/// default title-bar double-click action on macOS).
///
/// Only the bar's non-interactive areas should get this — tabs are draggable
/// (to reorder panels) and must not move the window.
pub fn title_bar_drag_handlers(
    this: Stateful<Div>,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    let state = window.use_state(cx, |_, _| WindowDragState { should_move: false });

    this.window_control_area(WindowControlArea::Drag)
        .on_mouse_down_out(window.listener_for(&state, |state, _, _, _| {
            state.should_move = false;
        }))
        .on_mouse_down(
            MouseButton::Left,
            window.listener_for(&state, |state, _, _, _| {
                state.should_move = true;
            }),
        )
        .on_mouse_up(
            MouseButton::Left,
            window.listener_for(&state, |state, _, _, _| {
                state.should_move = false;
            }),
        )
        .on_mouse_move(window.listener_for(&state, |state, _, window, _| {
            if state.should_move {
                state.should_move = false;
                window.start_window_move();
            }
        }))
        .on_click(|event, window, _| {
            if event.click_count() == 2 {
                if cfg!(target_os = "macos") {
                    window.titlebar_double_click();
                } else {
                    window.zoom_window();
                }
            }
        })
}
