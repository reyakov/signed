//! The resizable group: stacked panes with drag dividers, outside the dock.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Axis, Bounds, Context, ElementId, Entity,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, RenderOnce, SharedString,
    StyleRefinement, Styled, Window, canvas, div, px,
};

use crate::layout::StyledExt as _;
use crate::layout::resize_handle::ResizeHandle;
use crate::theme::{PANE_DEFAULT_SIZE, PANE_MIN_SIZE};

type DebugSelector = std::rc::Rc<dyn Fn() -> String>;

/// The retained pane sizes of one resizable group.
#[derive(Default)]
pub struct ResizableState {
    /// Sizes of every pane except the remainder pane, which fills what's left.
    sizes: Vec<Pixels>,
    /// The group's main-axis extent, captured at paint for drag clamping.
    measured: Option<Pixels>,
}

impl ResizableState {
    /// Creates state whose panes start at the default size.
    pub fn new(cx: &mut App) -> Entity<Self> {
        cx.new(|_| Self::default())
    }

    /// The stored size of each fixed pane; the remainder pane isn't listed.
    pub fn sizes(&self) -> &[Pixels] {
        &self.sizes
    }

    /// Restores pane sizes, e.g. from a persisted layout.
    pub fn set_sizes<I>(&mut self, sizes: I, cx: &mut Context<Self>)
    where
        I: IntoIterator<Item = Pixels>,
    {
        self.sizes = sizes.into_iter().collect();
        cx.notify();
    }

    /// Records the group's measured extent, notifying only when it changes.
    fn set_measured(&mut self, extent: Pixels, cx: &mut Context<Self>) {
        if self.measured != Some(extent) {
            self.measured = Some(extent);
            cx.notify();
        }
    }

    /// The equal split of the measured extent, or the default pane size unmeasured.
    fn equal_split(&self, pane_count: usize) -> Pixels {
        match self.measured {
            // usize can exceed f32's exact integer range; bound it via u32 before casting.
            Some(extent) => extent / u32::try_from(pane_count).unwrap_or(u32::MAX) as f32,
            None => PANE_DEFAULT_SIZE,
        }
    }

    /// The size pane `ix` renders at: its stored size, or the equal split.
    fn pane_size(&self, ix: usize, pane_count: usize) -> Pixels {
        self.sizes
            .get(ix)
            .copied()
            .unwrap_or_else(|| self.equal_split(pane_count))
    }

    /// Applies a drag at divider `ix` (between pane ix and ix+1).
    fn resize(&mut self, ix: usize, delta: Pixels, mins: &[Pixels], cx: &mut Context<Self>) {
        let Some(&floor) = mins.get(ix) else {
            return;
        };

        let pane_count = mins.len();

        let previous = self.pane_size(ix, pane_count);
        let mut size = previous + delta;
        size = size.max(floor);

        if let Some(extent) = self.measured {
            let floors = mins
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != ix)
                .fold(px(0.), |sum, (_, min)| sum + *min);
            let ceiling = extent - floors;
            // When the window is smaller than the other panes' floors, keep
            // the floor rather than shrinking this pane below it.
            size = size.min(ceiling.max(floor));
        }

        // Earlier panes hold the equal split they already render at,
        // so the first drag does not snap them to a backfilled default.
        let backfill = self.equal_split(pane_count);
        while self.sizes.len() <= ix {
            self.sizes.push(backfill);
        }

        self.sizes[ix] = size;

        // A drag clamped at its floor or ceiling lands here on every mouse move,
        // only a change to the rendered size is worth a frame.
        if size != previous {
            cx.notify();
        }
    }

    /// Sets pane `ix`'s width, backfilling earlier panes.
    pub fn set_pane_width(&mut self, ix: usize, width: Pixels, cx: &mut Context<Self>) {
        // Persisted layouts can carry negative sizes.
        let width = width.max(PANE_MIN_SIZE);
        while self.sizes.len() <= ix {
            self.sizes.push(width);
        }
        if self.sizes[ix] != width {
            self.sizes[ix] = width;
            cx.notify();
        }
    }

    /// Restores pane `ix` to the equal split, as a double-click does.
    fn reset_pane(&mut self, ix: usize, pane_count: usize, cx: &mut Context<Self>) {
        let split = self.equal_split(pane_count);
        while self.sizes.len() <= ix {
            self.sizes.push(split);
        }
        self.sizes[ix] = split;
        cx.notify();
    }
}

/// One pane of a resizable group: its content and its drag floor.
#[must_use]
pub struct ResizablePane {
    content: AnyElement,
    min: Pixels,
}

impl ResizablePane {
    /// The floor a pane keeps while dragging unless the caller lowers it.
    pub const DEFAULT_MIN: Pixels = PANE_MIN_SIZE;

    /// Wraps this element as a pane.
    pub fn new<E>(content: E) -> Self
    where
        E: IntoElement,
    {
        Self {
            content: content.into_any_element(),
            min: Self::DEFAULT_MIN,
        }
    }

    /// Sets the floor this pane keeps while dragging.
    pub fn min(mut self, min: Pixels) -> Self {
        self.min = min;
        self
    }
}

/// A group of panes stacked along one axis, separated by drag dividers.
#[derive(IntoElement)]
#[must_use]
pub struct Resizable {
    id: ElementId,
    state: Entity<ResizableState>,
    axis: Axis,
    panes: Vec<ResizablePane>,
    debug_selector: Option<DebugSelector>,
    style: StyleRefinement,
}

impl Resizable {
    /// Creates a group stacking its panes vertically, bound to `state`.
    pub fn new<I>(id: I, state: Entity<ResizableState>) -> Self
    where
        I: Into<ElementId>,
    {
        Self {
            id: id.into(),
            state,
            axis: Axis::Vertical,
            panes: Vec::new(),
            debug_selector: None,
            style: StyleRefinement::default(),
        }
    }

    /// Sets the axis the panes stack along.
    pub fn axis(mut self, axis: Axis) -> Self {
        self.axis = axis;
        self
    }

    /// Adds a pane with the default drag floor.
    pub fn pane<E>(mut self, content: E) -> Self
    where
        E: IntoElement,
    {
        self.panes.push(ResizablePane::new(content));
        self
    }

    /// Adds a pane that can't be dragged below `min`.
    pub fn pane_with_min<E>(mut self, content: E, min: Pixels) -> Self
    where
        E: IntoElement,
    {
        self.panes.push(ResizablePane::new(content).min(min));
        self
    }

    /// Sets the debug-bounds selector of each pane, suffixed with its index.
    pub fn debug_selector<F>(mut self, selector: F) -> Self
    where
        F: Fn() -> String + 'static,
    {
        self.debug_selector = Some(std::rc::Rc::new(selector));
        self
    }
}

impl Styled for Resizable {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Resizable {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let axis = self.axis;
        let state = self.state.clone();

        let pane_count = self.panes.len();
        let mins: Vec<Pixels> = self.panes.iter().map(|pane| pane.min).collect();
        let debug_selector = self.debug_selector;

        let sizes: Vec<Pixels> = (0..pane_count)
            .map(|ix| state.read(cx).pane_size(ix, pane_count))
            .collect();

        div()
            .flex()
            .overflow_hidden()
            .when(axis == Axis::Vertical, |group| group.flex_col())
            .when(axis == Axis::Horizontal, |group| group.flex_row())
            .refine_style(&self.style)
            .children(self.panes.into_iter().enumerate().flat_map(|(ix, pane)| {
                let remainder = ix + 1 == pane_count;
                let pane_element = div()
                    .overflow_hidden()
                    .when(remainder, |pane_element| {
                        pane_element
                            .flex_1()
                            .when(axis == Axis::Vertical, |p| p.min_h(pane.min))
                            .when(axis == Axis::Horizontal, |p| p.min_w(pane.min))
                    })
                    .when(!remainder, |pane_element| {
                        pane_element
                            .flex_none()
                            .when(axis == Axis::Vertical, |p| p.h(sizes[ix]).w_full())
                            .when(axis == Axis::Horizontal, |p| p.w(sizes[ix]).h_full())
                    })
                    .when_some(debug_selector.clone(), |pane_element, selector| {
                        let prefix = selector();
                        pane_element.debug_selector(move || format!("{prefix}-{ix}"))
                    })
                    .child(pane.content)
                    .into_any_element();

                let divider = (!remainder).then(|| {
                    // The positional index is a stable id: panes are append-only, never reordered.
                    ResizeHandle::new(SharedString::from(format!("{}-divider-{ix}", self.id)))
                        .axis(axis)
                        .on_resize({
                            let state = state.clone();
                            let mins = mins.clone();
                            move |delta, _, cx| {
                                state.update(cx, |state, cx| state.resize(ix, delta, &mins, cx));
                            }
                        })
                        .on_reset({
                            let state = state.clone();
                            let mins = mins.clone();
                            move |_, cx| {
                                state.update(cx, |state, cx| state.reset_pane(ix, mins.len(), cx));
                            }
                        })
                        .into_any_element()
                });

                std::iter::once(pane_element).chain(divider)
            }))
            .when(pane_count > 0, |group| {
                group.child(
                    canvas(
                        {
                            let state = state.clone();
                            move |bounds: Bounds<Pixels>, _, cx: &mut App| {
                                let extent = match axis {
                                    Axis::Vertical => bounds.size.height,
                                    Axis::Horizontal => bounds.size.width,
                                };
                                state.update(cx, |state, cx| state.set_measured(extent, cx));
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0(),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn resize_never_stores_sub_floor_size(cx: &mut TestAppContext) {
        let state = cx.new(|_| ResizableState::default());
        state.update(cx, |state, cx| {
            // The measured extent is smaller than the other pane's floor.
            state.set_measured(px(100.), cx);
            state.resize(0, px(500.), &[px(48.), px(150.)], cx);
            assert_eq!(state.sizes[0], px(48.));
        });
    }

    #[gpui::test]
    fn set_pane_width_clamps_negative_values(cx: &mut TestAppContext) {
        let state = cx.new(|_| ResizableState::default());
        state.update(cx, |state, cx| {
            // Persisted layouts can carry negative sizes.
            state.set_pane_width(0, px(-20.), cx);
            assert_eq!(state.pane_size(0, 1), PANE_MIN_SIZE);
        });
    }
}
