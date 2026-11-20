//! Renders a small text panel next to a hovered element.
//!
//! Most call sites hand [`Tooltip::text`] or [`Tooltip::anchored`] to gpui's
//! `.tooltip(...)` method; [`Tooltip::new`] creates the view for callers that
//! hold the [`gpui::Entity`] themselves.
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
    AnyView, App, AppContext as _, Bounds, Context, ElementId, Entity, InteractiveElement as _,
    IntoElement, ParentElement as _, Pixels, Render, Role, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, canvas, div, point, px,
};

use crate::components::common::{anchor_bounds, floating_panel, record_anchor_bounds};
use crate::theme::{
    ActiveTheme as _, BORDER_WIDTH, TOOLTIP_ANCHOR_GAP, TOOLTIP_MAX_WIDTH, TOOLTIP_OFFSET,
};

const PANEL_BOUNDS_ID: &str = "tooltip-panel-bounds";

/// A small text panel gpui shows next to a hovered element.
pub struct Tooltip {
    text: SharedString,
    anchor: Option<Bounds<Pixels>>,
}

impl Tooltip {
    /// Creates the tooltip view.
    pub fn new<L>(text: L, cx: &mut App) -> Entity<Self>
    where
        L: Into<SharedString>,
    {
        cx.new(|_| Self {
            text: text.into(),
            anchor: None,
        })
    }

    /// Returns the `.tooltip(...)` closure that pins the panel next to the
    /// element identified by `anchor_id`.
    ///
    /// [`Tooltip::text`] places the panel at the pointer instead.
    pub fn anchored<L, I>(
        text: L,
        anchor_id: I,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static
    where
        L: Into<SharedString>,
        I: Into<ElementId>,
    {
        let text = text.into();
        let anchor_id = anchor_id.into();
        move |window, cx| {
            let anchor = anchor_bounds(window.window_handle().window_id(), &anchor_id, cx);
            cx.new(|_| Self {
                text: text.clone(),
                anchor,
            })
            .into()
        }
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Copied out: the mutable reads below need `cx` free of this borrow.
        let theme = cx.theme();
        let surface_foreground = theme.colors.surface_foreground;
        let radius_sm = theme.radius.sm;
        let shadows = theme.elevations.popover.shadows();

        let build_panel = || {
            floating_panel(
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
                shadows.clone(),
            )
        };

        let (wrapper, panel) = match self.anchor {
            Some(parent) => {
                let origin = window.mouse_position() + point(TOOLTIP_OFFSET, TOOLTIP_OFFSET);
                let viewport_height = window.viewport_size().height;

                let height = anchor_bounds(
                    window.window_handle().window_id(),
                    &ElementId::from(PANEL_BOUNDS_ID),
                    cx,
                )
                .map(|bounds| bounds.size.height)
                .unwrap_or_else(|| window.line_height() + px(8.) + BORDER_WIDTH * 2.0);

                let below = viewport_height - TOOLTIP_ANCHOR_GAP - parent.bottom();
                let above = parent.top() - TOOLTIP_ANCHOR_GAP;
                let hang_below = below >= height || below >= above;

                let panel = build_panel()
                    .ml(parent.left() - origin.x)
                    .mt(if hang_below {
                        parent.bottom() + TOOLTIP_ANCHOR_GAP - origin.y
                    } else {
                        parent.top() - TOOLTIP_ANCHOR_GAP - height - origin.y
                    })
                    .child(
                        canvas(
                            record_anchor_bounds(ElementId::from(PANEL_BOUNDS_ID)),
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    );
                (div().debug_selector(|| "tooltip-anchor".to_string()), panel)
            }
            None => {
                let wrapper = div()
                    .debug_selector(|| "tooltip-anchor".to_string())
                    .pl_2()
                    .pt_2();
                (wrapper, build_panel())
            }
        };

        wrapper.child(panel)
    }
}
