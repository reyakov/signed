//! The window root: application content in the client-side frame, with the
//! toast, alert, and dialog overlays the root itself owns mounted above it.
//!
//! Applications mount [`Root`] as the window's root view and set their content
//! entity; the root owns the frame, the background, the floating surfaces, and
//! their layering. Code anywhere presents dialogs or pushes toasts through the
//! static helpers, which reach the mounted root via its global.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyView, App, AppContext as _, Bounds, BoxShadow, Context, CursorStyle, Decorations, Div,
    Entity, Global, Hitbox, HitboxBehavior, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement as _, Pixels, Point, Render, ResizeEdge, Size, StyleRefinement, Styled, Tiling,
    WeakEntity, Window, canvas, div, point, px, size,
};

use crate::components::alert::{Alerts, AlertsView};
use crate::components::grain::Grain;
use crate::components::toast::{Toasts, ToastsView};
use crate::layout::{StyledExt as _, v_flex};
use crate::theme::ActiveTheme as _;
use crate::theme::{BORDER_WIDTH, INACTIVE_SHADOW_OPACITY, WINDOW_RESIZE_INSET};
use crate::toast::Toast;

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

/// The mounted root's handle, for access from anywhere.
struct RootGlobal {
    root: WeakEntity<Root>,
}

impl Global for RootGlobal {}

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

    /// Wraps a window's content in the frame it owes.
    fn frame(&self, child: impl IntoElement, window: &mut Window, cx: &App) -> Div {
        let Decorations::Client { tiling } = window.window_decorations() else {
            return div().size_full().child(child);
        };

        let tiling = if window.is_maximized() {
            Tiling::tiled()
        } else {
            tiling
        };

        let inset = WINDOW_RESIZE_INSET;
        let border = cx.theme().colors.border;
        let radius = cx.theme().radius.overlay;

        let shadow = Self::frame_shadow(cx, window.is_window_active());
        window.set_client_inset(inset);

        div()
            .size_full()
            .child(
                canvas(
                    move |_, window, _| Self::resize_bands(window, inset, tiling),
                    |_, bands, window, _| {
                        for (edge, hitbox) in bands.iter().filter_map(|(edge, hitbox)| {
                            hitbox.as_ref().map(|hitbox| (*edge, hitbox))
                        }) {
                            window.set_cursor_style(Self::cursor(edge), hitbox);
                        }
                    },
                )
                .absolute()
                .size_full(),
            )
            .when(!tiling.top, |this| this.pt(inset))
            .when(!tiling.bottom, |this| this.pb(inset))
            .when(!tiling.left, |this| this.pl(inset))
            .when(!tiling.right, |this| this.pr(inset))
            .on_mouse_down(MouseButton::Left, move |event, window, _| {
                let size = window.window_bounds().get_bounds().size;
                if let Some(edge) = Self::resize_edge(event.position, inset, size)
                    .filter(|edge| Self::live(*edge, tiling))
                {
                    window.start_window_resize(edge);
                }
            })
            .child(
                div()
                    .size_full()
                    .overflow_hidden()
                    .border_color(border)
                    .when(!tiling.top, |surface| surface.border_t(BORDER_WIDTH))
                    .when(!tiling.bottom, |surface| surface.border_b(BORDER_WIDTH))
                    .when(!tiling.left, |surface| surface.border_l(BORDER_WIDTH))
                    .when(!tiling.right, |surface| surface.border_r(BORDER_WIDTH))
                    .when(!(tiling.top || tiling.left), |surface| {
                        surface.rounded_tl(radius)
                    })
                    .when(!(tiling.top || tiling.right), |surface| {
                        surface.rounded_tr(radius)
                    })
                    .when(!(tiling.bottom || tiling.left), |surface| {
                        surface.rounded_bl(radius)
                    })
                    .when(!(tiling.bottom || tiling.right), |surface| {
                        surface.rounded_br(radius)
                    })
                    .when(!tiling.is_tiled(), |surface| surface.shadow(shadow))
                    .child(child),
            )
    }

    /// The dialog elevation's shadow, dimmed while the window is inactive.
    fn frame_shadow(cx: &App, active: bool) -> Vec<BoxShadow> {
        let opacity = if active { 1.0 } else { INACTIVE_SHADOW_OPACITY };
        cx.theme()
            .elevations
            .dialog
            .shadows()
            .iter()
            .map(|shadow| {
                let mut dimmed = shadow.clone();
                dimmed.color.a *= opacity;
                dimmed
            })
            .collect()
    }

    /// Which edge a press at `pos` resizes, or `None` for a press on the content.
    fn resize_edge(pos: Point<Pixels>, inset: Pixels, size: Size<Pixels>) -> Option<ResizeEdge> {
        let (top, bottom) = (pos.y < inset, pos.y > size.height - inset);
        let (left, right) = (pos.x < inset, pos.x > size.width - inset);

        Some(match (top, bottom, left, right) {
            (true, _, true, _) => ResizeEdge::TopLeft,
            (true, _, _, true) => ResizeEdge::TopRight,
            (_, true, true, _) => ResizeEdge::BottomLeft,
            (_, true, _, true) => ResizeEdge::BottomRight,
            (true, ..) => ResizeEdge::Top,
            (_, true, ..) => ResizeEdge::Bottom,
            (_, _, true, _) => ResizeEdge::Left,
            (_, _, _, true) => ResizeEdge::Right,
            _ => return None,
        })
    }

    /// The eight resize bands, as the rects [`Self::resize_edge`] classifies to, each with the hitbox its live edges insert.
    fn resize_bands(
        window: &mut Window,
        inset: Pixels,
        tiling: Tiling,
    ) -> [(ResizeEdge, Option<Hitbox>); 8] {
        let window_size = window.window_bounds().get_bounds().size;
        let band = |x: Pixels, y: Pixels, width: Pixels, height: Pixels| Bounds {
            origin: point(x, y),
            size: size(width, height),
        };

        let (far_x, far_y) = (window_size.width - inset, window_size.height - inset);
        let (span_x, span_y) = (
            window_size.width - inset - inset,
            window_size.height - inset - inset,
        );
        let zero = px(0.0);

        [
            (ResizeEdge::TopLeft, band(zero, zero, inset, inset)),
            (ResizeEdge::TopRight, band(far_x, zero, inset, inset)),
            (ResizeEdge::BottomLeft, band(zero, far_y, inset, inset)),
            (ResizeEdge::BottomRight, band(far_x, far_y, inset, inset)),
            (ResizeEdge::Top, band(inset, zero, span_x, inset)),
            (ResizeEdge::Bottom, band(inset, far_y, span_x, inset)),
            (ResizeEdge::Left, band(zero, inset, inset, span_y)),
            (ResizeEdge::Right, band(far_x, inset, inset, span_y)),
        ]
        .map(|(edge, band)| {
            (
                edge,
                Self::live(edge, tiling)
                    .then(|| window.insert_hitbox(band, HitboxBehavior::Normal)),
            )
        })
    }

    /// Whether `edge` resizes: none of the sides it touches is flush.
    fn live(edge: ResizeEdge, tiling: Tiling) -> bool {
        let (top, bottom, left, right) = match edge {
            ResizeEdge::Top => (true, false, false, false),
            ResizeEdge::Bottom => (false, true, false, false),
            ResizeEdge::Left => (false, false, true, false),
            ResizeEdge::Right => (false, false, false, true),
            ResizeEdge::TopLeft => (true, false, true, false),
            ResizeEdge::TopRight => (true, false, false, true),
            ResizeEdge::BottomLeft => (false, true, true, false),
            ResizeEdge::BottomRight => (false, true, false, true),
        };
        !(top && tiling.top
            || bottom && tiling.bottom
            || left && tiling.left
            || right && tiling.right)
    }

    /// The pointer an edge shows.
    fn cursor(edge: ResizeEdge) -> CursorStyle {
        match edge {
            ResizeEdge::Top | ResizeEdge::Bottom => CursorStyle::ResizeUpDown,
            ResizeEdge::Left | ResizeEdge::Right => CursorStyle::ResizeLeftRight,
            ResizeEdge::TopLeft | ResizeEdge::BottomRight => CursorStyle::ResizeUpLeftDownRight,
            ResizeEdge::TopRight | ResizeEdge::BottomLeft => CursorStyle::ResizeUpRightDownLeft,
        }
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
            .child(
                self.frame(
                    v_flex()
                        .w_full()
                        .h_full()
                        .overflow_hidden()
                        .bg(background)
                        .refine_style(&self.style)
                        .when(self.transparent && self.grain, |layout| layout.child(Grain))
                        .when_some(self.content.clone(), |layout, content| {
                            layout.child(content)
                        }),
                    window,
                    cx,
                ),
            )
            .when_some(self.palette.clone(), |layout, palette| {
                layout.child(palette)
            })
            .children(self.dialogs.iter().cloned())
            .child(ToastsView::new("root-toasts", self.toasts.clone()))
            .child(AlertsView::new("root-alerts", self.alerts.clone()))
    }
}
