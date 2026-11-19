//! Renders a controlled checkbox that reports the next value on click, Enter, and Space.
//!
//! # Examples
//!
//! ```no_run
//! use ui::checkbox::Checkbox;
//!
//! let consent = Checkbox::new("consent")
//!     .label("Send release notes")
//!     .checked(false)
//!     .on_change(|next, _window, _cx| println!("checked: {next}"));
//! ```

use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, ElementId, InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce, Role,
    SharedString, StatefulInteractiveElement as _, StyleRefinement, Styled as _, Toggled, Window,
};

use crate::components::common::{Sizable, StyledExt as _, focus_ring, h_flex};
use crate::icons::{IconRole, icon};
use crate::theme::{ActiveTheme as _, DISABLED_OPACITY, Size};

/// The change reporter, fired with the next value by pointer, Enter, and Space.
type ChangeHandler = Box<dyn Fn(bool, &mut Window, &mut App)>;

/// A controlled checkbox component.
#[derive(IntoElement)]
#[must_use]
pub struct Checkbox {
    id: ElementId,
    label: Option<SharedString>,
    checked: bool,
    size: Size,
    disabled: bool,
    debug_selector: Option<Rc<dyn Fn() -> String>>,
    a11y_label: Option<SharedString>,
    on_change: Option<ChangeHandler>,
    style: StyleRefinement,
}

impl Checkbox {
    /// Creates an unchecked checkbox.
    pub fn new<I>(id: I) -> Self
    where
        I: Into<ElementId>,
    {
        Self {
            id: id.into(),
            label: None,
            checked: false,
            size: Size::default(),
            disabled: false,
            debug_selector: None,
            a11y_label: None,
            on_change: None,
            style: StyleRefinement::default(),
        }
    }

    /// Sets the visible label; clicking it toggles too.
    pub fn label<L>(mut self, label: L) -> Self
    where
        L: Into<SharedString>,
    {
        self.label = Some(label.into());
        self
    }

    /// Sets the current value; the checkbox never owns it.
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    /// Renders the checkbox inert, unfocusable, and dimmed; disabled always wins.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Sets the debug-bounds selector, for headless tests.
    pub fn debug_selector<F>(mut self, selector: F) -> Self
    where
        F: Fn() -> String + 'static,
    {
        self.debug_selector = Some(Rc::new(selector));
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

    /// Sets the change handler, fired with the next value by pointer, Enter, and Space.
    pub fn on_change<F>(mut self, handler: F) -> Self
    where
        F: Fn(bool, &mut Window, &mut App) + 'static,
    {
        self.on_change = Some(Box::new(handler));
        self
    }
}

impl gpui::Styled for Checkbox {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl Sizable for Checkbox {
    fn apply_size(&mut self, size: Size) {
        self.size = size;
    }
}

impl RenderOnce for Checkbox {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let colors = theme.colors;
        let ramps = theme.ramps;
        let metrics = theme.sizes.resolve(self.size);
        let on_change = self.on_change;
        let next = !self.checked;
        let a11y_label = self.a11y_label.or_else(|| self.label.clone());

        h_flex()
            .id(self.id)
            .gap_2()
            .text_size(metrics.text_size)
            .refine_style(&self.style)
            .when_some(self.debug_selector, |checkbox, selector| {
                checkbox.debug_selector(move || selector())
            })
            .role(Role::CheckBox)
            .when_some(a11y_label, |checkbox, label| checkbox.aria_label(label))
            .aria_toggled(Toggled::from(self.checked))
            .when(self.disabled, |checkbox| checkbox.opacity(DISABLED_OPACITY))
            .when(!self.disabled, |checkbox| {
                checkbox
                    .cursor_pointer()
                    .focusable()
                    .tab_stop(true)
                    .focus_visible(|style| style.shadow(focus_ring(colors.ring)))
                    .when_some(on_change, |checkbox, on_change| {
                        checkbox.on_click(move |_, window, cx| on_change(next, window, cx))
                    })
            })
            .child(
                h_flex()
                    .justify_center()
                    .flex_none()
                    .size(metrics.indicator_size)
                    .rounded(theme.radius.sm)
                    .border_1()
                    .when(self.checked, |indicator| {
                        indicator
                            .bg(ramps.primary.base)
                            .border_color(ramps.primary.base)
                            .hover(|style| {
                                style
                                    .bg(ramps.primary.hover)
                                    .border_color(ramps.primary.hover)
                            })
                            .child(
                                icon(IconRole::Check)
                                    .size(metrics.dense_icon_size)
                                    .color(colors.primary_foreground),
                            )
                    })
                    .when(!self.checked, |indicator| {
                        indicator
                            .bg(colors.input)
                            .border_color(colors.border)
                            .hover(|style| style.border_color(colors.muted_foreground))
                    }),
            )
            .when_some(self.label, |checkbox, label| checkbox.child(label))
    }
}
