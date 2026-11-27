//! Renders a user avatar that shows a picture when set and a deterministic
//! pixel fallback derived from the id otherwise.
//!
//! # Examples
//!
//! ```no_run
//! use ui::avatar::Avatar;
//!
//! let avatar = Avatar::new("user-42")
//!     .name("Ada Lovelace")
//!     .picture(Some("https://example.com/ada.png"));
//! ```

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Display;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, Bounds, ElementId, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    RenderOnce, Role, SharedString, StatefulInteractiveElement as _, StyleRefinement, Styled,
    Window, canvas, div, fill, img, point, size,
};

use crate::components::common::{ControlSizing, Sizable, StyledExt, h_flex};
use crate::theme::{
    ActiveTheme as _, Oklch, PIXEL_AVATAR_DARK_FACTOR, PIXEL_AVATAR_DARK_FLOOR,
    PIXEL_AVATAR_LIGHT_CAP, PIXEL_AVATAR_LIGHT_LIFT, PIXEL_AVATAR_TINT_ALPHA, Size, ThemeVariant,
};

/// Number of rows and columns in the pixel grid.
const GRID_SIZE: usize = 8;
/// Empty cells kept between the pattern and the avatar edge.
const MARGIN: usize = 1;
/// Probability that a cell in the left half of the pattern area is filled.
const FILL_PROBABILITY: f32 = 0.42;
/// Probability that a filled cell uses the shade instead of the main color.
const SHADE_PROBABILITY: f32 = 0.25;
/// Minimum number of filled left-half cells.
const MIN_FILLED: usize = 5;

// Computed identicon patterns per seed, so avatars do not reroll their PRNG every frame.
thread_local! {
    static PATTERN_CACHE: RefCell<HashMap<u64, [u8; GRID_SIZE * GRID_SIZE]>> =
        RefCell::new(HashMap::new());
}

/// An avatar for a user or other entity.
#[derive(IntoElement)]
#[must_use]
pub struct Avatar {
    id: SharedString,
    name: Option<SharedString>,
    picture: Option<SharedString>,
    size: Size,
    fully_rounded: bool,
    style: StyleRefinement,
}

impl Avatar {
    /// Creates an avatar for `id`, a public key or any other stable string.
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
            size: Size::default(),
            fully_rounded: false,
            style: StyleRefinement::default(),
        }
    }

    /// Sets the display name used as the image's accessibility label.
    pub fn name<N>(mut self, name: N) -> Self
    where
        N: Into<SharedString>,
    {
        self.name = Some(name.into());
        self
    }

    /// Sets the picture URL; `None` shows the pixel fallback.
    pub fn picture<P>(mut self, picture: Option<P>) -> Self
    where
        P: Into<SharedString>,
    {
        self.picture = picture.map(Into::into);
        self
    }

    /// Renders the avatar circular instead of the theme's small radius.
    pub fn fully_rounded(mut self, fully_rounded: bool) -> Self {
        self.fully_rounded = fully_rounded;
        self
    }
}

impl Sizable for Avatar {
    fn apply_size(&mut self, size: Size) {
        self.size = size;
    }
}

impl Styled for Avatar {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Avatar {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let size = self.size;
        let side = size.avatar_rems() * window.rem_size();

        let radius = if self.fully_rounded {
            theme.radius.full
        } else {
            theme.radius.sm
        };

        let has_picture = self.picture.is_some();
        let picture = self.picture;
        let seed = PixelAvatar::seed(&self.id);

        h_flex()
            .id(ElementId::from(self.id.clone()))
            .flex_none()
            .justify_center()
            .overflow_hidden()
            .avatar_size(size)
            .bg(theme.colors.secondary)
            .rounded(radius)
            .role(Role::Image)
            .aria_label(self.name.clone().unwrap_or_else(|| self.id.clone()))
            .refine_style(&self.style)
            .children(picture.map(|url| img(url).size_full().rounded(radius)))
            .when(!has_picture, |avatar| {
                avatar.child(PixelAvatar {
                    seed,
                    side,
                    radius,
                    fully_rounded: self.fully_rounded,
                })
            })
    }
}

/// A deterministic, offline pixel-art avatar.
#[derive(IntoElement)]
struct PixelAvatar {
    seed: u64,
    side: Pixels,
    radius: Pixels,
    fully_rounded: bool,
}

impl PixelAvatar {
    /// Hashes `id` into the pattern seed.
    fn seed(id: &str) -> u64 {
        // FNV-1a 64-bit hash, stable across platforms and runs.
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for &byte in id.as_bytes() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }

    /// The pattern for `seed`, computed once.
    fn pattern(seed: u64) -> [u8; GRID_SIZE * GRID_SIZE] {
        let cached = PATTERN_CACHE.with(|cache| cache.borrow().get(&seed).copied());
        if let Some(cached) = cached {
            return cached;
        }
        let computed = PixelRng::new(seed).pattern();
        PATTERN_CACHE.with(|cache| cache.borrow_mut().insert(seed, computed));
        computed
    }
}

impl RenderOnce for PixelAvatar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let pattern = Self::pattern(self.seed);

        let hue = (self.seed as f32 / u64::MAX as f32) * 360.0;
        let accent = Oklch::from_hsla(theme.colors.accent);
        let main = Oklch { h: hue, ..accent }.to_hsla();

        let shade = if theme.variant == ThemeVariant::Dark {
            Oklch {
                l: (accent.l * PIXEL_AVATAR_LIGHT_LIFT).min(PIXEL_AVATAR_LIGHT_CAP),
                ..Oklch::from_hsla(main)
            }
            .to_hsla()
        } else {
            Oklch {
                l: (accent.l * PIXEL_AVATAR_DARK_FACTOR).max(PIXEL_AVATAR_DARK_FLOOR),
                ..Oklch::from_hsla(main)
            }
            .to_hsla()
        };

        let inset = if self.fully_rounded {
            self.side / GRID_SIZE as f32
        } else {
            Pixels::ZERO
        };

        div()
            .flex_none()
            .size(self.side)
            .p(inset)
            .rounded(self.radius)
            .bg(Hsla {
                a: PIXEL_AVATAR_TINT_ALPHA,
                ..main
            })
            .child(
                canvas(
                    |bounds, _, _| bounds.size,
                    move |bounds, area, window, _| {
                        let cell = area.width / GRID_SIZE as f32;
                        for (index, value) in pattern.iter().enumerate() {
                            if *value == 0 {
                                continue;
                            }
                            let color = if *value == 2 { shade } else { main };
                            let top_left = bounds.origin
                                + point(
                                    (index % GRID_SIZE) as f32 * cell,
                                    (index / GRID_SIZE) as f32 * cell,
                                );
                            window.paint_quad(fill(
                                Bounds {
                                    origin: top_left,
                                    size: size(cell, cell),
                                },
                                color,
                            ));
                        }
                    },
                )
                .size_full(),
            )
    }
}

/// Tiny xorshift64* PRNG for deriving the pattern from the seed.
struct PixelRng(u64);

impl PixelRng {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    /// Fills the left half of the art area at random and mirrors it.
    fn pattern(&mut self) -> [u8; GRID_SIZE * GRID_SIZE] {
        /// Fill `cell (row, col)` and its horizontal mirror.
        fn set_cell(pattern: &mut [u8; GRID_SIZE * GRID_SIZE], row: usize, col: usize, value: u8) {
            pattern[row * GRID_SIZE + col] = value;
            pattern[row * GRID_SIZE + (GRID_SIZE - 1 - col)] = value;
        }

        let mut pattern = [0u8; GRID_SIZE * GRID_SIZE];
        let mut filled = 0usize;

        let art_rows = GRID_SIZE - 2 * MARGIN;
        let art_columns = GRID_SIZE / 2 - MARGIN;

        for row in MARGIN..GRID_SIZE - MARGIN {
            for col in MARGIN..GRID_SIZE / 2 {
                if self.chance(FILL_PROBABILITY) {
                    let shade = self.chance(SHADE_PROBABILITY);
                    set_cell(&mut pattern, row, col, if shade { 2 } else { 1 });
                    filled += 1;
                }
            }
        }

        if filled < MIN_FILLED {
            let total = art_rows * art_columns;
            let start = (self.next() % total as u64) as usize;

            for offset in 0..total {
                if filled >= MIN_FILLED {
                    break;
                }

                let flat_index = (start + offset) % total;
                let row = MARGIN + flat_index / art_columns;
                let col = MARGIN + flat_index % art_columns;

                if pattern[row * GRID_SIZE + col] == 0 {
                    set_cell(&mut pattern, row, col, 1);
                    filled += 1;
                }
            }
        }

        pattern
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
