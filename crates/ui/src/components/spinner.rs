//! Renders a loader icon that spins for unknown work.
//!
//! # Examples
//!
//! ```no_run
//! use ui::spinner::Spinner;
//!
//! let loading = Spinner::new("loading");
//! ```

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, ElementId, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce,
    Role, SharedString, StatefulInteractiveElement as _, StyleRefinement, Styled as _, Window, div,
};

use crate::components::common::{Sizable, StyledExt as _};
use crate::icons::{IconRole, icon};
use crate::theme::{ActiveTheme as _, SPINNER_PERIOD, Size};

/// A loader icon that spins for unknown work.
#[derive(IntoElement)]
#[must_use]
pub struct Spinner {
    id: ElementId,
    size: Size,
    color: Option<Hsla>,
    a11y_label: Option<SharedString>,
    style: StyleRefinement,
}

impl Spinner {
    /// Creates a spinning loader icon.
    pub fn new<I>(id: I) -> Self
    where
        I: Into<ElementId>,
    {
        Self {
            id: id.into(),
            size: Size::default(),
            color: None,
            a11y_label: None,
            style: StyleRefinement::default(),
        }
    }

    /// Overrides the icon color, for accent-filled surfaces.
    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
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
        let accent = self.color.unwrap_or(theme.ramps.primary.base);

        div()
            .id(self.id)
            .flex_none()
            .size(side)
            .refine_style(&self.style)
            .role(Role::ProgressIndicator)
            .when_some(self.a11y_label, |spinner, label| spinner.aria_label(label))
            .child(
                icon(IconRole::Loader)
                    .size(side)
                    .color(accent)
                    .with_rotation_animation(SPINNER_PERIOD),
            )
    }
}
