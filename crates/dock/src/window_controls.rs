use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, Hsla, InteractiveElement, IntoElement, MouseButton, ParentElement, RenderOnce,
    StatefulInteractiveElement, Styled, Window, div, px,
};
use gpui_component::{ActiveTheme, Icon, IconName, Sizable as _, h_flex};

use crate::TAB_BAR_HEIGHT;

/// The standard width of a window control button.
const CONTROL_WIDTH: f32 = 34.;

#[derive(IntoElement, Clone)]
enum ControlIcon {
    Minimize,
    Restore,
    Maximize,
    Close,
}

impl ControlIcon {
    fn minimize() -> Self {
        Self::Minimize
    }

    fn restore() -> Self {
        Self::Restore
    }

    fn maximize() -> Self {
        Self::Maximize
    }

    fn close() -> Self {
        Self::Close
    }

    fn id(&self) -> &'static str {
        match self {
            Self::Minimize => "minimize",
            Self::Restore => "restore",
            Self::Maximize => "maximize",
            Self::Close => "close",
        }
    }

    fn icon(&self) -> IconName {
        match self {
            Self::Minimize => IconName::WindowMinimize,
            Self::Restore => IconName::WindowRestore,
            Self::Maximize => IconName::WindowMaximize,
            Self::Close => IconName::WindowClose,
        }
    }

    fn window_control_area(&self) -> gpui::WindowControlArea {
        match self {
            Self::Minimize => gpui::WindowControlArea::Min,
            Self::Restore | Self::Maximize => gpui::WindowControlArea::Max,
            Self::Close => gpui::WindowControlArea::Close,
        }
    }

    fn is_close(&self) -> bool {
        matches!(self, Self::Close)
    }

    #[inline]
    fn hover_fg(&self, cx: &App) -> Hsla {
        if self.is_close() {
            cx.theme().danger_foreground
        } else {
            cx.theme().secondary_foreground
        }
    }

    #[inline]
    fn hover_bg(&self, cx: &App) -> Hsla {
        if self.is_close() {
            cx.theme().danger
        } else {
            cx.theme().secondary_hover
        }
    }

    #[inline]
    fn active_bg(&self, cx: &mut App) -> Hsla {
        if self.is_close() {
            cx.theme().danger_active
        } else {
            cx.theme().secondary_active
        }
    }
}

impl RenderOnce for ControlIcon {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let is_linux = cfg!(target_os = "linux");
        let is_windows = cfg!(target_os = "windows");
        let hover_fg = self.hover_fg(cx);
        let hover_bg = self.hover_bg(cx);
        let active_bg = self.active_bg(cx);
        let icon = self.clone();

        div()
            .id(self.id())
            .flex()
            .w(px(CONTROL_WIDTH))
            .h_full()
            .flex_shrink_0()
            .justify_center()
            .content_center()
            .items_center()
            .text_color(cx.theme().foreground)
            .hover(|style| style.bg(hover_bg).text_color(hover_fg))
            .active(|style| style.bg(active_bg).text_color(hover_fg))
            .when(is_windows, |this| {
                this.window_control_area(self.window_control_area())
            })
            .when(is_linux, |this| {
                this.on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                })
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    match icon {
                        Self::Minimize => window.minimize_window(),
                        Self::Restore | Self::Maximize => window.zoom_window(),
                        Self::Close => window.remove_window(),
                    }
                })
            })
            .child(Icon::new(self.icon()).small())
    }
}

pub(crate) fn window_controls(window: &mut Window, cx: &mut App) -> impl IntoElement {
    if cfg!(target_os = "macos") || cfg!(target_family = "wasm") {
        return div().id("window-controls");
    }

    let supported = window.window_controls();

    h_flex()
        .id("window-controls")
        .items_center()
        .flex_shrink_0()
        .h_full()
        // The controls span the title bar but never grow past the tab bar height.
        .when(cfg!(target_os = "windows"), |this| {
            this.max_h(TAB_BAR_HEIGHT)
        })
        .border_l_1()
        .border_b_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().tokens.tab_bar)
        .when(supported.minimize, |this| {
            this.child(ControlIcon::minimize())
        })
        .when(supported.maximize, |this| {
            this.child(if window.is_maximized() {
                ControlIcon::restore()
            } else {
                ControlIcon::maximize()
            })
        })
        .child(ControlIcon::close())
}
