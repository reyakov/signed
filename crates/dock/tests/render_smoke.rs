use dock::{BasePanel, Panel, SignedDockSkin, panel_handle};
use gpui::{
    App, AppContext, Context, Empty, EventEmitter, FocusHandle, Focusable, IntoElement, Render,
    TestAppContext, Window,
};
use gpui_base::dock::{DockArea, DockLayout, DockPlacement, PanelEvent};

struct Probe {
    focus_handle: FocusHandle,
}

impl Probe {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
        }
    }
}

impl BasePanel for Probe {
    fn panel_name(&self) -> &'static str {
        "Probe"
    }
}

impl Panel for Probe {
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        "Probe"
    }
}

impl EventEmitter<PanelEvent> for Probe {}

impl Focusable for Probe {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

#[gpui::test]
fn the_first_frame_renders_the_area_and_its_docks(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
    });
    let (area, cx) = cx.add_window_view(|window, cx| {
        let skin = SignedDockSkin::new(cx);
        DockArea::new("test", None, window, cx).with_renderer(skin)
    });

    let bottom = cx.update(|_, cx| cx.new(Probe::new));
    cx.update(|window, cx| {
        let left = cx.new(Probe::new);
        let center = cx.new(Probe::new);

        area.update(cx, |area, cx| {
            area.set_dock(
                DockPlacement::Left,
                DockLayout::tabs().panel_view(panel_handle(left), cx),
                window,
                cx,
            );
            area.set_center(
                DockLayout::tabs().panel_view(panel_handle(center), cx),
                window,
                cx,
            );
            area.set_dock(
                DockPlacement::Bottom,
                DockLayout::tabs().panel_view(panel_handle(bottom.clone()), cx),
                window,
                cx,
            );
        });
    });

    // The first frame walks every render hook, all of which read the dock area.
    cx.update(|window, cx| window.draw(cx).clear(cx));

    // Emptying a dock leaves an empty group, its render must also be safe.
    cx.update(|window, cx| {
        area.update(cx, |area, cx| {
            area.remove_panel(bottom, window, cx);
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
}
