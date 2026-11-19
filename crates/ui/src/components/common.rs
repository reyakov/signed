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
    App, Bounds, BoxShadow, Div, ElementId, FocusHandle, FontWeight, Global, Hsla, IntoElement,
    Pixels, Refineable as _, Rems, StatefulInteractiveElement, StyleRefinement, Styled, Window,
    WindowId, div, px, rems,
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

    /// Sets the medium weight.
    #[must_use]
    fn font_medium(mut self) -> Self {
        self.text_style().font_weight = Some(FontWeight::MEDIUM);
        self
    }

    /// Sets the semibold weight.
    #[must_use]
    fn font_semibold(mut self) -> Self {
        self.text_style().font_weight = Some(FontWeight::SEMIBOLD);
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

/// Resolves a [`Size`] into GPUI's native rem utilities.
pub trait ControlSizing: Styled + Sized {
    /// The control's height: `h_5`, `h_7`, `h_8`, `h_10`.
    fn control_height(self, size: Size) -> Self;
    /// The control's square width: `w_5`, `w_7`, `w_8`, `w_10`.
    fn control_width(self, size: Size) -> Self;
    /// Horizontal padding inside a control: `px_1`, `px_2`, `px_3`, `px_4`.
    fn control_padding_x(self, size: Size) -> Self;
    /// Tight horizontal padding: `px_1`, `px_2`, `px_2`, `px_3`.
    fn control_dense_padding_x(self, size: Size) -> Self;
    /// The control label size: `text_xs`, `text_xs`, `text_sm`, `text_base`.
    fn control_text(self, size: Size) -> Self;
    /// The dense caption size: `text_xs`, `text_xs`, `text_xs`, `text_sm`.
    fn control_caption_text(self, size: Size) -> Self;
    /// The selection indicator side: `size_2p5`, `size_3p5`, `size_4`, `size_5`.
    fn control_indicator(self, size: Size) -> Self;
    /// The inset to a raised inner plate: `p_0`, `p_0p5`, `p_0p5`, `p_1`.
    fn control_plate_inset(self, size: Size) -> Self;
    /// The avatar side: `size_5`, `size_5`, `size_6`, `size_8`.
    fn avatar_size(self, size: Size) -> Self;
    /// The switch track height: `h_3p5`, `h_5`, `h_5`, `h_6`.
    fn switch_track_height(self, size: Size) -> Self;
    /// The switch track width: `w_6`, `w_8`, `w_9`, `w_11`.
    fn switch_track_width(self, size: Size) -> Self;
    /// The switch knob side: `size_2`, `size_3`, `size_3p5`, `size_5`.
    fn switch_knob(self, size: Size) -> Self;
}

impl<E: Styled> ControlSizing for E {
    fn control_height(self, size: Size) -> Self {
        match size {
            Size::XSmall => self.h_5(),
            Size::Small => self.h_7(),
            Size::Medium => self.h_8(),
            Size::Large => self.h_10(),
        }
    }

    fn control_width(self, size: Size) -> Self {
        match size {
            Size::XSmall => self.w_5(),
            Size::Small => self.w_7(),
            Size::Medium => self.w_8(),
            Size::Large => self.w_10(),
        }
    }

    fn control_padding_x(self, size: Size) -> Self {
        match size {
            Size::XSmall => self.px_1(),
            Size::Small => self.px_2(),
            Size::Medium => self.px_3(),
            Size::Large => self.px_4(),
        }
    }

    fn control_dense_padding_x(self, size: Size) -> Self {
        match size {
            Size::XSmall => self.px_1(),
            Size::Small => self.px_2(),
            Size::Medium => self.px_2(),
            Size::Large => self.px_3(),
        }
    }

    fn control_text(self, size: Size) -> Self {
        match size {
            Size::XSmall => self.text_xs(),
            Size::Small => self.text_xs(),
            Size::Medium => self.text_sm(),
            Size::Large => self.text_base(),
        }
    }

    fn control_caption_text(self, size: Size) -> Self {
        match size {
            Size::XSmall => self.text_xs(),
            Size::Small => self.text_xs(),
            Size::Medium => self.text_xs(),
            Size::Large => self.text_sm(),
        }
    }

    fn control_indicator(self, size: Size) -> Self {
        match size {
            Size::XSmall => self.size_2p5(),
            Size::Small => self.size_3p5(),
            Size::Medium => self.size_4(),
            Size::Large => self.size_5(),
        }
    }

    fn control_plate_inset(self, size: Size) -> Self {
        match size {
            Size::XSmall => self.p_0(),
            Size::Small => self.p_0p5(),
            Size::Medium => self.p_0p5(),
            Size::Large => self.p_1(),
        }
    }

    fn avatar_size(self, size: Size) -> Self {
        match size {
            Size::XSmall => self.size_5(),
            Size::Small => self.size_5(),
            Size::Medium => self.size_6(),
            Size::Large => self.size_8(),
        }
    }

    fn switch_track_height(self, size: Size) -> Self {
        match size {
            Size::XSmall => self.h_3p5(),
            Size::Small => self.h_5(),
            Size::Medium => self.h_5(),
            Size::Large => self.h_6(),
        }
    }

    fn switch_track_width(self, size: Size) -> Self {
        match size {
            Size::XSmall => self.w_6(),
            Size::Small => self.w_8(),
            Size::Medium => self.w_9(),
            Size::Large => self.w_11(),
        }
    }

    fn switch_knob(self, size: Size) -> Self {
        match size {
            Size::XSmall => self.size_2(),
            Size::Small => self.size_3(),
            Size::Medium => self.size_3p5(),
            Size::Large => self.size_5(),
        }
    }
}

impl Size {
    /// The control height in rems; paints use the pixels this yields.
    pub fn control_height_rems(self) -> Rems {
        match self {
            Size::XSmall => rems(1.25),
            Size::Small => rems(1.75),
            Size::Medium => rems(2.0),
            Size::Large => rems(2.5),
        }
    }

    /// The horizontal control padding in rems.
    pub fn padding_x_rems(self) -> Rems {
        match self {
            Size::XSmall => rems(0.25),
            Size::Small => rems(0.5),
            Size::Medium => rems(0.75),
            Size::Large => rems(1.0),
        }
    }

    /// The tight horizontal control padding in rems.
    pub fn dense_padding_x_rems(self) -> Rems {
        match self {
            Size::XSmall => rems(0.25),
            Size::Small => rems(0.5),
            Size::Medium => rems(0.5),
            Size::Large => rems(0.75),
        }
    }

    /// The label text size in rems, for geometry that layers on the text.
    pub fn text_rems(self) -> Rems {
        match self {
            Size::XSmall => rems(0.75),
            Size::Small => rems(0.75),
            Size::Medium => rems(0.875),
            Size::Large => rems(1.0),
        }
    }

    /// The icon side in rems.
    pub fn icon_rems(self) -> Rems {
        match self {
            Size::XSmall => rems(0.625),
            Size::Small => rems(0.875),
            Size::Medium => rems(1.0),
            Size::Large => rems(1.25),
        }
    }

    /// The icon side for icons inline with dense rows in rems.
    pub fn dense_icon_rems(self) -> Rems {
        match self {
            Size::XSmall => rems(0.5),
            Size::Small => rems(0.75),
            Size::Medium => rems(0.75),
            Size::Large => rems(1.0),
        }
    }

    /// The selection indicator side in rems.
    pub fn indicator_rems(self) -> Rems {
        match self {
            Size::XSmall => rems(0.625),
            Size::Small => rems(0.875),
            Size::Medium => rems(1.0),
            Size::Large => rems(1.25),
        }
    }

    /// The avatar side in rems.
    pub fn avatar_rems(self) -> Rems {
        match self {
            Size::XSmall => rems(1.25),
            Size::Small => rems(1.25),
            Size::Medium => rems(1.5),
            Size::Large => rems(2.0),
        }
    }

    /// The inset between a control's edge and a raised plate in rems.
    pub fn plate_inset_rems(self) -> Rems {
        match self {
            Size::XSmall => rems(0.0),
            Size::Small => rems(0.125),
            Size::Medium => rems(0.125),
            Size::Large => rems(0.25),
        }
    }

    /// The switch track height in rems.
    pub fn switch_track_height_rems(self) -> Rems {
        match self {
            Size::XSmall => rems(0.875),
            Size::Small => rems(1.25),
            Size::Medium => rems(1.25),
            Size::Large => rems(1.5),
        }
    }

    /// The switch track width in rems.
    pub fn switch_track_width_rems(self) -> Rems {
        match self {
            Size::XSmall => rems(1.5),
            Size::Small => rems(2.0),
            Size::Medium => rems(2.25),
            Size::Large => rems(2.75),
        }
    }

    /// The switch knob side in rems.
    pub fn switch_knob_rems(self) -> Rems {
        match self {
            Size::XSmall => rems(0.5),
            Size::Small => rems(0.75),
            Size::Medium => rems(0.875),
            Size::Large => rems(1.25),
        }
    }

    /// The slider row height in rems.
    pub fn slider_row_rems(self) -> Rems {
        match self {
            Size::XSmall => rems(1.5),
            Size::Small => rems(2.25),
            Size::Medium => rems(2.5),
            Size::Large => rems(3.0),
        }
    }

    /// The slider track height in rems.
    pub fn slider_track_rems(self) -> Rems {
        match self {
            Size::XSmall => rems(0.125),
            Size::Small => rems(0.25),
            Size::Medium => rems(0.25),
            Size::Large => rems(0.375),
        }
    }
}
