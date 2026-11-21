//! The panel row: the main island fills the window; splitting makes two
//! half-width islands with a drag divider that resizes them.

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _,
    Pixels, Render, SharedString, Styled as _, TestAppContext, VisualTestContext, Window, div,
    point, px, size,
};
use ui::common::v_flex;
use ui::layout::panel::{Panel, PanelRow, PanelView};
use ui::tabs::Tabs;
use ui::theme::Theme;

const WINDOW_WIDTH: f32 = 1024.0;
const WINDOW_HEIGHT: f32 = 768.0;

/// A full-size marker the tests measure through its debug selector.
struct TagView {
    selector: SharedString,
}

impl TagView {
    fn new(selector: &'static str) -> Self {
        Self {
            selector: SharedString::from(selector),
        }
    }
}

impl Render for TagView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let selector = self.selector.clone();
        div()
            .debug_selector(move || selector.to_string())
            .size_full()
    }
}

impl Panel for TagView {
    fn title(&self, _cx: &App) -> SharedString {
        self.selector.clone()
    }
}

struct PanelHarness {
    split: bool,
    split_width: Option<Pixels>,
    resizable: bool,
    main: Entity<TagView>,
    split_view: Entity<TagView>,
}

impl Render for PanelHarness {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let panel = PanelRow::new("panel-harness")
            .main(self.main.clone())
            .resizable(self.resizable);
        let panel = match self.split_width {
            Some(width) => panel.split_at(self.split_view.clone(), width),
            None if self.split => panel.split(self.split_view.clone()),
            None => panel,
        };
        div().size_full().child(panel)
    }
}

fn panel_window(cx: &mut TestAppContext, split: bool) -> VisualTestContext {
    split_window(cx, split, None, true)
}

fn split_window(
    cx: &mut TestAppContext,
    split: bool,
    split_width: Option<Pixels>,
    resizable: bool,
) -> VisualTestContext {
    let (main, split_view) = cx.update(|cx| {
        Theme::init(cx);
        let main = cx.new(|_| TagView::new("panel-test-main"));
        let split = cx.new(|_| TagView::new("panel-test-split"));
        (main, split)
    });

    let window = cx.open_window(size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), |_, _| {
        PanelHarness {
            split,
            split_width,
            resizable,
            main,
            split_view,
        }
    });
    let cx = VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    cx
}

#[gpui::test]
fn main_fills_the_row_without_a_split(cx: &mut TestAppContext) {
    let mut cx = panel_window(cx, false);

    let main = cx
        .debug_bounds("panel-test-main")
        .expect("main panel laid out");

    // The row's padding and the island's inset are all main gives up.
    assert!(main.size.width >= px(WINDOW_WIDTH - 40.0));
    assert!(main.size.height >= px(WINDOW_HEIGHT - 40.0));
    assert!(main.origin.x >= px(0.0));
}

#[gpui::test]
fn split_makes_two_half_width_panels(cx: &mut TestAppContext) {
    let mut cx = panel_window(cx, true);

    let main = cx
        .debug_bounds("panel-test-main")
        .expect("main panel laid out");
    let split = cx
        .debug_bounds("panel-test-split")
        .expect("split panel laid out");

    // Both islands share the row at half width, main first, and neither
    // leaves the viewport. The divider's width lands on the remainder pane.
    assert!(
        (main.size.width - split.size.width).abs() <= px(10.0),
        "main={:?} split={:?}",
        main.size.width,
        split.size.width
    );
    assert!(main.size.width < px(WINDOW_WIDTH / 2.0));
    assert!(main.right() <= split.left());
    assert!(split.right() <= px(WINDOW_WIDTH));
    assert!(main.size.height >= px(WINDOW_HEIGHT - 40.0));
}

/// A split opened at a width lays the main pane out at exactly that width.
#[gpui::test]
fn split_at_opens_the_main_pane_at_the_width(cx: &mut TestAppContext) {
    let mut cx = split_window(cx, true, Some(px(300.0)), true);

    let main = cx
        .debug_bounds("panel-test-main")
        .expect("main panel laid out");
    let split = cx
        .debug_bounds("panel-test-split")
        .expect("split panel laid out");

    // The marker fills the island's content box, which extends under the
    // overlay border to the full pane width.
    assert_eq!(main.size.width, px(300.0));
    assert!(main.right() <= split.left());
    assert!(split.size.width > px(300.0));
    assert!(split.right() <= px(WINDOW_WIDTH));
}

/// With resizing disabled there is no divider and drags over the seam do nothing.
#[gpui::test]
fn a_static_split_ignores_drags(cx: &mut TestAppContext) {
    let mut cx = split_window(cx, true, Some(px(300.0)), false);

    let before = cx
        .debug_bounds("panel-test-main")
        .expect("main panel laid out");
    assert_eq!(before.size.width, px(300.0));
    assert!(
        cx.debug_bounds("resize-handle").is_none(),
        "a static split has no divider"
    );

    cx.simulate_event(MouseDownEvent {
        position: point(px(320.0), px(400.0)),
        button: MouseButton::Left,
        modifiers: Modifiers::default(),
        click_count: 1,
        first_mouse: false,
    });
    cx.simulate_event(MouseMoveEvent {
        position: point(px(420.0), px(400.0)),
        pressed_button: Some(MouseButton::Left),
        modifiers: Modifiers::default(),
    });
    cx.simulate_event(MouseUpEvent {
        position: point(px(420.0), px(400.0)),
        button: MouseButton::Left,
        modifiers: Modifiers::default(),
        click_count: 1,
    });
    cx.run_until_parked();

    let after = cx
        .debug_bounds("panel-test-main")
        .expect("main panel laid out after the drag");
    assert_eq!(after, before);
}

#[gpui::test]
fn dragging_the_divider_resizes_the_panels(cx: &mut TestAppContext) {
    let mut cx = panel_window(cx, true);

    let before = cx
        .debug_bounds("panel-test-main")
        .expect("main panel laid out");

    // Press the divider between the islands and drag it 100px to the right.
    let handle = cx.debug_bounds("resize-handle").expect("divider laid out");
    let seam = point(handle.center().x, px(400.0));
    cx.simulate_event(MouseDownEvent {
        position: seam,
        button: MouseButton::Left,
        modifiers: Modifiers::default(),
        click_count: 1,
        first_mouse: false,
    });
    cx.simulate_event(MouseMoveEvent {
        position: point(seam.x + px(100.0), seam.y),
        pressed_button: Some(MouseButton::Left),
        modifiers: Modifiers::default(),
    });
    cx.simulate_event(MouseUpEvent {
        position: point(seam.x + px(100.0), seam.y),
        button: MouseButton::Left,
        modifiers: Modifiers::default(),
        click_count: 1,
    });
    cx.run_until_parked();

    // The main panel grew by the drag; the split panel gave up the space.
    let after = cx
        .debug_bounds("panel-test-main")
        .expect("main panel laid out after the drag");
    assert_eq!(after.size.width, before.size.width + px(100.0));
}

struct MetaView {
    loading: bool,
    prefix: bool,
}

impl Render for MetaView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full()
    }
}

impl Panel for MetaView {
    fn title(&self, _cx: &App) -> SharedString {
        SharedString::from("Meta")
    }

    fn prefix(&self, _cx: &App) -> Option<AnyElement> {
        self.prefix.then(|| {
            div()
                .debug_selector(|| "meta-prefix".to_string())
                .into_any_element()
        })
    }

    fn loading(&self, _cx: &App) -> bool {
        self.loading
    }
}

struct MetaStripHarness {
    busy: Entity<MetaView>,
    idle: Entity<MetaView>,
    prefixed: Entity<MetaView>,
}

impl Render for MetaStripHarness {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.busy.clone();
        let idle = self.idle.clone();
        let prefixed = self.prefixed.clone();
        v_flex()
            .size_full()
            .gap_1()
            .child(
                Tabs::new("busy-strip")
                    .tabs(move |_, cx| vec![busy.tab("busy".into(), cx).closeable(false)]),
            )
            .child(
                Tabs::new("idle-strip")
                    .tabs(move |_, cx| vec![idle.tab("idle".into(), cx).closeable(false)]),
            )
            .child(
                Tabs::new("prefix-strip")
                    .tabs(move |_, cx| vec![prefixed.tab("prefix".into(), cx).closeable(false)]),
            )
    }
}

#[gpui::test]
fn panel_tab_uses_title_prefix_and_loading(cx: &mut TestAppContext) {
    cx.update(Theme::init);
    let busy = cx.new(|_| MetaView {
        loading: true,
        prefix: false,
    });
    let idle = cx.new(|_| MetaView {
        loading: false,
        prefix: false,
    });
    let prefixed = cx.new(|_| MetaView {
        loading: false,
        prefix: true,
    });
    let window = cx.open_window(size(px(800.), px(600.)), |_, _| MetaStripHarness {
        busy,
        idle,
        prefixed,
    });
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();

    assert!(
        cx.debug_bounds("busy-strip-tab-Meta").is_some(),
        "title becomes the tab label"
    );
    assert!(
        cx.debug_bounds("meta-prefix").is_some(),
        "prefix becomes the tab lead"
    );
    let busy = cx
        .debug_bounds("busy-strip-tab-Meta")
        .expect("busy tab laid out");
    let idle = cx
        .debug_bounds("idle-strip-tab-Meta")
        .expect("idle tab laid out");
    assert!(
        busy.size.width > idle.size.width,
        "loading adds the spinner to the tab"
    );
}
