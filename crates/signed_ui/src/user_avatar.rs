use gpui::prelude::*;
use gpui::{App, SharedString, StyleRefinement, Window};
use gpui_component::avatar::Avatar;
use gpui_component::{ActiveTheme, Sizable, StyledExt};

/// A user avatar: the gpui-component [`Avatar`] sized small and rounded with
/// the theme radius, showing the user's picture or a name-initials fallback.
#[derive(IntoElement)]
pub struct UserAvatar {
    name: SharedString,
    picture: Option<SharedString>,
    style: StyleRefinement,
}

impl UserAvatar {
    /// Create an avatar for `name`; the name seeds the initials fallback
    /// shown when no picture is set.
    pub fn new(name: impl Into<SharedString>) -> Self {
        Self {
            name: name.into(),
            picture: None,
            style: StyleRefinement::default(),
        }
    }

    /// The user's picture URL, if known.
    pub fn picture(mut self, picture: Option<impl Into<SharedString>>) -> Self {
        self.picture = picture.map(Into::into);
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
            .small()
    }
}
