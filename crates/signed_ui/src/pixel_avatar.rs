use gpui::prelude::*;
use gpui::{App, Pixels, StyleRefinement, Window, div, px};
use gpui_base::StyledExt;
use gpui_component::{ActiveTheme, Colorize};

/// Number of rows and columns in the pixel grid.
const GRID_SIZE: usize = 8;
/// Probability that a cell in the left half is filled.
const FILL_PROBABILITY: f32 = 0.42;
/// Probability that a filled cell uses the accent shade instead of the main color.
const ACCENT_PROBABILITY: f32 = 0.25;
/// Minimum number of filled left-half cells.
/// A sparse roll still yields a recognizable shape.
/// Each left-half cell is mirrored to a right-half one.
const MIN_FILLED: usize = 5;

/// Side length of the avatar in pixels, no setter.
const AVATAR_SIZE: Pixels = px(16.);

/// A deterministic, offline pixel-art avatar.
/// An 8×8 grid with horizontal mirror symmetry.
/// Seeded from a stable string such as the repository id and owner public key.
/// The same seed always renders the same avatar.
#[derive(IntoElement)]
pub struct PixelAvatar {
    seed: u64,
    style: StyleRefinement,
}

impl PixelAvatar {
    /// Create an avatar seeded from `seed`.
    /// The seed should be a stable string unique to the entity the avatar represents.
    pub fn new(seed: impl AsRef<str>) -> Self {
        Self {
            seed: fnv1a(seed.as_ref().as_bytes()),
            style: StyleRefinement::default(),
        }
    }
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

        let hue = self.seed as f32 / u64::MAX as f32;
        let main = theme.blue.hue(hue);
        let shade = if theme.is_dark() {
            main.lightness((main.l * 1.6).min(0.95))
        } else {
            main.lightness((main.l * 0.45).max(0.18))
        };

        let mut cells = Vec::new();
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
            .refine_style(&self.style)
            .grid()
            .grid_cols(GRID_SIZE as u16)
            .grid_rows(GRID_SIZE as u16)
            .size(AVATAR_SIZE)
            .flex_shrink_0()
            .overflow_hidden()
            .bg(main.opacity(0.16))
            .children(cells)
    }
}

/// Generate the 8×8 cell pattern for `seed`.
/// Cells are `0` for empty, `1` for main color and `2` for accent shade.
/// The right half mirrors the left half.
fn pattern(seed: u64) -> [u8; GRID_SIZE * GRID_SIZE] {
    let mut rng = PixelRng::new(seed);
    let mut pattern = [0u8; GRID_SIZE * GRID_SIZE];
    let mut filled = 0usize;

    for row in 0..GRID_SIZE {
        for col in 0..GRID_SIZE / 2 {
            if rng.chance(FILL_PROBABILITY) {
                let accent = rng.chance(ACCENT_PROBABILITY);
                set_cell(&mut pattern, row, col, if accent { 2 } else { 1 });
                filled += 1;
            }
        }
    }

    // Sparse rolls can come out nearly empty.
    // Top the pattern up to the minimum fill, scanning from a seeded starting cell.
    if filled < MIN_FILLED {
        let half = GRID_SIZE * GRID_SIZE / 2;
        let start = (rng.next() % half as u64) as usize;
        for offset in 0..half {
            if filled >= MIN_FILLED {
                break;
            }
            let ix = (start + offset) % half;
            let row = ix / (GRID_SIZE / 2);
            let col = ix % (GRID_SIZE / 2);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn count_filled(pattern: &[u8; GRID_SIZE * GRID_SIZE]) -> usize {
        pattern.iter().filter(|&&cell| cell != 0).count()
    }

    #[test]
    fn pattern_is_mirror_symmetric() {
        for seed in 0..50 {
            let pattern = pattern(seed);
            for row in 0..GRID_SIZE {
                for col in 0..GRID_SIZE {
                    assert_eq!(
                        pattern[row * GRID_SIZE + col],
                        pattern[row * GRID_SIZE + (GRID_SIZE - 1 - col)],
                        "asymmetric pattern for seed {seed} at ({row}, {col})"
                    );
                }
            }
        }
    }

    #[test]
    fn pattern_has_minimum_fill() {
        for seed in 0..50 {
            let pattern = pattern(seed);
            assert!(
                count_filled(&pattern) >= MIN_FILLED * 2,
                "pattern too sparse for seed {seed}"
            );
        }
    }

    #[test]
    fn pattern_is_deterministic() {
        for seed in [0, 1, 42, u64::MAX] {
            assert_eq!(pattern(seed), pattern(seed));
        }
        assert_ne!(pattern(42), pattern(43));
    }

    #[test]
    fn fnv1a_is_stable_and_distinct() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"repo"), fnv1a(b"repo"));
        assert_ne!(fnv1a(b"repo:a"), fnv1a(b"repo:b"));
    }
}
