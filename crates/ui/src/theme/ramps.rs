//! OKLCH color math for accent-ramp derivation and contrast-aware
//! foreground selection. Matrices follow Björn Ottosson's OKLab reference.
// The reference matrices are quoted at their published precision; f32
// truncation is deliberate, not a mistake.
#![allow(clippy::excessive_precision)]

use gpui::{Hsla, Rgba};

/// A color in the OKLCH space (perceptually uniform):
/// lightness, chroma, hue in degrees, plus the alpha carried through unchanged.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Oklch {
    pub l: f32,
    pub c: f32,
    pub h: f32,
    pub a: f32,
}

impl Oklch {
    pub fn from_hsla(color: Hsla) -> Self {
        let Rgba { r, g, b, a } = Rgba::from(color);
        let (l, c, h) = rgb_to_oklch(r, g, b);
        Self { l, c, h, a }
    }

    pub fn to_hsla(self) -> Hsla {
        let (r, g, b) = oklch_to_rgb(self.l, self.c, self.h);
        Hsla::from(Rgba { r, g, b, a: self.a })
    }

    /// Shifts lightness by `amount` (positive is lighter),
    /// and scales chroma by `chroma_scale`, preserving hue and alpha.
    pub fn shifted(self, amount: f32, chroma_scale: f32) -> Self {
        Self {
            l: (self.l + amount).clamp(0.0, 1.0),
            c: (self.c * chroma_scale).max(0.0),
            h: self.h,
            a: self.a,
        }
    }
}

/// The state shades components paint from for one accent color.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AccentShades {
    pub base: Hsla,
    pub hover: Hsla,
    pub pressed: Hsla,
    pub subtle: Hsla,
    pub foreground: Hsla,
}

/// Mixes `a` toward `b` by `t` in OKLCH, taking the shortest arc between hues.
///
/// `t = 0` returns `a`, `t = 1` returns `b`.
pub fn mix(a: Oklch, b: Oklch, t: f32) -> Oklch {
    let mut dh = b.h - a.h;

    if dh > 180.0 {
        dh -= 360.0;
    } else if dh < -180.0 {
        dh += 360.0;
    }

    Oklch {
        l: a.l + (b.l - a.l) * t,
        c: a.c + (b.c - a.c) * t,
        h: (a.h + dh * t + 360.0) % 360.0,
        a: a.a + (b.a - a.a) * t,
    }
}

fn rgb_to_oklch(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let (r, g, b) = (linearize(r), linearize(g), linearize(b));
    let l = 0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b;
    let m = 0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b;
    let s = 0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b;

    let (l_, m_, s_) = (cbrt(l), cbrt(m), cbrt(s));
    let lab_l = 0.2104542553 * l_ + 0.7936177850 * m_ - 0.0040720468 * s_;
    let lab_a = 1.9779984951 * l_ - 2.4285922050 * m_ + 0.4505937099 * s_;
    let lab_b = 0.0259040371 * l_ + 0.7827717662 * m_ - 0.8086757660 * s_;

    let c = (lab_a * lab_a + lab_b * lab_b).sqrt();
    let mut h = lab_b.atan2(lab_a).to_degrees();

    if h < 0.0 {
        h += 360.0;
    }

    (lab_l, c, h)
}

fn oklch_to_rgb(l: f32, c: f32, h: f32) -> (f32, f32, f32) {
    let hr = h.to_radians();
    let lab_a = c * hr.cos();
    let lab_b = c * hr.sin();

    let l_ = l + 0.3963377774 * lab_a + 0.2158037573 * lab_b;
    let m_ = l - 0.1055613458 * lab_a - 0.0638541728 * lab_b;
    let s_ = l - 0.0894841775 * lab_a - 1.2914855480 * lab_b;

    let l = l_ * l_ * l_;
    let m = m_ * m_ * m_;
    let s = s_ * s_ * s_;

    let r = 4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s;
    let g = -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s;
    let b = -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s;

    (delinearize(r), delinearize(g), delinearize(b))
}

fn linearize(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn delinearize(c: f32) -> f32 {
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn cbrt(v: f32) -> f32 {
    if v >= 0.0 { v.cbrt() } else { -(-v).cbrt() }
}

/// Calculate the foreground color that provides sufficient contrast against the background.
pub fn contrast_foreground(background: Hsla) -> Hsla {
    const MIN_CONTRAST: f32 = 3.0;

    let Rgba { r, g, b, .. } = Rgba::from(background);

    let luminance = 0.2126 * linearize(r) + 0.7152 * linearize(g) + 0.0722 * linearize(b);
    let white_contrast = 1.05 / (luminance + 0.05);
    let black_contrast = (luminance + 0.05) / 0.05;

    let white = Hsla::from(Rgba {
        r: 1.0,
        g: 1.0,
        b: 1.0,
        a: 1.0,
    });

    let black = Hsla::from(Rgba {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    });

    if white_contrast >= MIN_CONTRAST || white_contrast >= black_contrast {
        white
    } else {
        black
    }
}

/// Derives hover/pressed/subtle shades for an accent against its background.
///
/// Where a platform states ramps (WinUI, KDE) those are used instead,
/// every other platform derives its shades here.
pub fn derive_shades(accent: Hsla, background: Hsla, is_dark: bool) -> AccentShades {
    let accent_ok = Oklch::from_hsla(accent);
    let step = if is_dark { 0.05 } else { -0.05 };
    let bg = Oklch::from_hsla(background);
    AccentShades {
        base: accent,
        hover: accent_ok.shifted(step, 1.05).to_hsla(),
        pressed: accent_ok.shifted(step * 2.0, 1.1).to_hsla(),
        subtle: mix(accent_ok, bg, 0.88).to_hsla(),
        foreground: contrast_foreground(accent),
    }
}
