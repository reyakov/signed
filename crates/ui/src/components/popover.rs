//! Renders a floating panel anchored below its trigger, closed by outside
//! clicks and Escape.
//!
//! # Examples
//!
//! ```no_run
//! use gpui::{IntoElement as _, ParentElement as _, SharedString, div};
//! use ui::popover::Popover;
//!
//! let popover = Popover::new("options")
//!     .trigger(|_window, _cx| {
//!         div().child(SharedString::from("Options")).into_any_element()
//!     })
//!     .content(|_window, _cx| {
//!         div()
//!             .child(SharedString::from("Panel body"))
//!             .into_any_element()
//!     })
//!     .on_change(|open, _window, _cx| println!("open: {open}"));
//! ```

use std::cell::RefCell;
use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, ElementId, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement as _, RenderOnce, Role, SharedString, StatefulInteractiveElement as _,
    StyleRefinement, Styled as _, Window, div,
};

use crate::components::common::{ElementIdExt as _, StyledExt as _, v_flex};
use crate::components::dialog::Dismiss;
use crate::components::menu::{
    ChangeHandler, TriggerBoundsCell, anchored_overlay, focus_capture, trigger_toggle,
};
use crate::theme::{ActiveTheme as _, POPOVER_MIN_WIDTH};

/// Key context of the popover panel.
pub const POPOVER_CONTEXT: &str = "Popover";

/// The element the panel anchors below, built during render.
type TriggerFn = Box<dyn FnOnce(&mut Window, &mut App) -> AnyElement>;
/// The panel's contents, built when it opens.
type ContentFn = Box<dyn FnOnce(&mut Window, &mut App) -> AnyElement>;

/// A floating panel anchored below its trigger.
#[derive(IntoElement)]
#[must_use]
pub struct Popover {
    id: ElementId,
    open: bool,
    a11y_label: Option<SharedString>,
    on_change: Option<ChangeHandler>,
    trigger: Option<TriggerFn>,
    content: Option<ContentFn>,
    style: StyleRefinement,
}

impl Popover {
    /// Creates a closed popover.
    pub fn new<I>(id: I) -> Self
    where
        I: Into<ElementId>,
    {
        Self {
            id: id.into(),
            open: false,
            a11y_label: None,
            on_change: None,
            trigger: None,
            content: None,
            style: StyleRefinement::default(),
        }
    }

    /// Sets the current open state.
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// Reports the next open state, from dismissals and app changes alike.
    pub fn on_change<F>(mut self, handler: F) -> Self
    where
        F: Fn(bool, &mut Window, &mut App) + 'static,
    {
        self.on_change = Some(Rc::new(handler));
        self
    }

    /// Sets the element the panel anchors below, rendered in place.
    pub fn trigger<F>(mut self, trigger: F) -> Self
    where
        F: FnOnce(&mut Window, &mut App) -> AnyElement + 'static,
    {
        self.trigger = Some(Box::new(trigger));
        self
    }

    /// Sets the panel's contents.
    pub fn content<F>(mut self, content: F) -> Self
    where
        F: FnOnce(&mut Window, &mut App) -> AnyElement + 'static,
    {
        self.content = Some(Box::new(content));
        self
    }

    /// Overrides the accessibility label of the panel.
    pub fn accessibility_label<L>(mut self, label: L) -> Self
    where
        L: Into<SharedString>,
    {
        self.a11y_label = Some(label.into());
        self
    }
}

impl gpui::Styled for Popover {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Popover {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let (colors, radius, shadows, enter_duration) = {
            let theme = cx.theme();
            (
                theme.colors,
                theme.radius,
                theme.elevations.popover.shadows(),
                theme.motion.overlay,
            )
        };

        let trigger_bounds: TriggerBoundsCell = Rc::new(RefCell::new(None));

        // Keyboard focus rides the panel across frames.
        let focus = self.id.focus_handle(window, cx);
        self.id.track_overlay_focus(&focus, self.open, window, cx);

        let fallback_label = self
            .a11y_label
            .clone()
            .unwrap_or_else(|| SharedString::from(self.id.to_string()));

        div()
            .relative()
            .refine_style(&self.style)
            .child(trigger_toggle(
                &trigger_bounds,
                self.open,
                self.on_change.clone(),
                MouseButton::Left,
            ))
            .when_some(self.trigger, |wrapper, trigger| {
                wrapper.child(trigger(window, cx))
            })
            .when(self.open, |wrapper| {
                let on_change = self.on_change;
                let label = fallback_label.clone();
                let content = self.content.map(|content| content(window, cx));

                wrapper.child(anchored_overlay(
                    None,
                    v_flex()
                        .id((self.id.clone(), "popover-panel"))
                        .debug_selector(|| "popover-panel".to_string())
                        .occlude()
                        .track_focus(&focus)
                        .key_context(POPOVER_CONTEXT)
                        .role(Role::Dialog)
                        .aria_label(label)
                        .min_w(POPOVER_MIN_WIDTH)
                        .text_color(colors.surface_foreground)
                        .floating_panel(cx, radius.overlay, shadows)
                        .overflow_hidden()
                        .on_mouse_down_out({
                            let on_change = on_change.clone();
                            let trigger_bounds = trigger_bounds.clone();
                            move |event, window, cx| {
                                if trigger_bounds
                                    .borrow()
                                    .is_some_and(|bounds| bounds.contains(&event.position))
                                {
                                    return;
                                }
                                if let Some(on_change) = &on_change {
                                    on_change(false, window, cx);
                                }
                            }
                        })
                        .on_action({
                            let on_change = on_change.clone();
                            move |_: &Dismiss, window, cx| {
                                if let Some(on_change) = &on_change {
                                    on_change(false, window, cx);
                                }
                                cx.stop_propagation();
                            }
                        })
                        .when_some(content, |panel, content| panel.child(content))
                        .child(focus_capture(focus.clone(), move |window, cx| {
                            !focus.contains_focused(window, cx)
                        })),
                    Some(("popover-enter", enter_duration)),
                ))
            })
    }
}
