//! Renders a hairline rule between sibling content.
//!
//! # Examples
//!
//! ```no_run
//! use ui::separator::Separator;
//!
//! let rule = Separator::horizontal();
//! let divider = Separator::vertical();
//! ```

use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, InteractiveElement as _, IntoElement, RenderOnce, StyleRefinement, Styled as _, Window,
    div,
};

use crate::components::common::StyledExt as _;
use crate::theme::{ActiveTheme as _, BORDER_WIDTH};

/// The axis a [`Separator`] rule runs along.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SeparatorOrientation {
    /// A horizontal rule between stacked content.
    #[default]
    Horizontal,
    /// A vertical rule between side-by-side content.
    Vertical,
}

/// The debug-bounds selector, for headless tests.
type DebugSelector = Rc<dyn Fn() -> String>;

/// A hairline rule that divides sibling content.
#[derive(IntoElement)]
#[must_use]
pub struct Separator {
    orientation: SeparatorOrientation,
    debug_selector: Option<DebugSelector>,
    style: StyleRefinement,
}

impl Separator {
    /// Creates a horizontal rule.
    pub fn horizontal() -> Self {
        Self {
            orientation: SeparatorOrientation::Horizontal,
            debug_selector: None,
            style: StyleRefinement::default(),
        }
    }

    /// Creates a vertical rule.
    pub fn vertical() -> Self {
        Self {
            orientation: SeparatorOrientation::Vertical,
            debug_selector: None,
            style: StyleRefinement::default(),
        }
    }

    /// Sets which axis the rule runs along.
    pub fn orientation(mut self, orientation: SeparatorOrientation) -> Self {
        self.orientation = orientation;
        self
    }

    /// Sets the debug-bounds selector, for headless tests.
    pub fn debug_selector<F>(mut self, selector: F) -> Self
    where
        F: Fn() -> String + 'static,
    {
        self.debug_selector = Some(Rc::new(selector));
        self
    }
}

impl gpui::Styled for Separator {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Separator {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let border = cx.theme().colors.border;

        div()
            .flex_none()
            .bg(border)
            .when_some(self.debug_selector, |rule, selector| {
                rule.debug_selector(move || selector())
            })
            .map(|rule| match self.orientation {
                SeparatorOrientation::Horizontal => rule.w_full().h(BORDER_WIDTH),
                SeparatorOrientation::Vertical => rule.self_stretch().w(BORDER_WIDTH),
            })
            .refine_style(&self.style)
    }
}
