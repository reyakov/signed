use gpui::prelude::*;
use gpui::{App, SharedString, StyleRefinement, Window};
use gpui_base::{Avatar as BaseAvatar, AvatarFallback, AvatarImage};
use gpui_component::{ActiveTheme, Sizable, Size, StyledExt};

use crate::pixel_avatar::{PixelAvatar, side_length};

/// A user avatar built on the unstyled [`gpui_base::Avatar`].
///
/// It shows the user's picture when one is set, and falls back to the
/// deterministic pixel avatar seeded from the name otherwise.
#[derive(IntoElement)]
pub struct Avatar {
    name: SharedString,
    picture: Option<SharedString>,
    size: Size,
    style: StyleRefinement,
}

impl Avatar {
    /// Create an avatar for `name`.
    ///
    /// The name seeds the pixel fallback shown when no picture is set.
    pub fn new(name: impl Into<SharedString>) -> Self {
        Self {
            name: name.into(),
            picture: None,
            size: Size::Small,
            style: StyleRefinement::default(),
        }
    }

    /// The user's picture URL, if known.
    pub fn picture(mut self, picture: Option<impl Into<SharedString>>) -> Self {
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
        let fallback = AvatarFallback::new()
            .size_full()
            .child(PixelAvatar::new(self.name.clone()).size_full());

        BaseAvatar::new()
            .size(side_length(self.size))
            .flex_shrink_0()
            .rounded(cx.theme().radius)
            .overflow_hidden()
            .bg(cx.theme().secondary)
            .when_some(self.picture, |this, url| {
                this.image(AvatarImage::new(url).size_full().rounded(cx.theme().radius))
            })
            .fallback(fallback)
            .refine_style(&self.style)
    }
}
