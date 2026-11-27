//! Renders a control that shows the chosen value and opens a menu of the choices.
//!
//! # Examples
//!
//! ```no_run
//! use ui::select::{Select, SelectOption};
//!
//! let theme = Select::new("theme")
//!     .options([
//!         SelectOption::new("dark", "Dark"),
//!         SelectOption::new("light", "Light"),
//!     ])
//!     .selected("dark")
//!     .on_change(|value, _window, _cx| println!("{value:?}"));
//! ```

use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    App, ElementId, InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _,
    RenderOnce, Role, SharedString, StatefulInteractiveElement as _, StyleRefinement, Styled,
    Window, rems,
};

use crate::components::common::{ControlSizing, ElementIdExt as _, Sizable, StyledExt, h_flex};
use crate::components::menu::{Menu, MenuItem};
use crate::icons::{Icon, IconRole};
use crate::theme::{ActiveTheme as _, Size};

/// Reports the value the user chose. `None` means the select holds no value.
type SelectHandler<T> = Rc<dyn Fn(Option<T>, &mut Window, &mut App)>;

/// Select option for a [`Select`] control.
#[derive(Clone)]
#[must_use]
pub struct SelectOption<T> {
    value: T,
    label: SharedString,
    icon: Option<Icon>,
}

impl<T> SelectOption<T> {
    /// Creates a choice showing `label`; picking it reports `value`.
    pub fn new<L>(value: T, label: L) -> Self
    where
        L: Into<SharedString>,
    {
        Self {
            value,
            label: label.into(),
            icon: None,
        }
    }

    /// Sets a leading icon for the option's row.
    pub fn icon<I>(mut self, icon: I) -> Self
    where
        I: Into<Icon>,
    {
        self.icon = Some(icon.into());
        self
    }
}

impl<T, L> From<(T, L)> for SelectOption<T>
where
    L: Into<SharedString>,
{
    fn from((value, label): (T, L)) -> Self {
        Self {
            value,
            label: label.into(),
            icon: None,
        }
    }
}

// SelectOption is a data builder, not an element, adopt the combinators explicitly.
impl<T> FluentBuilder for SelectOption<T> {}

/// A control that shows the chosen value and opens a menu of the choices.
#[derive(IntoElement)]
#[must_use]
pub struct Select<T>
where
    T: Clone + PartialEq + 'static,
{
    id: ElementId,
    options: Vec<SelectOption<T>>,
    selected: Option<T>,
    placeholder: SharedString,
    a11y_label: Option<SharedString>,
    size: Size,
    style: StyleRefinement,
    on_change: Option<SelectHandler<T>>,
}

impl<T> Select<T>
where
    T: Clone + PartialEq + 'static,
{
    /// Creates a closed select holding no value.
    pub fn new<I>(id: I) -> Self
    where
        I: Into<ElementId>,
    {
        Self {
            id: id.into(),
            options: Vec::new(),
            selected: None,
            placeholder: SharedString::from("Select..."),
            a11y_label: None,
            size: Size::default(),
            style: StyleRefinement::default(),
            on_change: None,
        }
    }

    /// Sets the choice.
    pub fn options<I>(mut self, options: I) -> Self
    where
        I: IntoIterator,
        I::Item: Into<SelectOption<T>>,
    {
        self.options = options.into_iter().map(Into::into).collect();
        self
    }

    /// Sets the current value. `None` shows the placeholder.
    pub fn selected<S>(mut self, selected: S) -> Self
    where
        S: Into<Option<T>>,
    {
        self.selected = selected.into();
        self
    }

    /// Sets the text shown while no value is chosen.
    pub fn placeholder<L>(mut self, placeholder: L) -> Self
    where
        L: Into<SharedString>,
    {
        self.placeholder = placeholder.into();
        self
    }

    /// Reports the chosen value, when the user activates an option.
    pub fn on_change<F>(mut self, handler: F) -> Self
    where
        F: Fn(Option<T>, &mut Window, &mut App) + 'static,
    {
        self.on_change = Some(Rc::new(handler));
        self
    }

    /// Overrides the trigger's accessibility label.
    pub fn accessibility_label<L>(mut self, label: L) -> Self
    where
        L: Into<SharedString>,
    {
        self.a11y_label = Some(label.into());
        self
    }
}

impl<T> Sizable for Select<T>
where
    T: Clone + PartialEq + 'static,
{
    fn apply_size(&mut self, size: Size) {
        self.size = size;
    }
}

impl<T> Styled for Select<T>
where
    T: Clone + PartialEq + 'static,
{
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl<T> RenderOnce for Select<T>
where
    T: Clone + PartialEq + 'static,
{
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let border = theme.colors.border;
        let ring = theme.colors.ring;
        let input = theme.colors.input;
        let muted = theme.colors.muted;
        let foreground = theme.colors.foreground;
        let muted_foreground = theme.colors.muted_foreground;
        let radius = theme.radius.control;

        let label = self
            .a11y_label
            .clone()
            .unwrap_or_else(|| self.placeholder.clone());

        let open = window.with_element_namespace(self.id.clone(), |window| {
            window
                .use_keyed_state("select-open", cx, |_, _| false)
                .clone()
        });

        let focus = self.id.focus_handle(window, cx);
        let is_open = *open.read(cx);
        let open_for_items = open.clone();

        let rest_border = if is_open { ring } else { border };

        let selected_index = self.selected.as_ref().and_then(|value| {
            self.options
                .iter()
                .position(|option| &option.value == value)
        });

        // The trigger's text is fixed at render time; only it moves into the closure.
        let trigger_text = selected_index
            .and_then(|index| self.options.get(index))
            .map(|option| option.label.clone())
            .unwrap_or_else(|| self.placeholder.clone());

        let trigger_has_value = self.selected.is_some();
        let trigger_id = self.id.clone();
        let trigger_label = label.clone();
        let trigger_style = self.style;

        let trigger = {
            let open = open.clone();
            let focus = focus.clone();

            move |_window: &mut Window, _cx: &mut App| {
                h_flex()
                    .id((trigger_id.clone(), "select-trigger"))
                    .debug_selector(|| "select-trigger".to_string())
                    .role(Role::ComboBox)
                    .aria_label(trigger_label.clone())
                    .aria_expanded(is_open)
                    .track_focus(&focus)
                    .tab_stop(true)
                    .focus_visible(move |style| style.border_color(ring))
                    .on_key_down({
                        let open = open.clone();
                        move |event: &KeyDownEvent, _window, cx| {
                            if !matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                return;
                            }

                            open.update(cx, |open, _| *open = !*open);
                            cx.stop_propagation();
                        }
                    })
                    .cursor_pointer()
                    .justify_between()
                    .gap_3()
                    .w_full()
                    .control_height(self.size)
                    .control_dense_padding_x(self.size)
                    .pr(rems(self.size.plate_inset_rems().0 * 2.0))
                    .rounded(radius)
                    .border_1()
                    .border_color(rest_border)
                    .bg(input)
                    .control_text(self.size)
                    .text_color(if trigger_has_value {
                        foreground
                    } else {
                        muted_foreground
                    })
                    .hover(|style| style.bg(muted).border_color(rest_border))
                    .child(trigger_text.clone())
                    .child(
                        Icon::from(IconRole::ChevronDown)
                            .size(self.size.dense_icon_rems())
                            .color(muted_foreground),
                    )
                    .refine_style(&trigger_style)
                    .into_any_element()
            }
        };

        let items = move |_window: &mut Window, _cx: &mut App| {
            self.options
                .iter()
                .enumerate()
                .map(|(index, option)| {
                    let is_selected = self.selected.as_ref() == Some(&option.value);

                    let handler = {
                        let open = open_for_items.clone();
                        let value = option.value.clone();
                        let on_change = self.on_change.clone();
                        move |window: &mut Window, cx: &mut App| {
                            open.update(cx, |open, _| *open = false);
                            if let Some(on_change) = &on_change {
                                on_change(Some(value.clone()), window, cx);
                            }
                        }
                    };

                    MenuItem::new(option.label.clone())
                        .id(("option", index))
                        .when_some(option.icon.clone(), |row, icon| row.icon(icon))
                        .checked(is_selected)
                        .handler(handler)
                })
                .collect()
        };

        Menu::new(self.id)
            .open(is_open)
            .on_change({
                let open = open.clone();
                move |next, _window, cx| {
                    open.update(cx, |open, _| *open = next);
                }
            })
            .trigger(trigger)
            .items(items)
            .when_some(selected_index, |menu, index| menu.initial_selection(index))
    }
}
