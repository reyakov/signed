use anyhow::Context;
use gpui::{AssetSource, Result, SharedString};
use gpui_component::IconNamed;
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "assets"]
#[include = "icons/**/*.svg"]
#[include = "themes/**/*.json"]
#[include = "backgrounds/**/*.jpg"]
#[include = "backgrounds/**/*.png"]
#[exclude = "*.DS_Store"]
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<std::borrow::Cow<'static, [u8]>>> {
        Self::get(path)
            .map(|f| Some(f.data))
            .with_context(|| format!("loading asset at path {path:?}"))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(Self::iter()
            .filter_map(|p| {
                if p.starts_with(path) {
                    Some(p.into())
                } else {
                    None
                }
            })
            .collect())
    }
}

impl Assets {
    pub fn themes(&self) -> Vec<(String, String)> {
        Self::iter()
            .filter(|path| path.starts_with("themes/"))
            .filter_map(|path| {
                let data = Self::get(path.as_ref())?;
                let name = path.strip_prefix("themes/").unwrap_or(path.as_ref());
                let content = std::str::from_utf8(data.data.as_ref()).ok()?.to_owned();
                Some((name.to_owned(), content))
            })
            .collect()
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
        }
        .into()
    }
}
