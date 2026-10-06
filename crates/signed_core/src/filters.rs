use std::time::Duration;

use nostr::prelude::*;

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

const NOTIFICATION_KINDS: [Kind; 8] = [
    Kind::GitIssue,
    Kind::GitPullRequest,
    Kind::GitPatch,
    Kind::GitPullRequestUpdate,
    Kind::GitStatusOpen,
    Kind::GitStatusApplied,
    Kind::GitStatusClosed,
    Kind::GitStatusDraft,
];

const GIT_ROOT_KINDS: [Kind; 4] = [
    Kind::GitIssue,
    Kind::GitPatch,
    Kind::GitPullRequest,
    Kind::GitRepoAnnouncement,
];

pub fn is_repo_kind(kind: Kind) -> bool {
    kind == Kind::GitRepoAnnouncement
        || kind == Kind::RepoState
        || kind == Kind::EventDeletion
        || kind == Kind::RequestToVanish
        || ACTIVITY_KINDS.contains(&kind)
}

fn tag_kind(event: &Event, name: &str) -> Option<Kind> {
    event
        .tags
        .iter()
        .find(|tag| tag.kind() == name)
        .and_then(|tag| tag.content())
        .and_then(|value| value.parse::<Kind>().ok())
}

pub struct Filters;

impl Filters {
    const DELETIONS_LOOKBACK: Duration = Duration::from_secs(3 * 365 * 86_400);

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

    pub fn grasp_list(public_key: PublicKey) -> Filter {
        Filter::new()
            .kind(Kind::GitUserGraspList)
            .author(public_key)
    }

    /// NIP-51: code (people who produce NIP-34 events) follow list
    pub fn git_authors(public_key: PublicKey) -> Filter {
        Filter::new().kind(Kind::Custom(10017)).author(public_key)
    }

    /// NIP-51: NIP-34 followed repositories
    pub fn git_repos(public_key: PublicKey) -> Filter {
        Filter::new().kind(Kind::Custom(10018)).author(public_key)
    }

    // Replaceable events, so the latest of each kind is all we need.
    pub fn user_metadata(public_key: PublicKey) -> Vec<Filter> {
        vec![
            Self::grasp_list(public_key).limit(1),
            Self::git_authors(public_key).limit(1),
            Self::git_repos(public_key).limit(1),
            Filter::new()
                .kind(Kind::ContactList)
                .author(public_key)
                .limit(1),
            Filter::new()
                .kind(Kind::Metadata)
                .author(public_key)
                .limit(1),
            Filter::new()
                .kind(Kind::MuteList)
                .author(public_key)
                .limit(1),
            Filter::new()
                .kind(Kind::BlossomServerList)
                .author(public_key)
                .limit(1),
        ]
    }

    // Two filters: combining `#E` and `#e` would AND the conditions.
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

    fn notification_comments(me: PublicKey) -> Filter {
        Filter::new()
            .kind(Kind::Comment)
            .custom_tags(SingleLetterTag::UPPERCASE_P, [me.to_hex()])
            .custom_tags(SingleLetterTag::UPPERCASE_K, ["1621", "1617", "1618"])
    }

    // `Filter::pubkey` matches the git events' lowercase `p` tag.
    pub fn notifications(me: PublicKey) -> Vec<Filter> {
        vec![
            Self::notification_comments(me),
            Filter::new().kinds(NOTIFICATION_KINDS).pubkey(me),
        ]
    }

    // A comment on an unrelated kind matches too, so results must be filtered
    // through `GitEvent::is_git_activity` before display.
    pub fn authored_activity(me: PublicKey) -> Filter {
        Filter::new().kinds(ACTIVITY_KINDS).author(me)
    }

    pub fn all_announcements() -> Filter {
        Filter::new().kind(Kind::GitRepoAnnouncement)
    }

    pub fn all_states() -> Filter {
        Filter::new().kind(Kind::RepoState)
    }

    // Quantized to whole days so identical filters hash the same.
    fn deletions_since() -> Timestamp {
        let now = Timestamp::now().as_secs();
        Timestamp::from_secs(now - now % 86_400) - Self::DELETIONS_LOOKBACK
    }

    // Deletion requests must be known before any other event is shown.
    pub fn deletions() -> Filter {
        Filter::new()
            .kinds([Kind::EventDeletion, Kind::RequestToVanish])
            .since(Self::deletions_since())
    }
}

pub(crate) fn is_git_comment(event: &Event) -> bool {
    event.kind == Kind::Comment
        && tag_kind(event, "K").is_some_and(|kind| GIT_ROOT_KINDS.contains(&kind))
}

pub(crate) fn is_git_status(event: &Event) -> bool {
    tag_kind(event, "k").is_some_and(|kind| GIT_ROOT_KINDS.contains(&kind))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GitEvent;

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

        assert!(on_git.is_git_activity());
        assert!(on_repo.is_git_activity());
        assert!(!on_note.is_git_activity());
        assert!(!signed(&keys(1), Kind::Comment, Vec::new()).is_git_activity());
    }

    #[test]
    fn status_activity_depends_on_the_lowercase_k_tag() {
        let status = signed(
            &keys(1),
            Kind::GitStatusClosed,
            vec![kind_tag("k", Kind::GitPullRequest)],
        );
        let unrelated = signed(
            &keys(1),
            Kind::GitStatusClosed,
            vec![kind_tag("k", Kind::Metadata)],
        );

        assert!(status.is_git_activity());
        assert!(!unrelated.is_git_activity());
        assert!(!signed(&keys(1), Kind::GitStatusClosed, Vec::new()).is_git_activity());
    }
}
