//! Renders a loading ring that spins for unknown work, or holds a determinate
//! arc for known progress.
//!
//! # Examples
//!
//! ```no_run
//! use ui::spinner::{Spinner, SpinnerVariant};
//!
//! let loading = Spinner::new("loading");
//! let progress = Spinner::new("progress").value(0.5);
//! let glyph = Spinner::new("glyph").variant(SpinnerVariant::Icon);
//! ```

use std::cell::Cell;
use std::f32::consts::{PI, TAU};
use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    Animation, AnimationElement, AnimationExt as _, App, Canvas, ElementId, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, Path, PathBuilder, Pixels, Point,
    RenderOnce, Role, SharedString, StatefulInteractiveElement as _, StyleRefinement, Styled as _,
    Window, canvas, div, point, px,
};

use crate::components::common::{Sizable, StyledExt as _};
use crate::icons::{Icon, IconRole, icon};
use crate::theme::{ActiveTheme as _, SPINNER_PERIOD, Size};

/// The share of the circle the spinning arc paints.
const SPIN_SWEEP: f32 = 0.75;
/// The ring stroke as a share of the side.
const RING_STROKE: f32 = 0.125;

/// The visual language of a [`Spinner`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SpinnerVariant {
    /// The painted arc ring.
    #[default]
    Ring,
    /// The platform loader glyph, rotated.
    Icon,
}

/// A loading ring that spins for unknown work or paints a determinate arc.
#[derive(IntoElement)]
#[must_use]
pub struct Spinner {
    id: ElementId,
    value: Option<f32>,
    variant: SpinnerVariant,
    size: Size,
    color: Option<Hsla>,
    a11y_label: Option<SharedString>,
    style: StyleRefinement,
}

impl Spinner {
    /// Creates a spinning ring.
    pub fn new<I>(id: I) -> Self
    where
        I: Into<ElementId>,
    {
        Self {
            id: id.into(),
            value: None,
            variant: SpinnerVariant::default(),
            size: Size::default(),
            color: None,
            a11y_label: None,
            style: StyleRefinement::default(),
        }
    }

    /// Overrides the arc color, for accent-filled surfaces.
    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    /// Sets the spinner's visual language.
    pub fn variant(mut self, variant: SpinnerVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Paints a determinate arc at `value` (`0..=1`) instead of spinning.
    pub fn value(mut self, value: f32) -> Self {
        self.value = Some(value);
        self
    }

    /// Sets the accessibility label.
    pub fn accessibility_label<L>(mut self, label: L) -> Self
    where
        L: Into<SharedString>,
    {
        self.a11y_label = Some(label.into());
        self
    }
}

impl gpui::Styled for Spinner {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl Sizable for Spinner {
    fn apply_size(&mut self, size: Size) {
        self.size = size;
    }
}

impl RenderOnce for Spinner {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let side = self.size.icon_rems() * window.rem_size();
        let stroke = side * RING_STROKE;
        let track = theme.colors.border;
        let accent = self.color.unwrap_or(theme.ramps.primary.base);
        let value = self.value.map(|value| value.clamp(0.0, 1.0));

        div()
            .id(self.id)
            .flex_none()
            .size(side)
            .refine_style(&self.style)
            .role(Role::ProgressIndicator)
            .when_some(self.a11y_label, |spinner, label| spinner.aria_label(label))
            .when_some(value, |spinner, value| {
                spinner
                    .aria_numeric_value(f64::from(value))
                    .aria_min_numeric_value(0.0)
                    .aria_max_numeric_value(1.0)
            })
            .child(match (self.variant, value) {
                (SpinnerVariant::Ring, Some(value)) => {
                    determinate_ring(side, stroke, track, accent, value).into_any_element()
                }
                (SpinnerVariant::Ring, None) => {
                    spinning_ring(side, stroke, accent).into_any_element()
                }
                (SpinnerVariant::Icon, Some(_)) => icon(IconRole::Loader)
                    .size(side)
                    .color(accent)
                    .into_any_element(),
                (SpinnerVariant::Icon, None) => spinning_icon(side, accent).into_any_element(),
            })
    }
}

fn determinate_ring(
    side: Pixels,
    stroke: Pixels,
    track: Hsla,
    accent: Hsla,
    value: f32,
) -> Canvas<()> {
    canvas(
        |_, _, _| (),
        move |bounds, (), window, _| {
            let center = bounds.center();
            let radius = bounds.size.width / 2.0 - stroke / 2.0;
            paint_circle(center, radius, stroke, track, window);
            if value >= 1.0 {
                paint_circle(center, radius, stroke, accent, window);
            } else if value > 0.0
                && let Some(path) = arc_path(center, radius, stroke, 0.0, value * TAU)
            {
                window.paint_path(path, accent);
            }
        },
    )
    .size(side)
}

fn spinning_ring(side: Pixels, stroke: Pixels, accent: Hsla) -> AnimationElement<Canvas<()>> {
    let angle = Rc::new(Cell::new(0.0));
    let spun = angle.clone();
    canvas(
        |_, _, _| (),
        move |bounds, (), window, _| {
            let radius = bounds.size.width / 2.0 - stroke / 2.0;
            if let Some(path) = arc_path(
                bounds.center(),
                radius,
                stroke,
                spun.get(),
                SPIN_SWEEP * TAU,
            ) {
                window.paint_path(path, accent);
            }
        },
    )
    .size(side)
    .with_animation(
        "spin",
        Animation::new(SPINNER_PERIOD).repeat().with_max_fps(60.),
        move |ring, delta| {
            angle.set(delta * TAU);
            ring
        },
    )
}

fn spinning_icon(side: Pixels, accent: Hsla) -> Icon {
    icon(IconRole::Loader)
        .size(side)
        .color(accent)
        .with_rotation_animation(SPINNER_PERIOD)
}

/// Paints a stroked circle into the window.
fn paint_circle(
    center: Point<Pixels>,
    radius: Pixels,
    stroke: Pixels,
    color: Hsla,
    window: &mut Window,
) {
    if let Some(path) = circle_path(center, radius, stroke) {
        window.paint_path(path, color);
    }
}

/// A point on the ring at `angle`, clockwise from 12 o'clock.
fn ring_point(center: Point<Pixels>, radius: Pixels, angle: f32) -> Point<Pixels> {
    point(
        center.x + radius * angle.sin(),
        center.y - radius * angle.cos(),
    )
}

/// A stroked arc from `start`, sweeping clockwise by `sweep` radians.
fn arc_path(
    center: Point<Pixels>,
    radius: Pixels,
    stroke: Pixels,
    start: f32,
    sweep: f32,
) -> Option<Path> {
    let mut builder = PathBuilder::stroke(stroke);
    builder.move_to(ring_point(center, radius, start));
    builder.arc_to(
        point(radius, radius),
        px(0.),
        sweep > PI,
        true,
        ring_point(center, radius, start + sweep),
    );
    builder.build().ok()
}

/// A stroked full circle; `None` on degenerate geometry.
fn circle_path(center: Point<Pixels>, radius: Pixels, stroke: Pixels) -> Option<Path> {
    let mut builder = PathBuilder::stroke(stroke);
    builder.move_to(ring_point(center, radius, 0.0));
    builder.arc_to(
        point(radius, radius),
        px(0.),
        false,
        true,
        ring_point(center, radius, PI),
    );
    builder.arc_to(
        point(radius, radius),
        px(0.),
        false,
        true,
        ring_point(center, radius, TAU),
    );
    builder.build().ok()
}
