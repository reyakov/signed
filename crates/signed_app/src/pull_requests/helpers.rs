use gpui::SharedString;
use signed_core::RepoStatus;

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

pub fn pr_label(repo_name: &SharedString, pr_id: &str) -> SharedString {
    if repo_name.is_empty() {
        SharedString::from(pr_id)
    } else {
        SharedString::from(format!("{repo_name}/{pr_id}"))
    }
}

use std::path::PathBuf;

use nostr::prelude::PublicKey;
use signed_core::Announcement;
use utils::middle_truncate;

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

pub fn fork_display_name(announcement: &Announcement) -> SharedString {
    announcement
        .name
        .as_deref()
        .map(SharedString::from)
        .unwrap_or_else(|| SharedString::from(announcement.id.clone()))
}

pub fn shorten_owner(owner: &PublicKey) -> String {
    owner.to_hex().chars().take(10).collect()
}

pub fn truncate_label(label: &str) -> SharedString {
    SharedString::from(middle_truncate(label, 18, 0))
}
