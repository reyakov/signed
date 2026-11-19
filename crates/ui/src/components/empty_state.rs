//! Renders a centered panel for lists, tables, and searches with nothing to show.
//!
//! # Examples
//!
//! ```no_run
//! use ui::empty_state::EmptyState;
//!
//! let state = EmptyState::new("No projects yet")
//!     .hint("Create a project to get started");
//! ```

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, IntoElement, ParentElement as _, RenderOnce, SharedString, StyleRefinement, Styled as _,
    Window, div, rems,
};

use crate::components::common::{StyledExt, v_flex};
use crate::icons::Icon;
use crate::theme::ActiveTheme as _;

/// The centered "nothing here yet" panel for lists, tables, and searches.
#[derive(IntoElement)]
#[must_use]
pub struct EmptyState {
    icon: Option<Icon>,
    headline: SharedString,
    hint: Option<SharedString>,
    action: Option<gpui::AnyElement>,
    style: StyleRefinement,
}

impl EmptyState {
    /// Creates an empty state whose headline is the primary statement.
    pub fn new<L>(headline: L) -> Self
    where
        L: Into<SharedString>,
    {
        Self {
            icon: None,
            headline: headline.into(),
            hint: None,
            action: None,
            style: StyleRefinement::default(),
        }
    }

    /// Sets a large muted icon above the headline.
    pub fn icon<I>(mut self, icon: I) -> Self
    where
        I: Into<Icon>,
    {
        self.icon = Some(icon.into());
        self
    }

    /// Sets a secondary line under the headline.
    pub fn hint<L>(mut self, hint: L) -> Self
    where
        L: Into<SharedString>,
    {
        self.hint = Some(hint.into());
        self
    }

    /// Sets an action element, usually a [`crate::components::button::Button`], under the hint.
    pub fn action<E>(mut self, action: E) -> Self
    where
        E: IntoElement,
    {
        self.action = Some(action.into_any_element());
        self
    }
}

impl gpui::Styled for EmptyState {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for EmptyState {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let muted_foreground = theme.colors.muted_foreground;
        let foreground = theme.colors.foreground;

        v_flex()
            .w_full()
            .items_center()
            .justify_center()
            .text_center()
            .p_4()
            .when_some(self.icon, |state, icon| {
                state.child(
                    div()
                        .mb_2()
                        .child(icon.size(rems(2.0)).color(muted_foreground)),
                )
            })
            .child(
                div()
                    .text_base()
                    .font_semibold()
                    .text_color(foreground)
                    .child(self.headline),
            )
            .when_some(self.hint, |state, hint| {
                state.child(div().text_sm().text_color(muted_foreground).child(hint))
            })
            .when_some(self.action, |state, action| {
                state.child(div().mt_3().child(action))
            })
            .refine_style(&self.style)
    }
}
