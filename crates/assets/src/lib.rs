use anyhow::Context;
use gpui::{App, AssetSource, Result, SharedString};
use gpui_component::IconNamed;
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "assets"]
#[include = "icons/**/*.svg"]
#[include = "themes/**/*.json"]
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
    /// Returns the embedded theme files as `(file name, JSON content)` pairs,
    /// e.g. `("signed.json", ...)`. The content is a `ThemeSet` that can be
    /// loaded into the [`ThemeRegistry`](gpui_component::ThemeRegistry).
    pub fn themes(&self) -> Vec<(String, String)> {
        Self::iter()
            .filter(|path| path.starts_with("themes/"))
            .filter_map(|path| {
                let data = Self::get(path.as_ref())?;
                let name = path.strip_prefix("themes/").unwrap_or(path.as_ref());
                // Debug builds read files from disk (owned), release builds
                // embed them in the binary (borrowed).
                let content = match data.data {
                    std::borrow::Cow::Borrowed(bytes) => {
                        std::str::from_utf8(bytes).ok()?.to_owned()
                    }
                    std::borrow::Cow::Owned(bytes) => String::from_utf8(bytes).ok()?,
                };
                Some((name.to_owned(), content))
            })
            .collect()
    }

    pub fn load_fonts(&self, cx: &App) -> anyhow::Result<()> {
        let font_paths = self.list("fonts")?;
        let mut embedded_fonts = Vec::new();
        for font_path in font_paths {
            if font_path.ends_with(".ttf") {
                let font_bytes = cx
                    .asset_source()
                    .load(&font_path)?
                    .expect("Assets should never return None");
                embedded_fonts.push(font_bytes);
            }
        }

        cx.text_system().add_fonts(embedded_fonts)
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
        }
        .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signed_theme_set() -> gpui_component::ThemeSet {
        let themes = Assets.themes();
        assert_eq!(themes.len(), 1, "expected exactly one embedded theme file");
        let (name, content) = &themes[0];
        assert_eq!(name, "signed.json");
        serde_json::from_str(content).expect("theme file must be a valid ThemeSet")
    }

    #[test]
    fn signed_theme_set_parses() {
        let set = signed_theme_set();
        let names: Vec<&str> = set.themes.iter().map(|t| t.name.as_ref()).collect();
        assert_eq!(names, vec!["Signed Light", "Signed Dark"]);
    }

    #[test]
    fn signed_theme_palette_applies() {
        let set = signed_theme_set();
        let parse = |hex: &str| gpui_component::try_parse_color(hex).unwrap();
        for config in &set.themes {
            let mut theme = gpui_component::Theme::default();
            theme.apply_config(&std::rc::Rc::new(config.clone()));

            assert_eq!(theme.mode, config.mode);
            // The resolved colors must match the brand palette.
            assert_eq!(theme.primary, parse("#C6FF4D")); // nostr-lime
            assert_eq!(theme.success, parse("#2FBF71")); // merge
            assert_eq!(theme.primary_active, parse("#65A30D")); // lime-600

            if config.mode.is_dark() {
                // Dark theme chrome is neutral, mirroring the light theme.
                assert_eq!(theme.background, parse("#0A0A0A")); // neutral-950
                assert_eq!(theme.border, parse("#27272A")); // neutral-800
                assert_eq!(theme.green, parse("#22C55E")); // green-500
            } else {
                // Light theme chrome is neutral; lime is a brand accent only.
                assert_eq!(theme.background, parse("#FFFFFF"));
                assert_eq!(theme.foreground, parse("#18181B"));
                assert_eq!(theme.border, parse("#E4E4E7"));
                assert_eq!(theme.green, parse("#16A34A"));
            }
            // Active tab: a paler lime on light, a dim moss on dark — each
            // paired with readable, contrasting text.
            if config.mode.is_dark() {
                assert_eq!(theme.tab_active, parse("#19200A")); // dim lime
                assert_eq!(theme.tab_active_foreground, parse("#C6FF4D")); // nostr-lime
            } else {
                assert_eq!(theme.tab_active, parse("#EBFFC1")); // pale nostr-lime
                assert_eq!(theme.tab_active_foreground, parse("#3F6212")); // deep-lime
            }
        }
    }
}
