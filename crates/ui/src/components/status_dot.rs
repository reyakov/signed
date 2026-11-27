//! Renders a small colored bead that marks a row's state.
//!
//! # Examples
//!
//! ```no_run
//! use ui::status_dot::{StatusDot, StatusDotVariant};
//!
//! let dot = StatusDot::new()
//!     .variant(StatusDotVariant::Success)
//!     .label("Deployed");
//! ```

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, ElementId, Hsla, InteractiveElement as _, IntoElement, RenderOnce, Role, SharedString,
    StatefulInteractiveElement as _, StyleRefinement, Styled, Window, div,
};

use crate::components::common::{Sizable, StyledExt};
use crate::theme::{ActiveTheme as _, Size, Theme};

/// How loud a dot is, the same vocabulary as [`crate::components::badge::BadgeVariant`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StatusDotVariant {
    /// The theme's muted foreground color.
    #[default]
    Neutral,
    /// The theme's primary color.
    Accent,
    /// The theme's success color.
    Success,
    /// The theme's warning color.
    Warning,
    /// The theme's destructive color.
    Danger,
}

impl StatusDotVariant {
    fn color(self, theme: &Theme) -> Hsla {
        match self {
            Self::Neutral => theme.colors.muted_foreground,
            Self::Accent => theme.ramps.primary.base,
            Self::Success => theme.ramps.success.base,
            Self::Warning => theme.ramps.warning.base,
            Self::Danger => theme.ramps.destructive.base,
        }
    }
}

/// A small circular bead marking a row's state.
#[derive(Default, IntoElement)]
#[must_use]
pub struct StatusDot {
    variant: StatusDotVariant,
    size: Size,
    label: Option<SharedString>,
    style: StyleRefinement,
}

impl StatusDot {
    /// Creates an idle dot.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets how loud the dot is.
    pub fn variant(mut self, variant: StatusDotVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Sets the state the dot announces to screen readers.
    pub fn label<L>(mut self, label: L) -> Self
    where
        L: Into<SharedString>,
    {
        self.label = Some(label.into());
        self
    }
}

impl Sizable for StatusDot {
    fn apply_size(&mut self, size: Size) {
        self.size = size;
    }
}

impl Styled for StatusDot {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for StatusDot {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let size = self.size;
        let color = self.variant.color(theme);

        let dot = div()
            .flex_none()
            .rounded(theme.radius.full)
            .bg(color)
            .map(|dot| match size {
                Size::XSmall => dot.size_1p5(),
                Size::Small | Size::Medium => dot.size_2(),
                Size::Large => dot.size_2p5(),
            })
            .refine_style(&self.style);

        match self.label {
            Some(label) => dot
                .id((ElementId::from("status-dot"), label.clone()))
                .role(Role::Status)
                .aria_label(label)
                .into_any_element(),
            None => dot.into_any_element(),
        }
    }
}
