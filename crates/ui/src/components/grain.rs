//! Renders tiled film-grain noise over its container.
//!
//! # Examples
//!
//! ```no_run
//! use gpui::{ParentElement as _, Styled as _, div};
//! use ui::grain::grain;
//!
//! let overlay = grain();
//! let surface = div().relative().child(overlay);
//! ```

use std::sync::{Arc, OnceLock};

use gpui::{
    App, Bounds, Corners, IntoElement, RenderImage, RenderOnce, Styled as _, Window, canvas, point,
    px, size,
};
use image::{Frame, ImageBuffer, Rgba};

use crate::components::common::ResultLogErr as _;
use crate::theme::GRAIN_ALPHA;

/// The side of one noise tile, in device pixels.
const TILE_PX: u32 = 256;

/// A film-grain overlay that tiles monochrome noise at [`GRAIN_ALPHA`] strength.
#[derive(IntoElement)]
#[must_use]
pub struct Grain;

impl RenderOnce for Grain {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let texture = texture();
        canvas(
            |_, _, _| (),
            move |bounds, (), window, _| {
                let tile = px(TILE_PX as f32);
                let columns = (bounds.size.width / tile).ceil() as u32;
                let rows = (bounds.size.height / tile).ceil() as u32;

                for row in 0..rows {
                    for column in 0..columns {
                        let x = bounds.origin.x + column as f32 * tile;
                        let y = bounds.origin.y + row as f32 * tile;
                        let tile_bounds = Bounds {
                            origin: point(x, y),
                            size: size(
                                tile.min(bounds.origin.x + bounds.size.width - x),
                                tile.min(bounds.origin.y + bounds.size.height - y),
                            ),
                        };
                        window
                            .paint_image(
                                tile_bounds,
                                tile_bounds,
                                Corners::default(),
                                texture.clone(),
                                0,
                                false,
                            )
                            .log_err();
                    }
                }
            },
        )
        .absolute()
        .inset_0()
    }
}

/// Creates a grain overlay.
pub fn grain() -> Grain {
    Grain
}

/// Returns the shared noise tile so every tile reuses one atlas entry.
fn texture() -> Arc<RenderImage> {
    static TEXTURE: OnceLock<Arc<RenderImage>> = OnceLock::new();

    TEXTURE
        .get_or_init(|| {
            // An LCG keeps the grain deterministic across launches.
            let mut state = 0x9E37_79B9_u32;
            let mut noise = move || {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 24) as u8
            };
            // The grays are noise samples, not palette colors.
            let alpha = (GRAIN_ALPHA * 255.0) as u8;
            Arc::new(RenderImage::new([Frame::new(ImageBuffer::from_fn(
                TILE_PX,
                TILE_PX,
                |_, _| {
                    let gray = noise();
                    Rgba([gray, gray, gray, alpha])
                },
            ))]))
        })
        .clone()
}
