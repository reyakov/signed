//! Renders a push button that fires its handler on click, Enter, and Space.
//!
//! # Examples
//!
//! ```no_run
//! use ui::button::{Button, ButtonVariant};
//!
//! let save = Button::new("save")
//!     .label("Save")
//!     .variant(ButtonVariant::Primary)
//!     .on_click(|_event, _window, _cx| println!("saved"));
//! ```

use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AnyView, App, ClickEvent, ElementId, Hsla, InteractiveElement as _, IntoElement,
    ParentElement, RenderOnce, Role, SharedString, StatefulInteractiveElement as _,
    StyleRefinement, Styled as _, Window, canvas, relative, transparent_black,
};

use crate::components::common::{
    ControlSizing, Sizable, StyledExt as _, focus_ring, h_flex, record_anchor_bounds,
};
use crate::components::spinner::Spinner;
use crate::icons::Icon;
use crate::spinner::SpinnerVariant;
use crate::theme::{ActiveTheme as _, DISABLED_OPACITY, Size};

/// The single activation handler, shared by pointer, Enter, and Space.
type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
/// The tooltip view builder gpui's `.tooltip(...)` expects.
type TooltipBuilder = Rc<dyn Fn(&mut Window, &mut App) -> AnyView>;

/// Which visual treatment a button uses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonVariant {
    /// The platform's default action.
    #[default]
    Primary,
    /// The everyday companion to `Primary`.
    Secondary,
    /// Unpainted until hovered, for toolbars and dense UI.
    Ghost,
    /// The destructive action.
    Destructive,
}

/// The variant colors a button paints with, resolved from the active theme.
pub struct ButtonChrome {
    /// The resting background, `None` for unpainted variants like `Ghost`.
    pub background: Option<Hsla>,
    pub foreground: Hsla,
    pub hover: Hsla,
    pub pressed: Hsla,
    pub border: Hsla,
    pub hover_border: Hsla,
    pub pressed_border: Hsla,
}

impl ButtonVariant {
    /// The variant's colors from the active theme.
    pub fn chrome(&self, cx: &App) -> ButtonChrome {
        let theme = cx.theme();
        let colors = theme.colors;
        let ramps = theme.ramps;
        match self {
            Self::Primary => ButtonChrome {
                background: Some(ramps.primary.base),
                foreground: colors.primary_foreground,
                hover: ramps.primary.hover,
                pressed: ramps.primary.pressed,
                border: ramps.primary.base,
                hover_border: ramps.primary.hover,
                pressed_border: ramps.primary.pressed,
            },
            Self::Secondary => ButtonChrome {
                background: Some(colors.surface),
                foreground: colors.foreground,
                hover: colors.muted,
                pressed: colors.muted_hover,
                border: colors.border,
                hover_border: colors.border,
                pressed_border: colors.border,
            },
            Self::Ghost => ButtonChrome {
                background: None,
                foreground: colors.foreground,
                hover: colors.muted_hover,
                pressed: ramps.secondary.pressed,
                border: transparent_black(),
                hover_border: transparent_black(),
                pressed_border: transparent_black(),
            },
            Self::Destructive => ButtonChrome {
                background: Some(ramps.destructive.base),
                foreground: colors.destructive_foreground,
                hover: ramps.destructive.hover,
                pressed: ramps.destructive.pressed,
                border: ramps.destructive.base,
                hover_border: ramps.destructive.hover,
                pressed_border: ramps.destructive.pressed,
            },
        }
    }
}

/// Which edge the button's content aligns to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonSide {
    /// Centered content.
    #[default]
    Center,
    /// Content on the left edge, icon leading the label.
    Left,
    /// Content on the right edge, icon trailing the label.
    Right,
}

/// A push button that renders a label, an icon, or both, and fires
/// [`Button::on_click`] on click, Enter, or Space.
#[derive(IntoElement)]
#[must_use]
pub struct Button {
    id: ElementId,
    label: Option<SharedString>,
    icon: Option<Icon>,
    children: Vec<AnyElement>,
    variant: ButtonVariant,
    size: Size,
    disabled: bool,
    loading: bool,
    padded: bool,
    fully_rounded: bool,
    full_width: bool,
    side: ButtonSide,
    tab_stop: bool,
    debug_selector: Option<Rc<dyn Fn() -> String>>,
    a11y_label: Option<SharedString>,
    tooltip: Option<TooltipBuilder>,
    on_click: Option<ClickHandler>,
    style: StyleRefinement,
}

impl Button {
    /// Creates a primary button.
    pub fn new<I>(id: I) -> Self
    where
        I: Into<ElementId>,
    {
        Self {
            id: id.into(),
            label: None,
            icon: None,
            children: Vec::new(),
            variant: ButtonVariant::default(),
            size: Size::default(),
            disabled: false,
            loading: false,
            padded: true,
            fully_rounded: true,
            full_width: false,
            side: ButtonSide::default(),
            tab_stop: true,
            debug_selector: None,
            a11y_label: None,
            tooltip: None,
            on_click: None,
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

    /// Shows an icon beside the label; without a label the button renders square.
    pub fn icon<I>(mut self, icon: I) -> Self
    where
        I: Into<Icon>,
    {
        self.icon = Some(icon.into());
        self
    }

    /// Shows a tooltip view next to the button after it is hovered.
    pub fn tooltip<F>(mut self, tooltip: F) -> Self
    where
        F: Fn(&mut Window, &mut App) -> AnyView + 'static,
    {
        self.tooltip = Some(Rc::new(tooltip));
        self
    }

    /// Sets the button's visual language.
    pub fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Sets whether the button is a keyboard tab stop. Dense toolbars typically
    /// turn this off.
    pub fn tab_stop(mut self, tab_stop: bool) -> Self {
        self.tab_stop = tab_stop;
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

    /// Renders the button inert, unfocusable, and dimmed.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Shows the spinner in place of the content and suppresses activation.
    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    /// Sets whether the button pads its content. `false` removes all padding.
    pub fn padded(mut self, padded: bool) -> Self {
        self.padded = padded;
        self
    }

    /// Overrides the pill shape; buttons are fully rounded by default.
    pub fn fully_rounded(mut self, fully_rounded: bool) -> Self {
        self.fully_rounded = fully_rounded;
        self
    }

    /// Stretches the button and its content across the available width.
    pub fn full_width(mut self, full_width: bool) -> Self {
        self.full_width = full_width;
        self
    }

    /// Sets the edge the content aligns to, `Right` renders the icon after the label.
    pub fn side(mut self, side: ButtonSide) -> Self {
        self.side = side;
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

    /// Sets the single activation handler, fired by pointer, Enter, and Space.
    pub fn on_click<F>(mut self, handler: F) -> Self
    where
        F: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl ParentElement for Button {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl gpui::Styled for Button {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl Sizable for Button {
    fn apply_size(&mut self, size: Size) {
        self.size = size;
    }
}

impl RenderOnce for Button {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let ButtonChrome {
            background,
            foreground,
            hover,
            pressed,
            border,
            hover_border,
            pressed_border,
        } = self.variant.chrome(cx);

        let radius = if self.fully_rounded {
            let theme = cx.theme();
            theme.radius.full
        } else {
            let theme = cx.theme();
            theme.radius.control
        };

        let on_click = self.on_click;
        let a11y_label = self.a11y_label.or_else(|| self.label.clone());
        let is_icon_only = self.icon.is_some() && self.label.is_none() && self.children.is_empty();

        // A child-only button hugs its content as a circle when fully rounded.
        let is_child_only =
            self.icon.is_none() && self.label.is_none() && !self.children.is_empty();

        let is_round_child = is_child_only && self.fully_rounded;
        let icon_trailing = self.side == ButtonSide::Right;
        let ring = cx.theme().colors.ring;

        h_flex()
            .id(self.id.clone())
            .when_some(self.debug_selector, |button, selector| {
                button.debug_selector(move || selector())
            })
            .role(Role::Button)
            .map(|button| match self.side {
                ButtonSide::Center => button.justify_center(),
                ButtonSide::Left => button.justify_start(),
                ButtonSide::Right => button.justify_end(),
            })
            .control_height(self.size)
            .when(is_icon_only || is_round_child, |button| {
                button.control_width(self.size)
            })
            .when(self.full_width, |button| button.w_full())
            .when(self.padded && !is_icon_only && !is_round_child, |button| {
                button.control_padding_x(self.size)
            })
            .when(is_round_child, |button| button.overflow_hidden())
            .line_height(relative(1.0))
            .rounded(radius)
            .control_text(self.size)
            .border_1()
            .border_color(border)
            .when_some(background, |button, background| button.bg(background))
            .text_color(foreground)
            .refine_style(&self.style)
            .when_some(a11y_label, |button, label| button.aria_label(label))
            .child(
                h_flex()
                    .gap_1()
                    .when(self.full_width, |content| content.flex_1())
                    .when(self.loading, |content| content.invisible())
                    .map(|content| {
                        if icon_trailing {
                            content
                                .when_some(self.label, |content, label| content.child(label))
                                .children(self.children)
                                .when_some(self.icon, |content, icon| {
                                    content.child(icon.size(self.size.icon_rems()))
                                })
                        } else {
                            content
                                .when_some(self.icon, |content, icon| {
                                    content.child(icon.size(self.size.icon_rems()))
                                })
                                .when_some(self.label, |content, label| content.child(label))
                                .children(self.children)
                        }
                    }),
            )
            .when(self.loading, |button| {
                button.child(
                    h_flex().absolute().inset_0().justify_center().child(
                        Spinner::new((self.id.clone(), "spinner"))
                            .with_size(self.size)
                            .variant(SpinnerVariant::Icon)
                            .color(foreground),
                    ),
                )
            })
            .child(
                canvas(record_anchor_bounds(self.id.clone()), |_, _, _, _| {})
                    .absolute()
                    .inset_0(),
            )
            .when(self.disabled, |button| button.opacity(DISABLED_OPACITY))
            .when_some(self.tooltip, |button, tooltip| {
                button.tooltip(move |window, cx| tooltip(window, cx))
            })
            .when(!self.disabled && !self.loading, |button| {
                button
                    .cursor_pointer()
                    .focusable()
                    .tab_stop(self.tab_stop)
                    .hover(move |style| style.bg(hover).border_color(hover_border))
                    .active(move |style| style.bg(pressed).border_color(pressed_border))
                    .focus_visible(|style| style.shadow(focus_ring(ring)))
                    .when_some(on_click, |button, on_click| {
                        button.on_click(move |event, window, cx| on_click(event, window, cx))
                    })
            })
    }
}
