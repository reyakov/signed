use std::time::Duration;

use nostr::prelude::*;

use crate::RepoAddr;

/// Kinds that make up the activity of a repository.
pub const ACTIVITY_KINDS: [Kind; 9] = [
    Kind::Comment,
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
        .author(addr.public_key)
        .identifier(addr.identifier.clone())
}

/// Latest state event for a repository, carrying refs and HEAD.
pub fn state(addr: &RepoAddr) -> Filter {
    Filter::new()
        .kind(Kind::RepoState)
        .author(addr.public_key)
        .identifier(addr.identifier.clone())
}

/// All NIP-34 activity addressed to a repository via its `#a` tag.
/// Covers issues, PRs, patches, statuses and kind-1111 comments.
/// The `a` tag is optional on status events per NIP-34.
/// Statuses published without it are not matched here.
pub fn activity(addr: &RepoAddr) -> Filter {
    Filter::new().kinds(ACTIVITY_KINDS).coordinate(addr)
}

/// Status events, kinds `1630..=1633`, referencing any of the given root events.
/// They are matched via the `#e` tag. One filter covers all roots.
///
/// A negentropy sync reconciles them in a single session, not one per root.
pub fn statuses_for(roots: impl IntoIterator<Item = EventId>) -> Filter {
    Filter::new()
        .kinds([
            Kind::GitStatusOpen,
            Kind::GitStatusApplied,
            Kind::GitStatusClosed,
            Kind::GitStatusDraft,
        ])
        .events(roots)
}

/// Cover notes and NIP-32 label events referencing any of the given root events.
/// These are kinds 1624 and 1985, matched via the `#e` tag.
///
/// Because they carry no repository `a` tag, they are fetched by root like comments.
///
/// Batched, like [`statuses_for`].
pub fn annotations_for(roots: impl IntoIterator<Item = EventId>) -> Filter {
    Filter::new()
        .kinds([crate::COVER_NOTE_KIND, Kind::Label])
        .events(roots)
}

/// A user's grasp list, kind `10317`.
pub fn grasp_list(public_key: PublicKey) -> Filter {
    Filter::new()
        .kind(Kind::GitUserGraspList)
        .author(public_key)
}

/// NIP-22 comments, kind `1111`, referencing any of the given root events.
/// The roots are issues, patches and PRs.
///
/// Returns two filters, since combining `#E` and `#e` would AND the conditions.
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

/// All repository announcements, for global discovery.
pub fn all_announcements() -> Filter {
    Filter::new().kind(Kind::GitRepoAnnouncement)
}

/// How far back deletion requests are fetched and stored.
const DELETIONS_LOOKBACK: Duration = Duration::from_secs(3 * 365 * 86_400);

/// `now` minus [`DELETIONS_LOOKBACK`].
/// Quantized to whole days so identical filters hash the same.
///
/// This lets the backend's sync dedup match identical filters.
fn deletions_since() -> Timestamp {
    let now = Timestamp::now().as_secs();
    Timestamp::from_secs(now - now % 86_400) - DELETIONS_LOOKBACK
}

/// All deletion-related events within [`DELETIONS_LOOKBACK`].
/// These are NIP-09 kind `5` and NIP-62 kind `62`.
///
/// Deletion requests must be known before any other event is shown.
pub fn deletions() -> Filter {
    Filter::new()
        .kinds([Kind::EventDeletion, Kind::RequestToVanish])
        .since(deletions_since())
}

/// Deletion events relevant to a single repository.
///
/// Requests authored by the repository owner.
///
/// Requests addressed to the repository coordinate via its `#a` tag.
pub fn deletions_for_repo(addr: &RepoAddr) -> Vec<Filter> {
    vec![
        Filter::new()
            .kinds([Kind::EventDeletion, Kind::RequestToVanish])
            .author(addr.public_key),
        Filter::new().kind(Kind::EventDeletion).coordinate(addr),
    ]
}
