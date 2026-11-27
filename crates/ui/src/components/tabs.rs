//! Renders a strip of [`Tab`]s with selection, close controls, drag reordering, and overflow.
//!
//! # Examples
//!
//! ```no_run
//! use ui::tabs::{Tab, Tabs};
//!
//! let editors = Tabs::new("editors")
//!     .tabs(|_, _| {
//!         vec![
//!             Tab::new("readme", "README").closeable(true),
//!             Tab::new("changelog", "CHANGELOG").closeable(true),
//!         ]
//!     })
//!     .on_change(|_index, _window, _cx| println!("tab changed"));
//! ```

use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, AppContext as _, Bounds, ClickEvent, Context, Div, ElementId, Entity,
    FocusHandle, Global, Hsla, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton,
    ParentElement as _, Pixels, Point, Render, RenderOnce, Role, SharedString, Stateful,
    StatefulInteractiveElement as _, StyleRefinement, Styled, TextOverflow, Window, canvas, div,
    linear_color_stop, linear_gradient, px, rems, transparent_black,
};

use crate::components::button::{Button, ButtonVariant};
use crate::components::common::{
    ElementIdExt as _, SelectableRows as _, Sizable as _, StyledExt as _, v_flex,
};
use crate::components::popover::Popover;
use crate::components::scroll_area::{ScrollArea, ScrollAxes};
use crate::components::spinner::Spinner;
use crate::components::tooltip::Tooltip;
use crate::icons::IconRole;
use crate::theme::{ActiveTheme as _, POPUP_MAX_HEIGHT, TAB_MAX_WIDTH, TAB_MIN_WIDTH};

/// Reports the next selected tab by pointer and keys.
type ChangeHandler = Rc<dyn Fn(usize, &mut Window, &mut App)>;
/// Reports the tab's index by its close control.
type CloseHandler = Rc<dyn Fn(usize, &mut Window, &mut App)>;
/// Reports a tab moved from one index to another by a drag.
type ReorderHandler = Rc<dyn Fn(usize, usize, &mut Window, &mut App)>;
/// Decorates the rendered tab, e.g. to attach a drag payload.
type TabWrap = Box<dyn FnOnce(Stateful<Div>) -> AnyElement>;
/// The tab builder, run during render.
type TabsFn = Box<dyn Fn(&mut Window, &mut App) -> Vec<Tab>>;
/// The open-state reporter of the overflow popover.
type OpenHandler = Rc<dyn Fn(bool, &mut Window, &mut App)>;
/// Identifies one strip's measurements across windows.
type MeasureKey = (u64, ElementId);
/// Shared per-frame state that lets tab canvases read the strip's bounds.
type RootBoundsCell = Rc<RefCell<Option<Bounds<Pixels>>>>;

/// Displacement before a tab drag locks to an axis: horizontal reorders,
/// vertical cancels the reorder, like a browser tab strip.
const TAB_DRAG_AXIS_LOCK: Pixels = px(6.0);

/// One slot of a strip: a lone tab, or same-kind tabs collapsed into a group chip.
enum StripItem {
    /// A tab outside any two-member kind group, at its original index.
    Single(usize, Tab),
    /// Tabs of one kind collapsed into a chip, at the first member's index.
    Group {
        kind: SharedString,
        /// The collapsed tabs with their original indices.
        members: Vec<(usize, Tab)>,
    },
}

impl StripItem {
    /// The key the strip's geometry records the item under.
    fn measure_key(&self, tabs_id: &ElementId) -> ElementId {
        match self {
            StripItem::Single(_, tab) => tab.key.clone(),
            StripItem::Group { kind, .. } => Tabs::group_key(tabs_id, kind),
        }
    }
}

/// The visual language of a [`Tabs`] strip.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TabsVariant {
    /// A bottom indicator line on the active tab.
    #[default]
    Underline,
    /// Capsule tabs with an elevated active background.
    Pill,
    /// Floating capsule tabs sized for a window titlebar.
    Titlebar,
}

impl TabsVariant {
    /// Whether the variant draws capsule-shaped tabs.
    fn is_capsule(self) -> bool {
        matches!(self, Self::Pill | Self::Titlebar)
    }

    /// The label color shared by strip tabs and group chips.
    fn label_color(self, active: bool, disabled: bool, cx: &App) -> Hsla {
        let theme = cx.theme();
        if disabled {
            return theme.colors.muted_foreground;
        }
        match self {
            Self::Underline => {
                if active {
                    theme.ramps.primary.base
                } else {
                    theme.colors.foreground
                }
            }
            Self::Pill => theme.colors.foreground,
            Self::Titlebar => {
                if active {
                    theme.colors.foreground
                } else {
                    theme.colors.muted_foreground
                }
            }
        }
    }

    /// The variant treatment shared by strip tabs and group chips.
    fn chrome<R>(self, row: R, active: bool, cx: &App) -> R
    where
        R: Styled + IntoElement,
    {
        let theme = cx.theme();
        match self {
            Self::Underline => row
                .px_2()
                .py_1()
                .rounded_tl(theme.radius.sm)
                .rounded_tr(theme.radius.sm)
                .border_b_2()
                .border_color(transparent_black())
                .when(active, |row| row.border_color(theme.ramps.primary.base)),
            Self::Pill | Self::Titlebar => row
                .h_8()
                .px_2()
                .pr_0p5()
                .rounded(match self {
                    Self::Titlebar => theme.radius.full,
                    _ => theme.radius.control,
                })
                .when(self == Self::Titlebar, |row| row.bg(theme.colors.muted))
                .when(active, |row| {
                    row.bg(theme.colors.background)
                        .shadow(theme.elevations.control.shadows().to_vec())
                }),
        }
    }
}

/// One inline tab's geometry relative to the strip's left edge.
#[derive(Clone, Copy, Default)]
struct TabMeasure {
    left: Pixels,
    width: Pixels,
}

/// A strip's geometry captured the previous frame.
#[derive(Default)]
struct StripMeasure {
    container_width: Pixels,
    trigger_width: Option<Pixels>,
    tabs: HashMap<ElementId, TabMeasure>,
}

/// The capture context handed to inline tabs, only in popover mode.
#[derive(Clone)]
struct MeasureFrame {
    key: MeasureKey,
    root_bounds: RootBoundsCell,
}

/// Measurements read a frame late, so strips render one frame behind a resize.
#[derive(Default)]
struct TabsMeasurements {
    strips: HashMap<MeasureKey, StripMeasure>,
}

impl Global for TabsMeasurements {}

impl TabsMeasurements {
    fn record_container(&mut self, key: &MeasureKey, width: Pixels) {
        self.strips.entry(key.clone()).or_default().container_width = width;
    }

    fn record_trigger(&mut self, key: &MeasureKey, width: Pixels) {
        self.strips.entry(key.clone()).or_default().trigger_width = Some(width);
    }

    fn record_tab(&mut self, key: &MeasureKey, tab: ElementId, measure: TabMeasure) {
        self.strips
            .entry(key.clone())
            .or_default()
            .tabs
            .insert(tab, measure);
    }

    /// Drops measurements for tabs no longer rendered in the strip.
    fn retain_tabs(&mut self, key: &MeasureKey, rendered: &[ElementId]) {
        if let Some(strip) = self.strips.get_mut(key) {
            strip.tabs.retain(|tab, _| rendered.contains(tab));
        }
    }
}

/// Everything a tab needs to draw and interact, shared across a strip's slots.
#[derive(Clone)]
struct StripCtx {
    id: ElementId,
    selected: usize,
    variant: TabsVariant,
    on_change: Option<ChangeHandler>,
    on_close: Option<CloseHandler>,
    on_reorder: Option<ReorderHandler>,
    focus: FocusHandle,
    measure: Option<MeasureFrame>,
    /// Whether tabs drag onto each other to reorder; requires `on_reorder`.
    draggable: bool,
    /// The tab drag gesture's live state, for the drop indicator.
    drop_at: Entity<TabDrop>,
}

impl StripCtx {
    /// The context for tabs drawn inside a popover: no measurement, no dragging.
    fn for_popover(&self) -> Self {
        Self {
            measure: None,
            draggable: false,
            ..self.clone()
        }
    }
}

/// One tab drag gesture's live state.
#[derive(Clone, Copy, Default, PartialEq)]
struct TabDrop {
    /// The tab the drag currently hovers, for the drop indicator.
    onto: Option<usize>,
    /// Where the drag's first move landed, anchoring the axis lock.
    origin: Option<Point<Pixels>>,
    /// Whether the gesture locked as a horizontal (`Some(true)`) or vertical
    /// (`Some(false)`) drag; `None` until it moved [`TAB_DRAG_AXIS_LOCK`].
    horizontal: Option<bool>,
}

impl TabDrop {
    /// Whether the gesture locked as a vertical drag, canceling the reorder.
    fn canceled(&self) -> bool {
        self.horizontal == Some(false)
    }

    /// The index the tab at `from` occupies once dropped before the tab at `onto`.
    fn reorder_target(from: usize, onto: usize) -> Option<usize> {
        match from.cmp(&onto) {
            Ordering::Equal => None,
            Ordering::Less => Some(onto - 1),
            Ordering::Greater => Some(onto),
        }
    }
}

/// The payload dragged when reordering a tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabDrag {
    /// The dragged tab's index into the `tabs` vec.
    pub index: usize,
}

/// The floating pill that follows the cursor while a tab is dragged.
struct TabDragPreview {
    label: SharedString,
}

impl Render for TabDragPreview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .px_2()
            .py_1()
            .rounded(theme.radius.control)
            .border_1()
            .border_color(theme.colors.border)
            .bg(theme.colors.background)
            .text_sm()
            .text_color(theme.colors.foreground)
            .shadow(theme.elevations.control.shadows().to_vec())
            .child(self.label.clone())
    }
}

/// One tab of a [`Tabs`] strip.
#[must_use]
pub struct Tab {
    /// The stable identity for element state; must survive reordering.
    key: ElementId,
    label: SharedString,
    lead: Option<AnyElement>,
    busy: bool,
    disabled: bool,
    closeable: bool,
    pinned: bool,
    kind: Option<SharedString>,
    wrap: Option<TabWrap>,
}

impl Tab {
    /// Creates a tab shown as `label`, identified by `key`.
    pub fn new<K, L>(key: K, label: L) -> Self
    where
        K: Into<ElementId>,
        L: Into<SharedString>,
    {
        Self {
            key: key.into(),
            label: label.into(),
            lead: None,
            busy: false,
            disabled: false,
            closeable: false,
            pinned: false,
            kind: None,
            wrap: None,
        }
    }

    /// Sets the leading element, e.g. an icon or an avatar.
    pub fn lead<E>(mut self, lead: E) -> Self
    where
        E: IntoElement + 'static,
    {
        self.lead = Some(lead.into_any_element());
        self
    }

    /// Shows the loading slot.
    pub fn busy(mut self, busy: bool) -> Self {
        self.busy = busy;
        self
    }

    /// Renders the tab dimmed; clicks and keyboard skip it.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Shows a close control that reports the tab through [`Tabs::on_close`].
    pub fn closeable(mut self, closeable: bool) -> Self {
        self.closeable = closeable;
        self
    }

    /// Draws the tab with the active treatment at all times, selected or not.
    pub fn pinned(mut self, pinned: bool) -> Self {
        self.pinned = pinned;
        self
    }

    /// Sets the kind the tab belongs to; same-kind tabs collapse into one group.
    pub fn kind<K>(mut self, kind: K) -> Self
    where
        K: Into<SharedString>,
    {
        self.kind = Some(kind.into());
        self
    }

    /// Hands the rendered tab through `wrap`, e.g. to attach a drag payload.
    pub fn wrap<F>(mut self, wrap: F) -> Self
    where
        F: FnOnce(Stateful<Div>) -> AnyElement + 'static,
    {
        self.wrap = Some(Box::new(wrap));
        self
    }

    fn render_tab(mut self, ctx: &StripCtx, cx: &App, index: usize) -> AnyElement {
        let wrap = self.wrap.take();
        let element = self.tab_element(ctx, cx, index);
        match wrap {
            Some(wrap) => wrap(element),
            None => element.into_any_element(),
        }
    }

    fn tab_element(mut self, ctx: &StripCtx, cx: &App, index: usize) -> Stateful<Div> {
        let selected = index == ctx.selected;
        // A pinned tab always wears the active treatment, selected or not.
        let active = selected || self.pinned;

        let theme = cx.theme();
        let label_color = ctx.variant.label_color(active, self.disabled, cx);

        let titlebar = ctx.variant == TabsVariant::Titlebar;
        let group = SharedString::from(format!("{}-tab-{}", ctx.id, self.key));
        let close = ctx
            .on_close
            .as_ref()
            .filter(|_| self.closeable && !self.disabled)
            .map(|on_close| self.render_tab_close(&ctx.id, index, on_close));

        div()
            .child(
                div()
                    .flex_grow(1.0)
                    .when(titlebar, |label| {
                        let fade = if active {
                            theme.colors.background
                        } else {
                            theme.colors.muted
                        };

                        let hover_fade = if active {
                            theme.colors.background
                        } else {
                            theme.colors.muted_hover
                        };

                        let fade_width = rems(0.75);
                        let hover_fade_width = rems(1.5);

                        label
                            .min_w_0()
                            .relative()
                            .overflow_hidden()
                            .child(
                                div()
                                    .w_full()
                                    .whitespace_nowrap()
                                    .text_overflow(TextOverflow::Truncate(SharedString::default()))
                                    .child(self.label.clone()),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .right_0()
                                    .bottom_0()
                                    .w(fade_width)
                                    .bg(linear_gradient(
                                        90.,
                                        linear_color_stop(fade.opacity(0.), 0.),
                                        linear_color_stop(fade, 1.),
                                    )),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .right_0()
                                    .bottom_0()
                                    .w(hover_fade_width)
                                    .bg(linear_gradient(
                                        90.,
                                        linear_color_stop(hover_fade.opacity(0.), 0.),
                                        linear_color_stop(hover_fade, 1.),
                                    ))
                                    .invisible()
                                    .group_hover(group.clone(), |style| style.visible()),
                            )
                    })
                    .when(!titlebar, |label| {
                        label
                            .min_w_0()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(self.label.clone())
                    }),
            )
            .id((self.key.clone(), "tab"))
            .debug_selector({
                let tabs_id = ctx.id.clone();
                let label = self.label.clone();
                move || format!("{tabs_id}-tab-{}", label)
            })
            .flex()
            .items_center()
            .gap_2()
            .role(Role::Tab)
            .aria_selected(selected)
            .text_color(label_color)
            .when(titlebar, |strip_tab| strip_tab.group(group.clone()))
            .when(!self.disabled && !active, |strip_tab| {
                strip_tab.hover(|strip_tab| {
                    strip_tab.bg(match ctx.variant {
                        TabsVariant::Titlebar => theme.colors.muted_hover,
                        _ => theme.colors.muted,
                    })
                })
            })
            .map(|strip_tab| {
                ctx.variant
                    .chrome(strip_tab, active, cx)
                    .max_w(TAB_MAX_WIDTH)
                    .when(titlebar, |strip_tab| strip_tab.min_w(TAB_MIN_WIDTH))
            })
            .when_some(self.lead.take(), |strip_tab, lead| strip_tab.child(lead))
            .when(ctx.variant == TabsVariant::Pill, |strip_tab| {
                strip_tab.child(div().flex_none().w_4())
            })
            .when(self.busy && !self.disabled, |strip_tab| {
                strip_tab.child(
                    Spinner::new((self.key.clone(), "tab-busy"))
                        .small()
                        .accessibility_label("Loading"),
                )
            })
            .when_some(close, |strip_tab, close| {
                if titlebar {
                    strip_tab.child(
                        div()
                            .flex_none()
                            .invisible()
                            .group_hover(group, |style| style.visible())
                            .child(close),
                    )
                } else {
                    strip_tab.child(close)
                }
            })
            .when(
                {
                    let drop = ctx.drop_at.read(cx);
                    drop.onto == Some(index) && !drop.canceled()
                },
                |strip_tab| {
                    strip_tab.child(
                        div()
                            .absolute()
                            .left_0()
                            .top_0p5()
                            .bottom_0p5()
                            .w_0p5()
                            .rounded(theme.radius.sm)
                            .bg(theme.ramps.primary.base),
                    )
                },
            )
            .when_some(ctx.measure.clone(), |strip_tab, measure| {
                let key = measure.key.clone();
                let root_bounds = measure.root_bounds.clone();
                let tab_key = self.key.clone();
                strip_tab.child(
                    canvas(
                        move |bounds, _, cx| {
                            let root = root_bounds.borrow();
                            if let Some(root) = root.as_ref() {
                                cx.default_global::<TabsMeasurements>().record_tab(
                                    &key,
                                    tab_key,
                                    TabMeasure {
                                        left: bounds.origin.x - root.origin.x,
                                        width: bounds.size.width,
                                    },
                                );
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0(),
                )
            })
            .when(!self.disabled, |strip_tab| {
                strip_tab.cursor_pointer().on_click({
                    let selected = ctx.selected;
                    let on_change = ctx.on_change.clone();
                    let focus = ctx.focus.clone();
                    move |_, window, cx| {
                        window.focus(&focus, cx);
                        if index != selected
                            && let Some(on_change) = &on_change
                        {
                            on_change(index, window, cx);
                        }
                    }
                })
            })
            .when(ctx.draggable && !self.disabled, |strip_tab| {
                strip_tab
                    .on_mouse_down(MouseButton::Left, {
                        let drop_at = ctx.drop_at.clone();
                        move |event, _, cx| {
                            cx.stop_propagation();
                            drop_at.update(cx, |drop, cx| {
                                let fresh = TabDrop {
                                    origin: Some(event.position),
                                    ..TabDrop::default()
                                };
                                if *drop != fresh {
                                    *drop = fresh;
                                    cx.notify();
                                }
                            });
                        }
                    })
                    .on_drag(TabDrag { index }, {
                        let label = self.label.clone();
                        move |_, _, _, cx| {
                            cx.new(|_| TabDragPreview {
                                label: label.clone(),
                            })
                        }
                    })
                    .drag_over::<TabDrag>({
                        let drop_at = ctx.drop_at.clone();
                        move |style, drag, _, cx| {
                            let drop = *drop_at.read(cx);
                            let hovered = drag.index != index && !drop.canceled();
                            drop_at.update(cx, |drop, cx| match (hovered, drop.onto) {
                                (true, current) if current != Some(index) => {
                                    drop.onto = Some(index);
                                    cx.notify();
                                }
                                (false, Some(_)) => {
                                    drop.onto = None;
                                    cx.notify();
                                }
                                _ => {}
                            });
                            style
                        }
                    })
                    .on_drop::<TabDrag>({
                        let on_reorder = ctx.on_reorder.clone();
                        let drop_at = ctx.drop_at.clone();
                        move |drag, window, cx| {
                            let canceled = drop_at.read(cx).canceled();
                            drop_at.update(cx, |drop, cx| {
                                if drop.onto.is_some() {
                                    drop.onto = None;
                                    cx.notify();
                                }
                            });
                            let target = (!canceled)
                                .then(|| TabDrop::reorder_target(drag.index, index))
                                .flatten();
                            if let (Some(to), Some(on_reorder)) = (target, on_reorder.as_ref()) {
                                on_reorder(drag.index, to, window, cx);
                            }
                        }
                    })
            })
    }

    fn render_tab_close(
        &self,
        tabs_id: &ElementId,
        index: usize,
        on_close: &CloseHandler,
    ) -> Button {
        Button::new((self.key.clone(), "tab-close"))
            .icon(IconRole::Close)
            .variant(ButtonVariant::Ghost)
            .xsmall()
            .fully_rounded(true)
            .tab_stop(false)
            .debug_selector({
                let tabs_id = tabs_id.clone();
                let label = self.label.clone();
                move || format!("{tabs_id}-close-{}", label)
            })
            .tooltip(Tooltip::text("Close"))
            .accessibility_label(SharedString::from(format!("Close {}", self.label)))
            .on_click({
                let on_close = on_close.clone();
                move |_, window, cx| {
                    cx.stop_propagation();
                    on_close(index, window, cx);
                }
            })
    }
}

// Tab is a data builder, not an element, adopt the combinators explicitly.
impl FluentBuilder for Tab {}

/// A tab strip with selection, close, and reorder handling.
#[derive(IntoElement)]
#[must_use]
pub struct Tabs {
    id: ElementId,
    selected: usize,
    variant: TabsVariant,
    overflow_scroll: bool,
    overflow_popover: bool,
    overflow_open: bool,
    draggable: bool,
    a11y_label: Option<SharedString>,
    on_change: Option<ChangeHandler>,
    on_close: Option<CloseHandler>,
    on_reorder: Option<ReorderHandler>,
    on_overflow_open: Option<OpenHandler>,
    tabs: Option<TabsFn>,
    style: StyleRefinement,
}

impl Tabs {
    /// Creates a strip selecting the first tab.
    pub fn new<I>(id: I) -> Self
    where
        I: Into<ElementId>,
    {
        Self {
            id: id.into(),
            selected: 0,
            variant: TabsVariant::default(),
            overflow_scroll: false,
            overflow_popover: false,
            overflow_open: false,
            draggable: false,
            a11y_label: None,
            on_change: None,
            on_close: None,
            on_reorder: None,
            on_overflow_open: None,
            tabs: None,
            style: StyleRefinement::default(),
        }
    }

    /// Sets the current selection as an index into the `tabs` vec, not a tab key.
    pub fn selected(mut self, selected: usize) -> Self {
        self.selected = selected;
        self
    }

    /// Sets the strip's visual language.
    pub fn variant(mut self, variant: TabsVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Scrolls the strip horizontally when the tabs overflow.
    pub fn overflow_scroll(mut self, overflow_scroll: bool) -> Self {
        self.overflow_scroll = overflow_scroll;
        self
    }

    /// Shows a popover of the tabs that do not fit, via a trigger at the strip's end.
    pub fn overflow_popover(mut self, overflow_popover: bool) -> Self {
        self.overflow_popover = overflow_popover;
        self
    }

    /// The overflow popover's current open state; read only in [`Self::overflow_popover`] mode.
    pub fn overflow_open(mut self, overflow_open: bool) -> Self {
        self.overflow_open = overflow_open;
        self
    }

    /// Lets tabs be dragged onto each other to reorder them.
    pub fn draggable(mut self, draggable: bool) -> Self {
        self.draggable = draggable;
        self
    }

    /// Reports the overflow popover's next open state, from its trigger, dismissals, and selection.
    pub fn on_overflow_open<F>(mut self, handler: F) -> Self
    where
        F: Fn(bool, &mut Window, &mut App) + 'static,
    {
        self.on_overflow_open = Some(Rc::new(handler));
        self
    }

    /// Reports the next selected tab as an index into the `tabs` vec, not a tab key; indices
    /// shift on reorder.
    pub fn on_change<F>(mut self, handler: F) -> Self
    where
        F: Fn(usize, &mut Window, &mut App) + 'static,
    {
        self.on_change = Some(Rc::new(handler));
        self
    }

    /// Reports a closed tab as an index into the `tabs` vec, not a tab key; indices shift on
    /// reorder.
    pub fn on_close<F>(mut self, handler: F) -> Self
    where
        F: Fn(usize, &mut Window, &mut App) + 'static,
    {
        self.on_close = Some(Rc::new(handler));
        self
    }

    /// Reports a tab moved by a drag.
    pub fn on_reorder<F>(mut self, handler: F) -> Self
    where
        F: Fn(usize, usize, &mut Window, &mut App) + 'static,
    {
        self.on_reorder = Some(Rc::new(handler));
        self
    }

    /// Sets the tabs; the closure runs during render and must not mutate entities or notify.
    pub fn tabs<F>(mut self, tabs: F) -> Self
    where
        F: Fn(&mut Window, &mut App) -> Vec<Tab> + 'static,
    {
        self.tabs = Some(Box::new(tabs));
        self
    }

    /// Overrides the accessibility label, which defaults to the id.
    pub fn accessibility_label<L>(mut self, label: L) -> Self
    where
        L: Into<SharedString>,
    {
        self.a11y_label = Some(label.into());
        self
    }

    /// The number of items that fit before the popover trigger, from last frame's geometry.
    fn inline_count(tabs_id: &ElementId, items: &[StripItem], measure: &StripMeasure) -> usize {
        let limit = measure.container_width - measure.trigger_width.unwrap_or(Pixels::ZERO);
        let mut count = 0;
        for item in items {
            let Some(entry) = measure.tabs.get(&item.measure_key(tabs_id)) else {
                return items.len();
            };
            if entry.left + entry.width > limit {
                break;
            }
            count += 1;
        }
        count
    }

    fn render_overflow_popover(
        ctx: &StripCtx,
        overflow_items: Vec<StripItem>,
        open: bool,
        on_overflow_open: Option<OpenHandler>,
    ) -> Popover {
        let id = ctx.id.clone();
        let trigger_id = id.clone();
        let on_change = ctx.on_change.clone();

        let row_change: Option<ChangeHandler> = match (on_change, on_overflow_open.clone()) {
            (Some(on_change), Some(dismiss)) => Some(Rc::new(move |index, window, cx| {
                on_change(index, window, cx);
                dismiss(false, window, cx);
            })),
            (None, Some(dismiss)) => Some(Rc::new(move |_, window, cx| {
                dismiss(false, window, cx);
            })),
            (on_change, None) => on_change,
        };

        let row_ctx = StripCtx {
            on_change: row_change,
            ..ctx.for_popover()
        };

        Popover::new((id.clone(), "tabs-overflow"))
            .open(open)
            .trigger({
                let on_overflow_open = on_overflow_open.clone();
                move |_, _| {
                    Button::new((trigger_id.clone(), "tabs-overflow-trigger"))
                        .icon(IconRole::More)
                        .variant(ButtonVariant::Ghost)
                        .small()
                        .accessibility_label("More tabs")
                        .on_click(move |event, window, cx| {
                            // Pointer clicks toggle inside Popover; only the keyboard path lands here.
                            if let ClickEvent::Keyboard(_) = event
                                && let Some(on_overflow_open) = on_overflow_open.as_ref()
                            {
                                on_overflow_open(!open, window, cx);
                            }
                        })
                        .into_any_element()
                }
            })
            .on_change(move |next, window, cx| {
                if let Some(on_overflow_open) = on_overflow_open.as_ref() {
                    on_overflow_open(next, window, cx);
                }
            })
            .content(move |_, cx| {
                ScrollArea::new((id.clone(), "tabs-overflow-list"))
                    .axes(ScrollAxes::Vertical)
                    .max_h(POPUP_MAX_HEIGHT)
                    .child(
                        v_flex()
                            .gap_0p5()
                            .p_1()
                            .children(overflow_items.into_iter().flat_map(|item| {
                                match item {
                                    StripItem::Single(index, tab) => {
                                        vec![tab.render_tab(&row_ctx, cx, index)]
                                    }
                                    // A group that ran out of room lists its members inline.
                                    StripItem::Group { members, .. } => members
                                        .into_iter()
                                        .map(|(index, tab)| tab.render_tab(&row_ctx, cx, index))
                                        .collect::<Vec<_>>(),
                                }
                            })),
                    )
                    .into_any_element()
            })
    }

    /// Collapses same-kind tabs (two or more) into group chips, keeping strip order.
    fn group_by_kind(tabs: Vec<Tab>) -> Vec<StripItem> {
        let mut counts: HashMap<SharedString, usize> = HashMap::new();
        for tab in &tabs {
            if let Some(kind) = &tab.kind {
                *counts.entry(kind.clone()).or_default() += 1;
            }
        }

        let mut items: Vec<StripItem> = Vec::new();
        let mut group_slots: HashMap<SharedString, usize> = HashMap::new();

        for (index, tab) in tabs.into_iter().enumerate() {
            let grouped_kind = tab
                .kind
                .as_ref()
                .filter(|kind| counts.get(*kind).copied().unwrap_or(0) >= 2)
                .cloned();

            match grouped_kind {
                Some(kind) => match group_slots.get(&kind).copied() {
                    Some(slot) => {
                        if let StripItem::Group { members, .. } = &mut items[slot] {
                            members.push((index, tab));
                        }
                    }
                    None => {
                        group_slots.insert(kind.clone(), items.len());
                        items.push(StripItem::Group {
                            kind,
                            members: vec![(index, tab)],
                        });
                    }
                },
                None => items.push(StripItem::Single(index, tab)),
            }
        }

        items
    }

    /// The group chip's identity for one strip and kind.
    fn group_key(tabs_id: &ElementId, kind: &SharedString) -> ElementId {
        ElementId::from((
            ElementId::from((tabs_id.clone(), "tabs-group")),
            kind.clone(),
        ))
    }

    /// Renders a group chip labeled with the kind and its member count.
    fn render_group_tab(
        ctx: &StripCtx,
        kind: SharedString,
        members: Vec<(usize, Tab)>,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let count = members.len();
        let selected = members.iter().any(|(index, _)| *index == ctx.selected);
        // A pinned member keeps the chip in the active treatment, selected or not.
        let active = selected || members.iter().any(|(_, tab)| tab.pinned);

        let group_key = Self::group_key(&ctx.id, &kind);
        let open = window
            .use_keyed_state((group_key.clone(), "open"), cx, |_, _| false)
            .clone();

        // Selecting a row also dismisses the popover, mirroring the overflow popover.
        let row_change: ChangeHandler = {
            let open = open.clone();
            let on_change = ctx.on_change.clone();
            Rc::new(move |index, window, cx| {
                open.update(cx, |open, cx| {
                    if *open {
                        *open = false;
                        cx.notify();
                    }
                });
                if let Some(on_change) = &on_change {
                    on_change(index, window, cx);
                }
            })
        };

        let row_ctx = StripCtx {
            on_change: Some(row_change),
            ..ctx.for_popover()
        };

        Popover::new((group_key.clone(), "group-popover"))
            .open(*open.read(cx))
            .trigger({
                let ctx = ctx.clone();
                let chip_key = group_key.clone();
                move |_, cx| {
                    let theme = cx.theme();
                    let label_color = ctx.variant.label_color(active, false, cx);
                    div()
                        .id((chip_key.clone(), "group-tab"))
                        .debug_selector({
                            let tabs_id = ctx.id.clone();
                            let label = kind.clone();
                            move || format!("{tabs_id}-group-{label}")
                        })
                        .flex()
                        .items_center()
                        .gap_2()
                        .cursor_pointer()
                        .role(Role::Tab)
                        .aria_selected(selected)
                        .aria_label(SharedString::from(format!("{kind}, {count} tabs")))
                        .text_color(label_color)
                        .when(!active, |chip| {
                            chip.hover(|chip| {
                                chip.bg(match ctx.variant {
                                    TabsVariant::Titlebar => theme.colors.muted_hover,
                                    _ => theme.colors.muted,
                                })
                            })
                        })
                        .map(|chip| ctx.variant.chrome(chip, active, cx))
                        .child(SharedString::from(format!("{kind} ({count})")))
                        .when(ctx.variant.is_capsule(), |chip| {
                            chip.child(div().flex_none().w_4())
                        })
                        .when_some(ctx.measure.clone(), |chip, measure| {
                            let key = measure.key.clone();
                            let root_bounds = measure.root_bounds.clone();
                            let item_key = chip_key.clone();
                            chip.child(
                                canvas(
                                    move |bounds, _, cx| {
                                        let root = root_bounds.borrow();
                                        if let Some(root) = root.as_ref() {
                                            cx.default_global::<TabsMeasurements>().record_tab(
                                                &key,
                                                item_key,
                                                TabMeasure {
                                                    left: bounds.origin.x - root.origin.x,
                                                    width: bounds.size.width,
                                                },
                                            );
                                        }
                                    },
                                    |_, _, _, _| {},
                                )
                                .absolute()
                                .inset_0(),
                            )
                        })
                        .into_any_element()
                }
            })
            .on_change(move |next, _, cx| {
                open.update(cx, |open, cx| {
                    if *open != next {
                        *open = next;
                        cx.notify();
                    }
                });
            })
            .content(move |_, cx| {
                ScrollArea::new((group_key.clone(), "tabs-group-list"))
                    .axes(ScrollAxes::Vertical)
                    .max_h(POPUP_MAX_HEIGHT)
                    .child(
                        v_flex().gap_0p5().p_1().children(
                            members
                                .into_iter()
                                .map(|(index, tab)| tab.render_tab(&row_ctx, cx, index)),
                        ),
                    )
                    .into_any_element()
            })
            .into_any_element()
    }
}

impl Styled for Tabs {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Tabs {
    fn render(mut self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tabs = match self.tabs {
            Some(build) => build(window, cx),
            None => Vec::new(),
        };
        let mut items = Self::group_by_kind(tabs);

        let selectable: Vec<usize> = items
            .iter()
            .flat_map(|item| match item {
                StripItem::Single(index, tab) => (!tab.disabled)
                    .then_some(*index)
                    .into_iter()
                    .collect::<Vec<_>>(),
                StripItem::Group { members, .. } => members
                    .iter()
                    .filter(|(_, tab)| !tab.disabled)
                    .map(|(index, _)| *index)
                    .collect::<Vec<_>>(),
            })
            .collect();

        let focus = self.id.focus_handle(window, cx);
        // Theme reads end here, so the strip's group chips can take `cx` mutably.
        let (border_color, shadow_bleed) = {
            let theme = cx.theme();
            let shadow_bleed = match self.variant {
                TabsVariant::Pill | TabsVariant::Titlebar => theme.elevations.control.bleed(),
                TabsVariant::Underline => Pixels::ZERO,
            };
            (theme.colors.border, shadow_bleed)
        };

        let label = self
            .a11y_label
            .take()
            .unwrap_or_else(|| SharedString::from(self.id.to_string()));

        let measure_key: MeasureKey =
            (window.window_handle().window_id().as_u64(), self.id.clone());

        // Read last frame's geometry under the borrow instead of cloning the strip.
        let (measured, inline_count, trigger_width) = cx
            .try_global::<TabsMeasurements>()
            .filter(|_| self.overflow_popover)
            .and_then(|cache| {
                cache.strips.get(&measure_key).map(|measure| {
                    (
                        Self::inline_count(&self.id, &items, measure),
                        measure.trigger_width,
                    )
                })
            })
            .map(|(count, trigger_width)| (true, count, trigger_width))
            .unwrap_or((false, items.len(), None));
        let overflow_items = items.split_off(inline_count);

        // The strip records geometry per inline tab each frame; drop entries
        // for tabs no longer rendered inline.
        if cx.try_global::<TabsMeasurements>().is_some() {
            let rendered: Vec<ElementId> = items
                .iter()
                .map(|item| item.measure_key(&self.id))
                .collect();
            cx.global_mut::<TabsMeasurements>()
                .retain_tabs(&measure_key, &rendered);
        }

        let measure_frame = self.overflow_popover.then(|| MeasureFrame {
            key: measure_key.clone(),
            root_bounds: Rc::new(RefCell::new(None)),
        });

        let drop_at = window
            .use_keyed_state((self.id.clone(), "tab-drop"), cx, |_, _| TabDrop::default())
            .clone();

        let ctx = StripCtx {
            id: self.id.clone(),
            selected: self.selected,
            variant: self.variant,
            on_change: self.on_change.clone(),
            on_close: self.on_close,
            on_reorder: self.on_reorder.clone(),
            focus: focus.clone(),
            measure: measure_frame.clone(),
            draggable: self.draggable && self.on_reorder.is_some(),
            drop_at,
        };

        let strip: Vec<AnyElement> = items
            .into_iter()
            .map(|item| match item {
                StripItem::Single(index, tab) => tab.render_tab(&ctx, cx, index),
                StripItem::Group { kind, members } => {
                    Self::render_group_tab(&ctx, kind, members, window, cx)
                }
            })
            .collect();

        let popover =
            (self.overflow_popover && measured && !overflow_items.is_empty()).then(|| {
                Self::render_overflow_popover(
                    &ctx,
                    overflow_items,
                    self.overflow_open,
                    self.on_overflow_open.clone(),
                )
            });

        div()
            .id(self.id.clone())
            .flex()
            .text_sm()
            .refine_style(&self.style)
            .track_focus(&focus)
            .focusable()
            .tab_stop(true)
            .role(Role::TabList)
            .aria_label(label)
            .on_key_down({
                let selected = self.selected;
                let on_change = self.on_change.clone();
                move |event: &KeyDownEvent, window, cx| {
                    let target = match event.keystroke.key.as_str() {
                        "right" => selectable.adjacent(Some(selected), true),
                        "left" => selectable.adjacent(Some(selected), false),
                        "home" => selectable.first().copied(),
                        "end" => selectable.last().copied(),
                        _ => return,
                    };
                    if let Some(next) = target
                        && next != selected
                        && let Some(on_change) = &on_change
                    {
                        on_change(next, window, cx);
                    }
                    cx.stop_propagation();
                }
            })
            .when(ctx.draggable, |root| {
                root.on_drag_move::<TabDrag>({
                    let drop_at = ctx.drop_at.clone();
                    move |event, _, cx| {
                        let position = event.event.position;
                        let outside = !event.bounds.contains(&position);
                        drop_at.update(cx, |drop, cx| {
                            let origin = drop.origin.get_or_insert(position);
                            if drop.horizontal.is_none() {
                                let dx = (position.x - origin.x).abs();
                                let dy = (position.y - origin.y).abs();
                                if dx.max(dy) > TAB_DRAG_AXIS_LOCK {
                                    drop.horizontal = Some(dx >= dy);
                                }
                            }
                            if (outside || drop.canceled()) && drop.onto.is_some() {
                                drop.onto = None;
                                cx.notify();
                            }
                        });
                    }
                })
                // A drop over the strip's gaps lands on the last hovered slot.
                .on_drop::<TabDrag>({
                    let drop_at = ctx.drop_at.clone();
                    let on_reorder = ctx.on_reorder.clone();
                    move |drag, window, cx| {
                        let onto = drop_at.read(cx).onto;
                        let canceled = drop_at.read(cx).canceled();
                        drop_at.update(cx, |drop, cx| {
                            if drop.onto.is_some() {
                                drop.onto = None;
                                cx.notify();
                            }
                        });
                        if let (Some(onto), Some(on_reorder)) = (onto, on_reorder.as_ref())
                            && !canceled
                            && let Some(to) = TabDrop::reorder_target(drag.index, onto)
                        {
                            on_reorder(drag.index, to, window, cx);
                        }
                    }
                })
            })
            .map(|root| match self.variant {
                TabsVariant::Underline => root.gap_1().border_b_1().border_color(border_color),
                TabsVariant::Pill | TabsVariant::Titlebar => root.items_center().gap_1(),
            })
            .map(|root| {
                if self.overflow_popover {
                    let Some(root_bounds) = measure_frame.map(|frame| frame.root_bounds) else {
                        return root.children(strip);
                    };

                    let control = match popover {
                        Some(popover) => div().flex_none().relative().child(popover),
                        None => div()
                            .flex_none()
                            .relative()
                            .child(div().w(trigger_width.unwrap_or(Pixels::ZERO))),
                    };

                    root.min_w_0()
                        .flex_grow(1.0)
                        .overflow_hidden()
                        .p(shadow_bleed)
                        .m(-shadow_bleed)
                        .child(
                            canvas(
                                {
                                    let key = measure_key.clone();
                                    move |bounds, _, cx| {
                                        cx.default_global::<TabsMeasurements>()
                                            .record_container(&key, bounds.size.width);
                                        *root_bounds.borrow_mut() = Some(bounds);
                                    }
                                },
                                |_, _, _, _| {},
                            )
                            .absolute()
                            .inset(shadow_bleed),
                        )
                        .child(
                            div()
                                .flex_none()
                                .flex()
                                .gap_1()
                                .when(self.variant.is_capsule(), |row| row.items_center())
                                .children(strip),
                        )
                        .child(
                            control.child(
                                canvas(
                                    {
                                        let key = measure_key;
                                        move |bounds, _, cx| {
                                            cx.default_global::<TabsMeasurements>()
                                                .record_trigger(&key, bounds.size.width);
                                        }
                                    },
                                    |_, _, _, _| {},
                                )
                                .absolute()
                                .inset_0(),
                            ),
                        )
                } else if self.overflow_scroll {
                    root.min_w_0()
                        .overflow_x_scroll()
                        .p(shadow_bleed)
                        .m(-shadow_bleed)
                        .child(
                            div()
                                .flex_none()
                                .flex()
                                .gap_1()
                                .when(self.variant.is_capsule(), |row| row.items_center())
                                .children(strip),
                        )
                } else {
                    root.children(strip)
                }
            })
    }
}
