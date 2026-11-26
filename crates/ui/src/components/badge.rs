//! Renders a small pill that labels a count or a state.
//!
//! # Examples
//!
//! ```no_run
//! use ui::badge::{Badge, BadgeVariant};
//!
//! let unread = Badge::new("unread", "12").variant(BadgeVariant::Accent);
//! ```

use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyView, App, ElementId, Hsla, InteractiveElement as _, IntoElement, ParentElement as _,
    RenderOnce, SharedString, StatefulInteractiveElement as _, StyleRefinement, Styled as _,
    Window, relative, transparent_black,
};

use crate::components::common::{ControlSizing, Sizable, StyledExt, h_flex};
use crate::icons::Icon;
use crate::theme::{ActiveTheme as _, Size, Theme};

/// The tooltip view builder gpui's `.tooltip(...)` expects.
type TooltipBuilder = Rc<dyn Fn(&mut Window, &mut App) -> AnyView>;

/// The visual treatment of a [`Badge`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BadgeVariant {
    /// The default; quiet muted fill for counts.
    #[default]
    Neutral,
    /// The accent ramp; something active or newly added.
    Accent,
    /// The success ramp.
    Success,
    /// The warning ramp.
    Warning,
    /// The destructive ramp.
    Danger,
    /// No fill, just a border; the quietest badge.
    Outline,
}

impl BadgeVariant {
    fn tokens(self, theme: &Theme) -> (Hsla, Hsla, Hsla) {
        match self {
            Self::Neutral => (
                theme.colors.muted,
                theme.colors.muted_foreground,
                transparent_black(),
            ),
            Self::Accent => (
                theme.ramps.primary.base,
                theme.colors.primary_foreground,
                transparent_black(),
            ),
            Self::Success => (
                theme.ramps.success.base,
                theme.colors.success_foreground,
                transparent_black(),
            ),
            Self::Warning => (
                theme.ramps.warning.base,
                theme.colors.warning_foreground,
                transparent_black(),
            ),
            Self::Danger => (
                theme.ramps.destructive.base,
                theme.colors.destructive_foreground,
                transparent_black(),
            ),
            Self::Outline => (
                transparent_black(),
                theme.colors.foreground,
                theme.colors.border,
            ),
        }
    }
}

/// A small pill for a count or a state.
#[derive(IntoElement)]
#[must_use]
pub struct Badge {
    id: ElementId,
    label: SharedString,
    icon: Option<Icon>,
    variant: BadgeVariant,
    size: Size,
    tooltip: Option<TooltipBuilder>,
    style: StyleRefinement,
}

impl Badge {
    /// Creates a badge with `label`, a count or state word the caller formats.
    pub fn new<I, L>(id: I, label: L) -> Self
    where
        I: Into<ElementId>,
        L: Into<SharedString>,
    {
        Self {
            id: id.into(),
            label: label.into(),
            icon: None,
            variant: BadgeVariant::default(),
            size: Size::default(),
            tooltip: None,
            style: StyleRefinement::default(),
        }
    }

    /// Sets the badge's [`BadgeVariant`].
    pub fn variant(mut self, variant: BadgeVariant) -> Self {
        self.variant = variant;
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

    /// Shows a tooltip view next to the badge after it is hovered.
    pub fn tooltip<F>(mut self, tooltip: F) -> Self
    where
        F: Fn(&mut Window, &mut App) -> AnyView + 'static,
    {
        self.tooltip = Some(Rc::new(tooltip));
        self
    }
}

impl Sizable for Badge {
    fn apply_size(&mut self, size: Size) {
        self.size = size;
    }
}

impl gpui::Styled for Badge {
    fn style(&mut self) -> &mut gpui::StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Badge {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let size = self.size;
        let (background, foreground, border) = self.variant.tokens(theme);

        h_flex()
            .id(self.id.clone())
            .flex_none()
            .justify_center()
            .gap_1()
            .rounded(theme.radius.full)
            .border_1()
            .border_color(border)
            .bg(background)
            .text_color(foreground)
            .line_height(relative(1.0))
            .control_caption_text(size)
            .map(|badge| match size {
                Size::XSmall | Size::Small => badge.h_5().min_w_5().px_1p5(),
                Size::Medium => badge.h_6().min_w_6().px_1p5(),
                Size::Large => badge.h_7().min_w_7().px_2(),
            })
            .when_some(self.icon, |badge, icon| {
                badge.child(icon.size(size.dense_icon_rems()).color(foreground))
            })
            .child(self.label)
            .refine_style(&self.style)
            .when_some(self.tooltip, |badge, tooltip| {
                badge.tooltip(move |window, cx| tooltip(window, cx))
            })
    }
}

/// A pill pairing a green additions segment with a red deletions segment.
#[derive(IntoElement)]
#[must_use]
pub struct GroupedBadge {
    id: ElementId,
    insertions: usize,
    deletions: usize,
    style: StyleRefinement,
}

impl GroupedBadge {
    /// Creates a grouped badge from insertion and deletion counts.
    pub fn new<I>(id: I, insertions: usize, deletions: usize) -> Self
    where
        I: Into<ElementId>,
    {
        Self {
            id: id.into(),
            insertions,
            deletions,
            style: StyleRefinement::default(),
        }
    }
}

impl gpui::Styled for GroupedBadge {
    fn style(&mut self) -> &mut gpui::StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for GroupedBadge {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();

        let additions = (
            theme.ramps.success.base.opacity(0.2),
            theme.ramps.success.base,
        );

        let deletions = (
            theme.ramps.destructive.base.opacity(0.2),
            theme.ramps.destructive.base,
        );

        h_flex()
            .id(self.id.clone())
            .debug_selector({
                let id = self.id.clone();
                move || format!("{id}")
            })
            .flex_none()
            .text_xs()
            .font_medium()
            .when(self.insertions > 0, |group| {
                group.child(
                    h_flex()
                        .justify_center()
                        .h_5()
                        .px_1p5()
                        .bg(additions.0)
                        .text_color(additions.1)
                        .debug_selector({
                            let id = self.id.clone();
                            move || format!("{id}-additions")
                        })
                        .when(self.deletions == 0, |segment| segment.rounded_full())
                        .when(self.deletions > 0, |segment| segment.rounded_l_full())
                        .child(SharedString::from(format!("+{}", self.insertions))),
                )
            })
            .when(self.deletions > 0, |group| {
                group.child(
                    h_flex()
                        .justify_center()
                        .h_5()
                        .px_1p5()
                        .bg(deletions.0)
                        .text_color(deletions.1)
                        .debug_selector({
                            let id = self.id.clone();
                            move || format!("{id}-deletions")
                        })
                        .when(self.insertions == 0, |segment| segment.rounded_full())
                        .when(self.insertions > 0, |segment| segment.rounded_r_full())
                        .child(SharedString::from(format!("-{}", self.deletions))),
                )
            })
            .refine_style(&self.style)
    }
}
