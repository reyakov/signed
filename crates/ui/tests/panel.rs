//! The panel stack: the main island fills the window; splitting makes two
//! half-width islands with a drag divider that resizes them.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _,
    Render, SharedString, Styled as _, TestAppContext, VisualTestContext, Window, div, point, px,
    size,
};
use ui::common::v_flex;
use ui::input::{Input, InputState};
use ui::layout::panel::{Panel, PanelSlot, PanelStack, PanelView, SplitSide};
use ui::tabs::Tabs;
use ui::theme::Theme;

const WINDOW_WIDTH: f32 = 1024.0;
const WINDOW_HEIGHT: f32 = 768.0;

/// A full-size marker the tests measure through its debug selector.
struct TagView {
    selector: SharedString,
    /// Whether the marker asks for the zoom and close toolbar.
    toolbar: bool,
}

impl TagView {
    fn new(selector: &'static str) -> Self {
        Self {
            selector: SharedString::from(selector),
            toolbar: false,
        }
    }

    fn with_toolbar(selector: &'static str) -> Self {
        Self {
            toolbar: true,
            ..Self::new(selector)
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

    fn toolbar(&self, _cx: &App) -> bool {
        self.toolbar
    }
}

struct PanelHarness {
    stack: Entity<PanelStack>,
}

impl Render for PanelHarness {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.stack.clone())
    }
}

/// A split panel that counts its renders around a field.
struct FieldView {
    input: Entity<InputState>,
    renders: Rc<Cell<usize>>,
}

impl Render for FieldView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        div()
            .debug_selector(|| "panel-test-field".to_string())
            .size_full()
            .child(Input::new("split-field", self.input.clone()))
    }
}

impl Panel for FieldView {
    fn title(&self, _cx: &App) -> SharedString {
        "Field".into()
    }
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

/// The titlebar strip above the stack.
struct TabsHarness {
    stack: Entity<PanelStack>,
}

impl Render for TabsHarness {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .children(PanelStack::main_tabs(self.stack.clone(), cx))
            .child(self.stack.clone())
    }
}

/// A marker view.
fn tag(cx: &mut App, selector: &'static str) -> Entity<TagView> {
    cx.new(|_| TagView::new(selector))
}

/// A marker view that shows the zoom and close toolbar.
fn toolbar_tag(cx: &mut App, selector: &'static str) -> Entity<TagView> {
    cx.new(|_| TagView::with_toolbar(selector))
}

/// Opens the test window over a stack seeded by `seed`.
fn stack_window(
    cx: &mut TestAppContext,
    seed: impl FnOnce(&mut PanelStack, &mut Context<PanelStack>),
) -> (VisualTestContext, Entity<PanelStack>) {
    let stack = cx.update(|cx| {
        Theme::init_with(cx, Theme::fallback_source());
        let stack = cx.new(|cx| PanelStack::new("panel-harness", cx));
        stack.update(cx, seed);
        stack
    });
    let window = cx.open_window(size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), |_, _| {
        PanelHarness {
            stack: stack.clone(),
        }
    });
    let cx = VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    (cx, stack)
}

/// Seeds the main stack with one panel.
fn main_only(stack: &mut PanelStack, cx: &mut Context<PanelStack>) {
    stack.insert(
        PanelSlot::Main,
        "main",
        Arc::new(tag(cx, "panel-test-main")),
        cx,
    );
}

/// Seeds the main stack and the split stack with one panel each.
fn main_and_split(stack: &mut PanelStack, cx: &mut Context<PanelStack>) {
    main_only(stack, cx);
    stack.insert(
        PanelSlot::Split,
        "split",
        Arc::new(tag(cx, "panel-test-split")),
        cx,
    );
}

/// Seeds the empty state: a 300px pane with another beside it.
fn empty_state(stack: &mut PanelStack, cx: &mut Context<PanelStack>) {
    stack.set_empty(
        tag(cx, "panel-test-main"),
        Some((tag(cx, "panel-test-split").into(), px(300.))),
        cx,
    );
}

#[gpui::test]
fn main_fills_the_row_without_a_split(cx: &mut TestAppContext) {
    let (mut cx, _stack) = stack_window(cx, main_only);

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
    let (mut cx, _stack) = stack_window(cx, main_and_split);

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

/// An empty state's split opens its main pane at the given width.
#[gpui::test]
fn an_empty_state_split_opens_its_main_pane_at_the_width(cx: &mut TestAppContext) {
    let (mut cx, _stack) = stack_window(cx, empty_state);

    let main = cx
        .debug_bounds("panel-test-main")
        .expect("main panel laid out");
    let split = cx
        .debug_bounds("panel-test-split")
        .expect("split panel laid out");

    // The marker fills the island's content box, inside its 1px border.
    assert!((main.size.width - px(300.0)).abs() <= px(2.0));
    assert!(main.right() <= split.left());
    assert!(split.size.width > px(300.0));
    assert!(split.right() <= px(WINDOW_WIDTH));
}

/// With resizing disabled there is no divider and drags over the seam do nothing.
#[gpui::test]
fn a_static_split_ignores_drags(cx: &mut TestAppContext) {
    let (mut cx, _stack) = stack_window(cx, |stack, cx| {
        empty_state(stack, cx);
        stack.set_resizable(false, cx);
    });

    let before = cx
        .debug_bounds("panel-test-main")
        .expect("main panel laid out");
    assert!((before.size.width - px(300.0)).abs() <= px(2.0));
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
    let (mut cx, _stack) = stack_window(cx, main_and_split);

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

#[gpui::test]
fn panel_tab_uses_title_prefix_and_loading(cx: &mut TestAppContext) {
    cx.update(|cx| Theme::init_with(cx, Theme::fallback_source()));
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

/// The split pane exists only while the split stack holds a panel.
#[gpui::test]
fn the_split_pane_follows_the_split_stack(cx: &mut TestAppContext) {
    let (mut cx, stack) = stack_window(cx, main_only);
    assert!(
        cx.debug_bounds("panel-test-split").is_none(),
        "no split pane without a split panel"
    );

    cx.update(|_, cx| {
        stack.update(cx, |stack, cx| {
            stack.insert(
                PanelSlot::Split,
                "split",
                Arc::new(tag(cx, "panel-test-split")),
                cx,
            )
        })
    });
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("panel-test-split").is_some(),
        "a split panel opens the split pane"
    );

    cx.update(|_, cx| {
        stack.update(cx, |stack, cx| {
            stack.close(PanelSlot::Split, &SharedString::from("split"), cx)
        })
    });
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("panel-test-split").is_none(),
        "closing the last split panel closes the pane"
    );
}

/// A main panel replaces the empty state.
#[gpui::test]
fn a_main_panel_replaces_the_empty_state(cx: &mut TestAppContext) {
    let (mut cx, stack) = stack_window(cx, |stack, cx| {
        stack.set_empty(
            tag(cx, "panel-test-empty"),
            Some((tag(cx, "panel-test-side").into(), px(300.))),
            cx,
        );
    });
    assert!(cx.debug_bounds("panel-test-empty").is_some());
    assert!(cx.debug_bounds("panel-test-side").is_some());

    cx.update(|_, cx| {
        stack.update(cx, |stack, cx| {
            stack.insert(
                PanelSlot::Main,
                "main",
                Arc::new(tag(cx, "panel-test-main")),
                cx,
            )
        })
    });
    cx.run_until_parked();

    assert!(
        cx.debug_bounds("panel-test-empty").is_none(),
        "the empty state leaves when a main panel opens"
    );
    assert!(
        cx.debug_bounds("panel-test-side").is_none(),
        "its split pane leaves with it"
    );
    assert!(cx.debug_bounds("panel-test-main").is_some());
}

/// A split that leaves the screen and comes back starts at half again.
#[gpui::test]
fn a_reopened_split_starts_at_half_again(cx: &mut TestAppContext) {
    let (mut cx, stack) = stack_window(cx, main_and_split);
    let start = cx
        .debug_bounds("panel-test-main")
        .expect("main panel laid out")
        .size
        .width;

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
    let dragged = cx
        .debug_bounds("panel-test-main")
        .expect("main panel laid out after the drag")
        .size
        .width;
    assert_eq!(dragged, start + px(100.0));

    let split = SharedString::from("split");
    cx.update(|_, cx| stack.update(cx, |stack, cx| stack.close(PanelSlot::Split, &split, cx)));
    cx.run_until_parked();
    cx.update(|_, cx| {
        stack.update(cx, |stack, cx| {
            stack.insert(
                PanelSlot::Split,
                split,
                Arc::new(tag(cx, "panel-test-split")),
                cx,
            )
        })
    });
    cx.run_until_parked();

    let reopened = cx
        .debug_bounds("panel-test-main")
        .expect("main panel laid out after reopening")
        .size
        .width;
    assert_eq!(
        reopened, start,
        "the divider position is forgotten while no split is on screen"
    );
}

/// The titlebar strip lists the main stack; its tabs switch and close panels.
#[gpui::test]
fn main_tabs_switch_and_close_the_main_stack(cx: &mut TestAppContext) {
    let stack = cx.update(|cx| {
        Theme::init_with(cx, Theme::fallback_source());
        cx.new(|cx| PanelStack::new("panel-harness", cx))
    });
    let window = cx.open_window(size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), |_, _| {
        TabsHarness {
            stack: stack.clone(),
        }
    });
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("panel-harness-main-tabs-tab-panel-test-first")
            .is_none(),
        "no strip while the main stack is empty"
    );

    cx.update(|_, cx| {
        stack.update(cx, |stack, cx| {
            stack.insert(
                PanelSlot::Main,
                "first",
                Arc::new(tag(cx, "panel-test-first")),
                cx,
            );
            stack.insert(
                PanelSlot::Main,
                "second",
                Arc::new(tag(cx, "panel-test-second")),
                cx,
            );
        })
    });
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("panel-test-second").is_some(),
        "the newest panel opens shown"
    );

    let first = cx
        .debug_bounds("panel-harness-main-tabs-tab-panel-test-first")
        .expect("the strip lists the first panel");
    cx.simulate_click(first.center(), Modifiers::none());
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("panel-test-first").is_some(),
        "clicking a tab shows its panel"
    );
    assert!(cx.debug_bounds("panel-test-second").is_none());
    // The close control is laid out only while its tab is hovered.
    let second = cx
        .debug_bounds("panel-harness-main-tabs-tab-panel-test-second")
        .expect("the strip lists the second panel");
    cx.simulate_event(MouseMoveEvent {
        position: second.center(),
        pressed_button: None,
        modifiers: Modifiers::default(),
    });
    cx.run_until_parked();

    let close = cx
        .debug_bounds("panel-harness-main-tabs-close-panel-test-second")
        .expect("tabs close");
    cx.simulate_click(close.center(), Modifiers::none());
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("panel-harness-main-tabs-tab-panel-test-second")
            .is_none(),
        "closing a tab closes its panel"
    );
    assert!(
        cx.debug_bounds("panel-test-first").is_some(),
        "the shown panel stays"
    );
}

/// A split inserted at a width opens with it.
#[gpui::test]
fn a_split_inserted_at_a_width_opens_there(cx: &mut TestAppContext) {
    let (mut cx, stack) = stack_window(cx, main_only);
    cx.update(|_, cx| {
        stack.update(cx, |stack, cx| {
            stack.insert_split_at(
                "side",
                px(300.),
                SplitSide::Trailing,
                Arc::new(tag(cx, "panel-test-split")),
                cx,
            )
        })
    });
    cx.run_until_parked();

    let split = cx
        .debug_bounds("panel-test-split")
        .expect("split panel laid out");
    assert!((split.size.width - px(300.0)).abs() <= px(2.0));
    cx.update(|_, cx| {
        let stack = stack.read(cx);
        let side = SharedString::from("side");
        assert!(stack.is_open(PanelSlot::Split, &side));
        assert!(stack.is_active(PanelSlot::Split, &side));
    });
}

/// A split already on screen keeps its divider and side when another opens at a width.
#[gpui::test]
fn a_width_and_side_apply_to_a_split_new_to_the_screen_only(cx: &mut TestAppContext) {
    let (mut cx, stack) = stack_window(cx, main_and_split);
    let before = cx
        .debug_bounds("panel-test-main")
        .expect("main panel laid out");

    cx.update(|_, cx| {
        stack.update(cx, |stack, cx| {
            stack.insert_split_at(
                "side",
                px(300.),
                SplitSide::Leading,
                Arc::new(tag(cx, "panel-test-side")),
                cx,
            )
        })
    });
    cx.run_until_parked();

    let after = cx
        .debug_bounds("panel-test-main")
        .expect("main panel laid out");
    assert_eq!(after, before);
    cx.update(|_, cx| {
        let stack = stack.read(cx);
        let split = SharedString::from("split");
        assert!(stack.is_open(PanelSlot::Split, &split));
        assert!(!stack.is_active(PanelSlot::Split, &split));
    });
    let side = cx
        .debug_bounds("panel-test-side")
        .expect("the new split panel laid out");
    assert!(
        side.left() >= after.right(),
        "it still opens after the main panel"
    );
}

/// A split opened at a width keeps it across window resizes until the divider is dragged.
#[gpui::test]
fn a_width_split_follows_resizes_until_dragged(cx: &mut TestAppContext) {
    let (mut cx, _stack) = stack_window(cx, |stack, cx| {
        main_only(stack, cx);
        stack.insert_split_at(
            "side",
            px(300.),
            SplitSide::Trailing,
            Arc::new(tag(cx, "panel-test-split")),
            cx,
        );
    });

    cx.simulate_resize(size(px(WINDOW_WIDTH + 400.0), px(WINDOW_HEIGHT)));
    cx.run_until_parked();
    let split = cx
        .debug_bounds("panel-test-split")
        .expect("split panel laid out after the resize");
    assert!(
        (split.size.width - px(300.0)).abs() <= px(2.0),
        "the split keeps its width: {:?}",
        split.size.width
    );

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
    let dragged = cx
        .debug_bounds("panel-test-main")
        .expect("main panel laid out after the drag")
        .size
        .width;

    cx.simulate_resize(size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)));
    cx.run_until_parked();
    let resized = cx
        .debug_bounds("panel-test-main")
        .expect("main panel laid out after the resize")
        .size
        .width;
    assert_eq!(resized, dragged, "a dragged divider keeps its pane's width");
}

/// A leading split opens before the main panel and keeps its width, dragged or not, across window resizes.
#[gpui::test]
fn a_leading_split_opens_before_the_main_panel_at_its_width(cx: &mut TestAppContext) {
    let (mut cx, _stack) = stack_window(cx, |stack, cx| {
        main_only(stack, cx);
        stack.insert_split_at(
            "side",
            px(300.),
            SplitSide::Leading,
            Arc::new(tag(cx, "panel-test-split")),
            cx,
        );
    });

    let split = cx
        .debug_bounds("panel-test-split")
        .expect("split panel laid out");
    let main = cx
        .debug_bounds("panel-test-main")
        .expect("main panel laid out");
    assert!(split.right() <= main.left(), "the split comes first");
    assert!((split.size.width - px(300.0)).abs() <= px(2.0));

    cx.simulate_resize(size(px(WINDOW_WIDTH + 400.0), px(WINDOW_HEIGHT)));
    cx.run_until_parked();
    let split = cx
        .debug_bounds("panel-test-split")
        .expect("split panel laid out after the resize");
    assert!(
        (split.size.width - px(300.0)).abs() <= px(2.0),
        "the split keeps its width: {:?}",
        split.size.width
    );

    // Drag the divider 100px right, then put the window back: the split keeps the dragged width.
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

    cx.simulate_resize(size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)));
    cx.run_until_parked();
    let split = cx
        .debug_bounds("panel-test-split")
        .expect("split panel laid out after the resize");
    assert!(
        (split.size.width - px(400.0)).abs() <= px(2.0),
        "the split keeps its dragged width: {:?}",
        split.size.width
    );
}

/// Once no split is on screen, the next one opens after the main panel again.
#[gpui::test]
fn a_split_after_a_leading_one_opens_after_the_main_panel(cx: &mut TestAppContext) {
    let (mut cx, stack) = stack_window(cx, |stack, cx| {
        main_only(stack, cx);
        stack.insert_split_at(
            "side",
            px(300.),
            SplitSide::Leading,
            Arc::new(tag(cx, "panel-test-side")),
            cx,
        );
    });

    let side = SharedString::from("side");
    cx.update(|_, cx| {
        stack.update(cx, |stack, cx| {
            stack.close(PanelSlot::Split, &side, cx);
            stack.insert(
                PanelSlot::Split,
                "split",
                Arc::new(tag(cx, "panel-test-split")),
                cx,
            );
        })
    });
    cx.run_until_parked();

    let main = cx
        .debug_bounds("panel-test-main")
        .expect("main panel laid out");
    let split = cx
        .debug_bounds("panel-test-split")
        .expect("split panel laid out");
    assert!(
        main.right() <= split.left(),
        "the split opens after the main panel again"
    );
}

/// A panel's toolbar zooms it to fill the row and back, and closes it.
#[gpui::test]
fn the_toolbar_zooms_its_panel_and_closes_it(cx: &mut TestAppContext) {
    let (mut cx, _stack) = stack_window(cx, |stack, cx| {
        main_only(stack, cx);
        stack.insert(
            PanelSlot::Split,
            "split",
            Arc::new(toolbar_tag(cx, "panel-test-split")),
            cx,
        );
    });

    let pane = cx
        .debug_bounds("panel-test-split")
        .expect("split panel laid out");
    let zoom = cx
        .debug_bounds("panel-harness-zoom-split")
        .expect("the split pane shows a zoom button");
    assert!(
        zoom.right() <= pane.right() && zoom.top() >= pane.top(),
        "the toolbar sits over the pane's top right: {:?}",
        zoom
    );
    assert!(
        cx.debug_bounds("panel-harness-zoom-main").is_none(),
        "a panel without the toolbar shows none"
    );

    cx.simulate_click(zoom.center(), Modifiers::none());
    cx.run_until_parked();
    let zoomed = cx
        .debug_bounds("panel-test-split")
        .expect("split panel laid out zoomed");
    assert!(
        cx.debug_bounds("panel-test-main").is_none(),
        "zooming hides the main pane"
    );
    assert!(
        cx.debug_bounds("resize-handle").is_none(),
        "and the divider"
    );
    assert!(zoomed.size.width >= px(WINDOW_WIDTH - 40.0));

    cx.simulate_click(zoom.center(), Modifiers::none());
    cx.run_until_parked();
    let restored = cx
        .debug_bounds("panel-test-split")
        .expect("split panel laid out restored");
    assert!(
        cx.debug_bounds("panel-test-main").is_some(),
        "unzooming brings the main pane back"
    );
    assert!(restored.size.width < px(WINDOW_WIDTH / 2.0));

    let close = cx
        .debug_bounds("panel-harness-close-split")
        .expect("the split pane shows a close button");
    cx.simulate_click(close.center(), Modifiers::none());
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("panel-test-split").is_none(),
        "close closes the panel"
    );
    assert!(
        cx.debug_bounds("panel-harness-zoom-split").is_none(),
        "and its toolbar leaves with it"
    );
}

/// Without a split on screen, the toolbar's buttons do nothing.
#[gpui::test]
fn toolbar_buttons_do_nothing_without_a_split(cx: &mut TestAppContext) {
    let (mut cx, stack) = stack_window(cx, |stack, cx| {
        stack.insert(
            PanelSlot::Main,
            "main",
            Arc::new(toolbar_tag(cx, "panel-test-main")),
            cx,
        );
    });
    let before = cx
        .debug_bounds("panel-test-main")
        .expect("main panel laid out");
    let zoom = cx
        .debug_bounds("panel-harness-zoom-main")
        .expect("the main pane shows a zoom button");
    let close = cx
        .debug_bounds("panel-harness-close-main")
        .expect("the main pane shows a close button");

    cx.simulate_click(zoom.center(), Modifiers::none());
    cx.simulate_click(close.center(), Modifiers::none());
    cx.run_until_parked();

    assert_eq!(
        cx.debug_bounds("panel-test-main")
            .expect("main panel laid out"),
        before,
        "the buttons are disabled without a split"
    );
    cx.update(|_, cx| {
        assert!(
            stack
                .read(cx)
                .is_active(PanelSlot::Main, &SharedString::from("main"))
        );
    });
}

/// Zooming belongs to the panel: showing another split panel ends it.
#[gpui::test]
fn zoom_ends_when_another_split_panel_is_shown(cx: &mut TestAppContext) {
    let (mut cx, _stack) = stack_window(cx, |stack, cx| {
        main_only(stack, cx);
        stack.insert(
            PanelSlot::Split,
            "second",
            Arc::new(tag(cx, "panel-test-second")),
            cx,
        );
        stack.insert(
            PanelSlot::Split,
            "first",
            Arc::new(toolbar_tag(cx, "panel-test-first")),
            cx,
        );
    });
    let zoom = cx
        .debug_bounds("panel-harness-zoom-split")
        .expect("zoom button");
    cx.simulate_click(zoom.center(), Modifiers::none());
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("panel-test-main").is_none(),
        "the first panel is zoomed"
    );

    let second = cx
        .debug_bounds("panel-harness-split-tabs-tab-panel-test-second")
        .expect("the second panel's pill tab");
    cx.simulate_click(second.center(), Modifiers::none());
    cx.run_until_parked();
    assert!(cx.debug_bounds("panel-test-second").is_some());
    assert!(
        cx.debug_bounds("panel-test-main").is_some(),
        "the split is back once another panel is shown"
    );
}

/// The split panel is cached: it repaints for its own changes, not for other panels'.
#[gpui::test]
fn a_split_panel_repaints_for_its_own_changes_only(cx: &mut TestAppContext) {
    let renders = Rc::new(Cell::new(0));
    let (input, main, field) = cx.update(|cx| {
        let input = InputState::new(cx);
        let field = cx.new(|_| FieldView {
            input: input.clone(),
            renders: Rc::clone(&renders),
        });
        (input, tag(cx, "panel-test-main"), field)
    });
    let seeded_main = main.clone();
    let seeded_field = field.clone();
    let (mut cx, _stack) = stack_window(cx, move |stack, cx| {
        stack.insert(PanelSlot::Main, "main", Arc::new(seeded_main), cx);
        stack.insert(PanelSlot::Split, "split", Arc::new(seeded_field), cx);
    });
    let rendered = renders.get();

    cx.update(|window, cx| input.update(cx, |input, cx| input.set_value("hey", window, cx)));
    cx.run_until_parked();
    let typed = cx.update(|_, cx| input.read(cx).value().to_string());
    assert_eq!(typed, "hey");
    assert!(
        renders.get() > rendered,
        "an edit in the split panel repaints it"
    );

    let rendered = renders.get();
    cx.update(|_, cx| main.update(cx, |_, cx| cx.notify()));
    cx.run_until_parked();
    assert_eq!(
        renders.get(),
        rendered,
        "another panel's change leaves the split panel as it was"
    );

    cx.update(|_, cx| field.update(cx, |_, cx| cx.notify()));
    cx.run_until_parked();
    assert!(
        renders.get() > rendered,
        "the split panel's own change repaints it"
    );
}
