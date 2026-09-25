use std::cell::Cell;
use std::ops::Deref as _;
use std::rc::Rc;
use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Axis, Context, Div, Element, Empty, InteractiveElement as _,
    IntoElement, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Render, Stateful, Style,
    Styled as _, WeakEntity, Window, div, px,
};
use gpui_base::dock::{
    DockArea, DockAreaRenderer, DockContext, DockEvent, DockPlacement, NodeId, PanelState,
    PanelView, TabGroupRenderer,
};
use gpui_base::resize_handle;
use gpui_component::{ActiveTheme as _, Side};

use crate::invalid_panel::InvalidPanel;
use crate::panel_handle;
use crate::tab_panel::SignedTabGroupSkin;

/// State the skin shares with its per-container renderers.
pub(crate) struct SkinShared {
    area: WeakEntity<DockArea>,
    toggle_button_visible: Cell<bool>,
    /// The dock whose resize handle is being dragged, if any. Only one can be.
    resizing_dock: Cell<Option<DockPlacement>>,
}

impl SkinShared {
    pub(crate) fn area(&self) -> &WeakEntity<DockArea> {
        &self.area
    }

    pub(crate) fn is_toggle_button_visible(&self) -> bool {
        self.toggle_button_visible.get()
    }

    pub(crate) fn resizing_dock(&self) -> &Cell<Option<DockPlacement>> {
        &self.resizing_dock
    }

    /// Redraw the area after a setting changed. The skin is not an entity, so nothing else would.
    pub(crate) fn notify(&self, cx: &mut App) {
        _ = self.area.update(cx, |_, cx| cx.notify());
    }
}

/// The Signed appearance for a [`DockArea`].
/// Install it in the constructor, the only place the area's weak handle is available.
///
/// ```ignore
/// let dock = cx.new(|cx| {
///     let skin = SignedDockSkin::new(cx);
///     DockArea::new("dock", Some(1), window, cx).with_renderer(skin)
/// });
/// ```
pub struct SignedDockSkin {
    shared: Rc<SkinShared>,
}

impl SignedDockSkin {
    pub fn new(cx: &mut Context<DockArea>) -> Rc<Self> {
        Rc::new(Self {
            shared: Rc::new(SkinShared {
                area: cx.weak_entity(),
                toggle_button_visible: Cell::new(true),
                resizing_dock: Cell::new(None),
            }),
        })
    }

    pub(crate) fn shared(&self) -> &Rc<SkinShared> {
        &self.shared
    }

    /// Whether tab bars offer the affordance that collapses a neighbouring dock.
    pub fn is_toggle_button_visible(&self) -> bool {
        self.shared.is_toggle_button_visible()
    }

    pub fn set_toggle_button_visible(&self, visible: bool, cx: &mut App) {
        self.shared.toggle_button_visible.set(visible);
        self.shared.notify(cx);
    }
}

/// Payload a dock's resize handle drags.
///
/// It draws nothing, the handle element is the visible affordance.
#[derive(Clone)]
struct ResizePanel;

impl Render for ResizePanel {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

impl DockAreaRenderer for SignedDockSkin {
    fn frame(&self, _: &mut Window, _: &mut App) -> Stateful<Div> {
        div()
            .id("dock-area")
            .relative()
            .size_full()
            .overflow_hidden()
            .flex()
            .flex_row()
    }

    fn center_frame(&self, _: &mut Window, _: &mut App) -> Stateful<Div> {
        div()
            .id("dock-area-center")
            .flex()
            .flex_1()
            .flex_col()
            .overflow_hidden()
    }

    fn split_frame(&self, node: NodeId, _: Axis, _: &mut Window, cx: &mut App) -> Stateful<Div> {
        // `size_full` and `flex_1` stop the frame collapsing in an unsizing parent.
        div()
            .id(("dock-split-frame", node.as_u64()))
            .size_full()
            .flex_1()
            .min_h(px(0.))
            .overflow_hidden()
            .bg(cx.theme().tokens.tab_bar)
    }

    fn render_dock(
        &self,
        dock: &DockContext,
        content: AnyElement,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        div()
            .flex()
            .size_full()
            .relative()
            .child(content)
            .child(self.render_resize_handle(dock, window, cx))
            .child(DockResizeTracker {
                dock: dock.clone(),
                shared: self.shared().clone(),
            })
            .into_any_element()
    }

    /// Placeholder for a panel this build cannot construct.
    /// It dumps the state it was handed, so the layout survives a load and save.
    fn build_placeholder(
        &self,
        state: &PanelState,
        _: &mut Window,
        cx: &mut App,
    ) -> Option<Arc<dyn PanelView>> {
        let state = state.clone();
        Some(panel_handle(cx.new(|cx| {
            InvalidPanel::new(state.panel_name.clone(), state, cx)
        })))
    }

    fn tab_group_renderer(&self) -> Rc<dyn TabGroupRenderer> {
        Rc::new(SignedTabGroupSkin::new(self.shared().clone()))
    }
}

impl SignedDockSkin {
    fn render_resize_handle(
        &self,
        dock: &DockContext,
        _: &mut Window,
        _: &mut App,
    ) -> impl IntoElement {
        let placement = dock.placement();
        let shared = self.shared().clone();

        resize_handle("resize-handle", placement.axis())
            .when(placement.is_left(), |this| this.placement(Side::Left))
            .on_drag(ResizePanel, move |info, _, _, cx| {
                cx.stop_propagation();
                shared.resizing_dock().set(Some(placement));
                cx.new(|_| info.deref().clone())
            })
    }
}

/// Turns the window's mouse stream into dock resizing.
/// It draws nothing, the `paint` hook is the only window listener registration point.
struct DockResizeTracker {
    dock: DockContext,
    shared: Rc<SkinShared>,
}

impl IntoElement for DockResizeTracker {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for DockResizeTracker {
    type PrepaintState = ();
    type RequestLayoutState = ();

    fn id(&self) -> Option<gpui::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (gpui::LayoutId, Self::RequestLayoutState) {
        (window.request_layout(Style::default(), None, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: gpui::Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Window,
        _: &mut App,
    ) -> Self::PrepaintState {
    }

    fn paint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: gpui::Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        _: &mut App,
    ) {
        let placement = self.dock.placement();

        window.on_mouse_event({
            let dock = self.dock.clone();
            let shared = self.shared.clone();
            move |event: &MouseMoveEvent, phase, window, cx| {
                if !phase.bubble() || shared.resizing_dock().get() != Some(placement) {
                    return;
                }
                // Dragging a closed dock's handle reopens it.
                // Read the live state, the snapshot in `dock` would toggle it shut again.
                let open = shared
                    .area()
                    .upgrade()
                    .is_some_and(|area| area.read(cx).is_dock_open(placement));
                if !open {
                    dock.toggle(window, cx);
                }
                dock.resize_to(event.position, window, cx);
            }
        });

        window.on_mouse_event({
            let shared = self.shared.clone();
            move |_: &MouseUpEvent, phase, _, cx| {
                if !phase.bubble() || shared.resizing_dock().get() != Some(placement) {
                    return;
                }
                shared.resizing_dock().set(None);
                // The size lives on the dock, not the layout tree.
                // Nothing else tells a subscriber to persist it.
                _ = shared
                    .area()
                    .update(cx, |_, cx| cx.emit(DockEvent::LayoutChanged));
            }
        });
    }
}
