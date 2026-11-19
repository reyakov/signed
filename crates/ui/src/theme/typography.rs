//! One type ladder shared by every platform, scaled by the theme's root font
//! size.

use gpui::{App, FontWeight, Pixels, Styled, px};

use crate::theme::ActiveTheme;

/// The shared ladder at a 16px root: size, line height, then weight.
const LADDER: [Rung; 11] = [
    (24.0, 32.0, FontWeight::SEMIBOLD),
    (20.0, 28.0, FontWeight::SEMIBOLD),
    (18.0, 24.0, FontWeight::SEMIBOLD),
    (16.0, 24.0, FontWeight::SEMIBOLD),
    (14.0, 20.0, FontWeight::SEMIBOLD),
    (16.0, 24.0, FontWeight::NORMAL),
    (14.0, 20.0, FontWeight::NORMAL),
    (14.0, 20.0, FontWeight::NORMAL),
    (13.0, 18.0, FontWeight::NORMAL),
    (12.0, 16.0, FontWeight::NORMAL),
    (12.0, 16.0, FontWeight::MEDIUM),
];

/// One rung: size and line height at a 16px root, then the weight.
type Rung = (f32, f32, FontWeight);

/// A role in the unified type ladder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextRole {
    /// The largest heading — hero panels, greeters, welcome screens.
    LargeTitle,
    /// Window and page titles.
    Title,
    /// Section titles.
    Title2,
    /// Sub-section and dialog headings.
    Title3,
    /// Body-sized emphasis, list row titles, group labels.
    Headline,
    /// Control labels and reading text.
    Body,
    /// Slightly smaller body text.
    Callout,
    /// Secondary lines under a body row.
    Subheadline,
    /// Hints and auxiliary notes.
    Footnote,
    /// Timestamps, badges, and dense metadata.
    Caption,
    /// Emphasized captions.
    Caption2,
}

/// One role as it paints: size, line height, and weight.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TypeMetrics {
    pub size: Pixels,
    pub line_height: Pixels,
    pub weight: FontWeight,
}

/// The unified type ladder, resolved at the theme's root font size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Typography {
    pub large_title: TypeMetrics,
    pub title: TypeMetrics,
    pub title2: TypeMetrics,
    pub title3: TypeMetrics,
    pub headline: TypeMetrics,
    pub body: TypeMetrics,
    pub callout: TypeMetrics,
    pub subheadline: TypeMetrics,
    pub footnote: TypeMetrics,
    pub caption: TypeMetrics,
    pub caption2: TypeMetrics,
}

impl Typography {
    pub(crate) fn resolve(root_font_size: Pixels) -> Self {
        let scale = root_font_size / px(16.);

        let metrics = |(size, line_height, weight): Rung| TypeMetrics {
            size: px(size) * scale,
            line_height: px(line_height) * scale,
            weight,
        };

        let [
            large_title,
            title,
            title2,
            title3,
            headline,
            body,
            callout,
            subheadline,
            footnote,
            caption,
            caption2,
        ] = LADDER.map(metrics);

        Self {
            large_title,
            title,
            title2,
            title3,
            headline,
            body,
            callout,
            subheadline,
            footnote,
            caption,
            caption2,
        }
    }

    /// The metrics for `role`.
    pub fn get(&self, role: TextRole) -> TypeMetrics {
        match role {
            TextRole::LargeTitle => self.large_title,
            TextRole::Title => self.title,
            TextRole::Title2 => self.title2,
            TextRole::Title3 => self.title3,
            TextRole::Headline => self.headline,
            TextRole::Body => self.body,
            TextRole::Callout => self.callout,
            TextRole::Subheadline => self.subheadline,
            TextRole::Footnote => self.footnote,
            TextRole::Caption => self.caption,
            TextRole::Caption2 => self.caption2,
        }
    }
}

/// Paints a [`TextRole`] from the resolved theme, on anything styled.
pub trait Typeset: Styled + Sized {
    /// Sizes, leads, and weights the element as `role`.
    fn typeset(self, role: TextRole, cx: &App) -> Self {
        let theme = cx.theme();
        let metrics = theme.typography.get(role);
        self.text_size(metrics.size)
            .line_height(metrics.line_height)
            .font_weight(metrics.weight)
    }
}

impl<E: Styled> Typeset for E {}
