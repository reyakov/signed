//! Application window layout: a titlebar over a panel row.

use gpui::{
    App, BoxShadow, Div, ElementId, FocusHandle, Hsla, Refineable as _, StyleRefinement, Styled,
    Window, div, px,
};

use crate::theme::{FOCUS_RING_ALPHA, FOCUS_RING_SPREAD};

pub mod panel;
pub mod resizable;
pub mod resize_handle;
pub mod shell;
pub mod titlebar;
pub mod window_controls;

mod window_frame;

/// A row whose children center on the cross axis.
#[must_use]
pub fn h_flex() -> Div {
    div().flex().flex_row().items_center()
}

/// A column whose children stack on the main axis.
#[must_use]
pub fn v_flex() -> Div {
    div().flex().flex_col()
}

/// Extensions over [`Styled`], shared by the layout views.
pub trait StyledExt: Styled + Sized {
    #[must_use]
    fn refine_style(mut self, style: &StyleRefinement) -> Self {
        self.style().refine(style);
        self
    }
}

impl<E: Styled> StyledExt for E {}

/// Returns a focus handle for the given element ID.
pub fn focus_handle(id: &ElementId, window: &mut Window, cx: &mut App) -> FocusHandle {
    window
        .with_element_namespace(id.clone(), |window| {
            window.use_keyed_state("focus-handle", cx, |_, cx| cx.focus_handle())
        })
        .read(cx)
        .clone()
}

/// Returns a focus ring shadow with the given color at half ink.
pub fn focus_ring(color: Hsla) -> Vec<BoxShadow> {
    vec![
        BoxShadow::new(px(0.), px(0.), color.alpha(FOCUS_RING_ALPHA))
            .spread_radius(FOCUS_RING_SPREAD),
    ]
}
