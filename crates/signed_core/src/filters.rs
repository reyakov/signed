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

/// Status events (`1630..=1633`) referencing any of the given root events
/// (`#e` tag). Batched: one filter covers all roots, so a negentropy sync
/// reconciles them in a single session instead of one per root.
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

/// Cover notes (kind 1624) and NIP-32 label events (kind 1985) referencing
/// any of the given root events (`#e` tag), fetched per root like comments
/// and statuses because they carry no repository `a` tag. Batched, like
/// [`statuses_for`].
pub fn annotations_for(roots: impl IntoIterator<Item = EventId>) -> Filter {
    Filter::new()
        .kinds([crate::COVER_NOTE_KIND, Kind::Label])
        .events(roots)
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
/// Comments carry no repository `a` tag, so they must be fetched by their
/// root reference. NIP-22 defines the uppercase `E` tag as the thread root
/// (used by ngit), but some clients (including Signed) use a lowercase `e`
/// tag, so both are matched.
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

/// How far back deletion requests are fetched and stored.
///
/// A deletion request can only target events created before it, and NIP-34
/// events are all far younger than this window, so older requests can never
/// match anything shown. Bounding the window keeps the kind-5/62 set (one of
/// the largest on public relays) from being fully reconciled on every sync.
const DELETIONS_LOOKBACK: Duration = Duration::from_secs(3 * 365 * 86_400);

/// `now` minus [`DELETIONS_LOOKBACK`], quantized to whole days so identical
/// filters hash the same and the backend's sync dedup can match them.
fn deletions_since() -> Timestamp {
    let now = Timestamp::now().as_secs();
    Timestamp::from_secs(now - now % 86_400) - DELETIONS_LOOKBACK
}

/// All deletion-related events (NIP-09 kind `5`, NIP-62 kind `62`) within
/// [`DELETIONS_LOOKBACK`]. Deletion requests must be known before any other
/// event can be shown.
pub fn deletions() -> Filter {
    Filter::new()
        .kinds([Kind::EventDeletion, Kind::RequestToVanish])
        .since(deletions_since())
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
