use gpui::{AssetSource, Result, SharedString};
use gpui_native_kit::icons::IconNamed;
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "assets"]
#[include = "icons/**/*.svg"]
#[include = "backgrounds/**/*.jpg"]
#[include = "backgrounds/**/*.png"]
#[exclude = "*.DS_Store"]
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<std::borrow::Cow<'static, [u8]>>> {
        if let Some(file) = Self::get(path) {
            return Ok(Some(file.data));
        }
        gpui_native_kit::icons::IconAssets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths: Vec<SharedString> = Self::iter()
            .filter(|p| p.starts_with(path))
            .map(Into::into)
            .collect();
        paths.extend(gpui_native_kit::icons::IconAssets.list(path)?);
        Ok(paths)
    }
}

pub enum CustomIconName {
    CirclePlus,
    Unlock,
    Filter,
    GlobalOn,
    GlobalOff,
    GitFile,
    GitCommit,
    GitIssueDone,
    GitIssueOpen,
    GitIssueClosed,
    GitIssueOngoing,
    GitPullRequest,
    GitPullRequestClosed,
    GitPullRequestDraft,
    GitPullRequestMerged,
    GitClone,
    GitBranch,
    Tag,
    Markdown,
    Share,
    Trending,
    Recent,
    Refresh,
    Grid,
    Init,
    EllipsisVertical,
    ArrowUp,
    Trash,
}

impl IconNamed for CustomIconName {
    fn path(self) -> gpui::SharedString {
        match self {
            CustomIconName::CirclePlus => "icons/circle-plus.svg",
            CustomIconName::Unlock => "icons/unlock.svg",
            CustomIconName::Filter => "icons/filter.svg",
            CustomIconName::GlobalOn => "icons/global-on.svg",
            CustomIconName::GlobalOff => "icons/global-off.svg",
            CustomIconName::GitCommit => "icons/git-commit.svg",
            CustomIconName::GitFile => "icons/git-file.svg",
            CustomIconName::GitIssueDone => "icons/git-issue-done.svg",
            CustomIconName::GitIssueOpen => "icons/git-issue-open.svg",
            CustomIconName::GitIssueClosed => "icons/git-issue-close.svg",
            CustomIconName::GitIssueOngoing => "icons/git-issue-ongoing.svg",
            CustomIconName::GitPullRequest => "icons/git-pull-request.svg",
            CustomIconName::GitPullRequestClosed => "icons/git-pull-request-closed.svg",
            CustomIconName::GitPullRequestDraft => "icons/git-pull-request-draft.svg",
            CustomIconName::GitPullRequestMerged => "icons/git-pull-request-merged.svg",
            CustomIconName::GitClone => "icons/git-clone.svg",
            CustomIconName::GitBranch => "icons/git-branch.svg",
            CustomIconName::Tag => "icons/tag.svg",
            CustomIconName::Markdown => "icons/markdown.svg",
            CustomIconName::Share => "icons/share.svg",
            CustomIconName::Trending => "icons/trending.svg",
            CustomIconName::Refresh => "icons/refresh.svg",
            CustomIconName::Recent => "icons/recent.svg",
            CustomIconName::Grid => "icons/grid.svg",
            CustomIconName::Init => "icons/init.svg",
            CustomIconName::EllipsisVertical => "icons/ellipsis-vertical.svg",
            CustomIconName::ArrowUp => "icons/arrow-up.svg",
            CustomIconName::Trash => "icons/trash.svg",
        }
        .into()
    }
}
