//! Renders a push button with an attached menu of actions.
//!
//! # Examples
//!
//! ```no_run
//! use ui::dropdown_menu::DropdownMenu;
//! use ui::menu::MenuItem;
//!
//! let export = DropdownMenu::new("export")
//!     .label("Export")
//!     .on_open_change(|open, _window, _cx| println!("menu open: {open}"))
//!     .items(|_window, _cx| vec![MenuItem::new("PDF"), MenuItem::new("HTML")]);
//! ```

use std::cell::RefCell;
use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, ClickEvent, DefiniteLength, DispatchPhase, ElementId, InteractiveElement as _,
    IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, ParentElement as _, RenderOnce, Role,
    SharedString, StatefulInteractiveElement as _, StyleRefinement, Styled as _, Window, canvas,
    div, relative,
};

use crate::components::button::ButtonVariant;
use crate::components::common::{Sizable, StyledExt as _, focus_handle, focus_ring, h_flex};
use crate::components::menu::{
    ChangeHandler, MenuChrome, MenuItem, MenuPanelSpec, TriggerBoundsCell, anchored_menu_panel,
};
use crate::components::spinner::Spinner;
use crate::icons::{Icon, IconRole, icon};
use crate::theme::{ActiveTheme as _, DISABLED_OPACITY, Size};

/// The main zone's activation handler, shared by pointer, Enter, and Space.
type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
type ItemsFn = Box<dyn Fn(&mut Window, &mut App) -> Vec<MenuItem>>;

/// A push button with an attached dropdown menu.
#[derive(IntoElement)]
#[must_use]
pub struct DropdownMenu {
    id: ElementId,
    label: Option<SharedString>,
    icon: Option<Icon>,
    variant: ButtonVariant,
    size: Size,
    disabled: bool,
    loading: bool,
    tab_stop: bool,
    open: bool,
    a11y_label: Option<SharedString>,
    on_click: Option<ClickHandler>,
    on_open_change: Option<ChangeHandler>,
    items: Option<ItemsFn>,
    min_width: Option<DefiniteLength>,
    style: StyleRefinement,
}

impl DropdownMenu {
    /// Creates a closed dropdown menu rendered as a primary button.
    pub fn new<I>(id: I) -> Self
    where
        I: Into<ElementId>,
    {
        Self {
            id: id.into(),
            label: None,
            icon: None,
            variant: ButtonVariant::default(),
            size: Size::default(),
            disabled: false,
            loading: false,
            tab_stop: true,
            open: false,
            a11y_label: None,
            on_click: None,
            on_open_change: None,
            items: None,
            min_width: None,
            style: StyleRefinement::default(),
        }
    }

    /// Sets the visible label.
    pub fn label<L>(mut self, label: L) -> Self
    where
        L: Into<SharedString>,
    {
        self.label = Some(label.into());
        self
    }

    /// Shows an icon. Omitting the label renders an icon-only trigger.
    pub fn icon<I>(mut self, icon: I) -> Self
    where
        I: Into<Icon>,
    {
        self.icon = Some(icon.into());
        self
    }

    /// Sets the button's visual language.
    pub fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Sets whether the trigger is a keyboard tab stop. Dense toolbars turn it off.
    pub fn tab_stop(mut self, tab_stop: bool) -> Self {
        self.tab_stop = tab_stop;
        self
    }

    /// Renders the trigger inert, unfocusable, and dimmed.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Shows the spinner in place of the content and suppresses activation.
    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    /// Sets the current open state.
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// Overrides the accessibility label, which defaults to the visible label.
    pub fn accessibility_label<L>(mut self, label: L) -> Self
    where
        L: Into<SharedString>,
    {
        self.a11y_label = Some(label.into());
        self
    }

    /// Sets the main zone's action. Without one, activating the button opens the menu.
    pub fn on_click<F>(mut self, handler: F) -> Self
    where
        F: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    {
        self.on_click = Some(Box::new(handler));
        self
    }

    /// Reports the menu's next open state.
    pub fn on_open_change<F>(mut self, handler: F) -> Self
    where
        F: Fn(bool, &mut Window, &mut App) + 'static,
    {
        self.on_open_change = Some(Rc::new(handler));
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

    /// Sets the panel's minimum width, defaults to the standard menu width.
    pub fn min_width<W>(mut self, width: W) -> Self
    where
        W: Into<DefiniteLength>,
    {
        self.min_width = Some(width.into());
        self
    }
}

impl Sizable for DropdownMenu {
    fn apply_size(&mut self, size: Size) {
        self.size = size;
    }
}

impl gpui::Styled for DropdownMenu {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for DropdownMenu {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let colors = theme.colors;
        let ramps = theme.ramps;
        let metrics = theme.sizes.resolve(self.size);
        let radius = theme.radius.control;

        let ring = colors.ring;

        let (panel_chrome, enter_duration) = MenuChrome::read(cx);
        let (background, foreground, hover, pressed) = match self.variant {
            ButtonVariant::Primary => (
                Some(ramps.primary.base),
                colors.primary_foreground,
                ramps.primary.hover,
                ramps.primary.pressed,
            ),
            ButtonVariant::Secondary => (
                Some(colors.surface),
                colors.foreground,
                colors.muted,
                colors.muted_hover,
            ),
            ButtonVariant::Ghost => (
                None,
                colors.foreground,
                colors.muted_hover,
                ramps.secondary.pressed,
            ),
            ButtonVariant::Destructive => (
                Some(ramps.destructive.base),
                colors.destructive_foreground,
                ramps.destructive.hover,
                ramps.destructive.pressed,
            ),
        };

        let border = match self.variant {
            ButtonVariant::Primary => ramps.primary.base,
            ButtonVariant::Secondary => colors.border,
            ButtonVariant::Ghost => gpui::transparent_black(),
            ButtonVariant::Destructive => ramps.destructive.base,
        };

        let split = self.on_click.is_some();
        let icon_only = self.icon.is_some() && self.label.is_none() && !split;
        let inert = self.disabled || self.loading;

        let label = self
            .a11y_label
            .clone()
            .unwrap_or_else(|| SharedString::from(self.id.to_string()));

        let a11y_label = self.a11y_label.or_else(|| self.label.clone());
        let focus = focus_handle(&self.id, window, cx).tab_stop(self.tab_stop);
        // How the menu was last opened, read by the frame that mounts the panel.
        let keyboard_open =
            window.use_keyed_state((self.id.clone(), "dropdown-open-origin"), cx, |_, _| false);

        let on_open: ChangeHandler = {
            let focus = focus.clone();
            let on_open_change = self.on_open_change.clone();
            let keyboard_open = keyboard_open.clone();
            Rc::new(move |next, window, cx| {
                if !next {
                    window.focus(&focus, cx);
                    keyboard_open.update(cx, |keyboard_open, _| *keyboard_open = false);
                }
                if let Some(on_open_change) = &on_open_change {
                    on_open_change(next, window, cx);
                }
            })
        };

        let trigger_bounds: TriggerBoundsCell = Rc::new(RefCell::new(None));
        let chevron_bounds: TriggerBoundsCell = Rc::new(RefCell::new(None));

        let main = h_flex()
            .id(self.id.clone())
            .debug_selector({
                let id = self.id.clone();
                move || id.to_string()
            })
            .justify_center()
            .h_full()
            .when(icon_only, |zone| zone.w_full())
            .gap_2()
            .line_height(relative(1.0))
            .text_size(metrics.text_size)
            .text_color(foreground)
            .role(Role::Button)
            .when_some(a11y_label, |zone, label| zone.aria_label(label))
            .aria_expanded(self.open)
            .px(if icon_only {
                gpui::Pixels::ZERO
            } else {
                metrics.padding_x
            })
            .when(split, |zone| zone.rounded_l(radius))
            .when(!split, |zone| {
                zone.rounded(radius)
                    .when(self.open, |zone| zone.bg(pressed))
            })
            .when(!icon_only && !split, |zone| {
                zone.child(
                    icon(IconRole::ChevronDown)
                        .size(metrics.dense_icon_size)
                        .color(foreground),
                )
            })
            .child(
                h_flex()
                    .gap_2()
                    .when(self.loading, |content| content.invisible())
                    .when_some(self.icon, |content, icon| {
                        content.child(icon.size(metrics.icon_size).color(foreground))
                    })
                    .when_some(self.label, |content, label| content.child(label)),
            )
            .when(self.loading, |zone| {
                zone.child(
                    h_flex().absolute().inset_0().justify_center().child(
                        Spinner::new((self.id.clone(), "spinner"))
                            .with_size(self.size)
                            .variant(crate::components::spinner::SpinnerVariant::Icon)
                            .color(foreground),
                    ),
                )
            });

        let main = if inert {
            main
        } else {
            main.track_focus(&focus)
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
                .active(move |style| style.bg(pressed))
                .on_click({
                    let on_click = self.on_click;
                    let on_open = on_open.clone();
                    let keyboard_open = keyboard_open.clone();
                    let open = self.open;
                    move |event, window, cx| match (on_click.as_ref(), event) {
                        (Some(on_click), _) => on_click(event, window, cx),
                        (None, ClickEvent::Keyboard(_)) => {
                            keyboard_open.update(cx, |keyboard_open, _| *keyboard_open = true);
                            on_open(!open, window, cx);
                        }
                        (None, _) => {}
                    }
                })
                .on_key_down({
                    let on_open = on_open.clone();
                    let keyboard_open = keyboard_open.clone();
                    move |event: &KeyDownEvent, window, cx| {
                        if event.keystroke.key == "down" {
                            keyboard_open.update(cx, |keyboard_open, _| *keyboard_open = true);
                            on_open(true, window, cx);
                            cx.stop_propagation();
                        }
                    }
                })
        };

        let chevron_zone = h_flex()
            .id((self.id.clone(), "chevron"))
            .debug_selector({
                let id = self.id.clone();
                move || format!("{id}-chevron")
            })
            .relative()
            .h_full()
            .px(metrics.dense_padding_x)
            .rounded_r(radius)
            .rounded_l_none()
            .border_l_1()
            .border_color(border)
            .when(self.open, |zone| zone.bg(pressed))
            .when(!inert, |zone| {
                zone.cursor_pointer()
                    .hover(move |style| style.bg(hover))
                    .active(move |style| style.bg(pressed))
            })
            .child(
                icon(IconRole::ChevronDown)
                    .size(metrics.dense_icon_size)
                    .color(foreground),
            )
            .child(
                canvas(
                    {
                        let cell = chevron_bounds.clone();
                        move |bounds, _window, _cx| *cell.borrow_mut() = Some(bounds)
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            );

        let trigger = h_flex()
            .debug_selector({
                let id = self.id.clone();
                move || format!("{id}-trigger")
            })
            .h(metrics.control_height)
            .when(icon_only, |trigger| trigger.w(metrics.control_height))
            .rounded(radius)
            .border_1()
            .border_color(border)
            .when_some(background, |trigger, background| trigger.bg(background))
            .when(self.disabled, |trigger| trigger.opacity(DISABLED_OPACITY))
            .when(!inert, |trigger| {
                trigger
                    .track_focus(&focus)
                    .focus_visible(move |style| style.shadow(focus_ring(ring)))
            })
            .child(main);

        let trigger = if split {
            trigger.child(chevron_zone)
        } else {
            trigger
        };

        let items = if self.open {
            self.items
                .map(|build| Rc::from(build(window, cx)))
                .unwrap_or_else(|| Rc::from([]))
        } else {
            Rc::from([])
        };

        let on_close = Some(on_open.clone());

        let spec = MenuPanelSpec {
            open: self.open,
            items,
            initial_selection: None,
            preselect: *keyboard_open.read(cx),
            min_width: self.min_width,
            label,
            position: None,
            trigger_bounds: trigger_bounds.clone(),
            on_dismiss: on_close.clone(),
            on_activate: on_close,
        };

        div()
            .relative()
            .refine_style(&self.style)
            .child(
                canvas(
                    {
                        let cell = trigger_bounds.clone();
                        move |bounds, _window, _cx| *cell.borrow_mut() = Some(bounds)
                    },
                    {
                        let on_open = on_open.clone();
                        let trigger_bounds = trigger_bounds.clone();
                        let chevron_bounds = chevron_bounds.clone();
                        let keyboard_open = keyboard_open.clone();
                        let open = self.open;

                        move |_, _, window, _cx| {
                            window.on_mouse_event({
                                let on_open = on_open.clone();
                                let keyboard_open = keyboard_open.clone();
                                move |event: &MouseDownEvent, phase, window, cx| {
                                    if phase != DispatchPhase::Bubble
                                        || event.button != MouseButton::Left
                                        || inert
                                    {
                                        return;
                                    }

                                    let in_trigger = trigger_bounds
                                        .borrow()
                                        .is_some_and(|bounds| bounds.contains(&event.position));

                                    if !in_trigger {
                                        return;
                                    }

                                    let in_chevron = chevron_bounds
                                        .borrow()
                                        .is_some_and(|bounds| bounds.contains(&event.position));

                                    if !open && split && !in_chevron {
                                        return;
                                    }

                                    keyboard_open
                                        .update(cx, |keyboard_open, _| *keyboard_open = false);
                                    on_open(!open, window, cx);
                                    cx.stop_propagation();
                                }
                            });
                        }
                    },
                )
                .absolute()
                .inset_0(),
            )
            .child(trigger)
            .when_some(
                anchored_menu_panel(window, cx, &self.id, spec, &panel_chrome, enter_duration),
                |wrapper, panel| wrapper.child(panel),
            )
    }
}
