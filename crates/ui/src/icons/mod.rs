//! Signed's icon set: application-defined SVG glyphs.

use std::time::Duration;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AssetSource, Hsla, IntoElement, ParentElement as _, Pixels, RenderOnce,
    SharedString, Styled as _, TimedTransformation, Transformation, Window, div, px, svg,
};

const SMALL_ICON_SIZE: Pixels = px(14.0);
const DEFAULT_ICON_SIZE: Pixels = px(16.0);
const LARGE_ICON_SIZE: Pixels = px(20.0);
/// The spin presents at most this rate, rotating a sprite needs no more frames.
const SPIN_MAX_FPS: f32 = 60.0;

/// The semantic glyphs UI components need.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconRole {
    Check,
    Close,
    ChevronDown,
    ChevronRight,
    Plus,
    Search,
    Folder,
    Warning,
    Copy,
    Eye,
    EyeOff,
    WindowMinimize,
    WindowMaximize,
    WindowRestore,
    Loader,
    Microphone,
    Stop,
    Settings,
    Refresh,
    More,
    Grid,
    File,
    Clock,
    Unlock,
    CirclePlus,
    GitCommit,
    GitIssueDone,
    GitIssueClosed,
    GitIssueOngoing,
    GitPullRequest,
    GitPullRequestClosed,
    GitPullRequestDraft,
    GitPullRequestMerged,
    GitClone,
    GitBranch,
    Markdown,
    Trending,
    EllipsisVertical,
    ArrowUp,
    Inbox,
    Stack,
}

impl IconRole {
    /// The stable identifier used for element and debug ids.
    pub fn slug(self) -> &'static str {
        match self {
            Self::Check => "check",
            Self::Close => "close",
            Self::ChevronDown => "chevron-down",
            Self::ChevronRight => "chevron-right",
            Self::Plus => "plus",
            Self::Search => "search",
            Self::Folder => "folder",
            Self::Warning => "warning",
            Self::Copy => "copy",
            Self::Eye => "eye",
            Self::EyeOff => "eye-off",
            Self::WindowMinimize => "window-minimize",
            Self::WindowMaximize => "window-maximize",
            Self::WindowRestore => "window-restore",
            Self::Loader => "loader",
            Self::Microphone => "microphone",
            Self::Stop => "stop",
            Self::Settings => "settings",
            Self::Refresh => "refresh",
            Self::More => "more",
            Self::Grid => "grid",
            Self::File => "file",
            Self::Clock => "clock",
            Self::Unlock => "unlock",
            Self::CirclePlus => "circle-plus",
            Self::GitCommit => "git-commit",
            Self::GitIssueDone => "git-issue-done",
            Self::GitIssueClosed => "git-issue-closed",
            Self::GitIssueOngoing => "git-issue-ongoing",
            Self::GitPullRequest => "git-pull-request",
            Self::GitPullRequestClosed => "git-pull-request-closed",
            Self::GitPullRequestDraft => "git-pull-request-draft",
            Self::GitPullRequestMerged => "git-pull-request-merged",
            Self::GitClone => "git-clone",
            Self::GitBranch => "git-branch",
            Self::Markdown => "markdown",
            Self::Trending => "trending",
            Self::EllipsisVertical => "ellipsis-vertical",
            Self::ArrowUp => "arrow-up",
            Self::Inbox => "inbox",
            Self::Stack => "stack",
        }
    }

    /// The bundled SVG asset path for the role.
    fn asset_path(self) -> SharedString {
        SharedString::from(format!("icons/{}.svg", self.slug()))
    }
}

/// A glyph named by an application's own icon set.
pub trait IconNamed {
    /// The SVG asset path, e.g. `icons/git-commit.svg`.
    fn path(self) -> SharedString;
}

impl IconNamed for IconRole {
    fn path(self) -> SharedString {
        self.asset_path()
    }
}

/// The glyph an icon draws.
#[derive(Clone)]
enum Glyph {
    Role(IconRole),
    Named(SharedString),
}

/// An icon element. Painted with the ambient text color unless a color is set.
#[derive(Clone, IntoElement)]
pub struct Icon {
    glyph: Glyph,
    size: Pixels,
    color: Option<Hsla>,
    transformation: Option<Transformation>,
    rotation_period: Option<Duration>,
}

/// Creates an icon for the role at the default size.
pub fn icon(role: IconRole) -> Icon {
    Icon {
        glyph: Glyph::Role(role),
        size: DEFAULT_ICON_SIZE,
        color: None,
        transformation: None,
        rotation_period: None,
    }
}

impl Icon {
    /// Creates an icon from an application-defined icon set.
    pub fn named<I>(named: I) -> Self
    where
        I: IconNamed,
    {
        Self {
            glyph: Glyph::Named(named.path()),
            size: DEFAULT_ICON_SIZE,
            color: None,
            transformation: None,
            rotation_period: None,
        }
    }

    pub fn size(mut self, size: Pixels) -> Self {
        self.size = size;
        self
    }

    /// The small size.
    pub fn small(mut self) -> Self {
        self.size = SMALL_ICON_SIZE;
        self
    }

    /// The default medium size.
    pub fn medium(mut self) -> Self {
        self.size = DEFAULT_ICON_SIZE;
        self
    }

    /// The large size.
    pub fn large(mut self) -> Self {
        self.size = LARGE_ICON_SIZE;
        self
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    /// An extra static SVG transformation.
    pub fn with_transformation(mut self, transformation: Transformation) -> Self {
        self.transformation = Some(transformation);
        self
    }

    /// Spins the icon clockwise, one full turn per `period`.
    pub fn with_rotation_animation(mut self, period: Duration) -> Self {
        self.rotation_period = Some(period);
        self
    }

    fn render_svg_path(
        size: Pixels,
        color: Option<Hsla>,
        transformation: Option<Transformation>,
        rotation_period: Option<Duration>,
        path: SharedString,
    ) -> AnyElement {
        div()
            .flex_none()
            .size(size)
            .child(
                svg()
                    .size_full()
                    .path(path)
                    .when_some(color, |el, color| el.text_color(color))
                    .when_some(transformation, |el, transformation| {
                        el.with_transformation(transformation)
                    })
                    .when_some(rotation_period, |el, period| {
                        el.with_timed_transformation(
                            TimedTransformation::rotation(
                                period,
                                transformation.unwrap_or_default(),
                            )
                            .with_max_fps(SPIN_MAX_FPS),
                        )
                    }),
            )
            .into_any_element()
    }
}

impl<I> From<I> for Icon
where
    I: IconNamed,
{
    fn from(named: I) -> Self {
        Icon::named(named)
    }
}

impl RenderOnce for Icon {
    fn render(self, window: &mut Window, _cx: &mut gpui::App) -> impl IntoElement {
        let Icon {
            glyph,
            size,
            color,
            transformation,
            rotation_period,
        } = self;

        let color = color.unwrap_or_else(|| window.text_style().color);

        match glyph {
            Glyph::Named(path) => {
                Self::render_svg_path(size, Some(color), transformation, rotation_period, path)
            }
            Glyph::Role(role) => Self::render_svg_path(
                size,
                Some(color),
                transformation,
                rotation_period,
                role.asset_path(),
            ),
        }
    }
}

#[derive(rust_embed::RustEmbed)]
#[folder = "src/icons/assets"]
#[exclude = "*.DS_Store"]
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        let name = match path.strip_prefix("icons/") {
            Some(name) => name,
            None => return Ok(None),
        };
        Ok(Assets::get(name).map(|file| file.data))
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(Self::iter()
            .map(|file| SharedString::from(format!("icons/{file}")))
            .filter(|file| file.starts_with(path))
            .collect())
    }
}
