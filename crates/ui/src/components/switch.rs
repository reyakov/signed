//! Renders a controlled on/off switch with a spring-animated thumb.
//!
//! # Examples
//!
//! ```no_run
//! use ui::switch::Switch;
//!
//! let wifi = Switch::new("wifi")
//!     .label("Wi-Fi")
//!     .checked(false)
//!     .on_change(|checked, _window, _cx| println!("checked: {checked}"));
//! ```

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnimationExt as _, App, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
    RenderOnce, Role, SharedString, SpringAnimation, StatefulInteractiveElement as _,
    StyleRefinement, Styled, Toggled, Window, div, px,
};

use crate::components::common::{ControlSizing, Sizable, StyledExt as _, h_flex};
use crate::theme::{ActiveTheme as _, BORDER_WIDTH, CONTROL_SPRING, DISABLED_OPACITY, Size};

/// The change reporter, fired with the next value by pointer, Enter, and Space.
type ChangeHandler = Box<dyn Fn(bool, &mut Window, &mut App)>;

/// A controlled on/off switch that fires [`Switch::on_change`] on toggle.
#[derive(IntoElement)]
#[must_use]
pub struct Switch {
    id: ElementId,
    label: Option<SharedString>,
    checked: bool,
    size: Size,
    disabled: bool,
    a11y_label: Option<SharedString>,
    on_change: Option<ChangeHandler>,
    style: StyleRefinement,
}

impl Switch {
    /// Creates an unchecked switch.
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

    /// Sets the current value; the switch never owns it.
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    /// Renders the switch inert, unfocusable, and dimmed.
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

    /// Sets the change handler, fired with the next value by pointer, Enter, and Space.
    pub fn on_change<F>(mut self, handler: F) -> Self
    where
        F: Fn(bool, &mut Window, &mut App) + 'static,
    {
        self.on_change = Some(Box::new(handler));
        self
    }
}

impl Styled for Switch {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl Sizable for Switch {
    fn apply_size(&mut self, size: Size) {
        self.size = size;
    }
}

impl RenderOnce for Switch {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let colors = theme.colors;
        let ring = theme.focus_ring();
        let ramps = theme.ramps;

        let rem = window.rem_size();
        let track_height = self.size.switch_track_height_rems() * rem;
        let track_width = self.size.switch_track_width_rems() * rem;

        let knob = self.size.switch_knob_rems() * rem;
        // Absolute children sit inside the track's border, so remove it from the inset.
        let inset = (track_height - knob) / 2. - BORDER_WIDTH;
        let travel = track_width - BORDER_WIDTH * 2. - knob - inset * 2.;

        let next = !self.checked;
        let on_change = self.on_change;
        let a11y_label = self.a11y_label.or_else(|| self.label.clone());
        let id = self.id.clone();

        let anim =
            SpringAnimation::new(CONTROL_SPRING).to(if self.checked { travel } else { px(0.) });

        h_flex()
            .id(self.id)
            .gap_1()
            .control_text(self.size)
            .refine_style(&self.style)
            .role(Role::Switch)
            .when_some(a11y_label, |switch, label| switch.aria_label(label))
            .aria_toggled(Toggled::from(self.checked))
            .when(self.disabled, |switch| switch.opacity(DISABLED_OPACITY))
            .when(!self.disabled, |switch| {
                switch
                    .cursor_pointer()
                    .focusable()
                    .tab_stop(true)
                    .focus_visible(|style| style.shadow(ring))
                    .when_some(on_change, |switch, on_change| {
                        switch.on_click(move |_, window, cx| on_change(next, window, cx))
                    })
            })
            .child(
                div()
                    .flex_none()
                    .relative()
                    .debug_selector(move || format!("{id}-track"))
                    .switch_track_width(self.size)
                    .switch_track_height(self.size)
                    .rounded(theme.radius.full)
                    .border_1()
                    .when(self.checked, |track| {
                        track
                            .bg(ramps.primary.base)
                            .border_color(ramps.primary.base)
                            .hover(|style| {
                                style
                                    .bg(ramps.primary.hover)
                                    .border_color(ramps.primary.hover)
                            })
                    })
                    .when(!self.checked, |track| {
                        track.bg(colors.input).border_color(colors.border)
                    })
                    .child(
                        div()
                            .absolute()
                            .top(inset)
                            .left(inset)
                            .switch_knob(self.size)
                            .rounded(theme.radius.full)
                            .bg(if self.checked {
                                colors.primary_foreground
                            } else {
                                colors.muted_foreground
                            })
                            .with_spring("switch-thumb", anim, move |thumb, offset| {
                                thumb.left(inset + offset)
                            }),
                    ),
            )
            .when_some(self.label, |switch, label| switch.child(label))
    }
}
