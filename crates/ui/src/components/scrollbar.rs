//! Renders an overlay [`Scrollbar`] over a scrolled surface, driven by a
//! [`ScrollbarHandle`].
//!
//! # Examples
//!
//! ```no_run
//! use gpui::{div, ScrollHandle, InteractiveElement as _, StatefulInteractiveElement as _};
//! use ui::scrollbar::{Scrollbar, ScrollbarVisibility};
//!
//! let handle = ScrollHandle::new();
//! let content = div().id("log").overflow_y_scroll().track_scroll(&handle);
//! let bar = Scrollbar::vertical("log-bar", handle)
//!     .visibility(ScrollbarVisibility::Autohide);
//! ```

use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    App, Bounds, DispatchPhase, ElementId, Entity, EntityId, IntoElement, ListState, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, RenderOnce, ScrollEvent,
    ScrollHandle, SharedString, Styled as _, Task, UniformListScrollHandle, Window, canvas, fill,
    point,
};

use crate::components::common::ResultLogErr as _;
use crate::theme::{ActiveTheme as _, SCROLLBAR_AUTOHIDE_DELAY, SCROLLBAR_FADE_DURATION};

/// When an overlay scrollbar shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScrollbarVisibility {
    /// The running platform's behavior, from the theme.
    #[default]
    Platform,
    /// Always visible.
    Always,
    /// Shows on activity, then fades out.
    Autohide,
}

/// How often the autohide fade repaints while it runs.
const FADE_STEP: Duration = Duration::from_millis(50);

/// The shared autohide timeline for an overlay scrollbar.
#[derive(Clone)]
struct Autohide {
    view: EntityId,
    state: Entity<AutohideState>,
}

struct AutohideState {
    last_activity: Option<Instant>,
    hide_task: Option<Task<()>>,
}

impl Autohide {
    fn new(key: ElementId, window: &mut Window, cx: &mut App) -> Self {
        let view = window.current_view();
        let state = window.use_keyed_state(key, cx, |_, _| AutohideState {
            last_activity: None,
            hide_task: None,
        });
        Self { view, state }
    }

    /// Marks interaction.
    fn touch(&self, window: &mut Window, cx: &mut App) {
        let view = self.view;
        let reduce_motion = cx.theme().accessibility.reduce_motion;

        let task = window.spawn(cx, async move |cx| {
            if reduce_motion {
                cx.background_executor()
                    .timer(SCROLLBAR_AUTOHIDE_DELAY + SCROLLBAR_FADE_DURATION)
                    .await;
                cx.update(|_, cx| cx.notify(view)).log_err();
                return;
            }

            let steps = (SCROLLBAR_FADE_DURATION.as_millis() / FADE_STEP.as_millis()).max(1);
            cx.background_executor()
                .timer(SCROLLBAR_AUTOHIDE_DELAY)
                .await;

            for _ in 0..steps {
                cx.update(|_, cx| cx.notify(view)).log_err();
                cx.background_executor().timer(FADE_STEP).await;
            }

            cx.update(|_, cx| cx.notify(view)).log_err();
        });

        self.state.update(cx, |state, _| {
            state.last_activity = Some(Instant::now());
            state.hide_task = Some(task);
        });
    }

    /// The thumb's opacity.
    fn alpha(&self, cx: &App, dragging: bool) -> f32 {
        if dragging {
            return 1.0;
        }

        let Some(activity) = self.state.read(cx).last_activity else {
            return 0.0;
        };

        let idle = activity.elapsed();
        if idle <= SCROLLBAR_AUTOHIDE_DELAY {
            return 1.0;
        }

        let fade = idle.saturating_sub(SCROLLBAR_AUTOHIDE_DELAY);
        if fade >= SCROLLBAR_FADE_DURATION {
            return 0.0;
        }

        1.0 - fade.as_secs_f32() / SCROLLBAR_FADE_DURATION.as_secs_f32()
    }
}

/// The scroll state a scrollbar drives, implemented for gpui's scroll handles.
pub trait ScrollbarHandle: 'static {
    /// How far the content can scroll, positive along each axis.
    fn max_offset(&self) -> Point<Pixels>;
    /// The current offset, non-positive along each axis.
    fn offset(&self) -> Point<Pixels>;
    /// Applies the offset along one axis, keeping the other.
    fn set_offset(&self, offset: Point<Pixels>);
    /// The bounds of the scrolled surface.
    fn viewport(&self) -> Bounds<Pixels>;
    /// A thumb drag began.
    fn drag_started(&self) {}
    /// A thumb drag ended.
    fn drag_ended(&self) {}
}

impl ScrollbarHandle for ScrollHandle {
    fn max_offset(&self) -> Point<Pixels> {
        ScrollHandle::max_offset(self)
    }

    fn offset(&self) -> Point<Pixels> {
        ScrollHandle::offset(self)
    }

    fn set_offset(&self, offset: Point<Pixels>) {
        ScrollHandle::set_offset(self, offset);
    }

    fn viewport(&self) -> Bounds<Pixels> {
        self.bounds()
    }
}

impl ScrollbarHandle for UniformListScrollHandle {
    fn max_offset(&self) -> Point<Pixels> {
        self.0.borrow().base_handle.max_offset()
    }

    fn offset(&self) -> Point<Pixels> {
        self.0.borrow().base_handle.offset()
    }

    fn set_offset(&self, offset: Point<Pixels>) {
        self.0.borrow().base_handle.set_offset(offset);
    }

    fn viewport(&self) -> Bounds<Pixels> {
        self.0.borrow().base_handle.bounds()
    }
}

impl ScrollbarHandle for ListState {
    fn max_offset(&self) -> Point<Pixels> {
        ListState::max_offset_for_scrollbar(self)
    }

    fn offset(&self) -> Point<Pixels> {
        ListState::scroll_px_offset_for_scrollbar(self)
    }

    fn set_offset(&self, offset: Point<Pixels>) {
        ListState::set_offset_from_scrollbar(self, offset);
    }

    fn viewport(&self) -> Bounds<Pixels> {
        self.viewport_bounds()
    }

    fn drag_started(&self) {
        ListState::scrollbar_drag_started(self);
    }

    fn drag_ended(&self) {
        ListState::scrollbar_drag_ended(self);
    }
}

/// Which direction the scrollbar lies along.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Axis {
    Vertical,
    Horizontal,
}

impl Axis {
    fn along(self, position: Point<Pixels>) -> Pixels {
        match self {
            Axis::Vertical => position.y,
            Axis::Horizontal => position.x,
        }
    }

    fn with_along(self, position: Point<Pixels>, along: Pixels) -> Point<Pixels> {
        match self {
            Axis::Vertical => point(position.x, along),
            Axis::Horizontal => point(along, position.y),
        }
    }

    fn viewport_along(self, viewport: &Bounds<Pixels>) -> Pixels {
        match self {
            Axis::Vertical => viewport.size.height,
            Axis::Horizontal => viewport.size.width,
        }
    }
}

/// An overlay scrollbar, rendered over the scrolled surface.
#[derive(IntoElement)]
#[must_use]
pub struct Scrollbar {
    axis: Axis,
    handle: Rc<dyn ScrollbarHandle>,
    visibility: ScrollbarVisibility,
    id: ElementId,
}

impl Scrollbar {
    /// Creates a vertical scrollbar named by `id`, driven by `handle`.
    pub fn vertical<I, H>(id: I, handle: H) -> Self
    where
        I: Into<ElementId>,
        H: ScrollbarHandle,
    {
        Self {
            axis: Axis::Vertical,
            handle: Rc::new(handle),
            visibility: ScrollbarVisibility::default(),
            id: id.into(),
        }
    }

    /// Creates a horizontal scrollbar named by `id`, driven by `handle`.
    pub fn horizontal<I, H>(id: I, handle: H) -> Self
    where
        I: Into<ElementId>,
        H: ScrollbarHandle,
    {
        Self {
            axis: Axis::Horizontal,
            handle: Rc::new(handle),
            visibility: ScrollbarVisibility::default(),
            id: id.into(),
        }
    }

    /// Overrides when the scrollbar shows, instead of the platform default.
    pub fn visibility(mut self, visibility: ScrollbarVisibility) -> Self {
        self.visibility = visibility;
        self
    }

    fn state_key(name: &'static str, id: &ElementId) -> ElementId {
        ElementId::NamedChild(Arc::new(id.clone()), SharedString::from(name))
    }

    /// Pure thumb geometry for an overlay scrollbar.
    fn thumb_geometry(
        viewport: Pixels,
        content: Pixels,
        offset: Pixels,
        track: Pixels,
        min_thumb: Pixels,
    ) -> (Pixels, Pixels) {
        if content <= viewport || content <= Pixels::ZERO {
            return (track, Pixels::ZERO);
        }

        let length = (viewport / content * track).max(min_thumb).min(track);
        let overflow = content - viewport;
        let progress = (-offset / overflow).clamp(0., 1.);
        let travel = track - length;
        (length, travel * progress)
    }

    /// An overlay scrollbar thumb's style for one paint: `None` hides it.
    fn thumb_style(color: gpui::Hsla, alpha: f32) -> Option<gpui::Hsla> {
        (alpha > 0.0).then_some(gpui::Hsla {
            a: color.a * alpha,
            ..color
        })
    }
}

impl RenderOnce for Scrollbar {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        canvas(|_, _, _| {}, {
            let handle = self.handle;
            let axis = self.axis;
            let visibility = self.visibility;
            let id = self.id;
            move |_, _, window, cx| Self::paint(&handle, axis, visibility, &id, window, cx)
        })
        .absolute()
        .inset_0()
    }
}

/// The drag state shared between the scrollbar's mouse handlers.
type Drag = Option<(Pixels, Pixels)>;

impl Scrollbar {
    fn paint(
        handle: &Rc<dyn ScrollbarHandle>,
        axis: Axis,
        visibility: ScrollbarVisibility,
        id: &ElementId,
        window: &mut Window,
        cx: &mut App,
    ) {
        let view = window.current_view();
        let theme = cx.theme();

        let color = theme.colors.scrollbar_thumb;
        let width = theme.scrollbar.width;
        let inset = theme.scrollbar.inset;
        let min_thumb = theme.scrollbar.min_thumb;

        let viewport = handle.viewport();
        let viewport_along = axis.viewport_along(&viewport);
        let max_offset = axis.along(handle.max_offset());

        if max_offset <= Pixels::ZERO || viewport_along <= Pixels::ZERO {
            return;
        }

        let autohide = match visibility {
            ScrollbarVisibility::Platform => theme.scrollbar.autohide,
            ScrollbarVisibility::Always => false,
            ScrollbarVisibility::Autohide => true,
        };

        let autohide_state =
            autohide.then(|| Autohide::new(Self::state_key("autohide", id), window, cx));

        let track_length = viewport_along - inset * 2.0;
        let (thumb_length, thumb_travel) = Self::thumb_geometry(
            viewport_along,
            viewport_along + max_offset,
            axis.along(handle.offset()),
            track_length,
            min_thumb,
        );

        // The track strip is the thumb's rail plus the padding the thumb rides inside.
        let (thumb_bounds, track_bounds) = match axis {
            Axis::Vertical => {
                let thumb_x = viewport.right() - width - inset;
                (
                    Bounds::from_corners(
                        point(thumb_x, viewport.top() + inset + thumb_travel),
                        point(
                            thumb_x + width,
                            viewport.top() + inset + thumb_travel + thumb_length,
                        ),
                    ),
                    Bounds::from_corners(
                        point(viewport.right() - width, viewport.top() + inset),
                        point(viewport.right(), viewport.top() + inset + track_length),
                    ),
                )
            }
            Axis::Horizontal => {
                let thumb_y = viewport.bottom() - width - inset;
                (
                    Bounds::from_corners(
                        point(viewport.left() + inset + thumb_travel, thumb_y),
                        point(
                            viewport.left() + inset + thumb_travel + thumb_length,
                            thumb_y + width,
                        ),
                    ),
                    Bounds::from_corners(
                        point(viewport.left() + inset, viewport.bottom() - width),
                        point(viewport.left() + inset + track_length, viewport.bottom()),
                    ),
                )
            }
        };

        let drag = window.use_keyed_state(Self::state_key("thumb-drag", id), cx, |_, _| Drag::None);
        let dragging = drag.read(cx).is_some();

        let alpha = match &autohide_state {
            Some(state) => state.alpha(cx, dragging),
            None => 1.0,
        };

        if let Some(color) = Self::thumb_style(color, alpha) {
            window.paint_quad(fill(thumb_bounds, color).corner_radii(width / 2.0));
        }

        if let Some(state) = &autohide_state {
            window.on_mouse_event({
                let state = state.clone();
                move |event: &ScrollEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble || !viewport.contains(&event.position) {
                        return;
                    }
                    state.touch(window, cx);
                }
            });
        }

        window.on_mouse_event({
            let handle = handle.clone();
            let drag = drag.clone();
            let autohide_state = autohide_state.clone();
            move |event: &MouseDownEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                    return;
                }

                let in_thumb = thumb_bounds.contains(&event.position);
                let in_track = !in_thumb && track_bounds.contains(&event.position);

                if !in_thumb && !in_track {
                    return;
                }

                let start_offset = if in_thumb {
                    axis.along(handle.offset())
                } else if track_length > thumb_length {
                    let thumb_start = ((axis.along(event.position)
                        - axis.along(track_bounds.origin))
                        - thumb_length / 2.0)
                        .clamp(Pixels::ZERO, track_length - thumb_length);
                    let offset = -(thumb_start / (track_length - thumb_length)) * max_offset;
                    handle.set_offset(axis.with_along(handle.offset(), offset));
                    cx.notify(view);
                    offset
                } else {
                    return;
                };

                drag.update(cx, |drag, _| {
                    *drag = Some((axis.along(event.position), start_offset));
                });
                handle.drag_started();

                if let Some(state) = &autohide_state {
                    state.touch(window, cx);
                }

                cx.stop_propagation();
            }
        });

        window.on_mouse_event({
            let handle = handle.clone();
            let drag = drag.clone();
            let autohide_state = autohide_state.clone();
            move |event: &MouseMoveEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }

                if let Some(state) = &autohide_state
                    && track_bounds.contains(&event.position)
                    && drag.read(cx).is_none()
                {
                    state.touch(window, cx);
                }

                if event.pressed_button != Some(MouseButton::Left) {
                    return;
                }

                let Some((start_position, start_offset)) = *drag.read(cx) else {
                    return;
                };

                if track_length <= thumb_length {
                    return;
                }

                let scale = max_offset / (track_length - thumb_length);
                let offset = (start_offset - (axis.along(event.position) - start_position) * scale)
                    .clamp(-max_offset, Pixels::ZERO);
                handle.set_offset(axis.with_along(handle.offset(), offset));
                cx.notify(view);
            }
        });

        window.on_mouse_event({
            let handle = handle.clone();
            let drag = drag.clone();
            let autohide_state = autohide_state.clone();
            move |event: &MouseUpEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                    return;
                }

                if drag.read(cx).is_none() {
                    return;
                }

                drag.update(cx, |drag, _| {
                    *drag = None;
                });
                handle.drag_ended();

                // Unfreezing a ListState's height can change the thumb's geometry.
                cx.notify(view);

                if let Some(state) = &autohide_state {
                    state.touch(window, cx);
                }
            }
        });
    }
}
