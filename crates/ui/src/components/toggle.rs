//! Renders a two-state control that reports its next pressed state on click.
//!
//! # Examples
//!
//! ```no_run
//! use ui::toggle::Toggle;
//!
//! let bold = Toggle::new("bold")
//!     .label("Bold")
//!     .pressed(true)
//!     .on_change(|pressed, _window, _cx| println!("bold: {pressed}"));
//! ```

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, ElementId, InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce, Role,
    SharedString, StatefulInteractiveElement as _, StyleRefinement, Styled as _, Toggled, Window,
};

use crate::components::common::{Sizable, StyledExt as _, focus_ring, h_flex};
use crate::icons::Icon;
use crate::theme::{ActiveTheme as _, DISABLED_OPACITY, Size};

/// The change handler, fired with the next pressed state.
type ChangeHandler = Box<dyn Fn(bool, &mut Window, &mut App)>;

/// A two-state control that reports the next pressed state on click.
#[derive(IntoElement)]
#[must_use]
pub struct Toggle {
    id: ElementId,
    label: Option<SharedString>,
    icon: Option<Icon>,
    pressed: bool,
    size: Size,
    disabled: bool,
    a11y_label: Option<SharedString>,
    on_change: Option<ChangeHandler>,
    style: StyleRefinement,
}

impl Toggle {
    /// Creates an unpressed toggle.
    pub fn new<I>(id: I) -> Self
    where
        I: Into<ElementId>,
    {
        Self {
            id: id.into(),
            label: None,
            icon: None,
            pressed: false,
            size: Size::default(),
            disabled: false,
            a11y_label: None,
            on_change: None,
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

    /// Shows an icon. Omitting the label renders an icon-only toggle.
    pub fn icon<I>(mut self, icon: I) -> Self
    where
        I: Into<Icon>,
    {
        self.icon = Some(icon.into());
        self
    }

    /// Sets the current pressed state; the toggle never owns it.
    pub fn pressed(mut self, pressed: bool) -> Self {
        self.pressed = pressed;
        self
    }

    /// Renders the toggle inert, unfocusable, and dimmed; disabled always wins.
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

    /// Sets the change handler, fired with the next pressed state.
    pub fn on_change<F>(mut self, handler: F) -> Self
    where
        F: Fn(bool, &mut Window, &mut App) + 'static,
    {
        self.on_change = Some(Box::new(handler));
        self
    }
}

impl gpui::Styled for Toggle {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl Sizable for Toggle {
    fn apply_size(&mut self, size: Size) {
        self.size = size;
    }
}

impl RenderOnce for Toggle {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let colors = theme.colors;
        let ramps = theme.ramps;
        let metrics = theme.sizes.resolve(self.size);
        let on_change = self.on_change;
        let next = !self.pressed;
        let a11y_label = self.a11y_label.or_else(|| self.label.clone());
        let icon_only = self.icon.is_some() && self.label.is_none();

        h_flex()
            .id(self.id)
            .justify_center()
            .h(metrics.control_height)
            .when(icon_only, |toggle| {
                toggle.w(metrics.control_height).px(gpui::Pixels::ZERO)
            })
            .when(!icon_only, |toggle| toggle.px(metrics.dense_padding_x))
            .rounded(theme.radius.control)
            .text_size(metrics.text_size)
            .text_color(colors.foreground)
            .role(Role::Button)
            .when_some(a11y_label, |toggle, label| toggle.aria_label(label))
            .aria_toggled(Toggled::from(self.pressed))
            .when(self.pressed, |toggle| {
                toggle
                    .bg(ramps.secondary.hover)
                    .hover(|style| style.bg(ramps.secondary.pressed))
            })
            .when(!self.pressed, |toggle| {
                toggle
                    .hover(|style| style.bg(colors.muted))
                    .active(|style| style.bg(ramps.secondary.hover))
            })
            .refine_style(&self.style)
            .when(self.disabled, |toggle| toggle.opacity(DISABLED_OPACITY))
            .when(!self.disabled, |toggle| {
                toggle
                    .cursor_pointer()
                    .focusable()
                    .tab_stop(true)
                    .focus_visible(|style| style.shadow(focus_ring(colors.ring)))
                    .when_some(on_change, |toggle, on_change| {
                        toggle.on_click(move |_, window, cx| on_change(next, window, cx))
                    })
            })
            .child(
                h_flex()
                    .gap_2()
                    .when_some(self.icon, |content, icon| {
                        content.child(icon.size(metrics.icon_size))
                    })
                    .when_some(self.label, |content, label| content.child(label)),
            )
    }
}
