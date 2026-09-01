use gpui::prelude::*;
use gpui::{App, ClickEvent, ElementId, SharedString, StyleRefinement, Window, div, px, relative};
use gpui_base::{Button as BaseButton, StyledExt};
use gpui_component::ActiveTheme;

/// A small count badge shown after a label, e.g. on a segmented filter
/// button ("All 12") or a tab. Rendered from theme tokens; sized for the
/// compact header buttons it lives on.
#[derive(IntoElement)]
pub struct CountBadge {
    count: usize,
    style: StyleRefinement,
}

impl CountBadge {
    pub fn new(count: usize) -> Self {
        Self {
            count,
            style: StyleRefinement::default(),
        }
    }
}

impl Styled for CountBadge {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for CountBadge {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        div()
            .refine_style(&self.style)
            .h_flex()
            .justify_center()
            .ml_2()
            .px_1()
            .py_0p5()
            .min_w_4()
            .text_size(px(8.))
            .bg(cx.theme().muted)
            .text_color(cx.theme().muted_foreground)
            .rounded(cx.theme().radius)
            .line_height(relative(1.))
            .child(SharedString::from(self.count.to_string()))
    }
}

/// A segmented filter/tab button: an icon, a label, an optional [`CountBadge`]
/// and a selected (pressed) state, styled from the theme's button tokens.
///
/// Built on the unstyled `gpui_base::Button`, like the app's other custom
/// controls; the `primary` variant uses the primary button tokens for
/// call-to-action buttons ("New issue", "New PR").
#[allow(clippy::type_complexity)]
#[derive(IntoElement)]
pub struct SegmentButton {
    id: ElementId,
    style: StyleRefinement,
    icon: Option<gpui::AnyElement>,
    label: SharedString,
    count: Option<usize>,
    selected: bool,
    primary: bool,
    on_click: Option<Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>>,
}

impl SegmentButton {
    pub fn new<I, L>(id: I, label: L) -> Self
    where
        I: Into<ElementId>,
        L: Into<SharedString>,
    {
        Self {
            id: id.into(),
            label: label.into(),
            style: StyleRefinement::default(),
            icon: None,
            count: None,
            selected: false,
            primary: false,
            on_click: None,
        }
    }

    /// The leading icon, e.g. `Icon::new(CustomIconName::GitIssueDone)`.
    pub fn icon(mut self, icon: impl IntoElement) -> Self {
        self.icon = Some(icon.into_any_element());
        self
    }

    /// A count shown in a badge after the label.
    pub fn count(mut self, count: usize) -> Self {
        self.count = Some(count);
        self
    }

    /// Whether the button reflects an active filter/tab.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Use the primary button tokens (for call-to-action buttons).
    pub fn primary(mut self) -> Self {
        self.primary = true;
        self
    }

    pub fn on_click(
        mut self,
        listener: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(listener));
        self
    }
}

impl Styled for SegmentButton {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for SegmentButton {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Self {
            id,
            style,
            icon,
            label,
            count,
            selected,
            primary,
            on_click,
        } = self;

        let theme = cx.theme();
        let fg = if primary {
            theme.button_primary_foreground
        } else {
            theme.button_foreground
        };
        let base = if primary {
            theme.button_primary
        } else {
            theme.button_active
        };
        let hover = if primary {
            theme.button_primary_hover
        } else {
            theme.button_hover
        };
        let active = if primary {
            theme.button_primary_active
        } else {
            theme.button_active
        };

        BaseButton::new(id)
            .refine_style(&style)
            .flex()
            .items_center()
            .h_7()
            .px_2()
            .gap_1()
            .when_some(icon, |this, icon| this.child(icon))
            .child(div().text_sm().child(label))
            .when_some(count, |this, count| this.child(CountBadge::new(count)))
            .text_color(fg)
            .rounded(theme.radius)
            .hover(move |this| this.bg(hover))
            .active(move |this| this.bg(active))
            .selected(selected)
            .when(primary, |this| this.bg(base))
            .when(selected, |this| this.bg(active))
            .when_some(on_click, |this, listener| this.on_click(listener))
    }
}
