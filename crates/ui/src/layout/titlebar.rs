//! The window titlebar: the full-width strip drawn above the panel row —
//! the prefix, tab bar, and suffix in the middle, the platform's window
//! controls at macOS's leading edge and every other platform's trailing
//! edge. The bar itself moves the window, tab bar included; the prefix and
//! suffix opt out, so a press on a control never starts a drag.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
    RenderOnce, SharedString, StyleRefinement, Styled as _, Window,
};

use crate::layout::window_controls::{WindowControls, titlebar_strip};
use crate::layout::{StyledExt as _, h_flex};
use crate::theme::{ActiveTheme as _, TRAFFIC_LIGHT_PAD};

/// The window's titlebar.
#[derive(IntoElement)]
#[must_use]
pub struct Titlebar {
    id: ElementId,
    prefix: Vec<AnyElement>,
    tab_bar: Option<AnyElement>,
    suffix: Vec<AnyElement>,
    transparent: bool,
    style: StyleRefinement,
}

impl Titlebar {
    /// Creates the titlebar.
    pub fn new<I>(id: I) -> Self
    where
        I: Into<ElementId>,
    {
        Self {
            id: id.into(),
            prefix: Vec::new(),
            tab_bar: None,
            suffix: Vec::new(),
            transparent: false,
            style: StyleRefinement::default(),
        }
    }

    /// Sets the tab bar drawn between the prefix and the suffix.
    pub fn tab_bar<E>(mut self, tab_bar: E) -> Self
    where
        E: IntoElement,
    {
        self.tab_bar = Some(tab_bar.into_any_element());
        self
    }

    /// Sets the element drawn before the tab bar, after the leading window controls.
    pub fn prefix<E>(mut self, prefix: E) -> Self
    where
        E: IntoElement,
    {
        self.prefix = vec![prefix.into_any_element()];
        self
    }

    /// Sets the element drawn after the tab bar, before the trailing window controls.
    pub fn suffix<E>(mut self, suffix: E) -> Self
    where
        E: IntoElement,
    {
        self.suffix = vec![suffix.into_any_element()];
        self
    }

    /// Leaves the strip unpainted so the shell's material backdrop shows through.
    pub(crate) fn transparent(mut self, transparent: bool) -> Self {
        self.transparent = transparent;
        self
    }
}

impl gpui::Styled for Titlebar {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Titlebar {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let height = theme.titlebar_height;
        let shadow_bleed = theme.elevations.control.bleed();

        let id = self.id;
        let bar_id = SharedString::from(format!("{id}-bar"));

        titlebar_strip(
            h_flex()
                .id(id.clone())
                .debug_selector(move || bar_id.to_string())
                .flex_none()
                .w_full()
                .h(height)
                .gap_2()
                .when(!self.transparent, |bar| bar.bg(theme.colors.muted))
                .px_3()
                .refine_style(&self.style)
                .when(cfg!(target_os = "macos"), |bar| bar.pl(TRAFFIC_LIGHT_PAD))
                .when(!self.prefix.is_empty(), |bar| {
                    bar.child(
                        h_flex()
                            .occlude()
                            .flex_none()
                            .h_full()
                            .items_center()
                            .gap_1()
                            .children(self.prefix),
                    )
                })
                .when_some(self.tab_bar, |bar, tab_bar| {
                    bar.child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .overflow_hidden()
                            // Include the tabs' outer shadow in the clip without shifting content.
                            .px(shadow_bleed)
                            .mx(-shadow_bleed)
                            .when(cfg!(target_os = "windows"), |slot| slot.occlude())
                            .child(tab_bar),
                    )
                })
                .when(!self.suffix.is_empty(), |bar| {
                    bar.child(
                        h_flex()
                            .occlude()
                            .flex_none()
                            .h_full()
                            .items_center()
                            .gap_1()
                            .children(self.suffix),
                    )
                }),
            (id.clone(), "drag"),
            window,
            cx,
        )
        .child(WindowControls::new((id, "window-controls")))
    }
}
