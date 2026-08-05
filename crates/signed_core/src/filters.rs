use nostr::filter::{Alphabet, SingleLetterTag};
use nostr::prelude::*;

use crate::RepoAddr;

/// Kinds that make up the activity of a repository.
pub const ACTIVITY_KINDS: [Kind; 8] = [
    Kind::GitPatch,
    Kind::GitPullRequest,
    Kind::GitPullRequestUpdate,
    Kind::GitIssue,
    Kind::GitStatusOpen,
    Kind::GitStatusApplied,
    Kind::GitStatusClosed,
    Kind::GitStatusDraft,
];

/// Latest announcement event for a repository.
pub fn announcement(addr: &RepoAddr) -> Filter {
    Filter::new()
        .kind(Kind::GitRepoAnnouncement)
        .author(addr.owner)
        .identifier(addr.id.clone())
}

/// Latest state event (refs / HEAD) for a repository.
pub fn state(addr: &RepoAddr) -> Filter {
    Filter::new()
        .kind(Kind::RepoState)
        .author(addr.owner)
        .identifier(addr.id.clone())
}

/// All NIP-34 activity addressed to a repository (`#a` tag).
///
/// Note: the `a` tag on status events is optional per NIP-34, so statuses
/// published without it won't be matched here.
pub fn activity(addr: &RepoAddr) -> Filter {
    Filter::new()
        .kinds(ACTIVITY_KINDS)
        .custom_tag(SingleLetterTag::lowercase(Alphabet::A), addr.to_string())
}

/// Status events (`1630..=1633`) referencing a specific root event (`#e` tag).
pub fn statuses_for(root: EventId) -> Filter {
    Filter::new()
        .kinds([
            Kind::GitStatusOpen,
            Kind::GitStatusApplied,
            Kind::GitStatusClosed,
            Kind::GitStatusDraft,
        ])
        .event(root)
}

/// A user's grasp list (kind `10317`).
pub fn grasp_list(public_key: PublicKey) -> Filter {
    Filter::new()
        .kind(Kind::GitUserGraspList)
        .author(public_key)
}

/// All repositories announced by an author.
pub fn announcements_by(public_key: PublicKey) -> Filter {
    Filter::new()
        .kind(Kind::GitRepoAnnouncement)
        .author(public_key)
}

/// All repository announcements (for global discovery).
pub fn all_announcements(limit: usize) -> Filter {
    Filter::new()
        .kind(Kind::GitRepoAnnouncement)
        .limit(limit)
}
