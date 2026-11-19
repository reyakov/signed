//! Shared element constructors and small style helpers.
//!
//! # Examples
//!
//! ```no_run
//! use gpui::{ParentElement as _, Styled as _, div};
//! use ui::common::{h_flex, v_flex};
//!
//! let row = h_flex().gap_2().child(div()).child(div());
//! let column = v_flex().gap_2().child(row);
//! ```

use std::collections::HashMap;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, Bounds, BoxShadow, Div, ElementId, FocusHandle, Global, Hsla, IntoElement, Pixels,
    Refineable as _, StatefulInteractiveElement, StyleRefinement, Styled, Window, WindowId, div,
    px,
};

pub use crate::theme::Size;
use crate::theme::{ActiveTheme as _, FOCUS_RING_ALPHA, FOCUS_RING_SPREAD};

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

/// Logs the error and yields the success value, if any.
pub trait ResultLogErr<T> {
    /// Logs the error and yields the success value, if any.
    fn log_err(self) -> Option<T>;
}

impl<T, E: std::fmt::Display> ResultLogErr<T> for Result<T, E> {
    fn log_err(self) -> Option<T> {
        match self {
            Ok(value) => Some(value),
            Err(error) => {
                log::warn!("{error}");
                None
            }
        }
    }
}

/// Extensions over [`Styled`], shared by the components.
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

/// The painted bounds of elements that tooltips anchor to, per window and element id.
/// Entries are never evicted: bounds are bounded by live windows, though long-lived processes minting dynamic per-item ids could accumulate.
#[derive(Default)]
struct AnchorBoundsRegistry(HashMap<(WindowId, ElementId), Bounds<Pixels>>);

impl Global for AnchorBoundsRegistry {}

impl AnchorBoundsRegistry {
    fn record(&mut self, window: WindowId, id: ElementId, bounds: Bounds<Pixels>) {
        self.0.insert((window, id), bounds);
    }

    fn get(&self, window: WindowId, id: &ElementId) -> Option<Bounds<Pixels>> {
        self.0.get(&(window, id.clone())).copied()
    }
}

/// The last bounds an element painted at, in the given window.
pub fn anchor_bounds(window: WindowId, id: &ElementId, cx: &App) -> Option<Bounds<Pixels>> {
    cx.try_global::<AnchorBoundsRegistry>()
        .and_then(|registry| registry.get(window, id))
}

/// A canvas prepaint closure recording the element's painted bounds.
pub fn record_anchor_bounds(
    id: ElementId,
) -> impl Fn(Bounds<Pixels>, &mut Window, &mut App) + 'static {
    move |bounds, window, cx| {
        let window_id = window.window_handle().window_id();
        cx.default_global::<AnchorBoundsRegistry>()
            .record(window_id, id.clone(), bounds);
    }
}

/// Returns a focus ring shadow with the given color at half ink.
pub fn focus_ring(color: Hsla) -> Vec<BoxShadow> {
    vec![
        BoxShadow::new(px(0.), px(0.), color.alpha(FOCUS_RING_ALPHA))
            .spread_radius(FOCUS_RING_SPREAD),
    ]
}

/// The next selectable row in the direction, or `None` at the ends.
pub fn adjacent_selectable(
    selectable: &[usize],
    selected: Option<usize>,
    forward: bool,
) -> Option<usize> {
    if forward {
        selectable
            .iter()
            .copied()
            .find(|index| Some(*index) > selected)
    } else {
        selectable
            .iter()
            .copied()
            .rev()
            .find(|index| Some(*index) < selected)
    }
}

/// Whether the given key is a vertical navigation key.
pub fn nav_key<S>(selectable: &[usize], selected: Option<usize>, key: &str, select: S) -> bool
where
    S: FnOnce(Option<usize>),
{
    let target = match key {
        "down" => adjacent_selectable(selectable, selected, true),
        "up" => adjacent_selectable(selectable, selected, false),
        "home" => selectable.first().copied(),
        "end" => selectable.last().copied(),
        _ => return false,
    };
    if let Some(next) = target
        && Some(next) != selected
    {
        select(Some(next));
    }
    true
}

/// A selectable row's label and icon color.
pub fn selectable_label_color(cx: &App, selected: bool, disabled: bool) -> Hsla {
    let theme = cx.theme();
    if disabled {
        theme.colors.muted_foreground
    } else if selected {
        theme.colors.primary_foreground
    } else {
        theme.colors.foreground
    }
}

/// A selectable row's background.
#[must_use]
pub fn paint_selectable_row<R>(cx: &App, row: R, selected: bool, disabled: bool) -> R
where
    R: IntoElement + Styled + StatefulInteractiveElement,
{
    let theme = cx.theme();
    row.map(|row| {
        if disabled {
            row
        } else if selected {
            row.bg(theme.ramps.primary.base)
        } else {
            row.hover(|row| row.bg(theme.colors.muted))
        }
    })
}

/// A handler told to clean up after a dismissed surface.
pub type DismissHandler = std::rc::Rc<dyn Fn(&mut gpui::App)>;

/// Deferred draw priority of window-modal surfaces; default layer 0 stays for in-flow chrome like resize handles.
pub const DIALOG_LAYER: usize = 1;
/// Deferred draw priority of floating panels, so controls inside a dialog open above it.
pub const OVERLAY_LAYER: usize = 2;
/// Deferred draw priority of passive notifications, so they stay visible over dialogs.
pub const TOAST_LAYER: usize = 3;

/// A component that can be told which size to render at.
pub trait Sizable: Sized {
    /// Sets the size.
    fn with_size(mut self, size: Size) -> Self {
        self.apply_size(size);
        self
    }

    /// Applies the size to the component's fields.
    fn apply_size(&mut self, size: Size);

    /// Sets `Size::XSmall`.
    fn xsmall(self) -> Self
    where
        Self: Sized,
    {
        self.with_size(Size::XSmall)
    }

    /// Sets `Size::Small`.
    fn small(self) -> Self
    where
        Self: Sized,
    {
        self.with_size(Size::Small)
    }

    /// Sets `Size::Large`.
    fn large(self) -> Self
    where
        Self: Sized,
    {
        self.with_size(Size::Large)
    }
}
