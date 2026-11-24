//! Renders floating menus of [`MenuItem`] rows for [`Menu`], dropdowns, and
//! context menus.
//!
//! # Examples
//!
//! ```no_run
//! use gpui::{IntoElement as _, ParentElement as _, SharedString, div};
//! use ui::menu::{Menu, MenuItem};
//!
//! let menu = Menu::new("file-menu")
//!     .trigger(|_window, _cx| {
//!         div().child(SharedString::from("File")).into_any_element()
//!     })
//!     .items(|_window, _cx| {
//!         vec![
//!             MenuItem::new("New File").handler(|_window, _cx| println!("new")),
//!             MenuItem::separator(),
//!             MenuItem::new("Open Recent").submenu(vec![MenuItem::new("Project One")]),
//!         ]
//!     });
//! ```

use std::cell::RefCell;
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    Animation, AnimationExt as _, AnyElement, App, Bounds, DefiniteLength, DispatchPhase,
    ElementId, Entity, FocusHandle, InteractiveElement as _, IntoElement, KeyDownEvent,
    MouseButton, MouseDownEvent, ParentElement as _, Pixels, Point, RenderOnce, Role, ScrollHandle,
    SharedString, StatefulInteractiveElement as _, StyleRefinement, Styled as _, Window, anchored,
    canvas, deferred, div, ease_out_quint, point, px, relative, rems,
};

use crate::components::common::{
    OVERLAY_LAYER, StyledExt as _, floating_panel, focus_handle, track_overlay_focus, v_flex,
    wrapping_adjacent_selectable,
};
use crate::components::scroll_area::{ScrollArea, ScrollAxes};
use crate::icons::{Icon, IconRole, icon as icon_glyph};
use crate::theme::{ActiveTheme as _, MENU_MIN_WIDTH, POPUP_MAX_HEIGHT};

/// Runs the item's action. The menu closes itself before the handler runs.
pub type ItemHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// Reports the next open state of a menu: `true` to open, `false` to close.
pub(crate) type ChangeHandler = Rc<dyn Fn(bool, &mut Window, &mut App)>;
/// Flips a panel's flyout open state; `true` arms the first row for keyboard.
type FlyoutSetter = Rc<dyn Fn(Option<usize>, bool, &mut App)>;
/// Bounds captured during the last paint, shared with the dismissal listener.
pub(crate) type TriggerBoundsCell = Rc<RefCell<Option<Bounds<Pixels>>>>;
/// The element the panel anchors below, built during render.
type TriggerFn = Box<dyn Fn(&mut Window, &mut App) -> AnyElement>;
/// The menu's rows, rebuilt on each render while open.
type ItemsFn = Box<dyn Fn(&mut Window, &mut App) -> Vec<MenuItem>>;

/// One row of a [`Menu`]: an action, a checkable action, a submenu, or a separator.
#[derive(Clone)]
enum MenuItemKind {
    Action,
    Separator,
    Submenu(Rc<[MenuItem]>),
}

/// One row of a [`Menu`]: an action, a checkable action, a submenu, or a separator.
#[derive(Clone)]
#[must_use]
pub struct MenuItem {
    id: Option<ElementId>,
    kind: MenuItemKind,
    label: SharedString,
    icon: Option<Icon>,
    handler: Option<ItemHandler>,
    disabled: bool,
    destructive: bool,
    checked: bool,
}

impl MenuItem {
    /// Creates an action row with this label.
    pub fn new<L>(label: L) -> Self
    where
        L: Into<SharedString>,
    {
        Self {
            id: None,
            kind: MenuItemKind::Action,
            label: label.into(),
            icon: None,
            handler: None,
            disabled: false,
            destructive: false,
            checked: false,
        }
    }

    /// Creates a divider between groups of actions.
    pub fn separator() -> Self {
        Self {
            id: None,
            kind: MenuItemKind::Separator,
            label: SharedString::default(),
            icon: None,
            handler: None,
            disabled: false,
            destructive: false,
            checked: false,
        }
    }

    /// Overrides the row's element id, for stable identity across reorderings.
    pub fn id<I>(mut self, id: I) -> Self
    where
        I: Into<ElementId>,
    {
        self.id = Some(id.into());
        self
    }

    /// Runs this action when the item is activated. The menu closes first.
    pub fn handler<F>(mut self, handler: F) -> Self
    where
        F: Fn(&mut Window, &mut App) + 'static,
    {
        self.handler = Some(Rc::new(handler));
        self
    }

    /// Shows an icon before the label.
    pub fn icon<I>(mut self, icon: I) -> Self
    where
        I: Into<Icon>,
    {
        self.icon = Some(icon.into());
        self
    }

    /// Marks the item non-selectable and non-activatable.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Paints the row in the danger role.
    pub fn destructive(mut self, destructive: bool) -> Self {
        self.destructive = destructive;
        self
    }

    /// Shows a check slot before the label.
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    /// Opens a submenu with these rows instead of running an action.
    pub fn submenu<I>(mut self, items: I) -> Self
    where
        I: IntoIterator<Item = MenuItem>,
    {
        self.kind = MenuItemKind::Submenu(items.into_iter().collect::<Vec<_>>().into());
        self
    }

    /// The submenu's rows, when this item opens one.
    fn submenu_items(&self) -> Option<&[MenuItem]> {
        match &self.kind {
            MenuItemKind::Submenu(items) => Some(items),
            _ => None,
        }
    }

    /// Whether keyboard navigation and activation may land on this item.
    fn selectable(&self) -> bool {
        matches!(self.kind, MenuItemKind::Action | MenuItemKind::Submenu(_)) && !self.disabled
    }

    /// Renders one menu row.
    fn render_item(
        &self,
        window: &mut Window,
        cx: &mut App,
        index: usize,
        selected: bool,
        env: &RowEnv,
    ) -> AnyElement {
        match &self.kind {
            MenuItemKind::Separator => div()
                .h_px()
                .debug_selector(move || format!("{}-separator-{index}", env.debug_prefix))
                .mx_2()
                .my_1()
                .bg(env.chrome.border)
                .into_any_element(),
            MenuItemKind::Action => {
                let label_color = self.row_color(selected, false, env.chrome);
                let disabled = self.disabled;
                let checked = self.checked;
                let icon = self.icon.clone();

                self.row_shell(cx, index, selected, env)
                    .on_click({
                        let handler = self.handler.clone();
                        let on_activate = env.on_activate.clone();
                        move |_, window, cx| {
                            if disabled {
                                return;
                            }

                            if let Some(on_activate) = &on_activate {
                                on_activate(false, window, cx);
                            }

                            if let Some(handler) = &handler {
                                handler(window, cx);
                            }
                        }
                    })
                    .when_some(icon, |row, icon| {
                        row.child(icon.size(rems(1.0)).color(label_color))
                    })
                    .child(
                        div()
                            .flex_1()
                            .text_color(label_color)
                            .child(self.label.clone()),
                    )
                    .when(checked, |row| {
                        row.child(
                            icon_glyph(IconRole::Check)
                                .size(rems(0.75))
                                .color(label_color),
                        )
                    })
                    .into_any_element()
            }
            MenuItemKind::Submenu(items) => {
                let chrome = env.chrome;
                let flyout = env.flyout;
                let disabled = self.disabled;
                let open = *flyout.open.read(cx) == Some(index);
                let label_color = self.row_color(selected, open, chrome);
                let row_bounds: TriggerBoundsCell = Rc::new(RefCell::new(None));

                self.row_shell(cx, index, selected, env)
                    .on_click({
                        let set_flyout = flyout.set_flyout.clone();
                        move |_, _window, cx| {
                            if disabled {
                                return;
                            }
                            // Hover already opens the flyout; a click never closes it.
                            set_flyout(Some(index), false, cx);
                        }
                    })
                    .child(
                        div()
                            .flex_1()
                            .text_color(label_color)
                            .child(self.label.clone()),
                    )
                    .child(
                        icon_glyph(IconRole::ChevronRight)
                            .size(rems(0.75))
                            .color(label_color),
                    )
                    .aria_expanded(open)
                    .when(open, |row| {
                        row.child(
                            canvas(
                                {
                                    let cell = row_bounds.clone();
                                    move |bounds, _window, _cx| *cell.borrow_mut() = Some(bounds)
                                },
                                |_, _, _, _| {},
                            )
                            .absolute()
                            .inset_0(),
                        )
                        .child(
                            deferred(div().absolute().left_full().top_0().child(menu_panel(
                                window,
                                cx,
                                flyout.state,
                                MenuPanelParams {
                                    id: &flyout.id,
                                    items: items.clone(),
                                    initial_selection: None,
                                    preselect: *flyout.preselect.read(cx),
                                    min_width: flyout.min_width,
                                    label: self.label.clone(),
                                    dismiss_bounds: &row_bounds,
                                    bounds_sink: Some(flyout.bounds),
                                    outer_stack: Some(flyout.stack),
                                    on_dismiss: Some(flyout.dismiss.clone()),
                                    on_activate: flyout.on_activate.clone(),
                                    is_flyout: true,
                                    chrome,
                                    enter_duration: flyout.enter_duration,
                                    debug_prefix: "submenu",
                                },
                            )))
                            .with_priority(OVERLAY_LAYER),
                        )
                    })
                    .into_any_element()
            }
        }
    }

    /// The shared row shell: dimensions, highlight, and hover-selection.
    fn row_shell(
        &self,
        cx: &App,
        index: usize,
        selected: bool,
        env: &RowEnv,
    ) -> gpui::Stateful<gpui::Div> {
        let chrome = env.chrome;
        let selectable = self.selectable();
        let opens_submenu = self.submenu_items().is_some();
        let flyout_open = *env.flyout.open.read(cx) == Some(index);
        let row_id = self
            .id
            .clone()
            .unwrap_or_else(|| ("menu-item", index).into());

        div()
            .id(row_id)
            .role(Role::MenuItem)
            .debug_selector(move || format!("{}-item-{index}", env.debug_prefix))
            .flex()
            .items_center()
            .gap_2()
            .min_h_8()
            .px_4()
            .text_sm()
            .line_height(relative(1.0))
            .rounded(chrome.radius_full)
            .map(|row| {
                if self.disabled {
                    row
                } else if selected && flyout_open {
                    row.bg(chrome.muted_hover)
                } else if selected {
                    let highlight = if self.destructive {
                        chrome.destructive
                    } else {
                        chrome.primary
                    };
                    row.bg(highlight)
                } else {
                    row.hover(|row| row.bg(chrome.muted_hover))
                }
            })
            .on_mouse_move({
                let selection = env.selection.clone();
                let flyout_open = env.flyout.open.clone();
                let set_flyout = env.flyout.set_flyout.clone();
                let dismiss = env.flyout.dismiss.clone();
                move |_, window, cx| {
                    if !selectable {
                        return;
                    }

                    if *selection.read(cx) != Some(index) {
                        selection.update(cx, |selection, _| *selection = Some(index));
                    }

                    if opens_submenu {
                        set_flyout(Some(index), false, cx);
                    } else if flyout_open.read(cx).is_some() {
                        dismiss(false, window, cx);
                    }
                }
            })
    }

    /// The row's label/icon color.
    fn row_color(&self, selected: bool, flyout_open: bool, chrome: &MenuChrome) -> gpui::Hsla {
        if self.disabled {
            return chrome.muted_foreground;
        }
        if self.destructive {
            return if selected && !flyout_open {
                chrome.destructive_foreground
            } else {
                chrome.destructive
            };
        }
        if selected && !flyout_open {
            chrome.primary_foreground
        } else {
            chrome.foreground
        }
    }
}

// MenuItem is a data builder, not an element, adopt the combinators explicitly.
impl FluentBuilder for MenuItem {}

/// Menu chrome styles.
#[derive(Clone)]
pub(crate) struct MenuChrome {
    border: gpui::Hsla,
    foreground: gpui::Hsla,
    muted_foreground: gpui::Hsla,
    muted_hover: gpui::Hsla,
    primary: gpui::Hsla,
    primary_foreground: gpui::Hsla,
    destructive: gpui::Hsla,
    destructive_foreground: gpui::Hsla,
    radius_full: Pixels,
}

/// A floating menu of [`MenuItem`] rows anchored below its trigger.
#[derive(IntoElement)]
#[must_use]
pub struct Menu {
    id: ElementId,
    open: bool,
    initial_selection: Option<usize>,
    trigger_button: MouseButton,
    open_at: Option<Point<Pixels>>,
    a11y_label: Option<SharedString>,
    on_change: Option<ChangeHandler>,
    trigger: Option<TriggerFn>,
    items: Option<ItemsFn>,
    min_width: Option<DefiniteLength>,
    style: StyleRefinement,
}

impl MenuChrome {
    /// Reads the menu's chrome and enter duration from the active theme.
    pub(crate) fn read(cx: &App) -> (Self, std::time::Duration) {
        let theme = cx.theme();
        (
            Self {
                border: theme.colors.border,
                foreground: theme.colors.foreground,
                muted_foreground: theme.colors.muted_foreground,
                muted_hover: theme.colors.muted_hover,
                primary: theme.ramps.primary.base,
                primary_foreground: theme.colors.primary_foreground,
                destructive: theme.ramps.destructive.base,
                destructive_foreground: theme.colors.destructive_foreground,
                radius_full: theme.radius.full,
            },
            theme.motion.overlay,
        )
    }
}

impl Menu {
    /// Creates a closed menu.
    pub fn new<I>(id: I) -> Self
    where
        I: Into<ElementId>,
    {
        Self {
            id: id.into(),
            open: false,
            initial_selection: None,
            trigger_button: MouseButton::Left,
            open_at: None,
            a11y_label: None,
            on_change: None,
            trigger: None,
            items: None,
            min_width: None,
            style: StyleRefinement::default(),
        }
    }

    /// Sets the row that starts keyboard-selected when the menu opens.
    pub fn initial_selection(mut self, index: usize) -> Self {
        self.initial_selection = Some(index);
        self
    }

    /// Sets the mouse button that toggles the menu; right for context menus.
    pub fn trigger_button(mut self, button: MouseButton) -> Self {
        self.trigger_button = button;
        self
    }

    /// Positions the panel at this window point when it opens.
    pub fn open_at<P>(mut self, position: P) -> Self
    where
        P: Into<Option<Point<Pixels>>>,
    {
        self.open_at = position.into();
        self
    }

    /// Sets the current open state.
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// Reports the next open state, from dismissals, toggles, and activations alike.
    pub fn on_change<F>(mut self, handler: F) -> Self
    where
        F: Fn(bool, &mut Window, &mut App) + 'static,
    {
        self.on_change = Some(Rc::new(handler));
        self
    }

    /// Sets the element the panel anchors below.
    pub fn trigger<F>(mut self, trigger: F) -> Self
    where
        F: Fn(&mut Window, &mut App) -> AnyElement + 'static,
    {
        self.trigger = Some(Box::new(trigger));
        self
    }

    /// Sets the menu's items, built fresh each render.
    pub fn items<F>(mut self, items: F) -> Self
    where
        F: Fn(&mut Window, &mut App) -> Vec<MenuItem> + 'static,
    {
        self.items = Some(Box::new(items));
        self
    }

    /// Overrides the accessibility label of the panel.
    pub fn accessibility_label<L>(mut self, label: L) -> Self
    where
        L: Into<SharedString>,
    {
        self.a11y_label = Some(label.into());
        self
    }

    /// Sets the panel's minimum width; defaults to the standard menu width.
    pub fn min_width<W>(mut self, width: W) -> Self
    where
        W: Into<DefiniteLength>,
    {
        self.min_width = Some(width.into());
        self
    }
}

impl gpui::Styled for Menu {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Menu {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let (chrome, enter_duration) = MenuChrome::read(cx);
        let trigger_bounds: Rc<RefCell<Option<Bounds<Pixels>>>> = Rc::new(RefCell::new(None));
        let on_change = self.on_change.clone();

        let panel_focus = focus_handle(&(self.id.clone(), "panel").into(), window, cx);
        track_overlay_focus(&self.id, &panel_focus, self.open, window, cx);

        let fallback_label = self
            .a11y_label
            .clone()
            .unwrap_or_else(|| SharedString::from(self.id.to_string()));

        let items = if self.open {
            match self.items {
                Some(build) => Rc::from(build(window, cx)),
                None => Rc::from([]),
            }
        } else {
            Rc::from([])
        };

        let spec = MenuPanelSpec {
            open: self.open,
            items,
            initial_selection: self.initial_selection,
            preselect: false,
            min_width: self.min_width,
            label: fallback_label,
            position: self.open_at,
            trigger_bounds: trigger_bounds.clone(),
            on_dismiss: on_change.clone(),
            on_activate: self.on_change,
        };

        div()
            .relative()
            .refine_style(&self.style)
            .child(trigger_toggle(
                &trigger_bounds,
                self.open,
                on_change.clone(),
                self.trigger_button,
            ))
            .when_some(self.trigger, |wrapper, trigger| {
                wrapper.child(trigger(window, cx))
            })
            .when_some(
                anchored_menu_panel(window, cx, &self.id, spec, &chrome, enter_duration),
                |wrapper, panel| wrapper.child(panel),
            )
    }
}

/// Everything the floating menu panel needs from its owner.
pub(crate) struct MenuPanelSpec {
    pub open: bool,
    pub items: Rc<[MenuItem]>,
    pub initial_selection: Option<usize>,
    pub preselect: bool,
    pub min_width: Option<DefiniteLength>,
    pub label: SharedString,
    pub position: Option<Point<Pixels>>,
    pub trigger_bounds: TriggerBoundsCell,
    /// Closes this panel: outside clicks, `escape`, and `left` past a closed flyout.
    pub on_dismiss: Option<ChangeHandler>,
    /// Closes the whole menu when a row inside activates.
    pub on_activate: Option<ChangeHandler>,
}

/// Per-panel state that survives across frames while the panel stays mounted.
struct MenuPanelState {
    focus: FocusHandle,
    was_open: Entity<bool>,
    scroll: Entity<ScrollHandle>,
    selection: Entity<Option<usize>>,
}

struct MenuPanelParams<'a> {
    id: &'a ElementId,
    items: Rc<[MenuItem]>,
    initial_selection: Option<usize>,
    preselect: bool,
    min_width: Option<DefiniteLength>,
    label: SharedString,
    /// Clicks here do not dismiss the panel: the trigger or the owning row.
    dismiss_bounds: &'a TriggerBoundsCell,
    /// Receives the panel's painted bounds, for a parent's dismissal checks.
    bounds_sink: Option<&'a TriggerBoundsCell>,
    /// This panel's flyout bounds are pushed here.
    outer_stack: Option<&'a Rc<RefCell<Vec<TriggerBoundsCell>>>>,
    on_dismiss: Option<ChangeHandler>,
    on_activate: Option<ChangeHandler>,
    /// Flyout panels dismiss themselves on `left`; top-level panels ignore it.
    is_flyout: bool,
    chrome: &'a MenuChrome,
    enter_duration: std::time::Duration,
    debug_prefix: &'static str,
}

/// Wiring a panel's rows need to open their submenus.
struct FlyoutContext<'a> {
    state: &'a MenuPanelState,
    open: &'a Entity<Option<usize>>,
    preselect: &'a Entity<bool>,
    /// Opens or closes this level's flyout, re-arming it on every switch.
    set_flyout: FlyoutSetter,
    /// The open flyout's painted bounds; a parent ignores clicks inside them.
    bounds: &'a TriggerBoundsCell,
    /// Bounds of every open flyout below this panel, this level's included.
    stack: &'a Rc<RefCell<Vec<TriggerBoundsCell>>>,
    /// Closes this level's flyout and returns focus to this panel.
    dismiss: ChangeHandler,
    on_activate: Option<ChangeHandler>,
    id: ElementId,
    enter_duration: std::time::Duration,
    min_width: Option<DefiniteLength>,
}

struct RowEnv<'a> {
    chrome: &'a MenuChrome,
    selection: &'a Entity<Option<usize>>,
    on_activate: Option<ChangeHandler>,
    flyout: &'a FlyoutContext<'a>,
    debug_prefix: &'static str,
}

/// Everything key navigation needs from the panel.
struct MenuKeyEnv<'a> {
    items: &'a [MenuItem],
    selection: &'a Entity<Option<usize>>,
    set_flyout: &'a FlyoutSetter,
    on_dismiss: &'a Option<ChangeHandler>,
    on_activate: &'a Option<ChangeHandler>,
    left_dismisses: bool,
}

/// Canvas that records the trigger's painted bounds and toggles `open` on click.
pub(crate) fn trigger_toggle(
    bounds: &TriggerBoundsCell,
    open: bool,
    on_change: Option<ChangeHandler>,
    button: MouseButton,
) -> AnyElement {
    let painted = bounds.clone();

    canvas(
        {
            let cell = bounds.clone();
            move |painted, _window, _cx| *cell.borrow_mut() = Some(painted)
        },
        move |_, _, window, _cx| {
            let cell = painted.clone();
            window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || event.button != button {
                    return;
                }

                let on_trigger = cell
                    .borrow()
                    .is_some_and(|bounds| bounds.contains(&event.position));
                if !on_trigger {
                    return;
                }

                if let Some(on_change) = &on_change {
                    on_change(!open, window, cx);
                }
                cx.stop_propagation();
            });
        },
    )
    .absolute()
    .inset_0()
    .into_any_element()
}

/// Deferred anchored overlay on OVERLAY_LAYER, optionally fading in.
pub(crate) fn anchored_overlay(
    position: Option<Point<Pixels>>,
    panel: AnyElement,
    fade: Option<(&'static str, std::time::Duration)>,
) -> AnyElement {
    deferred(
        anchored()
            .when_some(position, |anchored, position| anchored.position(position))
            .offset(point(px(0.), px(4.)))
            // AnyElement lacks `Styled`, so the opacity animation rides a wrapper.
            .child(panel.when_some(fade, |panel, (id, duration)| {
                div()
                    .child(panel)
                    .with_animation(
                        id,
                        Animation::new(duration).with_easing(ease_out_quint()),
                        |panel, delta| panel.opacity(delta),
                    )
                    .into_any_element()
            })),
    )
    .with_priority(OVERLAY_LAYER)
    .into_any_element()
}

/// Canvas that grabs focus when `should_grab` holds.
pub(crate) fn focus_capture(
    focus: FocusHandle,
    should_grab: impl Fn(&mut Window, &mut App) -> bool + 'static,
) -> AnyElement {
    canvas(
        |_, _, _| {},
        move |_, _, window, cx| {
            if should_grab(window, cx) {
                window.focus(&focus, cx);
            }
        },
    )
    .absolute()
    .inset_0()
    .into_any_element()
}

/// Assembles the floating menu panel anchored to its trigger.
pub(crate) fn anchored_menu_panel(
    window: &mut Window,
    cx: &mut App,
    id: &ElementId,
    spec: MenuPanelSpec,
    chrome: &MenuChrome,
    enter_duration: std::time::Duration,
) -> Option<AnyElement> {
    let was_open = window.use_keyed_state((id.clone(), "menu-was-open"), cx, |_, _| false);

    if !spec.open {
        was_open.update(cx, |was_open, _| *was_open = false);
        // A closed menu drops any open flyout; fresh state on the next open.
        window
            .use_keyed_state::<Option<usize>>((id.clone(), "submenu-open"), cx, |_, _| None)
            .update(cx, |open, _| *open = None);
        window
            .use_keyed_state((id.clone(), "submenu-preselect"), cx, |_, _| false)
            .update(cx, |preselect, _| *preselect = false);
        return None;
    }

    let state = create_menu_panel_state(id, window, cx);
    let panel = menu_panel(
        window,
        cx,
        &state,
        MenuPanelParams {
            id,
            items: spec.items,
            initial_selection: spec.initial_selection,
            preselect: spec.preselect,
            min_width: spec.min_width,
            label: spec.label,
            dismiss_bounds: &spec.trigger_bounds,
            bounds_sink: None,
            outer_stack: None,
            on_dismiss: spec.on_dismiss,
            on_activate: spec.on_activate,
            is_flyout: false,
            chrome,
            enter_duration,
            debug_prefix: "menu",
        },
    );

    Some(anchored_overlay(spec.position, panel, None))
}

/// Builds one menu panel: chrome, rows, focus, navigation, and dismissals.
fn menu_panel(
    window: &mut Window,
    cx: &mut App,
    state: &MenuPanelState,
    params: MenuPanelParams,
) -> AnyElement {
    let MenuPanelState {
        focus,
        was_open,
        scroll,
        selection,
    } = state;

    // Render-phase residue: panels open from owner state alone, so this mount
    // frame is the only place fresh-selection init can read its params.
    let selected = if *was_open.read(cx) {
        *selection.read(cx)
    } else {
        let requested = params
            .initial_selection
            .map(|index| index.min(params.items.len().saturating_sub(1)))
            .or_else(|| {
                params
                    .preselect
                    .then(|| params.items.iter().position(MenuItem::selectable))
                    .flatten()
            });
        selection.update(cx, |selection, _| *selection = requested);
        requested
    };
    was_open.update(cx, |was_open, _| *was_open = true);

    // Submenu plumbing for this level; a flyout panel recurses with its own.
    let flyout_id: ElementId = (params.id.clone(), "submenu").into();
    let flyout_state = create_menu_panel_state(&flyout_id, window, cx);
    let flyout_open = window.use_keyed_state((params.id.clone(), "submenu-open"), cx, |_, _| None);
    let flyout_preselect =
        window.use_keyed_state((params.id.clone(), "submenu-preselect"), cx, |_, _| false);
    let flyout_bounds: TriggerBoundsCell = Rc::new(RefCell::new(None));
    let flyout_stack: Rc<RefCell<Vec<TriggerBoundsCell>>> = Rc::new(RefCell::new(Vec::new()));
    flyout_stack.borrow_mut().push(flyout_bounds.clone());

    if let Some(outer_stack) = params.outer_stack {
        outer_stack.borrow_mut().push(flyout_bounds.clone());
    }

    // Every open flip re-arms a clean flyout, so render never reconciles one.
    let set_flyout: FlyoutSetter = {
        let flyout_open = flyout_open.clone();
        let flyout_preselect = flyout_preselect.clone();
        let flyout_was_open = flyout_state.was_open.clone();
        let flyout_selection = flyout_state.selection.clone();
        Rc::new(move |index, opens_preselected, cx| {
            if index.is_some() && *flyout_open.read(cx) == index {
                return;
            }

            flyout_open.update(cx, |open, _| *open = index);
            flyout_preselect.update(cx, |preselect, _| *preselect = opens_preselected);
            flyout_was_open.update(cx, |was_open, _| *was_open = false);
            flyout_selection.update(cx, |selection, _| *selection = None);
        })
    };

    let dismiss_flyout: ChangeHandler = {
        let set_flyout = set_flyout.clone();
        let focus = focus.clone();
        Rc::new(move |_, window, cx| {
            set_flyout(None, false, cx);
            window.focus(&focus, cx);
        })
    };

    let flyout = FlyoutContext {
        state: &flyout_state,
        open: &flyout_open,
        preselect: &flyout_preselect,
        set_flyout: set_flyout.clone(),
        bounds: &flyout_bounds,
        stack: &flyout_stack,
        dismiss: dismiss_flyout,
        on_activate: params.on_activate.clone(),
        id: flyout_id,
        enter_duration: params.enter_duration,
        min_width: params.min_width,
    };

    let rows: Vec<AnyElement> = params
        .items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            item.render_item(
                window,
                cx,
                index,
                selected == Some(index),
                &RowEnv {
                    chrome: params.chrome,
                    selection,
                    on_activate: params.on_activate.clone(),
                    flyout: &flyout,
                    debug_prefix: params.debug_prefix,
                },
            )
        })
        .collect();

    floating_panel(
        cx,
        cx.theme().radius.overlay,
        v_flex()
            .id("menu-panel")
            .debug_selector(move || format!("{}-panel", params.debug_prefix))
            .occlude()
            .track_focus(focus)
            .role(Role::Menu)
            .aria_label(params.label)
            .when_some(params.min_width, |panel, width| panel.min_w(width))
            .when(params.min_width.is_none(), |panel| {
                panel.min_w(MENU_MIN_WIDTH)
            })
            .p_1()
            .text_color(params.chrome.foreground),
        cx.theme().elevations.popover.shadows(),
    )
    .overflow_hidden()
    .on_mouse_down_out({
        let on_dismiss = params.on_dismiss.clone();
        let dismiss_bounds = params.dismiss_bounds.clone();
        let flyout_stack = flyout_stack.clone();

        move |event, window, cx| {
            if dismiss_bounds
                .borrow()
                .is_some_and(|bounds| bounds.contains(&event.position))
            {
                return;
            }

            // Clicks inside any open flyout belong to that flyout.
            if flyout_stack.borrow().iter().any(|cell| {
                cell.borrow()
                    .is_some_and(|bounds| bounds.contains(&event.position))
            }) {
                return;
            }

            if let Some(on_dismiss) = &on_dismiss {
                on_dismiss(false, window, cx);
            }
        }
    })
    .on_key_down({
        let items = params.items.clone();
        let selection = selection.clone();
        let set_flyout = set_flyout.clone();
        let on_dismiss = params.on_dismiss.clone();
        let on_activate = params.on_activate.clone();
        let left_dismisses = params.is_flyout;
        move |event: &KeyDownEvent, window, cx| {
            handle_menu_key(
                event,
                MenuKeyEnv {
                    items: &items,
                    selection: &selection,
                    set_flyout: &set_flyout,
                    on_dismiss: &on_dismiss,
                    on_activate: &on_activate,
                    left_dismisses,
                },
                window,
                cx,
            );
        }
    })
    .child(
        ScrollArea::new("menu-items")
            .handle(scroll.read(cx).clone())
            .axes(ScrollAxes::Vertical)
            .max_h(POPUP_MAX_HEIGHT)
            .child(v_flex().gap_0p5().children(rows)),
    )
    .child(
        canvas(
            {
                let bounds_sink = params.bounds_sink.cloned();
                move |bounds, _window, _cx| {
                    if let Some(cell) = &bounds_sink {
                        *cell.borrow_mut() = Some(bounds);
                    }
                }
            },
            {
                let focus = focus.clone();
                let flyout_open = flyout_open.clone();
                move |_, _, window, cx| {
                    if flyout_open.read(cx).is_none() && !focus.contains_focused(window, cx) {
                        window.focus(&focus, cx);
                    }
                }
            },
        )
        .absolute()
        .inset_0(),
    )
    .with_animation(
        "menu-enter",
        Animation::new(params.enter_duration).with_easing(ease_out_quint()),
        |panel, delta| panel.opacity(delta),
    )
    .into_any_element()
}

/// Handles the panel's navigation keys.
fn handle_menu_key(event: &KeyDownEvent, env: MenuKeyEnv, window: &mut Window, cx: &mut App) {
    let selectable: Vec<usize> = env
        .items
        .iter()
        .enumerate()
        .filter(|(_, item)| item.selectable())
        .map(|(index, _)| index)
        .collect();

    let current = *env.selection.read(cx);

    match event.keystroke.key.as_str() {
        "down" | "up" | "home" | "end" => {
            if selectable.is_empty() {
                return;
            }
            let next = match event.keystroke.key.as_str() {
                "down" => wrapping_adjacent_selectable(&selectable, current, true),
                "up" => wrapping_adjacent_selectable(&selectable, current, false),
                "home" => selectable.first().copied(),
                _ => selectable.last().copied(),
            };
            if let Some(next) = next {
                env.selection
                    .update(cx, |selection, _| *selection = Some(next));
            }
        }
        "right" => {
            let opens = current
                .and_then(|current| env.items.get(current))
                .is_some_and(|item| item.selectable() && item.submenu_items().is_some());
            if !opens {
                return;
            }
            (env.set_flyout)(current, true, cx);
        }
        "enter" | "space" => {
            if let Some(current) = current
                && let Some(item) = env.items.get(current)
                && item.selectable()
            {
                if item.submenu_items().is_some() {
                    (env.set_flyout)(Some(current), true, cx);
                } else {
                    if let Some(on_activate) = env.on_activate {
                        on_activate(false, window, cx);
                    }
                    if let Some(handler) = &item.handler {
                        handler(window, cx);
                    }
                }
            }
        }
        "escape" => {
            if let Some(on_dismiss) = env.on_dismiss {
                on_dismiss(false, window, cx);
            }
        }
        "left" => {
            if !env.left_dismisses {
                return;
            }
            if let Some(on_dismiss) = env.on_dismiss {
                on_dismiss(false, window, cx);
            }
        }
        _ => return,
    }

    cx.stop_propagation();
}

/// Creates one panel's keyed state: focus, open tracking, scroll, and selection.
fn create_menu_panel_state(id: &ElementId, window: &mut Window, cx: &mut App) -> MenuPanelState {
    MenuPanelState {
        focus: focus_handle(&(id.clone(), "panel").into(), window, cx),
        was_open: window.use_keyed_state((id.clone(), "menu-was-open"), cx, |_, _| false),
        scroll: window.use_keyed_state((id.clone(), "menu-scroll"), cx, |_, _| ScrollHandle::new()),
        selection: window.use_keyed_state((id.clone(), "menu-selection"), cx, |_, _| None),
    }
}
