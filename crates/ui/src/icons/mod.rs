//! Signed's icon set: application-defined SVG glyphs.

use std::time::Duration;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AbsoluteLength, App, Div, Hsla, IntoElement, ParentElement as _, Pixels, Rems, RenderOnce,
    SharedString, Styled as _, TimedTransformation, Transformation, Window, div, rems, svg,
};

const SMALL_ICON_SIZE: Rems = rems(0.875);
const DEFAULT_ICON_SIZE: Rems = rems(1.0);
const LARGE_ICON_SIZE: Rems = rems(1.25);
/// The spin presents at most this rate, rotating a sprite needs no more frames.
const SPIN_MAX_FPS: f32 = 60.0;

#[derive(rust_embed::RustEmbed)]
#[folder = "src/icons/assets"]
#[exclude = "*.DS_Store"]
pub struct Assets;

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
    Bell,
    Stack,
    Message,
    PanelLeft,
    UnfoldVertical,
    Download,
    Maximize,
    Minimize,
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
            Self::Bell => "bell",
            Self::Stack => "stack",
            Self::Message => "message",
            Self::PanelLeft => "panel-left",
            Self::UnfoldVertical => "unfold-vertical",
            Self::Download => "download",
            Self::Maximize => "maximize",
            Self::Minimize => "minimize",
        }
    }

    /// The bundled SVG asset path for the role.
    pub fn asset_path(self) -> &'static str {
        match self {
            Self::Check => "icons/check.svg",
            Self::Close => "icons/close.svg",
            Self::ChevronDown => "icons/chevron-down.svg",
            Self::ChevronRight => "icons/chevron-right.svg",
            Self::Plus => "icons/plus.svg",
            Self::Search => "icons/search.svg",
            Self::Folder => "icons/folder.svg",
            Self::Warning => "icons/warning.svg",
            Self::Copy => "icons/copy.svg",
            Self::Eye => "icons/eye.svg",
            Self::EyeOff => "icons/eye-off.svg",
            Self::WindowMinimize => "icons/window-minimize.svg",
            Self::WindowMaximize => "icons/window-maximize.svg",
            Self::WindowRestore => "icons/window-restore.svg",
            Self::Loader => "icons/loader.svg",
            Self::Microphone => "icons/microphone.svg",
            Self::Stop => "icons/stop.svg",
            Self::Settings => "icons/settings.svg",
            Self::Refresh => "icons/refresh.svg",
            Self::More => "icons/more.svg",
            Self::Grid => "icons/grid.svg",
            Self::File => "icons/file.svg",
            Self::Clock => "icons/clock.svg",
            Self::Unlock => "icons/unlock.svg",
            Self::CirclePlus => "icons/circle-plus.svg",
            Self::GitCommit => "icons/git-commit.svg",
            Self::GitIssueDone => "icons/git-issue-done.svg",
            Self::GitIssueClosed => "icons/git-issue-closed.svg",
            Self::GitIssueOngoing => "icons/git-issue-ongoing.svg",
            Self::GitPullRequest => "icons/git-pull-request.svg",
            Self::GitPullRequestClosed => "icons/git-pull-request-closed.svg",
            Self::GitPullRequestDraft => "icons/git-pull-request-draft.svg",
            Self::GitPullRequestMerged => "icons/git-pull-request-merged.svg",
            Self::GitClone => "icons/git-clone.svg",
            Self::GitBranch => "icons/git-branch.svg",
            Self::Markdown => "icons/markdown.svg",
            Self::Trending => "icons/trending.svg",
            Self::EllipsisVertical => "icons/ellipsis-vertical.svg",
            Self::ArrowUp => "icons/arrow-up.svg",
            Self::Inbox => "icons/inbox.svg",
            Self::Bell => "icons/bell.svg",
            Self::Stack => "icons/stack.svg",
            Self::Message => "icons/message.svg",
            Self::PanelLeft => "icons/panel-left.svg",
            Self::UnfoldVertical => "icons/unfold-vertical.svg",
            Self::Download => "icons/download.svg",
            Self::Maximize => "icons/maximize.svg",
            Self::Minimize => "icons/minimize.svg",
        }
    }
}

/// A glyph named by an application's own icon set.
pub trait IconNamed {
    /// The SVG asset path, e.g. `icons/git-commit.svg`.
    fn path(self) -> SharedString;
}

impl IconNamed for IconRole {
    fn path(self) -> SharedString {
        SharedString::from(self.asset_path())
    }
}

/// The glyph an icon draws.
#[derive(Clone)]
enum Glyph {
    Named(SharedString),
}

/// An icon element. Painted with the ambient text color unless a color is set.
#[derive(Clone, IntoElement)]
pub struct Icon {
    glyph: Glyph,
    size: AbsoluteLength,
    color: Option<Hsla>,
    transformation: Option<Transformation>,
    rotation_period: Option<Duration>,
}

impl Icon {
    /// Creates an icon from an application-defined icon set.
    pub fn named<I>(named: I) -> Self
    where
        I: IconNamed,
    {
        Self {
            glyph: Glyph::Named(named.path()),
            size: AbsoluteLength::Rems(DEFAULT_ICON_SIZE),
            color: None,
            transformation: None,
            rotation_period: None,
        }
    }

    pub fn size(mut self, size: impl Into<AbsoluteLength>) -> Self {
        self.size = size.into();
        self
    }

    /// The small size.
    pub fn small(mut self) -> Self {
        self.size = AbsoluteLength::Rems(SMALL_ICON_SIZE);
        self
    }

    /// The default medium size.
    pub fn medium(mut self) -> Self {
        self.size = AbsoluteLength::Rems(DEFAULT_ICON_SIZE);
        self
    }

    /// The large size.
    pub fn large(mut self) -> Self {
        self.size = AbsoluteLength::Rems(LARGE_ICON_SIZE);
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
    ) -> Div {
        div().flex_none().size(size).child(
            svg()
                .size_full()
                .path(path)
                .when_some(color, |el, color| el.text_color(color))
                .when_some(transformation, |el, transformation| {
                    el.with_transformation(transformation)
                })
                .when_some(rotation_period, |el, period| {
                    el.with_timed_transformation(
                        TimedTransformation::rotation(period, transformation.unwrap_or_default())
                            .with_max_fps(SPIN_MAX_FPS),
                    )
                }),
        )
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
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let Icon {
            glyph,
            size,
            color,
            transformation,
            rotation_period,
        } = self;

        let size = size.to_pixels(window.rem_size());
        let color = color.unwrap_or_else(|| window.text_style().color);
        let Glyph::Named(path) = glyph;

        Self::render_svg_path(size, Some(color), transformation, rotation_period, path)
    }
}
