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
    Animation, AnimationExt as _, AnyElement, App, Bounds, DispatchPhase, ElementId,
    InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    ParentElement as _, Pixels, RenderOnce, Role, SharedString, StatefulInteractiveElement as _,
    StyleRefinement, Styled as _, Window, anchored, canvas, deferred, div, ease_out_quint, point,
    px,
};

use crate::components::common::{OVERLAY_LAYER, StyledExt as _, v_flex};
use crate::theme::{ActiveTheme as _, POPOVER_MIN_WIDTH};

/// Reports the next open state of a [`Popover`]: `true` to open, `false` to close.
type ChangeHandler = Rc<dyn Fn(bool, &mut Window, &mut App)>;
/// The element the panel anchors below, built during render.
type TriggerFn = Box<dyn FnOnce(&mut Window, &mut App) -> AnyElement>;
/// The panel's contents, built when it opens.
type ContentFn = Box<dyn FnOnce(&mut Window, &mut App) -> AnyElement>;
/// Bounds captured during the last paint, shared with the dismissal listener.
type BoundsCell = Rc<RefCell<Option<Bounds<Pixels>>>>;

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
        let (colors, radius, shadows, enter_duration, panel_border) = {
            let theme = cx.theme();
            (
                theme.colors,
                theme.radius,
                theme.elevations.popover.shadows(),
                theme.motion.overlay,
                theme.colors.border,
            )
        };

        let trigger_bounds: BoundsCell = Rc::new(RefCell::new(None));

        let fallback_label = self
            .a11y_label
            .clone()
            .unwrap_or_else(|| SharedString::from(self.id.to_string()));

        div()
            .relative()
            .refine_style(&self.style)
            .child(
                canvas(
                    {
                        let cell = trigger_bounds.clone();
                        move |bounds, _window, _cx| *cell.borrow_mut() = Some(bounds)
                    },
                    {
                        let on_change = self.on_change.clone();
                        let open = self.open;
                        let cell = trigger_bounds.clone();

                        move |_, _, window, _cx| {
                            window.on_mouse_event({
                                let on_change = on_change.clone();
                                let trigger_bounds = cell;
                                move |event: &MouseDownEvent, phase, window, cx| {
                                    if phase != DispatchPhase::Bubble
                                        || event.button != MouseButton::Left
                                    {
                                        return;
                                    }

                                    let on_trigger = trigger_bounds
                                        .borrow()
                                        .is_some_and(|bounds| bounds.contains(&event.position));

                                    if !on_trigger {
                                        return;
                                    }

                                    if let Some(on_change) = &on_change {
                                        on_change(!open, window, cx);
                                    }

                                    cx.stop_propagation();
                                }
                            });
                        }
                    },
                )
                .absolute()
                .inset_0(),
            )
            .when_some(self.trigger, |wrapper, trigger| {
                wrapper.child(trigger(window, cx))
            })
            .when(self.open, |wrapper| {
                let on_change = self.on_change;
                let label = fallback_label.clone();
                let focus = crate::components::common::focus_handle(&self.id, window, cx);
                let content = self.content.map(|content| content(window, cx));

                let panel = v_flex()
                    .id("popover-panel")
                    .debug_selector(|| "popover-panel".to_string())
                    .occlude()
                    .track_focus(&focus)
                    .role(Role::Dialog)
                    .aria_label(label)
                    .min_w(POPOVER_MIN_WIDTH)
                    .bg(colors.surface)
                    .text_color(colors.surface_foreground)
                    .rounded(radius.overlay)
                    .border_1()
                    .border_color(panel_border)
                    .shadow(shadows)
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
                    .on_key_down({
                        let on_change = on_change.clone();
                        move |event: &KeyDownEvent, window, cx| {
                            if event.keystroke.key == "escape" {
                                if let Some(on_change) = &on_change {
                                    on_change(false, window, cx);
                                }
                                cx.stop_propagation();
                            }
                        }
                    })
                    .when_some(content, |panel, content| panel.child(content))
                    .child(
                        canvas(|_, _, _| {}, {
                            let focus = focus;
                            move |_, _, window, cx| {
                                if !focus.contains_focused(window, cx) {
                                    window.focus(&focus, cx);
                                }
                            }
                        })
                        .absolute()
                        .inset_0(),
                    );

                wrapper.child(
                    deferred(anchored().offset(point(Pixels::ZERO, px(4.))).child(
                        panel.with_animation(
                            "popover-enter",
                            Animation::new(enter_duration).with_easing(ease_out_quint()),
                            |panel, delta| panel.opacity(delta),
                        ),
                    ))
                    .with_priority(OVERLAY_LAYER),
                )
            })
    }
}
