use nostr::prelude::*;

use crate::RepoAddr;

/// Kinds that make up the activity of a repository.
pub const ACTIVITY_KINDS: [Kind; 9] = [
    Kind::GitPatch,
    Kind::GitPullRequest,
    Kind::GitPullRequestUpdate,
    Kind::GitIssue,
    Kind::Comment,
    Kind::GitStatusOpen,
    Kind::GitStatusApplied,
    Kind::GitStatusClosed,
    Kind::GitStatusDraft,
];

/// Latest announcement event for a repository.
pub fn announcement(addr: &RepoAddr) -> Filter {
    Filter::new()
        .kind(Kind::GitRepoAnnouncement)
        .author(addr.public_key)
        .identifier(addr.identifier.clone())
}

/// Latest state event (refs / HEAD) for a repository.
pub fn state(addr: &RepoAddr) -> Filter {
    Filter::new()
        .kind(Kind::RepoState)
        .author(addr.public_key)
        .identifier(addr.identifier.clone())
}

/// All NIP-34 activity addressed to a repository (`#a` tag): issues, PRs,
/// patches, statuses and comments (kind 1111).
///
/// Note: the `a` tag on status events is optional per NIP-34, so statuses
/// published without it won't be matched here.
pub fn activity(addr: &RepoAddr) -> Filter {
    Filter::new().kinds(ACTIVITY_KINDS).coordinate(addr)
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

/// Cover notes (kind 1624) and NIP-32 label events (kind 1985) referencing a
/// specific root event (`#e` tag), fetched per root like comments and
/// statuses because they carry no repository `a` tag.
pub fn annotations_for(root: EventId) -> Filter {
    Filter::new()
        .kinds([crate::COVER_NOTE_KIND, Kind::Label])
        .event(root)
}

/// A user's grasp list (kind `10317`).
pub fn grasp_list(public_key: PublicKey) -> Filter {
    Filter::new()
        .kind(Kind::GitUserGraspList)
        .author(public_key)
}

/// NIP-22 comments (kind `1111`) referencing any of the given root events
/// (issues, patches, PRs).
///
/// Comments are not addressed to the repository — they carry no `a` tag with
/// the repo coordinate — so they must be fetched by their root reference
/// instead. NIP-22 defines the uppercase `E` tag as the root of the thread
/// (used by ngit) while some clients (including Signed itself) reference the
/// root with a lowercase `e` tag, so both are matched.
///
/// Returns two filters because `#E` and `#e` conditions would be ANDed if
/// combined into one.
pub fn comments_for(roots: impl IntoIterator<Item = EventId>) -> Vec<Filter> {
    let roots: Vec<String> = roots.into_iter().map(|id| id.to_hex()).collect();
    if roots.is_empty() {
        return Vec::new();
    }
    vec![
        Filter::new()
            .kind(Kind::Comment)
            .custom_tags(SingleLetterTag::UPPERCASE_E, roots.clone()),
        Filter::new()
            .kind(Kind::Comment)
            .custom_tags(SingleLetterTag::LOWERCASE_E, roots),
    ]
}

/// All repositories announced by an author.
pub fn announcements_by(public_key: PublicKey) -> Filter {
    Filter::new()
        .kind(Kind::GitRepoAnnouncement)
        .author(public_key)
}

/// All repository announcements (for global discovery).
///
/// Unbounded: intended for negentropy sync, which reconciles sets
/// efficiently regardless of size. Local database queries with this
/// filter are served by LMDB, so they stay fast as the database grows.
pub fn all_announcements() -> Filter {
    Filter::new().kind(Kind::GitRepoAnnouncement)
}

/// All deletion-related events (NIP-09 kind `5`, NIP-62 kind `62`).
///
/// Unbounded, like [`all_announcements`]: deletion requests must be known
/// before any other event can be shown.
pub fn deletions() -> Filter {
    Filter::new().kinds([Kind::EventDeletion, Kind::RequestToVanish])
}

/// Deletion events relevant to a single repository: requests authored by
/// the repository owner and requests addressed to the repository
/// coordinate (`#a` tag).
pub fn deletions_for_repo(addr: &RepoAddr) -> Vec<Filter> {
    vec![
        Filter::new()
            .kinds([Kind::EventDeletion, Kind::RequestToVanish])
            .author(addr.public_key),
        Filter::new().kind(Kind::EventDeletion).coordinate(addr),
    ]
}
