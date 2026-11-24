use std::path::PathBuf;
use std::sync::Arc;

use gpui::{SharedString, WeakEntity};
use nostr::prelude::PublicKey;
use signed_core::{Announcement, RepoAddr, RepoStatus};
use ui::menu::MenuItem;
use utils::middle_truncate;

use super::NewPullRequestView;
use crate::repo::helpers::announcement_display_name;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullRequestFilter {
    All,
    Open,
    Closed,
    Draft,
    Merged,
}

impl PullRequestFilter {
    pub fn matches(self, status: RepoStatus) -> bool {
        match self {
            Self::All => true,
            Self::Open => status == RepoStatus::Open,
            Self::Closed => status == RepoStatus::Closed,
            Self::Draft => status == RepoStatus::Draft,
            Self::Merged => status == RepoStatus::Applied,
        }
    }

    pub fn empty_message(self) -> &'static str {
        match self {
            Self::All => "No pull requests",
            Self::Open => "No open pull requests",
            Self::Closed => "No closed pull requests",
            Self::Draft => "No draft pull requests",
            Self::Merged => "No merged pull requests",
        }
    }
}

pub struct ForkCompare {
    pub announcement: Announcement,
    pub namespace: String,
    pub mirror_path: PathBuf,
}

impl ForkCompare {
    pub fn base_ref(name: &str) -> String {
        format!("refs/remotes/origin/{name}")
    }

    pub fn compare_ref(&self, name: &str) -> String {
        format!("refs/fork/{}/{}", self.namespace, name)
    }
}

pub fn shorten_owner(owner: &PublicKey) -> String {
    owner.to_hex().chars().take(10).collect()
}

pub fn truncate_label(label: &str) -> SharedString {
    SharedString::from(middle_truncate(label, 18, 0))
}

pub fn source_menu_items(
    view: WeakEntity<NewPullRequestView>,
    associated: Arc<Vec<PathBuf>>,
    active_path: Option<PathBuf>,
    candidates: Arc<Vec<Announcement>>,
    user: Option<PublicKey>,
    active_fork: Option<RepoAddr>,
) -> Vec<MenuItem> {
    let mut items = Vec::new();
    for (index, path) in associated.iter().enumerate() {
        let label = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());

        let target = view.clone();
        let associated = Arc::clone(&associated);

        items.push(
            MenuItem::new(truncate_label(&label))
                .checked(active_path.as_ref() == Some(path))
                .handler(move |window, cx| {
                    let Some(path) = associated.get(index).cloned() else {
                        return;
                    };
                    if let Err(error) =
                        target.update(cx, |this, cx| this.apply_folder_path(path, window, cx))
                    {
                        log::warn!("failed to apply checkout: {error}");
                    }
                }),
        );
    }

    let folder_view = view.clone();
    items.push(
        MenuItem::new("Choose another folder…").handler(move |window, cx| {
            if let Err(error) = folder_view.update(cx, |this, cx| this.choose_checkout(window, cx))
            {
                log::warn!("failed to choose checkout: {error}");
            }
        }),
    );
    items.push(MenuItem::separator());

    if candidates.is_empty() {
        items.push(MenuItem::new("No announced forks of this repository").disabled(true));
    } else {
        for (index, candidate) in candidates.iter().enumerate() {
            let subtitle = if Some(candidate.owner) == user {
                SharedString::from("Your fork")
            } else {
                SharedString::from(format!("by {}", shorten_owner(&candidate.owner)))
            };

            let title = format!("{} — {}", announcement_display_name(candidate), subtitle);
            let target = view.clone();
            let candidates = Arc::clone(&candidates);
            let checked = active_fork == Some(candidate.addr());

            items.push(
                MenuItem::new(truncate_label(&title))
                    .checked(checked)
                    .handler(move |window, cx| {
                        let Some(announcement) = candidates.get(index).cloned() else {
                            return;
                        };
                        if let Err(error) =
                            target.update(cx, |this, cx| this.choose_fork(announcement, window, cx))
                        {
                            log::warn!("failed to choose fork: {error}");
                        }
                    }),
            );
        }
    }

    items
}
