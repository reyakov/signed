//! Renders a controlled slider that reports its next value on click, drag, and arrow keys.
//!
//! # Examples
//!
//! ```no_run
//! use ui::slider::Slider;
//!
//! let opacity = Slider::new("opacity", 1.0)
//!     .step(0.25)
//!     .on_change(|value, _window, _cx| println!("opacity: {value}"));
//! ```

use std::cell::Cell;
use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, Bounds, DispatchPhase, ElementId, InteractiveElement as _, IntoElement, KeyDownEvent,
    MouseButton, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, RenderOnce, Role,
    SharedString, StatefulInteractiveElement as _, StyleRefinement, Styled as _, Window, canvas,
    div, relative,
};

use crate::components::common::{Sizable, StyledExt as _, focus_ring, h_flex};
use crate::theme::{ActiveTheme as _, DISABLED_OPACITY, Size};

/// The change reporter, fired with the next value by click, drag, and arrows.
type ChangeHandler = Rc<dyn Fn(f32, &mut Window, &mut App)>;

/// The pointer→value setter shared by the click and drag listeners.
type ValueSetter = Rc<dyn Fn(Pixels, &mut Window, &mut App)>;

/// The arrow-key stride as a share of the range when the slider is continuous.
const FALLBACK_STEP_FRACTION: f32 = 0.05;

impl Slider {
    /// Clamps `value` into `[min, max]` and snaps it to `step` when stepped.
    fn constrain(min: f32, max: f32, step: f32, value: f32) -> f32 {
        let clamped = value.clamp(min, max);
        if step > 0.0 {
            (min + ((clamped - min) / step).round() * step).clamp(min, max)
        } else {
            clamped
        }
    }
}

/// A controlled slider over `[min, max]`.
#[derive(IntoElement)]
#[must_use]
pub struct Slider {
    id: ElementId,
    min: f32,
    max: f32,
    step: f32,
    value: f32,
    size: Size,
    disabled: bool,
    a11y_label: Option<SharedString>,
    on_change: Option<ChangeHandler>,
    style: StyleRefinement,
}

impl Slider {
    /// Creates a slider over `0..=1` with no stepping, at `value`.
    pub fn new<I>(id: I, value: f32) -> Self
    where
        I: Into<ElementId>,
    {
        Self {
            id: id.into(),
            min: 0.0,
            max: 1.0,
            step: 0.0,
            value,
            size: Size::default(),
            disabled: false,
            a11y_label: None,
            on_change: None,
            style: StyleRefinement::default(),
        }
    }

    /// Sets the range's lower bound.
    pub fn min(mut self, min: f32) -> Self {
        self.min = min;
        self
    }

    /// Sets the range's upper bound.
    pub fn max(mut self, max: f32) -> Self {
        self.max = max;
        self
    }

    /// Sets the step; values snap to `min + n * step`. `0` is continuous.
    pub fn step(mut self, step: f32) -> Self {
        self.step = step;
        self
    }

    /// Renders the slider inert, unfocusable, and dimmed; disabled always wins.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Sets the accessibility label, required for an unlabeled slider.
    pub fn accessibility_label<L>(mut self, label: L) -> Self
    where
        L: Into<SharedString>,
    {
        self.a11y_label = Some(label.into());
        self
    }

    /// Sets the change handler, fired with the next value by click, drag, and arrows.
    pub fn on_change<F>(mut self, handler: F) -> Self
    where
        F: Fn(f32, &mut Window, &mut App) + 'static,
    {
        self.on_change = Some(Rc::new(handler));
        self
    }
}

impl gpui::Styled for Slider {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl Sizable for Slider {
    fn apply_size(&mut self, size: Size) {
        self.size = size;
    }
}

impl RenderOnce for Slider {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let colors = theme.colors;
        let ramps = theme.ramps;
        let radius = theme.radius;
        let metrics = theme.sizes.resolve(self.size);

        let row_height = metrics.icon_size * 2.5;
        let track_height = metrics.icon_size / 4.0;
        let knob = metrics.indicator_size;

        let value = self.value.clamp(self.min, self.max);
        let fraction = if self.max > self.min {
            (value - self.min) / (self.max - self.min)
        } else {
            0.0
        };

        let bounds_cell = Rc::new(Cell::new(None::<Bounds<Pixels>>));
        let focus_handle = crate::components::common::focus_handle(&self.id, window, cx);

        let dragging =
            window.use_keyed_state((self.id.clone(), "slider-dragging"), cx, |_, _| false);

        let set_value: ValueSetter = {
            let bounds_cell = bounds_cell.clone();
            let on_change = self.on_change.clone();
            let (min, max, step) = (self.min, self.max, self.step);

            Rc::new(move |x: Pixels, window: &mut Window, cx: &mut App| {
                if let (Some(bounds), Some(on_change)) = (bounds_cell.get(), on_change.as_ref())
                    && bounds.size.width > Pixels::ZERO
                {
                    let fraction = ((x - bounds.origin.x) / bounds.size.width).clamp(0.0, 1.0);
                    on_change(
                        Self::constrain(min, max, step, min + fraction * (max - min)),
                        window,
                        cx,
                    );
                }
            })
        };

        let down_dragging = dragging.clone();
        let down_set_value = set_value.clone();
        let disabled = self.disabled;

        h_flex()
            .id(self.id)
            .relative()
            .w_full()
            .h(row_height)
            .refine_style(&self.style)
            .role(Role::Slider)
            .when_some(self.a11y_label.clone(), |slider, label| {
                slider.aria_label(label)
            })
            .aria_numeric_value(f64::from(value))
            .aria_min_numeric_value(f64::from(self.min))
            .aria_max_numeric_value(f64::from(self.max))
            .when(self.disabled, |slider| slider.opacity(DISABLED_OPACITY))
            .when(!self.disabled, |slider| {
                slider
                    .track_focus(&focus_handle)
                    .tab_stop(true)
                    .focus_visible(|style| style.shadow(focus_ring(colors.ring)))
                    .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                        down_dragging.update(cx, |dragging, _| *dragging = true);
                        down_set_value(event.position.x, window, cx);
                    })
                    .on_key_down(move |event: &KeyDownEvent, window, cx| {
                        if !focus_handle.contains_focused(window, cx) {
                            return;
                        }

                        let single_step = if self.step > 0.0 {
                            self.step
                        } else {
                            (self.max - self.min) * FALLBACK_STEP_FRACTION
                        };

                        let next = match event.keystroke.key.as_str() {
                            "left" | "down" => Some(value - single_step),
                            "right" | "up" => Some(value + single_step),
                            "home" => Some(self.min),
                            "end" => Some(self.max),
                            _ => None,
                        };

                        if let Some(next) = next
                            && let Some(on_change) = self.on_change.as_ref()
                        {
                            on_change(
                                Self::constrain(self.min, self.max, self.step, next),
                                window,
                                cx,
                            );
                            cx.stop_propagation();
                        }
                    })
            })
            .child(
                canvas(
                    {
                        let bounds_cell = bounds_cell.clone();
                        move |bounds, _, _| bounds_cell.set(Some(bounds))
                    },
                    {
                        let dragging = dragging.clone();
                        let set_value = set_value.clone();
                        move |_, _, window, _cx| {
                            if disabled {
                                return;
                            }
                            window.on_mouse_event({
                                let dragging = dragging.clone();
                                let set_value = set_value.clone();
                                move |event: &MouseMoveEvent, phase, window, cx| {
                                    if phase == DispatchPhase::Bubble
                                        && *dragging.read(cx)
                                        && event.pressed_button == Some(MouseButton::Left)
                                    {
                                        set_value(event.position.x, window, cx);
                                    }
                                }
                            });
                            window.on_mouse_event({
                                let dragging = dragging.clone();
                                move |event: &MouseUpEvent, phase, _window, cx| {
                                    if phase == DispatchPhase::Bubble
                                        && event.button == MouseButton::Left
                                    {
                                        dragging.update(cx, |dragging, _| *dragging = false);
                                    }
                                }
                            });
                        }
                    },
                )
                .absolute()
                .inset_0(),
            )
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top((row_height - track_height) / 2.0)
                    .h(track_height)
                    .w_full()
                    .rounded(radius.full)
                    .bg(colors.border),
            )
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top((row_height - track_height) / 2.0)
                    .h(track_height)
                    .w(relative(fraction))
                    .rounded(radius.full)
                    .bg(ramps.primary.base),
            )
            .child(
                h_flex()
                    .absolute()
                    .left_0()
                    .top_0()
                    .h_full()
                    .w(relative(fraction))
                    .justify_end()
                    .items_center()
                    .child(
                        div()
                            .size(knob)
                            .flex_none()
                            .rounded(radius.full)
                            .bg(colors.primary_foreground)
                            .border_1()
                            .border_color(colors.border)
                            .mr(-knob / 2.0),
                    ),
            )
    }
}
