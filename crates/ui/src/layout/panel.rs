//! The panel stack: the window's main content, island-style over the app
//! background. It owns every panel it shows: a main stack, tabbed in the
//! titlebar through [`PanelStack::main_tabs`], and a split stack that opens a
//! second pane beside the main panel while it holds a panel, at half the
//! width or at a set width on either side of it
//! ([`PanelStack::insert_split_at`]), with a drag divider between the panes; a
//! double-click restores half. While no main panel is open the stack shows the
//! empty state its owner set ([`PanelStack::set_empty`]).
//! A panel can ask for a zoom and close toolbar over its pane's top right
//! corner ([`Panel::toolbar`]); zooming shows that panel alone until toggled back.

use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AnyView, App, Axis, Context, Div, ElementId, Entity, IntoElement,
    ParentElement as _, Pixels, Render, SharedString, StyleRefinement, Styled as _, Window, div,
};

use crate::components::button::{Button, ButtonVariant};
use crate::components::common::{ResultLogErr as _, Sizable as _, h_flex, v_flex};
use crate::components::tabs::{Tab, Tabs, TabsVariant};
use crate::components::tooltip::Tooltip;
use crate::icons::{Icon, IconRole};
use crate::layout::resizable::{Resizable, ResizableState};
use crate::theme::{ActiveTheme as _, PANE_MIN_SIZE, RESIZE_HANDLE_WIDTH};

/// A user-defined panel the stack can show.
#[allow(unused_variables)]
pub trait Panel: Render + 'static {
    /// The kind the panel belongs to; same-kind panels group in the tab bar.
    fn kind(&self, cx: &App) -> Option<SharedString> {
        None
    }

    /// The title shown in the panel's tab.
    fn title(&self, cx: &App) -> SharedString;

    /// The element drawn before the tab's title, e.g. an avatar or icon.
    fn prefix(&self, cx: &App) -> Option<AnyElement> {
        None
    }

    /// Whether the tab shows the loading spinner.
    fn loading(&self, cx: &App) -> bool {
        false
    }

    /// Whether the panel shows the zoom and close toolbar while a split is on screen.
    fn toolbar(&self, cx: &App) -> bool {
        false
    }
}

/// An object-safe handle to a panel, so apps can hold panels by kind.
#[allow(unused_variables)]
pub trait PanelView: 'static {
    /// The panel's view.
    fn view(&self) -> AnyView;

    /// The panel's kind, read live from the panel.
    fn kind(&self, cx: &App) -> Option<SharedString> {
        None
    }

    /// The panel's title, read live from the panel.
    fn title(&self, cx: &App) -> SharedString;

    /// The panel's tab prefix, read live from the panel.
    fn prefix(&self, cx: &App) -> Option<AnyElement> {
        None
    }

    /// Whether the panel's tab shows the loading spinner.
    fn loading(&self, cx: &App) -> bool {
        false
    }

    /// Whether the panel shows the zoom and close toolbar.
    fn toolbar(&self, cx: &App) -> bool {
        false
    }

    /// Builds the panel's tab from its title, prefix, and loading state.
    fn tab(&self, key: ElementId, cx: &App) -> Tab {
        let mut tab = Tab::new(key, self.title(cx)).busy(self.loading(cx));
        if let Some(prefix) = self.prefix(cx) {
            tab = tab.lead(prefix);
        }
        tab
    }
}

impl<T: Panel> PanelView for Entity<T> {
    fn view(&self) -> AnyView {
        self.clone().into()
    }

    fn kind(&self, cx: &App) -> Option<SharedString> {
        self.read(cx).kind(cx)
    }

    fn title(&self, cx: &App) -> SharedString {
        self.read(cx).title(cx)
    }

    fn prefix(&self, cx: &App) -> Option<AnyElement> {
        self.read(cx).prefix(cx)
    }

    fn loading(&self, cx: &App) -> bool {
        self.read(cx).loading(cx)
    }

    fn toolbar(&self, cx: &App) -> bool {
        self.read(cx).toolbar(cx)
    }
}

impl<P: PanelView + ?Sized> PanelView for Arc<P> {
    fn view(&self) -> AnyView {
        (**self).view()
    }

    fn kind(&self, cx: &App) -> Option<SharedString> {
        (**self).kind(cx)
    }

    fn title(&self, cx: &App) -> SharedString {
        (**self).title(cx)
    }

    fn prefix(&self, cx: &App) -> Option<AnyElement> {
        (**self).prefix(cx)
    }

    fn loading(&self, cx: &App) -> bool {
        (**self).loading(cx)
    }

    fn toolbar(&self, cx: &App) -> bool {
        (**self).toolbar(cx)
    }
}

/// Which stack, main or split, a panel opens into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelSlot {
    /// The stack tabbed in the titlebar.
    Main,
    /// The stack in the pane beside the main panel.
    Split,
}

/// The side of the main panel a split pane opens on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitSide {
    /// Before the main panel.
    Leading,
    /// After the main panel.
    Trailing,
}

/// An ordered stack of panels with one of them shown.
#[derive(Default)]
struct Stack {
    /// The open panels in tab order.
    panels: Vec<(SharedString, Arc<dyn PanelView>)>,
    /// The shown panel; `None` once the stack is empty.
    active: Option<SharedString>,
}

impl Stack {
    fn len(&self) -> usize {
        self.panels.len()
    }

    fn is_empty(&self) -> bool {
        self.panels.is_empty()
    }

    fn position(&self, id: &SharedString) -> Option<usize> {
        self.panels.iter().position(|(open, _)| open == id)
    }

    /// The tab index of the shown panel.
    fn selected(&self) -> Option<usize> {
        self.active.as_ref().and_then(|id| self.position(id))
    }

    fn active(&self) -> Option<Arc<dyn PanelView>> {
        let id = self.active.as_ref()?;
        self.panels
            .iter()
            .find(|(open, _)| open == id)
            .map(|(_, view)| view.clone())
    }

    /// Shows `view` under `id`, opening it at the end of the stack when new.
    fn insert(&mut self, id: SharedString, view: Arc<dyn PanelView>) {
        match self.position(&id) {
            Some(index) => {
                if let Some((_, open)) = self.panels.get_mut(index) {
                    *open = view;
                }
            }
            None => self.panels.push((id.clone(), view)),
        }
        self.active = Some(id);
    }

    fn activate(&mut self, id: &SharedString) {
        self.active = Some(id.clone());
    }

    /// Closes the panel at `index`; its neighbor is shown when it was shown.
    fn remove(&mut self, index: usize) -> bool {
        if index >= self.panels.len() {
            return false;
        }
        let (id, _) = self.panels.remove(index);
        if self.active.as_ref() == Some(&id) {
            self.active = self
                .panels
                .get(index)
                .or_else(|| index.checked_sub(1).and_then(|prev| self.panels.get(prev)))
                .map(|(open, _)| open.clone());
        }
        true
    }

    fn clear(&mut self) {
        self.panels.clear();
        self.active = None;
    }
}

/// What the stack shows while no main panel is open.
struct EmptyContent {
    main: AnyView,
    /// The view beside `main`, which then opens at this width.
    split: Option<(AnyView, Pixels)>,
}

/// The panel stack drawn under the titlebar: the main stack, and the split stack beside it.
pub struct PanelStack {
    id: ElementId,
    main: Stack,
    split: Stack,
    empty: Option<EmptyContent>,
    resizable: bool,
    /// The width the on-screen split pane opened at; `None` splits the row in half.
    split_width: Option<Pixels>,
    /// The side of the main panel the on-screen split pane opened on.
    split_side: SplitSide,
    /// The main pane width the divider was last seeded at, to tell a drag from a window resize.
    opened_at: Option<Pixels>,
    /// The panel shown alone while it is zoomed.
    zoomed: Option<SharedString>,
    /// The divider position, kept only while a split is on screen.
    sizes: Entity<ResizableState>,
}

/// The layout the cached panel content lays out with.
fn content_style() -> StyleRefinement {
    StyleRefinement::default().size_full()
}

/// The island chrome every panel paints.
fn island(panel: Div, cx: &App) -> Div {
    let theme = cx.theme();
    let colors = theme.colors;

    panel
        .size_full()
        .bg(colors.background)
        .border_1()
        .border_color(colors.border)
        .rounded(theme.radius.overlay)
        .overflow_hidden()
}

impl PanelStack {
    /// Creates the panel stack with the main and split stacks empty.
    pub fn new<I>(id: I, cx: &mut Context<Self>) -> Self
    where
        I: Into<ElementId>,
    {
        Self {
            id: id.into(),
            main: Stack::default(),
            split: Stack::default(),
            empty: None,
            resizable: true,
            split_width: None,
            split_side: SplitSide::Trailing,
            opened_at: None,
            zoomed: None,
            sizes: ResizableState::new(cx),
        }
    }

    /// Sets what the stack shows while no main panel is open: `main`, beside `split`, `main` at that width.
    pub fn set_empty(
        &mut self,
        main: impl Into<AnyView>,
        split: Option<(AnyView, Pixels)>,
        cx: &mut Context<Self>,
    ) {
        self.empty = Some(EmptyContent {
            main: main.into(),
            split,
        });
        self.forget_divider(cx);
        cx.notify();
    }

    /// Sets whether the divider can be dragged; a fixed split keeps its opening width.
    pub fn set_resizable(&mut self, resizable: bool, cx: &mut Context<Self>) {
        self.resizable = resizable;
        cx.notify();
    }

    /// Shows `view` in `slot` under `id`, opening it at the end of the stack when new.
    pub fn insert(
        &mut self,
        slot: PanelSlot,
        id: impl Into<SharedString>,
        view: Arc<dyn PanelView>,
        cx: &mut Context<Self>,
    ) {
        let id = id.into();
        self.switch_main(slot, &id);
        self.stack_mut(slot).insert(id, view);
        self.forget_divider(cx);
        cx.notify();
    }

    /// Shows the panel under `id`, building it only when it is not open yet.
    pub fn open<P, F>(
        &mut self,
        slot: PanelSlot,
        id: impl Into<SharedString>,
        build: F,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) where
        P: Panel + 'static,
        F: Fn(&mut Window, &mut App) -> Entity<P> + 'static,
    {
        let id = id.into();
        if self.activate(slot, &id, cx) {
            return;
        }
        let view: Arc<dyn PanelView> = Arc::new(build(window, cx));
        self.insert(slot, id, view, cx);
    }

    /// Whether the panel under `id` is open in `slot`.
    pub fn is_open(&self, slot: PanelSlot, id: &SharedString) -> bool {
        self.stack(slot).position(id).is_some()
    }

    /// Whether the panel under `id` is the one shown in `slot`.
    pub fn is_active(&self, slot: PanelSlot, id: &SharedString) -> bool {
        self.stack(slot).active.as_ref() == Some(id)
    }

    /// Shows `view` in the split stack under `id`; a split pane not on screen yet opens at `width` on `side`.
    pub fn insert_split_at<T>(
        &mut self,
        id: T,
        width: Pixels,
        side: SplitSide,
        view: Arc<dyn PanelView>,
        cx: &mut Context<Self>,
    ) where
        T: Into<SharedString>,
    {
        if !self.is_split_shown() {
            self.split_width = Some(width);
            self.split_side = side;
        }
        self.insert(PanelSlot::Split, id, view, cx);
    }

    /// Closes the panel under `id`.
    pub fn close(&mut self, slot: PanelSlot, id: &SharedString, cx: &mut Context<Self>) {
        if let Some(index) = self.stack(slot).position(id) {
            self.close_at(slot, index, cx);
        }
    }

    /// Closes every panel in both stacks.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.main.clear();
        self.split.clear();
        self.forget_divider(cx);
        cx.notify();
    }

    /// The main stack's tab strip for the titlebar; `None` while the stack is empty.
    pub fn main_tabs(stack: Entity<Self>, cx: &App) -> Option<Tabs> {
        let (id, panels, selected) = {
            let this = stack.read(cx);
            if this.main.is_empty() {
                return None;
            }
            (
                this.id.clone(),
                this.main.panels.clone(),
                this.main.selected().unwrap_or(usize::MAX),
            )
        };
        let change = stack.downgrade();
        let close = change.clone();

        Some(
            Tabs::new((id, "main-tabs"))
                .variant(TabsVariant::Titlebar)
                .overflow_scroll(true)
                .selected(selected)
                .tabs(move |_, cx| {
                    panels
                        .iter()
                        .map(|(id, panel)| panel.tab(id.clone().into(), cx).closeable(true))
                        .collect()
                })
                .on_change(move |index, _, cx| {
                    change
                        .update(cx, |stack, cx| {
                            stack.activate_at(PanelSlot::Main, index, cx)
                        })
                        .log_err();
                })
                .on_close(move |index, _, cx| {
                    close
                        .update(cx, |stack, cx| stack.close_at(PanelSlot::Main, index, cx))
                        .log_err();
                }),
        )
    }

    /// Shows the panel under `id`; `false` when it is not open.
    fn activate(&mut self, slot: PanelSlot, id: &SharedString, cx: &mut Context<Self>) -> bool {
        if self.stack(slot).position(id).is_none() {
            return false;
        }
        self.switch_main(slot, id);
        self.stack_mut(slot).activate(id);
        self.forget_divider(cx);
        cx.notify();
        true
    }

    /// Shows the panel at `index`.
    fn activate_at(&mut self, slot: PanelSlot, index: usize, cx: &mut Context<Self>) {
        let id = self.stack(slot).panels.get(index).map(|(id, _)| id.clone());
        if let Some(id) = id {
            self.activate(slot, &id, cx);
        }
    }

    /// Shows the slot's shown panel alone, or the split again when it already is.
    fn toggle_zoom(&mut self, slot: PanelSlot, cx: &mut Context<Self>) {
        let active = self.stack(slot).active.clone();
        self.zoomed = if self.is_split_shown() && self.zoomed != active {
            active
        } else {
            None
        };
        cx.notify();
    }

    fn close_active(&mut self, slot: PanelSlot, cx: &mut Context<Self>) {
        if let Some(index) = self.stack(slot).selected() {
            self.close_at(slot, index, cx);
        }
    }

    fn close_at(&mut self, slot: PanelSlot, index: usize, cx: &mut Context<Self>) {
        let was_shown = self.stack(slot).selected() == Some(index);
        if !self.stack_mut(slot).remove(index) {
            return;
        }
        if slot == PanelSlot::Main && was_shown {
            self.split.clear();
        }
        self.forget_divider(cx);
        cx.notify();
    }

    /// Closes every split panel when `id` is another main panel than the shown one.
    fn switch_main(&mut self, slot: PanelSlot, id: &SharedString) {
        if slot == PanelSlot::Main && self.main.active.as_ref() != Some(id) {
            self.split.clear();
        }
    }

    fn stack(&self, slot: PanelSlot) -> &Stack {
        match slot {
            PanelSlot::Main => &self.main,
            PanelSlot::Split => &self.split,
        }
    }

    fn stack_mut(&mut self, slot: PanelSlot) -> &mut Stack {
        match slot {
            PanelSlot::Main => &mut self.main,
            PanelSlot::Split => &mut self.split,
        }
    }

    /// Whether a split is on screen: the split stack beside a main panel, or the empty state's.
    fn is_split_shown(&self) -> bool {
        if self.main.active.is_some() {
            return !self.split.is_empty();
        }
        self.empty
            .as_ref()
            .is_some_and(|empty| empty.split.is_some())
    }

    /// Drops the divider position, width, side, and zoom while no split is on screen, so the next split opens at its start width.
    fn forget_divider(&mut self, cx: &mut Context<Self>) {
        if self.is_split_shown() {
            return;
        }

        self.split_width = None;
        self.split_side = SplitSide::Trailing;
        self.opened_at = None;
        self.zoomed = None;

        if !self.sizes.read(cx).sizes().is_empty() {
            self.sizes.update(cx, |state, cx| state.set_sizes([], cx));
        }
    }

    /// The split pane's content: the split stack's tabs over its shown panel.
    fn split_pane(&self, cx: &mut Context<Self>) -> AnyElement {
        let panels: Vec<(SharedString, Arc<dyn PanelView>)> = self.split.panels.clone();
        let selected = self.split.selected().unwrap_or(usize::MAX);
        let change = cx.weak_entity();
        let close = change.clone();

        v_flex()
            .size_full()
            .gap_2()
            .when(self.split.len() > 1, |pane| {
                pane.child(
                    Tabs::new((self.id.clone(), "split-tabs"))
                        .variant(TabsVariant::Pill)
                        .overflow_scroll(true)
                        .selected(selected)
                        .tabs(move |_, cx| {
                            panels
                                .iter()
                                .map(|(id, panel)| panel.tab(id.clone().into(), cx).closeable(true))
                                .collect()
                        })
                        .on_change(move |index, _, cx| {
                            change
                                .update(cx, |stack, cx| {
                                    stack.activate_at(PanelSlot::Split, index, cx)
                                })
                                .log_err();
                        })
                        .on_close(move |index, _, cx| {
                            close
                                .update(cx, |stack, cx| stack.close_at(PanelSlot::Split, index, cx))
                                .log_err();
                        }),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .children(self.split.active().map(|panel| panel.view())),
            )
            .into_any_element()
    }

    /// The zoom and close buttons over a pane's top right corner, when its shown panel asks for them.
    fn toolbar(&self, slot: PanelSlot, cx: &Context<Self>) -> Option<AnyElement> {
        if !self.stack(slot).active()?.toolbar(cx) {
            return None;
        }

        let zoomed = self.stack(slot).active == self.zoomed;
        let enabled = self.is_split_shown();

        let (zoom_id, close_id, pane) = match slot {
            PanelSlot::Main => ("zoom-main", "close-main", "main"),
            PanelSlot::Split => ("zoom-split", "close-split", "split"),
        };

        let change = cx.weak_entity();
        let close = change.clone();
        let zoom_prefix = self.id.clone();
        let close_prefix = zoom_prefix.clone();

        Some(
            h_flex()
                .absolute()
                .top_2()
                .right_2()
                .gap_1()
                .child(
                    Button::new((self.id.clone(), zoom_id))
                        .icon(Icon::named(if zoomed {
                            IconRole::Minimize
                        } else {
                            IconRole::Maximize
                        }))
                        .variant(ButtonVariant::Ghost)
                        .xsmall()
                        .fully_rounded(true)
                        .tab_stop(false)
                        .disabled(!enabled)
                        .tooltip(Tooltip::text(if zoomed {
                            "Unzoom panel"
                        } else {
                            "Zoom panel"
                        }))
                        .debug_selector(move || format!("{zoom_prefix}-zoom-{pane}"))
                        .on_click(move |_, _, cx| {
                            change
                                .update(cx, |stack, cx| stack.toggle_zoom(slot, cx))
                                .log_err();
                        }),
                )
                .child(
                    Button::new((self.id.clone(), close_id))
                        .icon(IconRole::Close)
                        .variant(ButtonVariant::Ghost)
                        .xsmall()
                        .fully_rounded(true)
                        .tab_stop(false)
                        .disabled(!enabled)
                        .tooltip(Tooltip::text("Close panel"))
                        .debug_selector(move || format!("{close_prefix}-close-{pane}"))
                        .on_click(move |_, _, cx| {
                            close
                                .update(cx, |stack, cx| stack.close_active(slot, cx))
                                .log_err();
                        }),
                )
                .into_any_element(),
        )
    }

    /// The island around a pane's content, with its toolbar over the top right corner.
    fn pane(&self, content: AnyElement, toolbar: Option<AnyElement>, cx: &App) -> Div {
        island(
            div()
                .size_full()
                .relative()
                .child(content)
                .children(toolbar),
            cx,
        )
    }
}

impl Render for PanelStack {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let gutter = RESIZE_HANDLE_WIDTH;
        let extent = window.viewport_size().width - gutter * 2.0;
        let cached = |view: AnyView| view.cached(content_style()).into_any_element();

        // The main pane's content, the split pane's when one is on screen, and how the first pane opens.
        let (main, split, width, leading) = match self.main.active() {
            Some(panel) => {
                let leading = self.split_side == SplitSide::Leading;
                // The pane opened at a width is the sized one; the other fills what is left.
                let width = if leading {
                    self.split_width
                } else {
                    self.split_width
                        .map(|split_width| extent - gutter - split_width)
                };
                (
                    cached(panel.view()),
                    (!self.split.is_empty()).then(|| self.split_pane(cx)),
                    width,
                    leading,
                )
            }
            None => match &self.empty {
                Some(empty) => {
                    let (split, width) = match &empty.split {
                        Some((view, width)) => (Some(cached(view.clone())), Some(*width)),
                        None => (None, None),
                    };
                    (cached(empty.main.clone()), split, width, false)
                }
                None => (div().into_any_element(), None, None, false),
            },
        };

        // The pane shown alone while its panel is zoomed.
        let zoomed = self.zoomed.as_ref().and_then(|zoomed| {
            [PanelSlot::Main, PanelSlot::Split]
                .into_iter()
                .find(|slot| split.is_some() && self.stack(*slot).active.as_ref() == Some(zoomed))
        });

        div().flex_1().min_w_0().h_full().p_2().pt_1().map(|this| {
            match split {
                Some(split) => match zoomed {
                    // A zoomed panel fills the row; its toolbar is the way back.
                    Some(slot) => {
                        let content = match slot {
                            PanelSlot::Main => main,
                            PanelSlot::Split => split,
                        };
                        this.child(div().w_full().h_full().child(self.pane(
                            content,
                            self.toolbar(slot, cx),
                            cx,
                        )))
                    }
                    None => {
                        let main = self.pane(main, self.toolbar(PanelSlot::Main, cx), cx);
                        let split = self.pane(split, self.toolbar(PanelSlot::Split, cx), cx);
                        let (first, last) = if leading {
                            (split, main)
                        } else {
                            (main, split)
                        };
                        let first_width =
                            width.unwrap_or((extent - gutter) / 2.0).max(PANE_MIN_SIZE);

                        if !self.resizable {
                            this.child(
                                div()
                                    .w_full()
                                    .h_full()
                                    .flex()
                                    .flex_row()
                                    .child(div().flex_none().w(first_width).h_full().child(first))
                                    .child(div().flex_none().w(gutter))
                                    .child(div().flex_1().min_w_0().h_full().child(last)),
                            )
                        } else {
                            // A split opened at a width keeps it across window resizes until the divider is dragged.
                            let current = self.sizes.read(cx).sizes().first().copied();
                            let keeps_width =
                                self.split_width.is_some() && current == self.opened_at;

                            if current.is_none()
                                || (keeps_width && self.opened_at != Some(first_width))
                            {
                                self.sizes.update(cx, |state, cx| {
                                    state.set_pane_width(0, first_width, cx);
                                });
                                self.opened_at = Some(first_width);
                            }

                            this.child(
                                div().w_full().h_full().child(
                                    Resizable::new((self.id.clone(), "split"), self.sizes.clone())
                                        .axis(Axis::Horizontal)
                                        .pane(first)
                                        .pane(last)
                                        .h_full(),
                                ),
                            )
                        }
                    }
                },
                None => this.child(div().w_full().h_full().child(self.pane(
                    main,
                    self.toolbar(PanelSlot::Main, cx),
                    cx,
                ))),
            }
        })
    }
}
