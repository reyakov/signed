//! Shared helpers for the editor's views: gutter cells, role colors and run
//! styles, code line text, language mapping and change washes.

use std::ops::Range;

use gpui::{
    App, HighlightStyle, Hsla, IntoElement, ParentElement as _, Styled as _, StyledText, div,
};

use crate::components::highlight::{Language, Line, Role};
use crate::theme::{
    ActiveTheme as _, CODE_GUTTER_WIDTH, ColorTokens, Ramps, WASH_ALPHA, WASH_ALPHA_GUTTER,
    WASH_ALPHA_STRONG,
};

impl Role {
    /// Maps the role to the theme's semantic palette.
    pub(crate) fn color(self, colors: ColorTokens, ramps: Ramps) -> Hsla {
        match self {
            Role::Text => colors.foreground,
            Role::Keyword => ramps.primary.base,
            Role::String | Role::Escape => ramps.success.base,
            Role::Comment | Role::Punctuation => colors.muted_foreground,
            Role::Function | Role::Property => ramps.info.base,
            Role::Type => ramps.warning.base,
            Role::Number | Role::Constant => ramps.secondary.base,
            Role::Operator => colors.foreground,
            Role::Tag => ramps.destructive.base,
        }
    }

    /// Builds the highlight style for one run of a code view row.
    pub(crate) fn run_style(
        self,
        selected: bool,
        colors: ColorTokens,
        ramps: Ramps,
    ) -> HighlightStyle {
        HighlightStyle {
            color: Some(self.color(colors, ramps)),
            background_color: selected.then_some(colors.selection),
            ..Default::default()
        }
    }
}

impl Language {
    /// Maps a fence info string to a language.
    pub(crate) fn from_fence_info(info: &str) -> Option<Language> {
        match info.to_ascii_lowercase().as_str() {
            "plain" | "text" | "txt" => Some(Language::Plain),
            "rust" | "rs" => Some(Language::Rust),
            "typescript" | "ts" => Some(Language::TypeScript),
            "javascript" | "js" => Some(Language::JavaScript),
            "python" | "py" => Some(Language::Python),
            _ => None,
        }
    }
}

/// A gutter cell sized to the whole row.
pub(crate) fn gutter_cell<E>(cx: &App, color: Hsla, content: E) -> gpui::Div
where
    E: IntoElement,
{
    let theme = cx.theme();

    div()
        .size(CODE_GUTTER_WIDTH)
        .flex()
        .items_center()
        .justify_center()
        .flex_none()
        .font_family(theme.mono_font)
        .text_xs()
        .line_height(gpui::relative(1.0))
        .text_color(color)
        .child(content)
}

/// A change wash: the ramp color at the alpha rows tint with.
pub(crate) fn wash(color: Hsla) -> Hsla {
    Hsla {
        a: color.a * WASH_ALPHA,
        ..color
    }
}

/// The stronger wash under number gutters and intra-line word changes.
pub(crate) fn wash_strong(color: Hsla) -> Hsla {
    Hsla {
        a: color.a * WASH_ALPHA_STRONG,
        ..color
    }
}

/// The subtle wash that sets the number gutter off from the code.
pub(crate) fn wash_gutter(colors: &ColorTokens) -> Hsla {
    Hsla {
        a: colors.muted.a * WASH_ALPHA_GUTTER,
        ..colors.muted
    }
}

/// One highlighted code line as styled text, roles resolved from the theme.
pub(crate) fn code_line(line: &Line, colors: ColorTokens, ramps: Ramps) -> StyledText {
    let mut highlights: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
    let mut offset = 0usize;

    for span in &line.spans {
        let end = offset + span.text.len();
        if span.role != Role::Text {
            highlights.push((
                offset..end,
                HighlightStyle {
                    color: Some(span.role.color(colors, ramps)),
                    ..Default::default()
                },
            ));
        }
        offset = end;
    }

    StyledText::new(line.text()).with_highlights(highlights)
}
