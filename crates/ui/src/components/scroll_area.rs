//! Renders children in a scrollable pane with an overlay [`Scrollbar`].
//!
//! # Examples
//!
//! ```no_run
//! use gpui::{div, px, ParentElement as _, Styled as _};
//! use ui::scroll_area::{ScrollArea, ScrollAxes};
//!
//! let log = ScrollArea::new("event-log")
//!     .axes(ScrollAxes::Vertical)
//!     .h(px(320.))
//!     .children((0..24).map(|line| div().child(format!("event {line}"))));
//! ```

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, ElementId, InteractiveElement as _, IntoElement, ParentElement, RenderOnce,
    ScrollHandle, SharedString, StatefulInteractiveElement as _, StyleRefinement, Styled, Window,
    div,
};

use crate::components::common::StyledExt as _;
use crate::components::scrollbar::{Scrollbar, ScrollbarVisibility};

/// Which directions the content scrolls in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ScrollAxes {
    /// Vertical only; the default.
    #[default]
    Vertical,
    /// Horizontal only.
    Horizontal,
    /// Both directions.
    Both,
}

/// A scrollable container with an overlay scrollbar.
#[derive(IntoElement)]
#[must_use]
pub struct ScrollArea {
    id: ElementId,
    handle: Option<ScrollHandle>,
    axes: ScrollAxes,
    visibility: ScrollbarVisibility,
    a11y_label: Option<SharedString>,
    style: StyleRefinement,
    children: Vec<AnyElement>,
}

impl ParentElement for ScrollArea {
    fn extend(&mut self, children: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(children);
    }
}

impl Styled for ScrollArea {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl ScrollArea {
    /// Creates a vertically scrolling area.
    pub fn new<I>(id: I) -> Self
    where
        I: Into<ElementId>,
    {
        Self {
            id: id.into(),
            handle: None,
            axes: ScrollAxes::Vertical,
            visibility: ScrollbarVisibility::default(),
            a11y_label: None,
            style: StyleRefinement::default(),
            children: Vec::new(),
        }
    }

    /// Sets which directions the content scrolls in.
    pub fn axes(mut self, axes: ScrollAxes) -> Self {
        self.axes = axes;
        self
    }

    /// Overrides when the scrollbar shows, instead of the platform default.
    pub fn visibility(mut self, visibility: ScrollbarVisibility) -> Self {
        self.visibility = visibility;
        self
    }

    /// Scrolls with a handle held on the owning view.
    pub fn handle(mut self, handle: ScrollHandle) -> Self {
        self.handle = Some(handle);
        self
    }

    /// Overrides the accessibility label of the container.
    pub fn accessibility_label<L>(mut self, label: L) -> Self
    where
        L: Into<SharedString>,
    {
        self.a11y_label = Some(label.into());
        self
    }
}

impl RenderOnce for ScrollArea {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let mut pane_style = self.style.clone();
        pane_style.padding = Default::default();

        div()
            .relative()
            .min_h_0()
            .min_w_0()
            .refine_style(&self.style)
            .child(
                div()
                    .id(self.id.clone())
                    .h_full()
                    .overflow_hidden()
                    .when(self.axes != ScrollAxes::Horizontal, |area| {
                        area.overflow_y_scroll()
                    })
                    .when(self.axes != ScrollAxes::Vertical, |area| {
                        area.overflow_x_scroll()
                    })
                    .when_some(self.handle.clone(), |area, handle| {
                        area.track_scroll(&handle)
                    })
                    .when_some(self.a11y_label, |area, label| area.aria_label(label))
                    .on_scroll(|_, _, cx| {
                        // Keep wheel events from scrolling an outer scroll area too.
                        cx.stop_propagation();
                    })
                    .refine_style(&pane_style)
                    .children(self.children),
            )
            .when_some(self.handle, |wrapper, handle| {
                let id = self.id.clone();

                wrapper
                    .when(self.axes != ScrollAxes::Horizontal, |wrapper| {
                        wrapper.child(
                            Scrollbar::vertical((id.clone(), "v-scrollbar"), handle.clone())
                                .visibility(self.visibility),
                        )
                    })
                    .when(self.axes != ScrollAxes::Vertical, |wrapper| {
                        wrapper.child(
                            Scrollbar::horizontal((id, "h-scrollbar"), handle)
                                .visibility(self.visibility),
                        )
                    })
            })
    }
}
