//! Renders a group of radio buttons that reports the clicked choice.
//!
//! # Examples
//!
//! ```no_run
//! use ui::radio::{Radio, RadioGroup};
//!
//! let plan = RadioGroup::new("plan")
//!     .selected(Some("monthly"))
//!     .child(Radio::new("monthly", "monthly").label("Monthly"))
//!     .child(Radio::new("yearly", "yearly").label("Yearly"))
//!     .on_change(|value, _window, _cx| println!("selected: {value}"));
//! ```

use gpui::prelude::FluentBuilder;
use gpui::{
    App, ElementId, InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce, Role,
    SharedString, StatefulInteractiveElement as _, StyleRefinement, Styled as _, Toggled, Window,
    div,
};

use crate::components::common::{
    ControlSizing, Sizable, StyledExt as _, focus_ring, h_flex, v_flex,
};
use crate::theme::{ActiveTheme as _, DISABLED_OPACITY, Size};

/// The group's change reporter, fired with the clicked radio's value.
type ChangeHandler<V> = std::rc::Rc<dyn Fn(V, &mut Window, &mut App)>;

/// A single choice in a [`RadioGroup`].
///
/// Each radio is its own tab stop; the full ARIA radiogroup pattern (single
/// stop plus arrow-key roving focus) needs a group owner that does not exist yet.
#[must_use]
pub struct Radio<V> {
    id: ElementId,
    value: V,
    label: Option<SharedString>,
    disabled: bool,
    a11y_label: Option<SharedString>,
}

impl<V> Radio<V>
where
    V: Clone + PartialEq + 'static,
{
    /// Creates a radio representing `value`.
    pub fn new<I>(id: I, value: V) -> Self
    where
        I: Into<ElementId>,
    {
        Self {
            id: id.into(),
            value,
            label: None,
            disabled: false,
            a11y_label: None,
        }
    }

    /// Sets the visible label; clicking it selects the radio too.
    pub fn label<L>(mut self, label: L) -> Self
    where
        L: Into<SharedString>,
    {
        self.label = Some(label.into());
        self
    }

    /// Renders the radio inert, unfocusable, and dimmed.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
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
}

// Radio is a data builder, not an element, adopt the combinators explicitly.
impl<V> FluentBuilder for Radio<V> {}

/// A set of [`Radio`] choices that reports clicks through [`RadioGroup::on_change`].
#[derive(IntoElement)]
#[must_use]
pub struct RadioGroup<V>
where
    V: Clone + PartialEq + 'static,
{
    id: ElementId,
    selected: Option<V>,
    children: Vec<Radio<V>>,
    size: Size,
    a11y_label: Option<SharedString>,
    on_change: Option<ChangeHandler<V>>,
    style: StyleRefinement,
}

impl<V> RadioGroup<V>
where
    V: Clone + PartialEq + 'static,
{
    /// Creates an empty group with nothing selected.
    pub fn new<I>(id: I) -> Self
    where
        I: Into<ElementId>,
    {
        Self {
            id: id.into(),
            selected: None,
            children: Vec::new(),
            size: Size::default(),
            a11y_label: None,
            on_change: None,
            style: StyleRefinement::default(),
        }
    }

    /// Sets the currently selected value.
    pub fn selected(mut self, selected: Option<V>) -> Self {
        self.selected = selected;
        self
    }

    /// Adds a radio to the group.
    pub fn child(mut self, radio: Radio<V>) -> Self {
        self.children.push(radio);
        self
    }

    /// Sets the group's accessibility label.
    pub fn accessibility_label<L>(mut self, label: L) -> Self
    where
        L: Into<SharedString>,
    {
        self.a11y_label = Some(label.into());
        self
    }

    /// Sets the handler fired with the clicked radio's value.
    pub fn on_change<F>(mut self, handler: F) -> Self
    where
        F: Fn(V, &mut Window, &mut App) + 'static,
    {
        self.on_change = Some(std::rc::Rc::new(handler));
        self
    }
}

impl<V> Sizable for RadioGroup<V>
where
    V: Clone + PartialEq + 'static,
{
    fn apply_size(&mut self, size: Size) {
        self.size = size;
    }
}

impl<V> gpui::Styled for RadioGroup<V>
where
    V: Clone + PartialEq + 'static,
{
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl<V> RenderOnce for RadioGroup<V>
where
    V: Clone + PartialEq + 'static,
{
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let colors = theme.colors;
        let ramps = theme.ramps;
        let radius = theme.radius;
        let size = self.size;
        let selected = self.selected;
        let on_change = self.on_change;
        let group_label = self.a11y_label;

        v_flex()
            .id(self.id)
            .gap_2()
            .control_text(size)
            .refine_style(&self.style)
            .role(Role::RadioGroup)
            .when_some(group_label.clone(), |group, label| group.aria_label(label))
            .children(self.children.into_iter().map(move |radio| {
                let is_selected = selected.as_ref() == Some(&radio.value);
                let disabled = radio.disabled;
                let row_on_change = on_change.clone();

                let a11y_label = radio
                    .a11y_label
                    .or(radio.label.clone())
                    .or_else(|| group_label.clone());

                h_flex()
                    .id(radio.id)
                    .gap_2()
                    .role(Role::RadioButton)
                    .when_some(a11y_label, |row, label| row.aria_label(label))
                    .aria_toggled(Toggled::from(is_selected))
                    .when(disabled, |row| row.opacity(DISABLED_OPACITY))
                    .when(!disabled, move |row| {
                        let value = radio.value.clone();
                        row.cursor_pointer()
                            .focusable()
                            .tab_stop(true)
                            .focus_visible(|style| style.shadow(focus_ring(colors.ring)))
                            .when_some(row_on_change, |row, on_change| {
                                row.on_click(move |_, window, cx| {
                                    on_change(value.clone(), window, cx)
                                })
                            })
                    })
                    .child(
                        h_flex()
                            .justify_center()
                            .flex_none()
                            .control_indicator(size)
                            .rounded(radius.full)
                            .border_1()
                            .when(is_selected, |indicator| {
                                indicator
                                    .bg(ramps.primary.base)
                                    .border_color(ramps.primary.base)
                                    .hover(|style| {
                                        style
                                            .bg(ramps.primary.hover)
                                            .border_color(ramps.primary.hover)
                                    })
                                    .child({
                                        let dot = match size {
                                            Size::XSmall => div().size_1p5(),
                                            Size::Small | Size::Medium => div().size_2(),
                                            Size::Large => div().size_2p5(),
                                        };
                                        dot.rounded(radius.full).bg(colors.primary_foreground)
                                    })
                            })
                            .when(!is_selected, |indicator| {
                                indicator
                                    .bg(colors.input)
                                    .border_color(colors.border)
                                    .hover(|style| style.border_color(colors.muted_foreground))
                            }),
                    )
                    .when_some(radio.label, |row, label| row.child(label))
            }))
    }
}
