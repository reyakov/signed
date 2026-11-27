//! The window controls: minimize, maximize/restore, close — in the platform's
//! own dress and behavior. Titlebar strips are app-owned and move the window
//! themselves (see [`crate::layout::titlebar::Titlebar::strip`]), so a dragged
//! tab or button press never moves it; macOS draws its traffic lights in the
//! shell's reserved pad and needs no controls of ours.

/// macOS traffic-lights zone: the pad a leading titlebar strip reserves.
pub use crate::theme::TRAFFIC_LIGHT_PAD;
#[cfg(target_os = "linux")]
use gpui::Decorations;
#[cfg(any(target_os = "windows", target_os = "linux"))]
use gpui::SharedString;
#[cfg(any(target_os = "windows", target_os = "linux"))]
use gpui::Styled as _;
#[cfg(any(target_os = "windows", target_os = "linux"))]
use gpui::prelude::FluentBuilder as _;
use gpui::{App, ElementId, IntoElement, RenderOnce, Window, div};
#[cfg(any(target_os = "windows", target_os = "linux"))]
use gpui::{
    Div, InteractiveElement as _, ParentElement as _, Stateful, StatefulInteractiveElement as _,
    WindowControlArea, rems,
};

#[cfg(any(target_os = "windows", target_os = "linux"))]
use crate::components::common::h_flex;
#[cfg(any(target_os = "windows", target_os = "linux"))]
use crate::icons::{Icon, IconRole};
#[cfg(any(target_os = "windows", target_os = "linux"))]
use crate::theme::ActiveTheme as _;
#[cfg(any(target_os = "windows", target_os = "linux"))]
use crate::theme::OVERLAY_ICON_SIZE;
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

    /// One Windows caption button.
    #[cfg(target_os = "windows")]
    fn caption_button(
        cx: &App,
        id: ElementId,
        role: IconRole,
        area: WindowControlArea,
        enabled: bool,
        close: bool,
    ) -> Stateful<Div> {
        let theme = cx.theme();

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
            .child(Icon::from(role).size(OVERLAY_ICON_SIZE))
    }

    /// Adwaita's headerbar control: a small circle around a symbolic glyph.
    #[cfg(target_os = "linux")]
    fn circle_button(
        cx: &App,
        id: ElementId,
        role: IconRole,
        enabled: bool,
        action: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Stateful<Div> {
        let theme = cx.theme();

        div()
            .id(id)
            .debug_selector(move || format!("window-control-{}", role.slug()))
            .flex()
            .items_center()
            .justify_center()
            .size_7()
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
            .child(Icon::from(role).size(OVERLAY_ICON_SIZE))
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
                    row.child(Self::caption_button(
                        cx,
                        ElementId::from((id.clone(), "minimize")),
                        IconRole::WindowMinimize,
                        WindowControlArea::Min,
                        window.is_minimizable(),
                        false,
                    ))
                })
                .when(controls.maximize, |row| {
                    row.child(Self::caption_button(
                        cx,
                        ElementId::from((id.clone(), "maximize")),
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
                .child(Self::caption_button(
                    cx,
                    ElementId::from((id, "close")),
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

            h_flex()
                .flex_none()
                .h_full()
                .gap_1()
                .when(client_decorated && controls.minimize, |row| {
                    row.child(Self::circle_button(
                        cx,
                        ElementId::from((id.clone(), "minimize")),
                        IconRole::WindowMinimize,
                        window.is_minimizable(),
                        |window, _cx| window.minimize_window(),
                    ))
                })
                .when(client_decorated && controls.maximize, |row| {
                    row.child(Self::circle_button(
                        cx,
                        ElementId::from((id.clone(), "maximize")),
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
                    row.child(Self::circle_button(
                        cx,
                        ElementId::from((id, "close")),
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
