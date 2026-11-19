//! The application shell: the titlebar above the panel row, wrapped in the
//! client-side window frame, with the toast and alert overlays mounted at
//! the window root.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, ElementId, Entity, IntoElement, ParentElement as _, RenderOnce,
    StyleRefinement, Styled as _, Window, div,
};

use crate::components::alert::{Alerts, AlertsView};
use crate::components::grain::grain as grain_overlay;
use crate::components::toast::{Toasts, ToastsView};
use crate::layout::titlebar::Titlebar;
use crate::layout::window_frame::frame;
use crate::layout::{StyledExt as _, v_flex};
use crate::theme::ActiveTheme as _;

/// The application shell.
#[derive(IntoElement)]
#[must_use]
pub struct Shell {
    id: ElementId,
    titlebar: Option<Titlebar>,
    panel: Option<AnyElement>,
    transparent: bool,
    grain: bool,
    toasts: Option<Entity<Toasts>>,
    alerts: Option<Entity<Alerts>>,
    style: StyleRefinement,
}

impl Shell {
    /// Creates the shell.
    pub fn new<I>(id: I) -> Self
    where
        I: Into<ElementId>,
    {
        Self {
            id: id.into(),
            titlebar: None,
            panel: None,
            transparent: false,
            grain: false,
            toasts: None,
            alerts: None,
            style: StyleRefinement::default(),
        }
    }

    /// Sets the titlebar drawn above the panel row.
    pub fn titlebar(mut self, titlebar: Titlebar) -> Self {
        self.titlebar = Some(titlebar);
        self
    }

    /// Sets the panel row filling the rest of the window.
    pub fn panel<E>(mut self, panel: E) -> Self
    where
        E: IntoElement,
    {
        self.panel = Some(panel.into_any_element());
        self
    }

    /// Paints the app background with the translucent material fill.
    pub fn transparent(mut self, transparent: bool) -> Self {
        self.transparent = transparent;
        self
    }

    /// Textures the app background with a film-grain overlay.
    pub fn grain(mut self, grain: bool) -> Self {
        self.grain = grain;
        self
    }

    /// Hosts the toast overlay raised on `toasts`.
    pub fn toasts(mut self, toasts: Entity<Toasts>) -> Self {
        self.toasts = Some(toasts);
        self
    }

    /// Hosts the alert overlay stacked on `alerts`.
    pub fn alerts(mut self, alerts: Entity<Alerts>) -> Self {
        self.alerts = Some(alerts);
        self
    }
}

impl gpui::Styled for Shell {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Shell {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let id = self.id.clone();
        let theme = cx.theme();

        let background = if self.transparent {
            theme.material.background
        } else {
            theme.colors.muted
        };

        frame(
            v_flex()
                .w_full()
                .h_full()
                .overflow_hidden()
                .bg(background)
                .refine_style(&self.style)
                .when(self.transparent && self.grain, |layout| {
                    layout.child(grain_overlay())
                })
                .when_some(self.titlebar, |layout, titlebar| {
                    layout.child(titlebar.transparent(self.transparent))
                })
                .child(self.panel.unwrap_or_else(|| div().into_any_element())),
            window,
            cx,
        )
        .when_some(self.toasts, |frame, toasts| {
            frame.child(ToastsView::new((id.clone(), "toasts"), toasts))
        })
        .when_some(self.alerts, |frame, alerts| {
            frame.child(AlertsView::new((id, "alerts"), alerts))
        })
    }
}
