//! Renders an editable field that filters its options while typing.
//!
//! The state is the view: [`ComboboxState`] owns the query field and the
//! options, and the owner renders the entity itself. Typing filters, the
//! arrow keys move a highlight, Enter commits, and what happens to unmatched
//! text when the field loses focus is decided by [`UnmatchedText`], which
//! reverts by default.
//!
//! # Examples
//!
//! ```no_run
//! use gpui::prelude::*;
//! use gpui::{App, Entity, Window};
//! use ui::combobox::{Combobox, ComboboxState};
//!
//! # fn build(window: &mut Window, cx: &mut App) -> Entity<ComboboxState<&'static str>> {
//! let picker = Combobox::new(
//!     "theme",
//!     "Theme",
//!     vec![("dark", "Dark".into()), ("light", "Light".into())],
//! )
//! .on_change(|value, _window, _cx| println!("{value:?}"));
//! let state = cx.new(|cx| ComboboxState::new(picker, window, cx));
//! # state
//! # }
//! ```

use std::cell::RefCell;
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    App, Bounds, Context, ElementId, Entity, InteractiveElement as _, IntoElement, KeyBinding,
    MouseButton, MouseDownEvent, ParentElement as _, Pixels, Render, Role, ScrollHandle,
    SharedString, StatefulInteractiveElement as _, StyleRefinement, Styled, Subscription, Task,
    Window, actions, anchored, canvas, deferred, div, point, px,
};

use crate::components::common::{
    ControlSizing, OVERLAY_LAYER, ResultLogErr as _, Sizable, StyledExt as _, h_flex, v_flex,
};
use crate::components::input::{Input, InputState, InputStateEvent};
use crate::components::scroll_area::ScrollArea;
use crate::icons::{IconRole, icon};
use crate::theme::{ActiveTheme as _, COMBOBOX_MIN_WIDTH, COMBOBOX_WIDTH, POPUP_MAX_HEIGHT, Size};

actions!(
    combobox,
    [HighlightNext, HighlightPrevious, Choose, Dismiss]
);

/// The key context the combobox's wrapper declares.
pub const COMBOBOX_CONTEXT: &str = "Combobox";

/// Binds the combobox's four keys: down, up, enter, and escape.
pub fn bind_combobox_keys(cx: &mut App) {
    let under_input = Some("Combobox > Input");
    cx.bind_keys([
        KeyBinding::new("down", HighlightNext, under_input),
        KeyBinding::new("up", HighlightPrevious, under_input),
        KeyBinding::new("enter", Choose, under_input),
        KeyBinding::new("escape", Dismiss, under_input),
        KeyBinding::new("down", HighlightNext, Some(COMBOBOX_CONTEXT)),
        KeyBinding::new("up", HighlightPrevious, Some(COMBOBOX_CONTEXT)),
        KeyBinding::new("enter", Choose, Some(COMBOBOX_CONTEXT)),
        KeyBinding::new("escape", Dismiss, Some(COMBOBOX_CONTEXT)),
    ]);
}

/// What happens to text that matches no option when the field loses focus.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UnmatchedText {
    /// Restore the chosen option's label, or empty the field.
    #[default]
    Revert,
    /// Leave the text and leave the value cleared.
    Keep,
    /// Hand the text to the `on_create` handler.
    Create,
}

/// An option for a [`Combobox`]: a value and the label the filter matches on.
pub type ComboboxOption<T> = (T, SharedString);
/// Reports the value whenever it changes.
pub type ComboboxHandler<T> = Rc<dyn Fn(Option<T>, &mut Window, &mut App)>;
/// Receives unmatched text when the field loses focus in create mode.
pub type ComboboxCreateHandler = Rc<dyn Fn(SharedString, &mut Window, &mut App)>;
/// Decides which options survive a query: the query, the value, the label.
pub type ComboboxFilter<T> = Rc<dyn Fn(&str, &T, &SharedString) -> bool>;

/// Builder for a combobox, consumed by [`ComboboxState::new`].
#[must_use]
pub struct Combobox<T>
where
    T: Clone + PartialEq + 'static,
{
    id: ElementId,
    name: SharedString,
    options: Vec<ComboboxOption<T>>,
    selected: Option<T>,
    placeholder: SharedString,
    on_change: Option<ComboboxHandler<T>>,
    on_create: Option<ComboboxCreateHandler>,
    filter: Option<ComboboxFilter<T>>,
    unmatched: UnmatchedText,
    size: Size,
    style: StyleRefinement,
}

impl<T> Combobox<T>
where
    T: Clone + PartialEq + 'static,
{
    pub fn new<I, N>(id: I, name: N, options: Vec<ComboboxOption<T>>) -> Self
    where
        I: Into<ElementId>,
        N: Into<SharedString>,
    {
        Self {
            id: id.into(),
            name: name.into(),
            options,
            selected: None,
            placeholder: SharedString::from("Select..."),
            on_change: None,
            on_create: None,
            filter: None,
            unmatched: UnmatchedText::default(),
            size: Size::default(),
            style: StyleRefinement::default(),
        }
    }

    /// Sets the initially selected value; its label seeds the field.
    pub fn selected<S>(mut self, selected: S) -> Self
    where
        S: Into<Option<T>>,
    {
        self.selected = selected.into();
        self
    }

    /// Sets the text shown while the field is empty.
    pub fn placeholder<L>(mut self, placeholder: L) -> Self
    where
        L: Into<SharedString>,
    {
        self.placeholder = placeholder.into();
        self
    }

    /// Reports the value whenever it changes.
    pub fn on_change<F>(mut self, handler: F) -> Self
    where
        F: Fn(Option<T>, &mut Window, &mut App) + 'static,
    {
        self.on_change = Some(Rc::new(handler));
        self
    }

    /// Hands unmatched text to `handler` when the field loses focus.
    pub fn on_create<F>(mut self, handler: F) -> Self
    where
        F: Fn(SharedString, &mut Window, &mut App) + 'static,
    {
        self.on_create = Some(Rc::new(handler));
        self.unmatched = UnmatchedText::Create;
        self
    }

    /// Sets the matcher that decides which options survive a query.
    pub fn filter<F>(mut self, filter: F) -> Self
    where
        F: Fn(&str, &T, &SharedString) -> bool + 'static,
    {
        self.filter = Some(Rc::new(filter));
        self
    }

    /// Leaves unmatched text in the field on blur.
    pub fn keep_unmatched_text(mut self) -> Self {
        self.unmatched = UnmatchedText::Keep;
        self
    }

    /// Overrides the accessibility label.
    pub fn accessibility_label<L>(mut self, label: L) -> Self
    where
        L: Into<SharedString>,
    {
        self.name = label.into();
        self
    }
}

/// Style refinements set the combobox's width.
impl<T> Styled for Combobox<T>
where
    T: Clone + PartialEq + 'static,
{
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

// Combobox is a data builder, not an element, adopt the combinators explicitly.
impl<T> FluentBuilder for Combobox<T> where T: Clone + PartialEq + 'static {}

/// The stateful combobox.
///
/// # Examples
///
/// ```no_run
/// # use gpui::prelude::*;
/// # use gpui::{App, Entity, Window};
/// # use ui::combobox::{Combobox, ComboboxState};
/// #
/// # fn build(window: &mut Window, cx: &mut App) -> Entity<ComboboxState<&'static str>> {
/// # let picker = Combobox::new("theme", "Theme", vec![("dark", "Dark".into())]);
/// # let state = cx.new(|cx| ComboboxState::new(picker, window, cx));
/// state.update(cx, |state, cx| state.set_selected(Some("dark"), window, cx));
/// assert_eq!(state.read(cx).selected(), Some(&"dark"));
/// # state
/// # }
/// ```
pub struct ComboboxState<T>
where
    T: Clone + PartialEq + 'static,
{
    id: ElementId,
    name: SharedString,
    placeholder: SharedString,
    options: Vec<ComboboxOption<T>>,
    /// The chosen value. `None` while the text names nothing.
    selected: Option<T>,
    query: Entity<InputState>,
    /// Indices into `options` that survived the filter, in popup row order.
    visible: Vec<usize>,
    /// The highlighted popup *row*, when the popup is open.
    highlight: Option<usize>,
    on_change: Option<ComboboxHandler<T>>,
    on_create: Option<ComboboxCreateHandler>,
    filter: ComboboxFilter<T>,
    unmatched: UnmatchedText,
    size: Size,
    /// Width refinements from the builder, applied to the wrapper each render.
    style: StyleRefinement,
    /// Handler reports in flight; held so they die with the state.
    tasks: Vec<Task<()>>,
    /// Keeps the query-event subscription tied to the state's lifetime.
    _subscription: Subscription,
}

impl<T> Sizable for ComboboxState<T>
where
    T: Clone + PartialEq + 'static,
{
    fn apply_size(&mut self, size: Size) {
        self.size = size;
    }
}

impl<T> ComboboxState<T>
where
    T: Clone + PartialEq + 'static,
{
    /// Builds the state, its query field, and the subscription tying them.
    pub fn new(combobox: Combobox<T>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = InputState::new(cx);

        // The field starts showing the label of whatever was chosen,
        // so text and value agree from the first frame.
        if let Some(selected) = &combobox.selected
            && let Some((_, label)) = combobox.options.iter().find(|(value, _)| value == selected)
        {
            let label = label.to_string();
            query.update(cx, |query, cx| query.set_value_silent(label, window, cx));
        }

        let subscription =
            cx.subscribe_in(&query, window, |state, _, event, window, cx| match event {
                InputStateEvent::Focused => state.query_focused(window, cx),
                InputStateEvent::TextChanged => state.text_changed(window, cx),
                InputStateEvent::Blurred => state.blurred(window, cx),
                _ => {}
            });

        let visible = (0..combobox.options.len()).collect();

        Self {
            id: combobox.id,
            name: combobox.name,
            placeholder: combobox.placeholder,
            options: combobox.options,
            selected: combobox.selected,
            query,
            visible,
            highlight: None,
            on_change: combobox.on_change,
            on_create: combobox.on_create,
            filter: combobox.filter.unwrap_or_else(|| {
                Rc::new(|query, _value, label| label.to_lowercase().contains(&query.to_lowercase()))
            }),
            unmatched: combobox.unmatched,
            size: combobox.size,
            style: combobox.style,
            tasks: Vec::new(),
            _subscription: subscription,
        }
    }

    /// The text currently in the field.
    pub fn text(&self, cx: &App) -> String {
        self.query.read(cx).value().to_string()
    }

    /// Focuses the query field.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.query.read(cx).focus_handle(), cx);
    }

    /// Whether the option list is showing.
    pub fn is_open(&self) -> bool {
        self.highlight.is_some()
    }

    /// The chosen value.
    pub fn selected(&self) -> Option<&T> {
        self.selected.as_ref()
    }

    /// Replaces the value; reports it when it actually changed.
    pub fn set_selected(&mut self, value: Option<T>, window: &mut Window, cx: &mut Context<Self>) {
        self.set_value(value, window, cx);
    }

    /// Replaces the choice list and re-runs the current filter.
    pub fn set_options(&mut self, options: Vec<ComboboxOption<T>>, cx: &mut Context<Self>) {
        self.options = options;
        let query = self.query.read(cx).value().to_string();
        self.refilter(&query);
        cx.notify();
    }

    /// A copy of the current choice list.
    pub fn options_snapshot(&self) -> Vec<ComboboxOption<T>> {
        self.options.clone()
    }

    /// The label of the chosen option, if there is one.
    fn selected_label(&self) -> Option<SharedString> {
        let selected = self.selected.as_ref()?;
        self.options
            .iter()
            .find(|(value, _)| value == selected)
            .map(|(_, label)| label.clone())
    }

    /// Recomputes `visible` for `query` and clamps the highlight onto it.
    fn refilter(&mut self, query: &str) {
        let filter = self.filter.clone();
        self.visible = self
            .options
            .iter()
            .enumerate()
            .filter(|(_, (value, label))| filter(query, value, label))
            .map(|(index, _)| index)
            .collect();
        self.highlight = self.highlight.filter(|row| *row < self.visible.len());
    }

    fn query_text(&self, cx: &Context<Self>) -> String {
        self.query.read(cx).value().trim().to_string()
    }

    /// Sets the value and reports it, including when it is `None`.
    fn set_value(&mut self, value: Option<T>, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected == value {
            return;
        }
        self.selected = value.clone();
        if let Some(on_change) = self.on_change.clone() {
            let value = value.clone();
            let task = window.spawn(cx, async move |cx| {
                cx.update(|window, cx| on_change(value, window, cx))
                    .log_err();
            });
            self.tasks.push(task);
        }
        cx.notify();
    }

    /// Commits the option at popup *row*.
    fn choose_row(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(option_index) = self.visible.get(row).copied() else {
            return;
        };

        let Some((value, label)) = self.options.get(option_index).cloned() else {
            return;
        };

        self.query.update(cx, |query, cx| {
            query.set_value_silent(label.to_string(), window, cx)
        });
        self.highlight = None;
        self.refilter("");
        self.set_value(Some(value), window, cx);
    }

    /// Typing: re-filter, open, reset the highlight, and clear the value.
    fn text_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.query_text(cx);
        self.refilter(&query);
        self.highlight = Some(0);
        self.set_value(None, window, cx);
        cx.notify();
    }

    /// Gaining focus.
    fn query_focused(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_some() {
            return;
        }
        self.refilter("");
        self.highlight = Some(0);
        cx.notify();
    }

    /// Losing focus. [`UnmatchedText`] decides what the text becomes.
    fn blurred(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.highlight = None;
        let text = self.query_text(cx);

        if self.selected_label().as_deref() == Some(text.as_str()) {
            cx.notify();
            return;
        }

        match self.unmatched {
            UnmatchedText::Revert => {
                let restored = self.selected_label().unwrap_or_default().to_string();
                self.query
                    .update(cx, |query, cx| query.set_value_silent(restored, window, cx));
            }
            UnmatchedText::Keep => {}
            UnmatchedText::Create => {
                if !text.is_empty()
                    && let Some(on_create) = self.on_create.clone()
                {
                    let text = SharedString::from(text);
                    let task = window.spawn(cx, async move |cx| {
                        cx.update(|window, cx| on_create(text, window, cx))
                            .log_err();
                    });
                    self.tasks.push(task);
                }
            }
        }
        cx.notify();
    }

    /// Down and Up over the visible rows.
    fn move_highlight(&mut self, delta: i32, cx: &mut Context<Self>) {
        if self.visible.is_empty() {
            return;
        }

        let count = self.visible.len();
        let current = self.highlight.filter(|row| *row < count);
        let next = match current {
            Some(row) => match row.checked_add_signed(isize::try_from(delta).unwrap_or(0)) {
                Some(next) if next < count => next,
                Some(_) => 0,
                None => count - 1,
            },
            None => {
                if delta > 0 {
                    0
                } else {
                    count - 1
                }
            }
        };
        self.highlight = Some(next);
        cx.notify();
    }

    /// Enter. Commits the highlighted row.
    fn choose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.highlight else {
            cx.propagate();
            return;
        };
        self.choose_row(row, window, cx);
    }

    /// Escape. Closes the picker.
    fn dismiss(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.highlight.is_none() {
            cx.propagate();
            return;
        }
        self.highlight = None;
        cx.notify();
    }
}

impl<T> Render for ComboboxState<T>
where
    T: Clone + PartialEq + 'static,
{
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (colors, radius, panel_border, shadows) = {
            let theme = cx.theme();

            (
                theme.colors,
                theme.radius,
                theme.colors.border,
                theme.elevations.popover.shadows(),
            )
        };

        let id = self.id.clone();
        let state = cx.entity();
        let open = self.highlight.is_some();

        let chevron = h_flex()
            .id((id.clone(), "chevron"))
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click({
                let state = state.clone();
                move |_, window, cx| {
                    let focus = state.read(cx).query.read(cx).focus_handle();
                    window.focus(&focus, cx);
                    state.update(cx, |state, cx| {
                        if state.highlight.is_some() {
                            state.highlight = None;
                        } else {
                            if state.selected.is_some() {
                                state.refilter("");
                            } else {
                                let query = state.query.read(cx).value().to_string();
                                state.refilter(&query);
                            }
                            state.highlight = Some(0);
                        }
                        cx.notify();
                    });
                }
            })
            .child(
                icon(IconRole::ChevronDown)
                    .size(self.size.dense_icon_rems())
                    .color(colors.muted_foreground),
            );

        let wrapper_bounds: Rc<RefCell<Option<Bounds<Pixels>>>> = Rc::new(RefCell::new(None));

        div()
            .id(id.clone())
            .key_context(COMBOBOX_CONTEXT)
            .relative()
            .w(COMBOBOX_WIDTH)
            .min_w(COMBOBOX_MIN_WIDTH)
            .refine_style(&self.style)
            .role(Role::ComboBox)
            .aria_expanded(open)
            .aria_label(self.name.clone())
            .on_action({
                let state = state.clone();
                move |_: &HighlightNext, _window, cx| {
                    state.update(cx, |state, cx| state.move_highlight(1, cx));
                }
            })
            .on_action({
                let state = state.clone();
                move |_: &HighlightPrevious, _window, cx| {
                    state.update(cx, |state, cx| state.move_highlight(-1, cx));
                }
            })
            .on_action({
                let state = state.clone();
                move |_: &Choose, window, cx| {
                    state.update(cx, |state, cx| state.choose(window, cx));
                }
            })
            .on_action({
                let state = state.clone();
                move |_: &Dismiss, _window, cx| {
                    state.update(cx, |state, cx| state.dismiss(_window, cx));
                }
            })
            .child(
                canvas(
                    {
                        let cell = wrapper_bounds.clone();
                        move |bounds, _window, _cx| *cell.borrow_mut() = Some(bounds)
                    },
                    |_, _, _window, _cx| {},
                )
                .absolute()
                .inset_0(),
            )
            .child(
                Input::new((id.clone(), "query"), self.query.clone())
                    .placeholder(self.placeholder.clone())
                    .placeholder_when_focused(true)
                    .with_size(self.size),
            )
            .child(
                h_flex()
                    .absolute()
                    .right(self.size.dense_padding_x_rems())
                    .top_0()
                    .bottom_0()
                    .items_center()
                    .child(chevron),
            )
            .when(open, |wrapper| {
                let rows: Vec<gpui::AnyElement> = self
                    .visible
                    .iter()
                    .enumerate()
                    .map(|(row, option_index)| {
                        let Some((value, label)) = self.options.get(*option_index) else {
                            return div().into_any_element();
                        };

                        let highlighted = self.highlight == Some(row);
                        let checked = self.selected.as_ref() == Some(value);
                        let state = state.clone();
                        let row_index = row;

                        div()
                            .id(("option", *option_index))
                            .debug_selector({
                                let id = id.clone();
                                move || format!("{id}-option-{row}")
                            })
                            .role(Role::ListBoxOption)
                            .aria_label(label.clone())
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_2()
                            .py_1()
                            .control_height(self.size)
                            .control_text(self.size)
                            .rounded(radius.sm)
                            .when(highlighted, |row| row.bg(colors.muted))
                            .when(!highlighted, |row| row.hover(|row| row.bg(colors.muted)))
                            .on_mouse_move({
                                let state = state.clone();
                                move |_, _window, cx| {
                                    if state.read(cx).highlight != Some(row_index) {
                                        state.update(cx, |state, cx| {
                                            state.highlight = Some(row_index);
                                            cx.notify();
                                        });
                                    }
                                }
                            })
                            .on_click({
                                let state = state.clone();
                                move |_, window, cx| {
                                    state.update(cx, |state, cx| {
                                        state.choose_row(row_index, window, cx)
                                    });
                                }
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(label.clone()),
                            )
                            .when(checked, |row| {
                                row.child(
                                    icon(IconRole::Check)
                                        .size(self.size.dense_icon_rems())
                                        .color(colors.foreground),
                                )
                            })
                            .into_any_element()
                    })
                    .collect();

                let rows: gpui::AnyElement = if rows.is_empty() {
                    div()
                        .px_2()
                        .py_1()
                        .control_caption_text(self.size)
                        .text_color(colors.muted_foreground)
                        .child("No matches")
                        .into_any_element()
                } else {
                    let scroll = window.use_keyed_state((id.clone(), "rows-scroll"), cx, |_, _| {
                        ScrollHandle::new()
                    });

                    ScrollArea::new((id.clone(), "rows"))
                        .handle(scroll.read(cx).clone())
                        .axes(crate::components::scroll_area::ScrollAxes::Vertical)
                        .max_h(POPUP_MAX_HEIGHT)
                        .children(rows)
                        .into_any_element()
                };

                let panel_bounds: Rc<RefCell<Option<Bounds<Pixels>>>> = Rc::new(RefCell::new(None));

                let popup_width = wrapper_bounds
                    .borrow()
                    .map(|bounds| bounds.size.width)
                    .unwrap_or(COMBOBOX_WIDTH);

                let panel = v_flex()
                    .id((id.clone(), "panel"))
                    .debug_selector({
                        let id = id.clone();
                        move || format!("{id}-panel")
                    })
                    .w(popup_width)
                    .p_1()
                    .bg(colors.surface)
                    .text_color(colors.foreground)
                    .rounded(radius.overlay)
                    .border_1()
                    .border_color(panel_border)
                    .shadow(shadows)
                    .overflow_hidden()
                    .child(rows)
                    .child(
                        canvas(
                            {
                                let cell = panel_bounds.clone();
                                move |bounds, _window, _cx| *cell.borrow_mut() = Some(bounds)
                            },
                            {
                                let state = state.clone();
                                let panel_bounds = panel_bounds.clone();
                                let wrapper_bounds = wrapper_bounds.clone();
                                move |_, _, window, _cx| {
                                    window.on_mouse_event({
                                        let state = state.clone();
                                        let panel_bounds = panel_bounds.clone();
                                        let wrapper_bounds = wrapper_bounds.clone();
                                        move |event: &MouseDownEvent, phase, _window, cx| {
                                            if phase != gpui::DispatchPhase::Bubble {
                                                return;
                                            }
                                            let on_panel =
                                                panel_bounds.borrow().is_some_and(|bounds| {
                                                    bounds.contains(&event.position)
                                                });
                                            let on_wrapper =
                                                wrapper_bounds.borrow().is_some_and(|bounds| {
                                                    bounds.contains(&event.position)
                                                });
                                            if on_panel || on_wrapper {
                                                return;
                                            }
                                            state.update(cx, |state, cx| {
                                                state.highlight = None;
                                                cx.notify();
                                            });
                                        }
                                    });
                                }
                            },
                        )
                        .absolute()
                        .inset_0(),
                    );

                wrapper.child(
                    deferred(anchored().offset(point(px(0.), px(4.))).child(panel))
                        .with_priority(OVERLAY_LAYER),
                )
            })
    }
}
