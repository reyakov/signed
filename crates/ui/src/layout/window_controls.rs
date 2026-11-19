//! The window controls: minimize, maximize/restore, close — in the platform's
//! own dress and behavior. Titlebar strips are app-owned: they move the
//! window themselves via [`titlebar_strip`], so a dragged tab or button press
//! never moves it; macOS draws its traffic lights in the shell's reserved pad
//! and needs no controls of ours.

#[cfg(target_os = "linux")]
use gpui::Decorations;
#[cfg(any(target_os = "windows", target_os = "linux"))]
use gpui::SharedString;
#[cfg(any(target_os = "windows", target_os = "linux"))]
use gpui::Styled as _;
#[cfg(any(target_os = "windows", target_os = "linux"))]
use gpui::prelude::FluentBuilder as _;
#[cfg(any(target_os = "windows", target_os = "linux"))]
use gpui::{AnyElement, ParentElement as _};
use gpui::{
    App, ClickEvent, Div, ElementId, InteractiveElement as _, IntoElement, MouseButton, RenderOnce,
    Stateful, StatefulInteractiveElement as _, Window, WindowControlArea, div,
};

#[cfg(any(target_os = "windows", target_os = "linux"))]
use crate::icons::{IconRole, icon};
#[cfg(any(target_os = "windows", target_os = "linux"))]
use crate::theme::ActiveTheme as _;
/// macOS traffic-lights zone: the pad a leading titlebar strip reserves.
pub use crate::theme::TRAFFIC_LIGHT_PAD;
#[cfg(target_os = "windows")]
use crate::theme::{
    WINDOWS_CAPTION_WIDTH as CAPTION_WIDTH, WINDOWS_CLOSE_FOREGROUND, WINDOWS_CLOSE_HOVER,
    WINDOWS_CLOSE_PRESSED,
};

/// The window controls for the trailing end of a titlebar strip.
#[derive(IntoElement)]
pub struct WindowControls {
    id: ElementId,
}

impl WindowControls {
    /// Creates the controls row.
    pub fn new<I>(id: I) -> Self
    where
        I: Into<ElementId>,
    {
        Self { id: id.into() }
    }
}

impl RenderOnce for WindowControls {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let id = self.id.clone();

        #[cfg(target_os = "macos")]
        let controls = {
            // macOS draws native traffic lights in the shell's reserved pad; nothing to render.
            let _ = (id, window, cx);
            div()
        };

        #[cfg(target_os = "windows")]
        let controls = {
            let controls = window.window_controls();
            div()
                .flex_none()
                .flex()
                .flex_row()
                .h_full()
                .when(controls.minimize, |row| {
                    row.child(caption_button(
                        cx,
                        SharedString::from(format!("{id}-minimize")),
                        IconRole::WindowMinimize,
                        WindowControlArea::Min,
                        window.is_minimizable(),
                        false,
                    ))
                })
                .when(controls.maximize, |row| {
                    row.child(caption_button(
                        cx,
                        SharedString::from(format!("{id}-maximize")),
                        if window.is_maximized() {
                            IconRole::WindowRestore
                        } else {
                            IconRole::WindowMaximize
                        },
                        WindowControlArea::Max,
                        window.is_resizable(),
                        false,
                    ))
                })
                .child(caption_button(
                    cx,
                    SharedString::from(format!("{id}-close")),
                    IconRole::Close,
                    WindowControlArea::Close,
                    true,
                    true,
                ))
        };

        #[cfg(target_os = "linux")]
        let controls = {
            let controls = window.window_controls();
            let client_decorated =
                matches!(window.window_decorations(), Decorations::Client { .. });

            div()
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .when(client_decorated && controls.minimize, |row| {
                    row.child(circle_button(
                        cx,
                        SharedString::from(format!("{id}-minimize")),
                        IconRole::WindowMinimize,
                        window.is_minimizable(),
                        |window, _cx| window.minimize_window(),
                    ))
                })
                .when(client_decorated && controls.maximize, |row| {
                    row.child(circle_button(
                        cx,
                        SharedString::from(format!("{id}-maximize")),
                        if window.is_maximized() {
                            IconRole::WindowRestore
                        } else {
                            IconRole::WindowMaximize
                        },
                        window.is_resizable(),
                        |window, _cx| window.zoom_window(),
                    ))
                })
                .when(client_decorated, |row| {
                    row.child(circle_button(
                        cx,
                        SharedString::from(format!("{id}-close")),
                        IconRole::Close,
                        true,
                        |window, _cx| window.remove_window(),
                    ))
                })
        };

        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        let controls = {
            let _ = (id, window, cx);
            div()
        };

        controls
    }
}

/// Double-click on a titlebar strip maximizes where the platform does not do it.
fn maximize_on_double_click() -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    move |event, window, _cx| {
        if event.click_count() != 2 {
            return;
        }
        #[cfg(target_os = "macos")]
        window.titlebar_double_click();
        #[cfg(target_os = "linux")]
        window.zoom_window();
        // Windows handles titlebar double-click natively via WindowControlArea::Drag.
        #[cfg(target_os = "windows")]
        {
            let _ = (event, window);
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        {
            let _ = (event, window);
        }
    }
}

/// Makes an element a titlebar strip; `id` keys its drag state, which
/// `use_state` would otherwise share among every strip in the window.
pub fn titlebar_strip<I>(
    this: Stateful<Div>,
    id: I,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div>
where
    I: Into<ElementId>,
{
    let should_move = window.use_keyed_state(id.into(), cx, |_, _| false);
    this.window_control_area(WindowControlArea::Drag)
        .on_mouse_down_out(window.listener_for(&should_move, |should_move, _, _, _| {
            *should_move = false;
        }))
        .on_mouse_down(
            MouseButton::Left,
            window.listener_for(&should_move, |should_move, _, _, _| {
                *should_move = true;
            }),
        )
        .on_mouse_up(
            MouseButton::Left,
            window.listener_for(&should_move, |should_move, _, _, _| {
                *should_move = false;
            }),
        )
        .on_mouse_move(
            window.listener_for(&should_move, |should_move, _, window, cx| {
                if *should_move {
                    *should_move = false;
                    if !cx.has_active_drag() {
                        window.start_window_move();
                    }
                }
            }),
        )
        .on_click(maximize_on_double_click())
}

/// One Windows caption button.
#[cfg(target_os = "windows")]
fn caption_button(
    cx: &App,
    id: ElementId,
    role: IconRole,
    area: WindowControlArea,
    enabled: bool,
    close: bool,
) -> AnyElement {
    let theme = cx.theme();
    let glyph_size = theme.sizes.small.dense_icon_size;

    let hover = if close {
        WINDOWS_CLOSE_HOVER
    } else {
        theme.colors.muted_hover
    };

    div()
        .id(id)
        .debug_selector({
            let id = SharedString::from(format!("window-control-{}", role.slug()));
            move || id.to_string()
        })
        .occlude()
        .flex()
        .items_center()
        .justify_center()
        .w(CAPTION_WIDTH)
        .h_full()
        .window_control_area(area)
        .text_color(if enabled {
            theme.colors.foreground
        } else {
            theme.colors.muted_foreground
        })
        .when(enabled, |button| {
            button
                .cursor_pointer()
                .hover(|hovered| {
                    hovered.bg(hover).text_color(if close {
                        WINDOWS_CLOSE_FOREGROUND
                    } else {
                        theme.colors.foreground
                    })
                })
                .active(|pressed| {
                    pressed
                        .bg(if close {
                            WINDOWS_CLOSE_PRESSED
                        } else {
                            theme.ramps.secondary.pressed
                        })
                        .text_color(theme.colors.foreground)
                })
        })
        .child(icon(role).size(glyph_size))
        .into_any_element()
}

/// Adwaita's headerbar control: a small circle around a symbolic glyph.
#[cfg(target_os = "linux")]
fn circle_button<I, F>(cx: &App, id: I, role: IconRole, enabled: bool, action: F) -> AnyElement
where
    I: Into<ElementId>,
    F: Fn(&mut Window, &mut App) + 'static,
{
    let theme = cx.theme();
    let small = theme.sizes.small;

    div()
        .id(id)
        .debug_selector(move || format!("window-control-{}", role.slug()))
        .flex()
        .items_center()
        .justify_center()
        .size(small.control_height)
        .rounded(theme.radius.full)
        .text_color(if enabled {
            theme.colors.foreground
        } else {
            theme.colors.muted_foreground
        })
        .when(enabled, |button| {
            button
                .cursor_pointer()
                .hover(|hovered| hovered.bg(theme.colors.muted_hover))
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    action(window, cx);
                })
        })
        .child(icon(role).size(small.dense_icon_size))
        .into_any_element()
}
