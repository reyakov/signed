//! Renders a small text panel next to a hovered element.
//!
//! Most call sites hand [`Tooltip::text`] to gpui's `.tooltip(...)` method;
//! [`Tooltip::new`] creates the view for callers that hold the [`gpui::Entity`]
//! themselves. The panel follows the pointer.
//!
//! # Examples
//!
//! ```no_run
//! use ui::tooltip::Tooltip;
//!
//! # let mut cx: gpui::App = unimplemented!();
//! let tip = Tooltip::new("Delete the selected project", &mut cx);
//! ```

use gpui::{
    AnyView, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, Role, SharedString, StatefulInteractiveElement as _, Styled as _,
    Window, div,
};

use crate::components::common::floating_panel;
use crate::theme::{ActiveTheme as _, TOOLTIP_MAX_WIDTH};

/// A small text panel gpui shows next to a hovered element.
pub struct Tooltip {
    text: SharedString,
}

impl Tooltip {
    /// Creates the tooltip view.
    pub fn new<L>(text: L, cx: &mut App) -> Entity<Self>
    where
        L: Into<SharedString>,
    {
        cx.new(|_| Self { text: text.into() })
    }

    /// Returns the `.tooltip(...)` closure for a plain text tooltip.
    pub fn text<L>(text: L) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static
    where
        L: Into<SharedString>,
    {
        let text = text.into();
        move |_, cx| Tooltip::new(text.clone(), cx).into()
    }
}

impl Render for Tooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let surface_foreground = theme.colors.surface_foreground;
        let radius_sm = theme.radius.sm;
        let shadows = theme.elevations.popover.shadows();

        div()
            .debug_selector(|| "tooltip-anchor".to_string())
            .pl_2()
            .pt_2()
            .child(floating_panel(
                cx,
                radius_sm,
                div()
                    .id("tooltip-panel")
                    .role(Role::Tooltip)
                    .debug_selector(|| "tooltip-panel".to_string())
                    .px_2()
                    .py_1()
                    .max_w(TOOLTIP_MAX_WIDTH)
                    .text_sm()
                    .text_color(surface_foreground)
                    .child(self.text.clone()),
                shadows,
            ))
    }
}
