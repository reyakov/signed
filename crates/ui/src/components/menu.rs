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
use std::mem;
use std::rc::Rc;
use std::time::Duration;

use gpui::prelude::FluentBuilder;
use gpui::{
    Anchor, Animation, AnimationExt as _, AnyElement, App, Bounds, DefiniteLength, DispatchPhase,
    Div, ElementId, Entity, FocusHandle, Hsla, InteractiveElement as _, IntoElement, KeyBinding,
    ListAlignment, ListSizingBehavior, ListState, MouseButton, MouseDownEvent, ParentElement as _,
    Pixels, Point, RenderOnce, Role, SharedString, Stateful, StatefulInteractiveElement as _,
    StyleRefinement, Styled, Window, actions, anchored, canvas, deferred, div, ease_out_quint,
    list, point, px, relative, rems,
};

use crate::components::common::{
    ElementIdExt as _, OVERLAY_LAYER, SelectableRows as _, StyledExt as _, v_flex,
};
use crate::icons::{Icon, IconRole};
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

actions!(
    menu,
    [
        SelectNext,
        SelectPrevious,
        SelectFirst,
        SelectLast,
        OpenSubmenu,
        Activate,
        Dismiss,
        DismissLeft,
        Leave
    ]
);

/// The key context the menu panel declares.
pub const MENU_CONTEXT: &str = "Menu";

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
        let flyout = env.flyout.as_ref();

        match &self.kind {
            MenuItemKind::Separator => div()
                .h_px()
                .debug_selector(move || format!("{}-separator-{index}", env.debug_prefix))
                .mx_2()
                .my_1()
                .bg(env.chrome.border)
                .into_any_element(),
            MenuItemKind::Action => {
                let label_color = self.row_color(selected, false, &env.chrome);
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
                            .min_w_0()
                            .truncate()
                            .text_color(label_color)
                            .child(self.label.clone()),
                    )
                    .when(checked, |row| {
                        row.child(
                            Icon::from(IconRole::Check)
                                .size(rems(0.75))
                                .color(label_color),
                        )
                    })
                    .into_any_element()
            }
            MenuItemKind::Submenu(items) => {
                let flyout_open = flyout.is_some_and(|flyout| *flyout.open.read(cx) == Some(index));
                let label_color = self.row_color(selected, flyout_open, &env.chrome);
                let disabled = self.disabled;
                let row_bounds: TriggerBoundsCell = Rc::new(RefCell::new(None));

                self.row_shell(cx, index, selected, env)
                    .on_click({
                        let set_flyout = flyout.map(|flyout| flyout.set.clone());
                        move |_, _window, cx| {
                            if disabled {
                                return;
                            }
                            // Hover already opens the flyout; a click never closes it.
                            if let Some(set_flyout) = &set_flyout {
                                set_flyout(Some(index), false, cx);
                            }
                        }
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(label_color)
                            .child(self.label.clone()),
                    )
                    .child(
                        Icon::from(IconRole::ChevronRight)
                            .size(rems(0.75))
                            .color(label_color),
                    )
                    .aria_expanded(flyout_open)
                    .when(flyout_open, |row| {
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
                        .when_some(flyout, |row, flyout| {
                            row.child(
                                deferred(div().absolute().left_full().top_0().child(
                                    flyout.state.panel(
                                        window,
                                        cx,
                                        MenuPanelParams {
                                            id: &flyout.id,
                                            items: items.clone(),
                                            initial_selection: None,
                                            preselect: *flyout.preselect.read(cx),
                                            min_width: flyout.min_width,
                                            label: self.label.clone(),
                                            dismiss_bounds: row_bounds.clone(),
                                            bounds_sink: Some(&flyout.bounds),
                                            outer_stack: Some(&flyout.stack),
                                            on_dismiss: Some(flyout.dismiss.clone()),
                                            on_activate: flyout.on_activate.clone(),
                                            is_flyout: true,
                                            chrome: &env.chrome,
                                            enter_duration: flyout.enter_duration,
                                            debug_prefix: "submenu",
                                        },
                                    ),
                                ))
                                .with_priority(OVERLAY_LAYER),
                            )
                        })
                    })
                    .into_any_element()
            }
        }
    }

    /// The shared row shell: dimensions, highlight, and hover-selection.
    fn row_shell(&self, cx: &App, index: usize, selected: bool, env: &RowEnv) -> Stateful<Div> {
        let chrome = env.chrome.clone();
        let selectable = self.selectable();
        let opens_submenu = self.submenu_items().is_some();

        let flyout_open = env
            .flyout
            .as_ref()
            .is_some_and(|flyout| *flyout.open.read(cx) == Some(index));

        // Positional: rows never reorder while the panel is open.
        let row_id = self
            .id
            .clone()
            .unwrap_or_else(|| ("menu-item", index).into());

        let selection = env.selection.clone();
        let hover = env.flyout.as_ref().map(|flyout| {
            (
                flyout.open.clone(),
                flyout.set.clone(),
                flyout.dismiss.clone(),
            )
        });

        div()
            .id(row_id)
            .role(Role::MenuItem)
            .debug_selector(move || format!("{}-item-{index}", env.debug_prefix))
            .flex()
            .w_full()
            .items_center()
            .gap_2()
            .h_8()
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
            .on_mouse_move(move |_, window, cx| {
                if !selectable {
                    return;
                }

                if *selection.read(cx) != Some(index) {
                    selection.update(cx, |selection, _| *selection = Some(index));
                }

                if let Some((open, set_flyout, dismiss)) = &hover {
                    if opens_submenu {
                        set_flyout(Some(index), false, cx);
                    } else if open.read(cx).is_some() {
                        dismiss(false, window, cx);
                    }
                }
            })
    }

    /// The row's label/icon color.
    fn row_color(&self, selected: bool, flyout_open: bool, chrome: &MenuChrome) -> Hsla {
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
    border: Hsla,
    foreground: Hsla,
    muted_foreground: Hsla,
    muted_hover: Hsla,
    primary: Hsla,
    primary_foreground: Hsla,
    destructive: Hsla,
    destructive_foreground: Hsla,
    radius_full: Pixels,
}

impl MenuChrome {
    /// Reads the menu's chrome and enter duration from the active theme.
    pub(crate) fn read(cx: &App) -> (Self, Duration) {
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

    /// Binds the menu panel's navigation keys.
    pub fn bind_keys(cx: &mut App) {
        let context = Some(MENU_CONTEXT);
        cx.bind_keys([
            KeyBinding::new("down", SelectNext, context),
            KeyBinding::new("up", SelectPrevious, context),
            KeyBinding::new("home", SelectFirst, context),
            KeyBinding::new("end", SelectLast, context),
            KeyBinding::new("right", OpenSubmenu, context),
            KeyBinding::new("enter", Activate, context),
            KeyBinding::new("space", Activate, context),
            KeyBinding::new("escape", Dismiss, context),
            KeyBinding::new("left", DismissLeft, context),
            KeyBinding::new("tab", Leave, context),
            KeyBinding::new("shift-tab", Leave, context),
        ]);
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

impl Styled for Menu {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Menu {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let (chrome, enter_duration) = MenuChrome::read(cx);
        // Keyed so the panel can anchor to bounds captured by the last paint.
        let trigger_bounds: TriggerBoundsCell = window
            .use_keyed_state((self.id.clone(), "trigger-bounds"), cx, |_, _| {
                Rc::new(RefCell::new(None)) as TriggerBoundsCell
            })
            .read(cx)
            .clone();
        let on_change = self.on_change.clone();

        let panel_focus = ElementId::from((self.id.clone(), "panel")).focus_handle(window, cx);
        self.id
            .track_overlay_focus(&panel_focus, self.open, window, cx);

        let (items, label) = if self.open {
            let items = match self.items {
                Some(build) => Rc::from(build(window, cx)),
                None => Rc::from([]),
            };
            let label = self
                .a11y_label
                .clone()
                .unwrap_or_else(|| SharedString::from(self.id.to_string()));
            (items, label)
        } else {
            // A closed panel mounts no rows; it only resets its keyed state.
            (Rc::from([]), SharedString::default())
        };

        let spec = MenuPanelSpec {
            open: self.open,
            items,
            initial_selection: self.initial_selection,
            preselect: false,
            min_width: self.min_width,
            label,
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
                on_change,
                self.trigger_button,
            ))
            .when_some(self.trigger, |wrapper, trigger| {
                wrapper.child(trigger(window, cx))
            })
            .when_some(
                spec.anchored(window, cx, &self.id, &chrome, enter_duration),
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

impl MenuPanelSpec {
    pub(crate) fn anchored<'a>(
        self,
        window: &mut Window,
        cx: &mut App,
        id: &ElementId,
        chrome: &'a MenuChrome,
        enter_duration: Duration,
    ) -> Option<impl IntoElement + 'a> {
        let was_open = window.use_keyed_state((id.clone(), "menu-was-open"), cx, |_, _| false);

        if !self.open {
            if *was_open.read(cx) {
                was_open.update(cx, |was_open, _| *was_open = false);
                // A closed menu drops any open flyout; fresh state on the next open.
                window
                    .use_keyed_state::<Option<usize>>((id.clone(), "submenu-open"), cx, |_, _| None)
                    .update(cx, |open, _| *open = None);
                window
                    .use_keyed_state((id.clone(), "submenu-preselect"), cx, |_, _| false)
                    .update(cx, |preselect, _| *preselect = false);
                window
                    .use_keyed_state((id.clone(), "menu-painted"), cx, |_, _| false)
                    .update(cx, |painted, _| *painted = false);
            }
            return None;
        }

        let target = self
            .trigger_bounds
            .borrow()
            .map(|bounds| TriggerAnchor::below(window, bounds));
        let state = MenuPanelState::new(id, window, cx);
        let panel = state.panel(
            window,
            cx,
            MenuPanelParams {
                id,
                items: self.items,
                initial_selection: self.initial_selection,
                preselect: self.preselect,
                min_width: self.min_width,
                label: self.label,
                dismiss_bounds: self.trigger_bounds.clone(),
                bounds_sink: None,
                outer_stack: None,
                on_dismiss: self.on_dismiss,
                on_activate: self.on_activate,
                is_flyout: false,
                chrome,
                enter_duration,
                debug_prefix: "menu",
            },
        );

        Some(anchored_overlay(self.position, target, panel, None))
    }
}

/// One frame's inputs for a menu panel.
struct MenuPanelParams<'a> {
    id: &'a ElementId,
    items: Rc<[MenuItem]>,
    initial_selection: Option<usize>,
    preselect: bool,
    min_width: Option<DefiniteLength>,
    label: SharedString,
    /// Clicks here do not dismiss the panel: the trigger or the owning row.
    dismiss_bounds: TriggerBoundsCell,
    /// Receives the panel's painted bounds, for a parent's dismissal checks.
    bounds_sink: Option<&'a TriggerBoundsCell>,
    /// This panel's flyout bounds are pushed here.
    outer_stack: Option<&'a Rc<RefCell<Vec<TriggerBoundsCell>>>>,
    on_dismiss: Option<ChangeHandler>,
    on_activate: Option<ChangeHandler>,
    /// Flyout panels dismiss themselves on `left`; top-level panels ignore it.
    is_flyout: bool,
    chrome: &'a MenuChrome,
    enter_duration: Duration,
    debug_prefix: &'static str,
}

/// Per-panel state that survives across frames while the panel stays mounted.
#[derive(Clone)]
struct MenuPanelState {
    focus: FocusHandle,
    was_open: Entity<bool>,
    list: Entity<ListState>,
    painted: Entity<bool>,
    selection: Entity<Option<usize>>,
}

impl MenuPanelState {
    /// Creates one panel's keyed state: focus, open tracking, list, and selection.
    fn new(id: &ElementId, window: &mut Window, cx: &mut App) -> Self {
        Self {
            focus: ElementId::from((id.clone(), "panel")).focus_handle(window, cx),
            was_open: window.use_keyed_state((id.clone(), "menu-was-open"), cx, |_, _| false),
            list: window.use_keyed_state((id.clone(), "menu-list"), cx, |_, _| {
                ListState::new(0, ListAlignment::Top, px(32.))
            }),
            painted: window.use_keyed_state((id.clone(), "menu-painted"), cx, |_, _| false),
            selection: window.use_keyed_state((id.clone(), "menu-selection"), cx, |_, _| None),
        }
    }

    /// Builds one menu panel: chrome, rows, focus, navigation, and dismissals.
    fn panel(
        &self,
        window: &mut Window,
        cx: &mut App,
        params: MenuPanelParams,
    ) -> impl IntoElement {
        let flyout = params
            .items
            .iter()
            .any(|item| item.submenu_items().is_some())
            .then(|| Flyout::new(&params, &self.focus, window, cx));
        let flyout_stack = flyout.as_ref().map(|flyout| flyout.stack.clone());

        let MenuPanelParams {
            id: _,
            items,
            initial_selection,
            preselect,
            min_width,
            label,
            dismiss_bounds,
            bounds_sink,
            outer_stack: _,
            on_dismiss,
            on_activate,
            is_flyout,
            chrome,
            enter_duration,
            debug_prefix,
        } = params;

        // Render-phase residue: panels open from owner state alone.
        if !*self.was_open.read(cx) {
            let requested = initial_selection
                .map(|index| index.min(items.len().saturating_sub(1)))
                .or_else(|| {
                    preselect
                        .then(|| items.iter().position(MenuItem::selectable))
                        .flatten()
                });
            self.selection
                .update(cx, |selection, _| *selection = requested);
            self.was_open.update(cx, |was_open, _| *was_open = true);
        }

        if self.list.read(cx).item_count() != items.len() {
            self.list.update(cx, |list, _| list.reset(items.len()));
        }

        let keys = Rc::new(MenuKeys {
            items: items.clone(),
            selection: self.selection.clone(),
            set_flyout: flyout.as_ref().map(|flyout| flyout.set.clone()),
            on_dismiss: on_dismiss.clone(),
            on_activate: on_activate.clone(),
            left_dismisses: is_flyout,
        });

        let row_env = RowEnv {
            items,
            chrome: chrome.clone(),
            selection: self.selection.clone(),
            on_activate,
            flyout,
            debug_prefix,
        };

        let rows = list(self.list.read(cx).clone(), move |index, window, cx| {
            let selected = *row_env.selection.read(cx) == Some(index);
            row_env
                .items
                .get(index)
                .map(|item| item.render_item(window, cx, index, selected, &row_env))
                .unwrap_or_else(|| div().into_any_element())
        });

        v_flex()
            .id("menu-panel")
            .debug_selector(move || format!("{}-panel", debug_prefix))
            .occlude()
            .track_focus(&self.focus)
            .key_context(MENU_CONTEXT)
            .role(Role::Menu)
            .aria_label(label)
            .when_some(min_width, |panel, width| panel.min_w(width))
            .when(min_width.is_none(), |panel| panel.min_w(MENU_MIN_WIDTH))
            .p_1()
            .text_color(chrome.foreground)
            .floating_panel(
                cx,
                cx.theme().radius.overlay,
                cx.theme().elevations.popover.shadows(),
            )
            .overflow_hidden()
            .on_mouse_down_out({
                let on_dismiss = on_dismiss.clone();
                let dismiss_bounds = dismiss_bounds.clone();
                let flyout_stack = flyout_stack.clone();

                move |event, window, cx| {
                    if dismiss_bounds
                        .borrow()
                        .is_some_and(|bounds| bounds.contains(&event.position))
                    {
                        return;
                    }

                    // Clicks inside any open flyout belong to that flyout.
                    if flyout_stack.as_ref().is_some_and(|stack| {
                        stack.borrow().iter().any(|cell| {
                            cell.borrow()
                                .is_some_and(|bounds| bounds.contains(&event.position))
                        })
                    }) {
                        return;
                    }

                    if let Some(on_dismiss) = &on_dismiss {
                        on_dismiss(false, window, cx);
                    }
                }
            })
            .on_action({
                let keys = keys.clone();
                move |_: &SelectNext, _, cx| keys.step(true, cx)
            })
            .on_action({
                let keys = keys.clone();
                move |_: &SelectPrevious, _, cx| keys.step(false, cx)
            })
            .on_action({
                let keys = keys.clone();
                move |_: &SelectFirst, _, cx| keys.jump(true, cx)
            })
            .on_action({
                let keys = keys.clone();
                move |_: &SelectLast, _, cx| keys.jump(false, cx)
            })
            .on_action({
                let keys = keys.clone();
                move |_: &OpenSubmenu, _, cx| keys.open_submenu(cx)
            })
            .on_action({
                let keys = keys.clone();
                move |_: &Activate, window, cx| keys.activate(window, cx)
            })
            .on_action({
                let keys = keys.clone();
                move |_: &Dismiss, window, cx| keys.dismiss(window, cx)
            })
            .on_action({
                let keys = keys.clone();
                move |_: &DismissLeft, window, cx| keys.dismiss_left(window, cx)
            })
            // Tab leaves the panel and lets outer bindings move focus from there.
            .on_action({
                let keys = keys.clone();
                move |_: &Leave, window, cx| {
                    keys.dismiss(window, cx);
                    cx.propagate();
                }
            })
            .child(
                v_flex()
                    .max_h(POPUP_MAX_HEIGHT)
                    .min_h_0()
                    .child(rows.with_sizing_behavior(ListSizingBehavior::Infer)),
            )
            .child(
                canvas(
                    {
                        let bounds_sink = bounds_sink.cloned();
                        move |bounds, _window, _cx| {
                            if let Some(cell) = &bounds_sink {
                                *cell.borrow_mut() = Some(bounds);
                            }
                        }
                    },
                    {
                        let painted = self.painted.clone();
                        let focus = self.focus.clone();
                        move |_, _, window, cx| {
                            let first =
                                painted.update(cx, |painted, _| mem::replace(painted, true));
                            if first {
                                window.defer(cx, move |window, cx| window.focus(&focus, cx));
                            }
                        }
                    },
                )
                .absolute()
                .inset_0(),
            )
            .with_animation(
                "menu-enter",
                Animation::new(enter_duration).with_easing(ease_out_quint()),
                |panel, delta| panel.opacity(delta),
            )
    }
}

/// One panel's submenu wiring.
#[derive(Clone)]
struct Flyout {
    state: MenuPanelState,
    open: Entity<Option<usize>>,
    preselect: Entity<bool>,
    set: FlyoutSetter,
    bounds: TriggerBoundsCell,
    /// Bounds of every open flyout below this panel, this level's included.
    stack: Rc<RefCell<Vec<TriggerBoundsCell>>>,
    /// Closes this level's flyout and returns focus to this panel.
    dismiss: ChangeHandler,
    on_activate: Option<ChangeHandler>,
    id: ElementId,
    enter_duration: Duration,
    min_width: Option<DefiniteLength>,
}

impl Flyout {
    /// Wires the flyout of the panel built from `params`, returning focus to `focus`.
    fn new(
        params: &MenuPanelParams,
        focus: &FocusHandle,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        let id: ElementId = (params.id.clone(), "submenu").into();
        let state = MenuPanelState::new(&id, window, cx);
        let open = window.use_keyed_state((params.id.clone(), "submenu-open"), cx, |_, _| None);
        let preselect =
            window.use_keyed_state((params.id.clone(), "submenu-preselect"), cx, |_, _| false);
        let bounds: TriggerBoundsCell = Rc::new(RefCell::new(None));
        let stack: Rc<RefCell<Vec<TriggerBoundsCell>>> = Rc::new(RefCell::new(Vec::new()));
        stack.borrow_mut().push(bounds.clone());

        if let Some(outer_stack) = params.outer_stack {
            outer_stack.borrow_mut().push(bounds.clone());
        }

        // Every open flip re-arms a clean flyout, so render never reconciles one.
        let set: FlyoutSetter = {
            let open = open.clone();
            let preselect = preselect.clone();
            let was_open = state.was_open.clone();
            let selection = state.selection.clone();

            Rc::new(move |index, opens_preselected, cx| {
                if index.is_some() && *open.read(cx) == index {
                    return;
                }

                open.update(cx, |open, _| *open = index);
                preselect.update(cx, |preselect, _| *preselect = opens_preselected);
                was_open.update(cx, |was_open, _| *was_open = false);
                selection.update(cx, |selection, _| *selection = None);
            })
        };

        let dismiss: ChangeHandler = {
            let set = set.clone();
            let focus = focus.clone();
            Rc::new(move |_, window, cx| {
                set(None, false, cx);
                window.focus(&focus, cx);
            })
        };

        Self {
            state,
            open,
            preselect,
            set,
            bounds,
            stack,
            dismiss,
            on_activate: params.on_activate.clone(),
            id,
            enter_duration: params.enter_duration,
            min_width: params.min_width,
        }
    }
}

/// What one frame's rows read: their items, chrome, selection, and flyout.
#[derive(Clone)]
struct RowEnv {
    items: Rc<[MenuItem]>,
    chrome: MenuChrome,
    selection: Entity<Option<usize>>,
    on_activate: Option<ChangeHandler>,
    flyout: Option<Flyout>,
    debug_prefix: &'static str,
}

/// Everything key navigation needs from the panel.
struct MenuKeys {
    items: Rc<[MenuItem]>,
    selection: Entity<Option<usize>>,
    set_flyout: Option<FlyoutSetter>,
    on_dismiss: Option<ChangeHandler>,
    on_activate: Option<ChangeHandler>,
    left_dismisses: bool,
}

impl MenuKeys {
    /// The indices of the rows navigation may land on.
    fn selectable(&self) -> Vec<usize> {
        self.items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.selectable())
            .map(|(index, _)| index)
            .collect()
    }

    fn select(&self, next: Option<usize>, cx: &mut App) {
        if let Some(next) = next {
            self.selection.update(cx, |selection, _| {
                *selection = Some(next);
            });
        }
    }

    /// Moves the selection one selectable row in the direction, wrapping at the ends.
    fn step(&self, forward: bool, cx: &mut App) {
        let selectable = self.selectable();

        if selectable.is_empty() {
            cx.propagate();
            return;
        }

        self.select(
            selectable.wrapping_adjacent(*self.selection.read(cx), forward),
            cx,
        );
    }

    /// Moves the selection to the first or last selectable row.
    fn jump(&self, first: bool, cx: &mut App) {
        let selectable = self.selectable();

        if selectable.is_empty() {
            cx.propagate();
            return;
        }

        let target = if first {
            selectable.first().copied()
        } else {
            selectable.last().copied()
        };

        self.select(target, cx);
    }

    /// Whether the row at `index` is a selectable submenu.
    fn opens_submenu(&self, index: usize) -> bool {
        self.items
            .get(index)
            .is_some_and(|item| item.selectable() && item.submenu_items().is_some())
    }

    /// Opens the selected row's flyout with its first row armed.
    fn open_submenu(&self, cx: &mut App) {
        let current = *self.selection.read(cx);
        let Some((current, set_flyout)) = current
            .filter(|index| self.opens_submenu(*index))
            .zip(self.set_flyout.as_ref())
        else {
            cx.propagate();
            return;
        };

        set_flyout(Some(current), true, cx);
    }

    /// Runs the selected row's action, or opens its flyout.
    fn activate(&self, window: &mut Window, cx: &mut App) {
        let Some(current) = *self.selection.read(cx) else {
            cx.propagate();
            return;
        };

        let Some(item) = self.items.get(current).filter(|item| item.selectable()) else {
            cx.propagate();
            return;
        };

        if item.submenu_items().is_some() {
            if let Some(set_flyout) = &self.set_flyout {
                set_flyout(Some(current), true, cx);
            }
            return;
        }

        if let Some(on_activate) = &self.on_activate {
            on_activate(false, window, cx);
        }

        if let Some(handler) = &item.handler {
            handler(window, cx);
        }
    }

    /// Closes this panel.
    fn dismiss(&self, window: &mut Window, cx: &mut App) {
        if let Some(on_dismiss) = &self.on_dismiss {
            on_dismiss(false, window, cx);
        } else {
            cx.propagate();
        }
    }

    /// Closes a flyout on `left`; a top-level panel ignores the key.
    fn dismiss_left(&self, window: &mut Window, cx: &mut App) {
        if !self.left_dismisses {
            cx.propagate();
            return;
        }
        self.dismiss(window, cx);
    }
}

/// Canvas that records the trigger's painted bounds and toggles `open` on click.
pub(crate) fn trigger_toggle(
    bounds: &TriggerBoundsCell,
    open: bool,
    on_change: Option<ChangeHandler>,
    button: MouseButton,
) -> impl IntoElement {
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
}

/// Where an overlay hangs off its trigger.
#[derive(Clone, Copy)]
pub(crate) struct TriggerAnchor {
    target: Bounds<Pixels>,
    /// The panel's corner that meets the trigger.
    anchor: Anchor,
    /// The trigger's corner the panel hangs from.
    corner: Anchor,
}

impl TriggerAnchor {
    /// Hangs the panel below the trigger, aligned to the trigger edge nearer the window's center.
    pub(crate) fn below(window: &Window, target: Bounds<Pixels>) -> Self {
        if target.center().x > window.viewport_size().width / 2.0 {
            Self {
                target,
                anchor: Anchor::TopRight,
                corner: Anchor::BottomRight,
            }
        } else {
            Self {
                target,
                anchor: Anchor::TopLeft,
                corner: Anchor::BottomLeft,
            }
        }
    }
}

/// Deferred anchored overlay on OVERLAY_LAYER, optionally fading in.
pub(crate) fn anchored_overlay<E: IntoElement>(
    position: Option<Point<Pixels>>,
    trigger: Option<TriggerAnchor>,
    panel: E,
    fade: Option<(&'static str, Duration)>,
) -> AnyElement {
    let wrapper = match fade {
        Some((id, duration)) => div()
            .with_animation(
                id,
                Animation::new(duration).with_easing(ease_out_quint()),
                |wrapper, delta| wrapper.opacity(delta),
            )
            .child(panel)
            .into_any_element(),
        None => div().child(panel).into_any_element(),
    };

    let overlay = match (position, trigger) {
        (Some(position), _) => anchored().position(position),
        (None, Some(trigger)) => anchored()
            .anchor(trigger.anchor)
            .attach(trigger.target, trigger.corner),
        (None, None) => anchored(),
    };

    deferred(overlay.offset(point(px(0.), px(4.))).child(wrapper))
        .with_priority(OVERLAY_LAYER)
        .into_any_element()
}

/// Canvas that grabs focus when `should_grab` holds.
pub(crate) fn focus_capture(
    focus: FocusHandle,
    should_grab: impl Fn(&mut Window, &mut App) -> bool + 'static,
) -> impl IntoElement {
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
}
