//! Renders a progress bar with a determinate fill or an indeterminate sweep.
//!
//! # Examples
//!
//! ```no_run
//! use ui::progress::Progress;
//!
//! let upload = Progress::new("upload", 0.4);
//! let loading = Progress::new("loading", 0.0).indeterminate(true);
//! ```

use gpui::prelude::FluentBuilder as _;
use gpui::{
    Animation, AnimationExt as _, App, ElementId, InteractiveElement as _, IntoElement,
    ParentElement as _, RenderOnce, Role, SharedString, StatefulInteractiveElement as _,
    StyleRefinement, Styled as _, Window, div, relative,
};

use crate::components::common::{Sizable, StyledExt as _};
use crate::theme::{ActiveTheme as _, PROGRESS_ANIMATION_PERIOD, Size};

/// A bar that paints a determinate fill from its `value`, or an indeterminate sweep.
#[derive(IntoElement)]
#[must_use]
pub struct Progress {
    id: ElementId,
    value: f32,
    indeterminate: bool,
    size: Size,
    a11y_label: Option<SharedString>,
    style: StyleRefinement,
}

impl Progress {
    /// Creates a determinate progress bar at `value` (`0..=1`).
    pub fn new<I>(id: I, value: f32) -> Self
    where
        I: Into<ElementId>,
    {
        Self {
            id: id.into(),
            value,
            indeterminate: false,
            size: Size::default(),
            a11y_label: None,
            style: StyleRefinement::default(),
        }
    }

    /// Sweeps instead of painting a fill, for work of unknown length.
    pub fn indeterminate(mut self, indeterminate: bool) -> Self {
        self.indeterminate = indeterminate;
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

impl gpui::Styled for Progress {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl Sizable for Progress {
    fn apply_size(&mut self, size: Size) {
        self.size = size;
    }
}

impl RenderOnce for Progress {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let colors = theme.colors;
        let ramps = theme.ramps;
        let radius = theme.radius;
        let size = self.size;

        let value = self.value.clamp(0.0, 1.0);

        let track = div().w_full().rounded(radius.full).bg(colors.border);
        let track = match size {
            Size::XSmall => track.h_0p5(),
            Size::Small | Size::Medium => track.h_1(),
            Size::Large => track.h_1p5(),
        };

        div()
            .id(self.id)
            .w_full()
            .refine_style(&self.style)
            .role(Role::ProgressIndicator)
            .when_some(self.a11y_label, |progress, label| {
                progress.aria_label(label)
            })
            .when(!self.indeterminate, |progress| {
                progress
                    .aria_numeric_value(f64::from(value))
                    .aria_min_numeric_value(0.0)
                    .aria_max_numeric_value(1.0)
            })
            .child(track.child(if self.indeterminate {
                div()
                    .h_full()
                    .w(relative(0.3))
                    .rounded(radius.full)
                    .bg(ramps.primary.base)
                    .with_animation(
                        "indeterminate",
                        Animation::new(PROGRESS_ANIMATION_PERIOD)
                            .repeat()
                            .with_max_fps(30.),
                        |pill, delta| pill.ml(relative(delta * 0.7)),
                    )
                    .into_any_element()
            } else {
                div()
                    .h_full()
                    .w(relative(value))
                    .rounded(radius.full)
                    .bg(ramps.primary.base)
                    .into_any_element()
            }))
    }
}
