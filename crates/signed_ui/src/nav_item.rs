use gpui::prelude::*;
use gpui::{App, ClickEvent, ElementId, SharedString, StyleRefinement, Window, div};
use gpui_component::{ActiveTheme, StyledExt, h_flex};

/// A single navigation entry in a sidebar: an arbitrary leading element
/// (an icon, avatar, ...) and a text label with a hover highlight,
/// an optional trailing suffix (e.g. a status icon) and an optional click handler.
#[allow(clippy::type_complexity)]
#[derive(IntoElement)]
pub struct NavItem {
    id: ElementId,
    style: StyleRefinement,
    icon: gpui::AnyElement,
    label: SharedString,
    /// Trailing element rendered at the right edge of the row, after the (ellipsized) label.
    suffix: Option<gpui::AnyElement>,
    on_click: Option<Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>>,
}

impl NavItem {
    pub fn new<I, L, N>(id: I, label: L, icon: N) -> Self
    where
        I: Into<ElementId>,
        L: Into<SharedString>,
        N: IntoElement,
    {
        Self {
            id: id.into(),
            icon: icon.into_any_element(),
            label: label.into(),
            style: StyleRefinement::default(),
            suffix: None,
            on_click: None,
        }
    }

    /// A trailing element rendered at the right edge of the row
    pub fn suffix(mut self, suffix: impl IntoElement) -> Self {
        self.suffix = Some(suffix.into_any_element());
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

impl RenderOnce for NavItem {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        h_flex()
            .id(self.id)
            .refine_style(&self.style)
            .px_2()
            .py_1()
            .w_full()
            .gap_2()
            .rounded(cx.theme().radius)
            .child(self.icon)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(self.label),
            )
            .when_some(self.suffix, |this, suffix| {
                this.child(div().flex_shrink_0().child(suffix))
            })
            .hover(|this| this.bg(cx.theme().list_hover))
            .when_some(self.on_click, |this, listener| this.on_click(listener))
    }
}
