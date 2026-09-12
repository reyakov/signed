use gpui::prelude::*;
use gpui::{App, SharedString, StyleRefinement, Window};
use gpui_component::avatar::Avatar;
use gpui_component::{ActiveTheme, Sizable, Size, StyledExt};

/// A small user avatar from gpui-component [`Avatar`], rounded with the theme radius.
/// It shows the user's picture or falls back to name initials.
#[derive(IntoElement)]
pub struct UserAvatar {
    name: SharedString,
    picture: Option<SharedString>,
    size: Size,
    style: StyleRefinement,
}

impl UserAvatar {
    /// Create an avatar for `name`.
    /// The name seeds the initials fallback shown when no picture is set.
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

impl Sizable for UserAvatar {
    fn with_size(mut self, size: impl Into<Size>) -> Self {
        self.size = size.into();
        self
    }
}

impl Styled for UserAvatar {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for UserAvatar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        Avatar::new()
            .name(self.name)
            .when_some(self.picture, |this, url| this.src(url))
            .rounded(cx.theme().radius)
            .refine_style(&self.style)
            .with_size(self.size)
    }
}
