use std::time::Duration;

use nostr::prelude::*;

use crate::{COVER_NOTE_KIND, RepoAddr};

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

/// Kinds that notify a user when they tag them via their `p` tag.
pub const NOTIFICATION_KINDS: [Kind; 9] = [
    Kind::GitIssue,
    Kind::GitPullRequest,
    Kind::GitPatch,
    Kind::GitPullRequestUpdate,
    COVER_NOTE_KIND,
    Kind::GitStatusOpen,
    Kind::GitStatusApplied,
    Kind::GitStatusClosed,
    Kind::GitStatusDraft,
];

/// Git root kinds that make a comment or cover note count as git activity.
const GIT_ROOT_KINDS: [Kind; 4] = [
    Kind::GitIssue,
    Kind::GitPatch,
    Kind::GitPullRequest,
    Kind::GitRepoAnnouncement,
];

/// Value of the first tag named `name` on `event`.
fn tag_value<'a>(event: &'a Event, name: &str) -> Option<&'a str> {
    event
        .tags
        .iter()
        .find(|tag| tag.kind() == name)
        .and_then(|tag| tag.content())
}

/// Kind named by the first tag `name` on `event`.
fn tag_kind(event: &Event, name: &str) -> Option<Kind> {
    tag_value(event, name)?.parse::<Kind>().ok()
}

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

/// NIP-22 comments on our issues, patches and pull requests.
/// They are matched via the uppercase `P` and `K` tags, not authorship.
pub fn notification_comments(me: PublicKey) -> Filter {
    Filter::new()
        .kind(Kind::Comment)
        .custom_tags(SingleLetterTag::UPPERCASE_P, [me.to_hex()])
        .custom_tags(SingleLetterTag::UPPERCASE_K, ["1621", "1617", "1618"])
}

/// Activity directed at us: comments on our roots, and git events tagging us
/// via their lowercase `p` tag. `Filter::pubkey` sets that `p` tag.
pub fn notifications(me: PublicKey) -> Vec<Filter> {
    vec![
        notification_comments(me),
        Filter::new().kinds(NOTIFICATION_KINDS).pubkey(me),
    ]
}

/// Git activity authored by `me`, for "Continue where you left off".
///
/// A comment on an unrelated kind is matched too, so results must be filtered
/// through [`is_git_activity`] before display.
pub fn authored_activity(me: PublicKey) -> Filter {
    Filter::new()
        .kinds(
            ACTIVITY_KINDS
                .into_iter()
                .chain(std::iter::once(COVER_NOTE_KIND)),
        )
        .author(me)
}

/// Whether a kind-1111 comment targets a git root, checked via its `K` tag.
fn is_git_comment(event: &Event) -> bool {
    event.kind == Kind::Comment
        && tag_kind(event, "K").is_some_and(|kind| GIT_ROOT_KINDS.contains(&kind))
}

/// Whether a kind-1624 cover note targets a git root, checked via its `k` tag.
fn is_git_cover_note(event: &Event) -> bool {
    event.kind == COVER_NOTE_KIND
        && tag_kind(event, "k").is_some_and(|kind| GIT_ROOT_KINDS.contains(&kind))
}

/// Whether a status event references a git root, checked via its `k` tag.
fn is_git_status(event: &Event) -> bool {
    tag_kind(event, "k").is_some_and(|kind| GIT_ROOT_KINDS.contains(&kind))
}

/// Whether `event` is git activity worth showing in the activity list.
pub fn is_git_activity(event: &Event) -> bool {
    match event.kind {
        Kind::GitIssue | Kind::GitPatch | Kind::GitPullRequest => true,
        Kind::Comment => is_git_comment(event),
        Kind::GitStatusOpen
        | Kind::GitStatusApplied
        | Kind::GitStatusClosed
        | Kind::GitStatusDraft => is_git_status(event),
        kind => kind == COVER_NOTE_KIND && is_git_cover_note(event),
    }
}

/// All repository announcements, for global discovery.
pub fn all_announcements() -> Filter {
    Filter::new().kind(Kind::GitRepoAnnouncement)
}

/// All repository state events, carrying each repository's refs and last push time.
pub fn all_states() -> Filter {
    Filter::new().kind(Kind::RepoState)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(seed: u8) -> Keys {
        let mut hex = "00000000000000000000000000000000000000000000000000000000000000".to_string();
        hex.push_str(&format!("{seed:02x}"));
        Keys::new(SecretKey::from_hex(&hex).expect("valid secret key"))
    }

    fn signed(author: &Keys, kind: Kind, tags: Vec<Tag>) -> Event {
        EventBuilder::new(kind, "")
            .tags(tags)
            .finalize(author)
            .expect("signed event")
    }

    fn kind_tag(name: &str, kind: Kind) -> Tag {
        Tag::parse([name, &kind.as_u16().to_string()]).expect("valid kind tag")
    }

    #[test]
    fn comment_activity_depends_on_the_uppercase_k_tag() {
        let on_git = signed(&keys(1), Kind::Comment, vec![kind_tag("K", Kind::GitIssue)]);
        let on_repo = signed(
            &keys(1),
            Kind::Comment,
            vec![kind_tag("K", Kind::GitRepoAnnouncement)],
        );
        let on_note = signed(&keys(1), Kind::Comment, vec![kind_tag("K", Kind::TextNote)]);

        assert!(is_git_activity(&on_git));
        assert!(is_git_activity(&on_repo));
        assert!(!is_git_activity(&on_note));
        assert!(!is_git_activity(&signed(
            &keys(1),
            Kind::Comment,
            Vec::new()
        )));
    }

    #[test]
    fn status_and_cover_note_activity_depend_on_the_lowercase_k_tag() {
        let status = signed(
            &keys(1),
            Kind::GitStatusClosed,
            vec![kind_tag("k", Kind::GitPullRequest)],
        );
        let cover = signed(
            &keys(1),
            COVER_NOTE_KIND,
            vec![kind_tag("k", Kind::GitPatch)],
        );
        let unrelated = signed(
            &keys(1),
            Kind::GitStatusClosed,
            vec![kind_tag("k", Kind::Metadata)],
        );

        assert!(is_git_activity(&status));
        assert!(is_git_activity(&cover));
        assert!(!is_git_activity(&unrelated));
        assert!(!is_git_activity(&signed(
            &keys(1),
            Kind::GitStatusClosed,
            Vec::new()
        )));
    }
}
