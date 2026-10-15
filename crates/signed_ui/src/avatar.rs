use std::fmt::Display;

use gpui::prelude::*;
use gpui::{App, Pixels, SharedString, StyleRefinement, Window, div, px};
use gpui_base::{AvatarImage, StyledExt};
use gpui_component::{ActiveTheme, Colorize, Sizable, Size};

/// Number of rows and columns in the pixel grid.
const GRID_SIZE: usize = 8;
/// Empty cells kept between the pattern and the avatar edge.
const MARGIN: usize = 1;
/// Probability that a cell in the left half of the pattern area is filled.
const FILL_PROBABILITY: f32 = 0.42;
/// Probability that a filled cell uses the accent shade instead of the main color.
const ACCENT_PROBABILITY: f32 = 0.25;
/// Minimum number of filled left-half cells.
const MIN_FILLED: usize = 5;

/// An avatar for a user or other entity.
#[derive(IntoElement)]
pub struct Avatar {
    id: SharedString,
    name: Option<SharedString>,
    picture: Option<SharedString>,
    size: Size,
    style: StyleRefinement,
}

impl Avatar {
    /// Create an avatar for `id`, a public key or any other stable string.
    ///
    /// The id seeds the pixel fallback shown when no picture is set.
    pub fn new<T>(id: T) -> Self
    where
        T: Display,
    {
        Self {
            id: id.to_string().into(),
            name: None,
            picture: None,
            size: Size::Small,
            style: StyleRefinement::default(),
        }
    }

    /// The display name of the avatar's subject, if known.
    pub fn name<T>(mut self, name: T) -> Self
    where
        T: Into<Option<SharedString>>,
    {
        self.name = name.into();
        self
    }

    /// The user's picture URL, if known.
    pub fn picture<T>(mut self, picture: Option<T>) -> Self
    where
        T: Into<SharedString>,
    {
        self.picture = picture.map(Into::into);
        self
    }
}

impl Sizable for Avatar {
    fn with_size(mut self, size: impl Into<Size>) -> Self {
        self.size = size.into();
        self
    }
}

impl Styled for Avatar {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Avatar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let radius = cx.theme().radius;

        match self.picture {
            Some(url) => div()
                .size(side_length(self.size))
                .flex_shrink_0()
                .rounded(radius)
                .overflow_hidden()
                .bg(cx.theme().secondary)
                .child(AvatarImage::new(url).size_full().rounded(radius))
                .refine_style(&self.style)
                .into_any_element(),
            None => PixelAvatar {
                seed: fnv1a(self.id.as_bytes()),
                side: side_length(self.size),
                style: self.style,
            }
            .into_any_element(),
        }
    }
}

/// A deterministic, offline pixel-art avatar.
#[derive(IntoElement)]
struct PixelAvatar {
    seed: u64,
    side: Pixels,
    style: StyleRefinement,
}

impl Styled for PixelAvatar {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for PixelAvatar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let pattern = pattern(self.seed);
        let mut cells = Vec::new();

        let hue = self.seed as f32 / u64::MAX as f32;
        let main = theme.blue.hue(hue);

        let shade = if theme.is_dark() {
            main.lightness((main.l * 1.6).min(0.95))
        } else {
            main.lightness((main.l * 0.45).max(0.18))
        };

        for row in 0..GRID_SIZE {
            for col in 0..GRID_SIZE {
                let value = pattern[row * GRID_SIZE + col];
                if value != 0 {
                    let color = if value == 2 { shade } else { main };
                    cells.push(
                        div()
                            .row_start(row as i16 + 1)
                            .row_end(row as i16 + 2)
                            .col_start(col as i16 + 1)
                            .col_end(col as i16 + 2)
                            .bg(color),
                    );
                }
            }
        }

        div()
            .grid()
            .grid_cols(GRID_SIZE as u16)
            .grid_rows(GRID_SIZE as u16)
            .size(self.side)
            .flex_shrink_0()
            .rounded(theme.radius)
            .overflow_hidden()
            .bg(main.opacity(0.16))
            .children(cells)
            .refine_style(&self.style)
    }
}

/// The rendered side length of an avatar at `size`.
fn side_length(size: Size) -> Pixels {
    match size {
        Size::XSmall => px(16.),
        Size::Small => px(24.),
        Size::Medium => px(48.),
        Size::Large => px(80.),
        Size::Size(size) => size,
    }
}

fn pattern(seed: u64) -> [u8; GRID_SIZE * GRID_SIZE] {
    let mut rng = PixelRng::new(seed);
    let mut pattern = [0u8; GRID_SIZE * GRID_SIZE];
    let mut filled = 0usize;

    // Only the inner rows and the inner left half are candidates; mirroring
    // then keeps the art within the same inset, leaving the outer ring empty.
    let art_rows = GRID_SIZE - 2 * MARGIN;
    let art_columns = GRID_SIZE / 2 - MARGIN;

    for row in MARGIN..GRID_SIZE - MARGIN {
        for col in MARGIN..GRID_SIZE / 2 {
            if rng.chance(FILL_PROBABILITY) {
                let accent = rng.chance(ACCENT_PROBABILITY);
                set_cell(&mut pattern, row, col, if accent { 2 } else { 1 });
                filled += 1;
            }
        }
    }

    if filled < MIN_FILLED {
        let total = art_rows * art_columns;
        let start = (rng.next() % total as u64) as usize;

        for offset in 0..total {
            if filled >= MIN_FILLED {
                break;
            }

            let ix = (start + offset) % total;
            let row = MARGIN + ix / art_columns;
            let col = MARGIN + ix % art_columns;

            if pattern[row * GRID_SIZE + col] == 0 {
                set_cell(&mut pattern, row, col, 1);
                filled += 1;
            }
        }
    }

    pattern
}

/// Fill `cell (row, col)` and its horizontal mirror.
fn set_cell(pattern: &mut [u8; GRID_SIZE * GRID_SIZE], row: usize, col: usize, value: u8) {
    pattern[row * GRID_SIZE + col] = value;
    pattern[row * GRID_SIZE + (GRID_SIZE - 1 - col)] = value;
}

/// FNV-1a 64-bit hash, stable across platforms and runs.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for &byte in bytes {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Tiny xorshift64* PRNG for deriving the pattern from the seed.
struct PixelRng(u64);

impl PixelRng {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn chance(&mut self, probability: f32) -> bool {
        self.next() as f32 / (u64::MAX as f32) < probability
    }
}
