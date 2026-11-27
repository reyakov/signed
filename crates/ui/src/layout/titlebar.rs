//! The window titlebar: the full-width strip drawn above the panel stack —
//! the prefix, tab bar, and suffix in the middle, the platform's window
//! controls at macOS's leading edge and every other platform's trailing
//! edge. The bar itself moves the window, tab bar included; the prefix and
//! suffix opt out, so a press on a control never starts a drag.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, ClickEvent, Div, ElementId, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement as _, RenderOnce, Stateful, StatefulInteractiveElement as _, StyleRefinement,
    Styled, Window, WindowControlArea,
};

use crate::layout::window_controls::WindowControls;
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

    /// Makes `this` a titlebar strip that moves the window; `id` keys its drag state, which `use_state` would otherwise share among every strip in the window.
    pub fn strip<I>(id: I, this: Stateful<Div>, window: &mut Window, cx: &mut App) -> Stateful<Div>
    where
        I: Into<ElementId>,
    {
        let should_move = window.use_keyed_state(id.into(), cx, |_, _| false);
        this.window_control_area(WindowControlArea::Drag)
            .on_mouse_down_out(window.listener_for(&should_move, |should_move, _, _, _| {
                *should_move = false;
            }))
            .on_mouse_down(
                MouseButton::Left,
                window.listener_for(&should_move, |should_move, _, _, _| {
                    *should_move = true;
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                window.listener_for(&should_move, |should_move, _, _, _| {
                    *should_move = false;
                }),
            )
            .on_mouse_move(
                window.listener_for(&should_move, |should_move, _, window, cx| {
                    if *should_move {
                        *should_move = false;
                        if !cx.has_active_drag() {
                            window.start_window_move();
                        }
                    }
                }),
            )
            .on_click(Self::on_titlebar_click)
    }

    /// Maximizes on a double-click where the platform does not do it.
    pub fn on_titlebar_click(event: &ClickEvent, window: &mut Window, _cx: &mut App) {
        if event.click_count() != 2 {
            return;
        }
        #[cfg(target_os = "macos")]
        window.titlebar_double_click();
        #[cfg(target_os = "linux")]
        window.zoom_window();
        // Windows handles titlebar double-click natively via WindowControlArea::Drag.
        #[cfg(any(
            target_os = "windows",
            not(any(target_os = "macos", target_os = "linux"))
        ))]
        let _ = (event, window, _cx);
    }
}

impl Styled for Titlebar {
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

        Self::strip(
            (id.clone(), "drag"),
            h_flex()
                .id(id.clone())
                .debug_selector({
                    let id = id.clone();
                    move || format!("{id}-bar")
                })
                .flex_none()
                .w_full()
                .h(height)
                .gap_2()
                .bg(theme.colors.muted)
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
            window,
            cx,
        )
        .child(WindowControls::new((id, "window-controls")))
    }
}
