//! The client-side window frame: the border, corners, shadow, and resize
//! edges of a window the system does not decorate.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, Bounds, BoxShadow, CursorStyle, Decorations, Div, HitboxBehavior, InteractiveElement as _,
    IntoElement, MouseButton, ParentElement as _, Pixels, Point, ResizeEdge, Size, Styled as _,
    Tiling, Window, canvas, div, point, px, size,
};

use crate::theme::{ActiveTheme as _, BORDER_WIDTH, INACTIVE_SHADOW_OPACITY, WINDOW_RESIZE_INSET};

/// Wraps a window's root in the frame it owes.
pub fn frame<E>(child: E, window: &mut Window, cx: &App) -> Div
where
    E: IntoElement,
{
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

    let shadow = frame_shadow(cx, window.is_window_active());
    window.set_client_inset(inset);

    div()
        .size_full()
        .child(
            canvas(
                move |_, window, _| {
                    let size = window.window_bounds().get_bounds().size;
                    bands(size, inset)
                        .into_iter()
                        .filter(|(edge, _)| live(*edge, tiling))
                        .map(|(edge, band)| {
                            (edge, window.insert_hitbox(band, HitboxBehavior::Normal))
                        })
                        .collect::<Vec<_>>()
                },
                |_, bands, window, _| {
                    for (edge, band) in bands {
                        window.set_cursor_style(cursor(edge), &band);
                    }
                },
            )
            .absolute()
            .size_full(),
        )
        .when(!tiling.top, |frame| frame.pt(inset))
        .when(!tiling.bottom, |frame| frame.pb(inset))
        .when(!tiling.left, |frame| frame.pl(inset))
        .when(!tiling.right, |frame| frame.pr(inset))
        .on_mouse_down(MouseButton::Left, move |event, window, _| {
            let size = window.window_bounds().get_bounds().size;
            if let Some(edge) =
                resize_edge(event.position, inset, size).filter(|edge| live(*edge, tiling))
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

/// The eight bands, as the rects [`resize_edge`] classifies to.
fn bands(window: Size<Pixels>, inset: Pixels) -> [(ResizeEdge, Bounds<Pixels>); 8] {
    let band = |x: Pixels, y: Pixels, width: Pixels, height: Pixels| Bounds {
        origin: point(x, y),
        size: size(width, height),
    };

    let (far_x, far_y) = (window.width - inset, window.height - inset);
    let (span_x, span_y) = (window.width - inset - inset, window.height - inset - inset);
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
    !(top && tiling.top || bottom && tiling.bottom || left && tiling.left || right && tiling.right)
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
