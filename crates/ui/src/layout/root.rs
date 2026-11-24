//! The window root: application content in the client-side frame, with the
//! toast, alert, and dialog overlays the root itself owns mounted above it.
//!
//! Applications mount [`Root`] as the window's root view and set their content
//! entity; the root owns the frame, the background, the floating surfaces, and
//! their layering. Code anywhere presents dialogs or pushes toasts through the
//! static helpers, which reach the mounted root via its global.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyView, App, AppContext as _, Context, Entity, Global, IntoElement, ParentElement as _,
    Render, StyleRefinement, Styled, WeakEntity, Window, div,
};

use crate::components::alert::{Alerts, AlertsView};
use crate::components::grain::grain as grain_overlay;
use crate::components::toast::{Toasts, ToastsView};
use crate::layout::window_frame::frame;
use crate::layout::{StyledExt as _, v_flex};
use crate::theme::ActiveTheme as _;
use crate::toast::Toast;

/// The mounted root's handle, for access from anywhere.
struct RootGlobal {
    root: WeakEntity<Root>,
}

impl Global for RootGlobal {}

/// Root presentation helpers on gpui contexts.
pub trait RootContext {
    /// Pushes a toast on the mounted root's store.
    fn toast(&mut self, toast: Toast);

    /// Mounts the command palette view on the root.
    ///
    /// Mounted once at startup; the palette renders only while open.
    fn open_palette(&mut self, view: impl Into<AnyView>);

    /// Presents a dialog view above the mounted root's content.
    fn open_dialog(&mut self, view: impl Into<AnyView>);
}

impl RootContext for App {
    fn toast(&mut self, toast: Toast) {
        if let Some(root) = Root::global(self) {
            let toasts = root.read(self).toasts();
            toasts.update(self, |toasts, cx| {
                toasts.push(toast, cx);
            });
        }
    }

    fn open_palette(&mut self, view: impl Into<AnyView>) {
        if let Some(root) = Root::global(self) {
            root.update(self, |root, cx| root.mount_palette(view, cx));
        }
    }

    fn open_dialog(&mut self, view: impl Into<AnyView>) {
        if let Some(root) = Root::global(self) {
            root.update(self, |root, cx| {
                root.mount_dialog(view, cx);
            });
        }
    }
}

/// The window's content and floating-surface host.
pub struct Root {
    content: Option<AnyView>,
    palette: Option<AnyView>,
    dialogs: Vec<AnyView>,
    toasts: Entity<Toasts>,
    alerts: Entity<Alerts>,
    transparent: bool,
    grain: bool,
    style: StyleRefinement,
}

impl Root {
    /// Creates the root and its toast and alert stores.
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let toasts = cx.new(|_| Toasts::default());
        let alerts = cx.new(|_| Alerts::default());
        let weak = cx.weak_entity();
        cx.set_global(RootGlobal { root: weak });

        Self {
            content: None,
            palette: None,
            dialogs: Vec::new(),
            toasts,
            alerts,
            transparent: false,
            grain: false,
            style: StyleRefinement::default(),
        }
    }

    /// The mounted root, if the window has opened one.
    pub fn global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<RootGlobal>()
            .and_then(|global| global.root.upgrade())
    }

    /// Sets the application content view.
    pub fn set_content(&mut self, content: impl Into<AnyView>, cx: &mut Context<Self>) {
        self.content = Some(content.into());
        cx.notify();
    }

    /// The root's toast store.
    pub fn toasts(&self) -> Entity<Toasts> {
        self.toasts.clone()
    }

    /// The root's alert store.
    pub fn alerts(&self) -> Entity<Alerts> {
        self.alerts.clone()
    }

    /// Mounts the command palette view above the content.
    fn mount_palette(&mut self, view: impl Into<AnyView>, cx: &mut Context<Self>) {
        self.palette = Some(view.into());
        cx.notify();
    }

    /// Mounts a dialog view above the content, replacing an open dialog of the same view type.
    fn mount_dialog(&mut self, view: impl Into<AnyView>, cx: &mut Context<Self>) {
        let view = view.into();
        if let Some(existing) = self
            .dialogs
            .iter_mut()
            .find(|dialog| dialog.entity_type() == view.entity_type())
        {
            *existing = view;
        } else {
            self.dialogs.push(view);
        }
        cx.notify();
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
}

impl Styled for Root {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl Render for Root {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let background = if self.transparent {
            theme.material.background
        } else {
            theme.colors.muted
        };

        div()
            .relative()
            .size_full()
            .font_family(theme.ui_font())
            .text_color(theme.colors.foreground)
            .child(frame(
                v_flex()
                    .w_full()
                    .h_full()
                    .overflow_hidden()
                    .bg(background)
                    .refine_style(&self.style)
                    .when(self.transparent && self.grain, |layout| {
                        layout.child(grain_overlay())
                    })
                    .child(
                        self.content
                            .clone()
                            .map(AnyView::into_any_element)
                            .unwrap_or_else(|| div().into_any_element()),
                    ),
                window,
                cx,
            ))
            .when_some(self.palette.clone(), |layout, palette| {
                layout.child(palette)
            })
            .children(self.dialogs.clone())
            .child(ToastsView::new("root-toasts", self.toasts.clone()))
            .child(AlertsView::new("root-alerts", self.alerts.clone()))
    }
}
