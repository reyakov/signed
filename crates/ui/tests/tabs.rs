//! Grouped tabs: same-kind tabs collapse into one chip that opens a popover
//! listing the members; picking one selects it by its original index. Tabs
//! also drag onto each other to reorder, and pinned tabs wear the active
//! treatment at all times.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    Context, IntoElement, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement as _, Render, Styled as _, TestAppContext, VisualTestContext, Window, div, point,
    px, size,
};
use ui::tabs::{Tab, Tabs, TabsVariant};
use ui::theme::Theme;

struct StripHarness {
    changed: Rc<RefCell<Vec<usize>>>,
    reordered: Rc<RefCell<Vec<(usize, usize)>>>,
    pinned: bool,
}

impl Render for StripHarness {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let changed = self.changed.clone();
        let reordered = self.reordered.clone();
        div().size_full().child(
            Tabs::new("strip")
                .variant(TabsVariant::Pill)
                .draggable(true)
                .tabs({
                    let pinned = self.pinned;
                    move |_, _| {
                        vec![
                            Tab::new("a", "Alpha").kind("Kind"),
                            Tab::new("b", "Beta").kind("Kind"),
                            Tab::new("c", "Gamma"),
                            Tab::new("d", "Delta").kind("Lone").pinned(pinned),
                        ]
                    }
                })
                .on_change(move |index, _, _| changed.borrow_mut().push(index))
                .on_reorder(move |from, to, _, _| reordered.borrow_mut().push((from, to))),
        )
    }
}

fn strip_window(
    cx: &mut TestAppContext,
    changed: Rc<RefCell<Vec<usize>>>,
    reordered: Rc<RefCell<Vec<(usize, usize)>>>,
    pinned: bool,
) -> VisualTestContext {
    cx.update(Theme::init);
    let window = cx.open_window(size(px(800.), px(600.)), |_, _| StripHarness {
        changed,
        reordered,
        pinned,
    });
    let cx = VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    cx
}

struct CloseHarness {
    closed: Rc<RefCell<Vec<usize>>>,
}

impl Render for CloseHarness {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let closed = self.closed.clone();
        div().size_full().child(
            Tabs::new("strip")
                .variant(TabsVariant::Pill)
                .tabs(move |_, _| {
                    vec![
                        Tab::new("a", "Alpha").closeable(true),
                        Tab::new("b", "Beta").closeable(true),
                    ]
                })
                .on_close(move |index, _, _| closed.borrow_mut().push(index)),
        )
    }
}

#[gpui::test]
fn a_pinned_tab_draws_inline_and_selects_itself(cx: &mut TestAppContext) {
    let changed = Rc::new(RefCell::new(Vec::new()));
    let mut cx = strip_window(cx, changed.clone(), Rc::new(RefCell::new(Vec::new())), true);

    let delta = cx
        .debug_bounds("strip-tab-Delta")
        .expect("a pinned tab draws itself");
    cx.simulate_click(delta.center(), Modifiers::none());
    cx.run_until_parked();

    assert_eq!(
        changed.borrow().as_slice(),
        &[3],
        "clicking the pinned tab reports its index"
    );
}

#[gpui::test]
fn a_closeable_tab_reports_its_close(cx: &mut TestAppContext) {
    cx.update(Theme::init);
    let closed = Rc::new(RefCell::new(Vec::new()));
    let window = cx.open_window(size(px(800.), px(600.)), |_, _| CloseHarness {
        closed: closed.clone(),
    });
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();

    let close = cx
        .debug_bounds("strip-close-Alpha")
        .expect("a closeable tab draws its close control");
    cx.simulate_click(close.center(), Modifiers::none());
    cx.run_until_parked();

    assert_eq!(
        closed.borrow().as_slice(),
        &[0],
        "the close control reports the tab's index"
    );
}

#[gpui::test]
fn same_kind_tabs_collapse_into_one_group(cx: &mut TestAppContext) {
    let mut cx = strip_window(
        cx,
        Rc::new(RefCell::new(Vec::new())),
        Rc::new(RefCell::new(Vec::new())),
        false,
    );

    assert!(
        cx.debug_bounds("strip-group-Kind").is_some(),
        "two same-kind tabs draw one group chip"
    );
    assert!(
        cx.debug_bounds("strip-tab-Alpha").is_none(),
        "grouped members do not draw inline"
    );
    assert!(cx.debug_bounds("strip-tab-Beta").is_none());

    // A tab outside any two-member kind group draws itself, kinded or not.
    assert!(cx.debug_bounds("strip-tab-Gamma").is_some());
    assert!(cx.debug_bounds("strip-tab-Delta").is_some());
    assert!(
        cx.debug_bounds("strip-group-Lone").is_none(),
        "a lone kinded tab is not grouped"
    );
}

#[gpui::test]
fn picking_a_group_member_selects_it(cx: &mut TestAppContext) {
    let changed = Rc::new(RefCell::new(Vec::new()));
    let mut cx = strip_window(
        cx,
        changed.clone(),
        Rc::new(RefCell::new(Vec::new())),
        false,
    );

    let chip = cx
        .debug_bounds("strip-group-Kind")
        .expect("group chip drawn");
    cx.simulate_click(chip.center(), Modifiers::none());
    cx.run_until_parked();

    let beta = cx
        .debug_bounds("strip-tab-Beta")
        .expect("the popover lists the members");
    cx.simulate_click(beta.center(), Modifiers::none());
    cx.run_until_parked();

    assert_eq!(
        changed.borrow().as_slice(),
        &[1],
        "picking a member reports its original index"
    );
    assert!(
        cx.debug_bounds("strip-tab-Beta").is_none(),
        "a selection dismisses the popover"
    );
}

#[gpui::test]
fn dragging_a_tab_onto_another_reorders_it(cx: &mut TestAppContext) {
    let reordered = Rc::new(RefCell::new(Vec::new()));
    let mut cx = strip_window(
        cx,
        Rc::new(RefCell::new(Vec::new())),
        reordered.clone(),
        false,
    );

    // The strip's group chip hides Alpha and Beta; Gamma draws inline, so the
    // drag runs from Gamma onto Delta: moving Gamma before Delta is a no-op
    // shift — use Delta onto Gamma instead, reporting (3, 2).
    let gamma = cx.debug_bounds("strip-tab-Gamma").expect("gamma drawn");
    let delta = cx.debug_bounds("strip-tab-Delta").expect("delta drawn");

    cx.simulate_event(MouseDownEvent {
        position: delta.center(),
        button: MouseButton::Left,
        modifiers: Modifiers::default(),
        click_count: 1,
        first_mouse: false,
    });
    cx.simulate_event(MouseMoveEvent {
        position: gamma.center(),
        pressed_button: Some(MouseButton::Left),
        modifiers: Modifiers::default(),
    });
    cx.simulate_event(MouseUpEvent {
        position: gamma.center(),
        button: MouseButton::Left,
        modifiers: Modifiers::default(),
        click_count: 1,
    });
    cx.run_until_parked();

    assert_eq!(
        reordered.borrow().as_slice(),
        &[(3, 2)],
        "dropping Delta before Gamma reports the move"
    );
}

#[gpui::test]
fn dropping_a_tab_back_on_itself_reorders_nothing(cx: &mut TestAppContext) {
    let reordered = Rc::new(RefCell::new(Vec::new()));
    let mut cx = strip_window(
        cx,
        Rc::new(RefCell::new(Vec::new())),
        reordered.clone(),
        false,
    );

    let gamma = cx.debug_bounds("strip-tab-Gamma").expect("gamma drawn");
    cx.simulate_event(MouseDownEvent {
        position: gamma.center(),
        button: MouseButton::Left,
        modifiers: Modifiers::default(),
        click_count: 1,
        first_mouse: false,
    });
    cx.simulate_event(MouseMoveEvent {
        position: gamma.center() + point(px(2.0), px(0.0)),
        pressed_button: Some(MouseButton::Left),
        modifiers: Modifiers::default(),
    });
    cx.simulate_event(MouseUpEvent {
        position: gamma.center(),
        button: MouseButton::Left,
        modifiers: Modifiers::default(),
        click_count: 1,
    });
    cx.run_until_parked();

    assert!(
        reordered.borrow().is_empty(),
        "a self-drop is a no-op, not a reorder"
    );
}

#[gpui::test]
fn a_vertical_drag_cancels_the_reorder(cx: &mut TestAppContext) {
    let reordered = Rc::new(RefCell::new(Vec::new()));
    let mut cx = strip_window(
        cx,
        Rc::new(RefCell::new(Vec::new())),
        reordered.clone(),
        false,
    );

    // Drag Delta straight down past the axis lock, then back onto Gamma:
    // the gesture locked vertical, so the drop must not reorder.
    let delta = cx.debug_bounds("strip-tab-Delta").expect("delta drawn");
    let gamma = cx.debug_bounds("strip-tab-Gamma").expect("gamma drawn");
    let start = delta.center();

    cx.simulate_event(MouseDownEvent {
        position: start,
        button: MouseButton::Left,
        modifiers: Modifiers::default(),
        click_count: 1,
        first_mouse: false,
    });
    // The drag engages a few pixels down; land the origin there.
    cx.simulate_event(MouseMoveEvent {
        position: start + point(px(0.0), px(3.0)),
        pressed_button: Some(MouseButton::Left),
        modifiers: Modifiers::default(),
    });
    // Past the axis lock, vertically: dy 30 vs dx 0.
    cx.simulate_event(MouseMoveEvent {
        position: start + point(px(0.0), px(30.0)),
        pressed_button: Some(MouseButton::Left),
        modifiers: Modifiers::default(),
    });
    // Back over Gamma — still canceled.
    cx.simulate_event(MouseMoveEvent {
        position: gamma.center(),
        pressed_button: Some(MouseButton::Left),
        modifiers: Modifiers::default(),
    });
    cx.simulate_event(MouseUpEvent {
        position: gamma.center(),
        button: MouseButton::Left,
        modifiers: Modifiers::default(),
        click_count: 1,
    });
    cx.run_until_parked();

    assert!(
        reordered.borrow().is_empty(),
        "a vertical drag never reorders, even dropped on another tab"
    );
}
