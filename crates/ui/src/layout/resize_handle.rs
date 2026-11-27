//! The drag divider: a thin handle in the gap between panels that resizes
//! them. It shows its hairline only while hovered or dragged.

use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, Axis, Bounds, CursorStyle, DispatchPhase, ElementId, HitboxBehavior,
    InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Point, RenderOnce, Role,
    SharedString, StatefulInteractiveElement as _, StyleRefinement, Styled, Window, canvas,
    deferred, div, fill, point, px,
};

use crate::components::common::ElementIdExt as _;
use crate::layout::StyledExt as _;
use crate::theme::{
    ActiveTheme as _, BORDER_WIDTH, DISABLED_OPACITY, HAIRLINE_WIDTH, RESIZE_HANDLE_WIDTH,
    RESIZE_KEYBOARD_STEP,
};

/// How far one keyboard arrow step moves the seam.
const KEYBOARD_STEP: Pixels = RESIZE_KEYBOARD_STEP;

type ResizeHandler = Rc<dyn Fn(Pixels, &mut Window, &mut App)>;
type ResetHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// The pointer drag state, keyed to the handle's element id.
#[derive(Clone, Copy)]
struct DragState {
    dragging: bool,
    last: Point<Pixels>,
}

/// A thin drag divider that resizes the adjacent region.
#[derive(IntoElement)]
#[must_use]
pub struct ResizeHandle {
    id: ElementId,
    /// The axis the drag moves along: a vertical seam drags horizontally.
    axis: Axis,
    disabled: bool,
    accessibility_label: Option<SharedString>,
    on_resize: Option<ResizeHandler>,
    on_reset: Option<ResetHandler>,
    style: StyleRefinement,
}

impl ResizeHandle {
    /// Creates a horizontal resize handle (a vertical seam).
    pub fn new<I>(id: I) -> Self
    where
        I: Into<ElementId>,
    {
        Self {
            id: id.into(),
            axis: Axis::Horizontal,
            disabled: false,
            accessibility_label: None,
            on_resize: None,
            on_reset: None,
            style: StyleRefinement::default(),
        }
    }

    /// Sets the axis the drag moves along.
    pub fn axis(mut self, axis: Axis) -> Self {
        self.axis = axis;
        self
    }

    /// Renders the handle inert and dimmed.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Overrides the accessibility label, which defaults to "Resize".
    pub fn accessibility_label<L>(mut self, label: L) -> Self
    where
        L: Into<SharedString>,
    {
        self.accessibility_label = Some(label.into());
        self
    }

    /// Runs for every pointer movement while the seam is dragged.
    pub fn on_resize<F>(mut self, handler: F) -> Self
    where
        F: Fn(Pixels, &mut Window, &mut App) + 'static,
    {
        self.on_resize = Some(Rc::new(handler));
        self
    }

    /// Runs when the seam is double-clicked, to restore the default size.
    pub fn on_reset<F>(mut self, handler: F) -> Self
    where
        F: Fn(&mut Window, &mut App) + 'static,
    {
        self.on_reset = Some(Rc::new(handler));
        self
    }
}

impl Styled for ResizeHandle {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for ResizeHandle {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let ring = theme.colors.ring;
        let focus_ring = theme.focus_ring();
        let thickness = RESIZE_HANDLE_WIDTH;

        let label = self
            .accessibility_label
            .clone()
            .unwrap_or_else(|| SharedString::from("Resize"));

        let drag = window.with_element_namespace(self.id.clone(), |window| {
            window
                .use_keyed_state("resize-drag", cx, |_, _| DragState {
                    dragging: false,
                    last: point(px(0.), px(0.)),
                })
                .clone()
        });

        let focus = self.id.focus_handle(window, cx);
        let dragging = drag.read(cx).dragging;
        let axis = self.axis;
        let disabled = self.disabled;

        let cursor = match axis {
            Axis::Horizontal => CursorStyle::ResizeLeftRight,
            Axis::Vertical => CursorStyle::ResizeUpDown,
        };

        let on_reset = self.on_reset;

        deferred(
            div()
                .id(self.id.clone())
                .debug_selector(|| "resize-handle".to_string())
                .role(Role::Splitter)
                .aria_label(label)
                .flex_none()
                .when(axis == Axis::Horizontal, |handle| {
                    handle.w(thickness).h_full()
                })
                .when(axis == Axis::Vertical, |handle| {
                    handle.w_full().h(thickness)
                })
                .refine_style(&self.style)
                .when(self.disabled, |handle| handle.opacity(DISABLED_OPACITY))
                .when(!self.disabled, |handle| {
                    handle
                        .track_focus(&focus)
                        .focusable()
                        .tab_stop(true)
                        .focus_visible(|style| style.shadow(focus_ring))
                        .on_mouse_down_out({
                            let focus = focus.clone();
                            move |_, window, cx| {
                                if focus.is_focused(window) {
                                    window.blur(cx);
                                }
                            }
                        })
                        .on_mouse_down(MouseButton::Left, {
                            let drag = drag.clone();
                            let focus = focus.clone();

                            move |event: &MouseDownEvent, window, cx| {
                                window.focus(&focus, cx);
                                cx.stop_propagation();

                                if event.click_count >= 2 {
                                    drag.update(cx, |drag, cx| {
                                        drag.dragging = false;
                                        cx.notify();
                                    });
                                    if let Some(on_reset) = on_reset.clone() {
                                        on_reset(window, cx);
                                    }
                                    return;
                                }

                                drag.update(cx, |drag, cx| {
                                    drag.dragging = true;
                                    drag.last = event.position;
                                    cx.notify();
                                });
                            }
                        })
                        .on_key_down({
                            let on_resize = self.on_resize.clone();
                            move |event: &KeyDownEvent, window, cx| {
                                let delta = match (axis, event.keystroke.key.as_str()) {
                                    (Axis::Horizontal, "left") => Some(-KEYBOARD_STEP),
                                    (Axis::Horizontal, "right") => Some(KEYBOARD_STEP),
                                    (Axis::Vertical, "up") => Some(-KEYBOARD_STEP),
                                    (Axis::Vertical, "down") => Some(KEYBOARD_STEP),
                                    _ => None,
                                };
                                if let Some(delta) = delta {
                                    cx.stop_propagation();
                                    if let Some(on_resize) = on_resize.clone() {
                                        on_resize(delta, window, cx);
                                    }
                                }
                            }
                        })
                })
                .child(
                    canvas(
                        {
                            move |bounds, window, _| {
                                let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
                                (hitbox, bounds)
                            }
                        },
                        {
                            let drag = drag.clone();
                            let on_resize = self.on_resize.clone();
                            move |_, (hitbox, bounds), window, _cx| {
                                let hovered = hitbox.is_hovered(window);

                                if !disabled {
                                    window.set_cursor_style(cursor, &hitbox);
                                }

                                if disabled || !(dragging || hovered) {
                                    return;
                                }

                                let line = match axis {
                                    Axis::Horizontal => {
                                        let x = bounds.center().x - HAIRLINE_WIDTH;
                                        Bounds::from_corners(
                                            point(x, bounds.top()),
                                            point(x + BORDER_WIDTH, bounds.bottom()),
                                        )
                                    }
                                    Axis::Vertical => {
                                        let y = bounds.center().y - HAIRLINE_WIDTH;
                                        Bounds::from_corners(
                                            point(bounds.left(), y),
                                            point(bounds.right(), y + BORDER_WIDTH),
                                        )
                                    }
                                };

                                window.paint_quad(fill(line, ring));

                                window.on_mouse_event({
                                    let drag = drag.clone();
                                    let on_resize = on_resize.clone();
                                    move |event: &MouseMoveEvent, phase, window, cx| {
                                        if phase != DispatchPhase::Bubble {
                                            return;
                                        }

                                        let (dragging, last) = {
                                            let drag = drag.read(cx);
                                            (drag.dragging, drag.last)
                                        };

                                        if !dragging {
                                            return;
                                        }

                                        if event.pressed_button != Some(MouseButton::Left) {
                                            drag.update(cx, |drag, cx| {
                                                drag.dragging = false;
                                                cx.notify();
                                            });
                                            return;
                                        }

                                        let delta = match axis {
                                            Axis::Horizontal => event.position.x - last.x,
                                            Axis::Vertical => event.position.y - last.y,
                                        };

                                        drag.update(cx, |drag, _| drag.last = event.position);

                                        if let Some(on_resize) = on_resize.clone() {
                                            on_resize(delta, window, cx);
                                        }
                                    }
                                });

                                window.on_mouse_event({
                                    let drag = drag.clone();
                                    move |event: &MouseUpEvent, phase, _, cx| {
                                        if phase != DispatchPhase::Bubble
                                            || event.button != MouseButton::Left
                                        {
                                            return;
                                        }

                                        if drag.read(cx).dragging {
                                            drag.update(cx, |drag, cx| {
                                                drag.dragging = false;
                                                cx.notify();
                                            });
                                        }
                                    }
                                });
                            }
                        },
                    )
                    .absolute()
                    .inset_0(),
                ),
        )
    }
}
