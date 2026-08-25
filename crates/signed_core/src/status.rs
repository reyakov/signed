use nostr::prelude::*;

/// Status of a root patch, pull request or issue (kinds `1630..=1633`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RepoStatus {
    Open,
    Applied,
    Closed,
    Draft,
}

impl RepoStatus {
    pub fn from_kind(kind: Kind) -> Option<Self> {
        match kind {
            Kind::GitStatusOpen => Some(Self::Open),
            Kind::GitStatusApplied => Some(Self::Applied),
            Kind::GitStatusClosed => Some(Self::Closed),
            Kind::GitStatusDraft => Some(Self::Draft),
            _ => None,
        }
    }

    pub fn kind(self) -> Kind {
        match self {
            Self::Open => Kind::GitStatusOpen,
            Self::Applied => Kind::GitStatusApplied,
            Self::Closed => Kind::GitStatusClosed,
            Self::Draft => Kind::GitStatusDraft,
        }
    }
}

/// Check whether an event references the given root event via an `e` or `E`
/// tag. NIP-10 / NIP-34 use the lowercase `e` tag; NIP-22 comments (kind
/// `1111`) use the uppercase `E` tag for the root of the thread.
pub fn references_root(event: &Event, root: &EventId) -> bool {
    let root = root.to_hex();
    event
        .tags
        .iter()
        .any(|tag| matches!(tag.kind(), "e" | "E") && tag.content() == Some(root.as_str()))
}

/// Resolve the status of a root event per NIP-34:
/// the most recent status event from the root author or a maintainer wins.
/// Defaults to [`RepoStatus::Open`].
pub fn resolve_status<'a, I>(
    status_events: I,
    root_author: &PublicKey,
    maintainers: &[PublicKey],
) -> RepoStatus
where
    I: IntoIterator<Item = &'a Event>,
{
    status_events
        .into_iter()
        .filter(|e| RepoStatus::from_kind(e.kind).is_some())
        .filter(|e| &e.pubkey == root_author || maintainers.contains(&e.pubkey))
        .max_by_key(|e| e.created_at)
        .and_then(|e| RepoStatus::from_kind(e.kind))
        .unwrap_or(RepoStatus::Open)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT_ID_HEX: &str = "1111111111111111111111111111111111111111111111111111111111111111";
    const OTHER_ID_HEX: &str = "2222222222222222222222222222222222222222222222222222222222222222";

    fn keys_from_hex(hex: &str) -> Keys {
        Keys::new(SecretKey::from_hex(hex).expect("valid secret key"))
    }

    fn root_event_id() -> EventId {
        EventId::from_hex(ROOT_ID_HEX).expect("valid event id")
    }

    /// Build a signed status event with a controlled `created_at`.
    fn status_event(author: &Keys, kind: Kind, root: EventId, created_at: u64) -> Event {
        EventBuilder::new(kind, "")
            .tags([Tag::event(root)])
            .custom_created_at(Timestamp::from(created_at))
            .finalize(author)
            .expect("signed event")
    }

    #[test]
    fn references_root_matches_e_tag() {
        let root = root_event_id();
        let event = EventBuilder::new(Kind::GitStatusOpen, "")
            .tags([Tag::event(root)])
            .finalize(&keys_from_hex(
                "0000000000000000000000000000000000000000000000000000000000000001",
            ))
            .expect("signed event");

        assert!(references_root(&event, &root));
        assert!(!references_root(
            &event,
            &EventId::from_hex(OTHER_ID_HEX).expect("valid id")
        ));
    }

    #[test]
    fn references_root_matches_uppercase_e_tag() {
        let root = root_event_id();
        let event = EventBuilder::new(Kind::Comment, "")
            .tags([Tag::parse(["E", ROOT_ID_HEX]).expect("valid E tag")])
            .finalize(&keys_from_hex(
                "0000000000000000000000000000000000000000000000000000000000000001",
            ))
            .expect("signed event");

        assert!(references_root(&event, &root));
        assert!(!references_root(
            &event,
            &EventId::from_hex(OTHER_ID_HEX).expect("valid id")
        ));
    }

    #[test]
    fn references_root_false_without_e_tags() {
        let event = EventBuilder::new(Kind::GitStatusOpen, "")
            .finalize(&keys_from_hex(
                "0000000000000000000000000000000000000000000000000000000000000001",
            ))
            .expect("signed event");

        assert!(!references_root(&event, &root_event_id()));
    }

    #[test]
    fn defaults_to_open_without_status_events() {
        let owner =
            keys_from_hex("0000000000000000000000000000000000000000000000000000000000000001");
        let maintainer =
            keys_from_hex("0000000000000000000000000000000000000000000000000000000000000002");

        let statuses: Vec<Event> = Vec::new();

        assert_eq!(
            resolve_status(
                statuses.iter(),
                &owner.public_key(),
                &[maintainer.public_key()]
            ),
            RepoStatus::Open
        );
    }

    #[test]
    fn latest_status_wins() {
        let owner =
            keys_from_hex("0000000000000000000000000000000000000000000000000000000000000001");
        let maintainer =
            keys_from_hex("0000000000000000000000000000000000000000000000000000000000000002");
        let root = root_event_id();

        let statuses = [
            status_event(&maintainer, Kind::GitStatusClosed, root, 100),
            status_event(&owner, Kind::GitStatusOpen, root, 200),
        ];

        assert_eq!(
            resolve_status(
                statuses.iter(),
                &owner.public_key(),
                &[maintainer.public_key()]
            ),
            RepoStatus::Open
        );
    }

    #[test]
    fn ignores_statuses_from_others() {
        let owner =
            keys_from_hex("0000000000000000000000000000000000000000000000000000000000000001");
        let maintainer =
            keys_from_hex("0000000000000000000000000000000000000000000000000000000000000002");
        let stranger =
            keys_from_hex("0000000000000000000000000000000000000000000000000000000000000003");
        let root = root_event_id();

        let statuses = [
            status_event(&stranger, Kind::GitStatusClosed, root, 300),
            status_event(&maintainer, Kind::GitStatusDraft, root, 100),
        ];

        assert_eq!(
            resolve_status(
                statuses.iter(),
                &owner.public_key(),
                &[maintainer.public_key()]
            ),
            RepoStatus::Draft
        );
    }

    #[test]
    fn ignores_non_status_kinds() {
        let owner =
            keys_from_hex("0000000000000000000000000000000000000000000000000000000000000001");
        let root = root_event_id();

        let statuses = [status_event(&owner, Kind::GitIssue, root, 100)];

        assert_eq!(
            resolve_status(statuses.iter(), &owner.public_key(), &[]),
            RepoStatus::Open
        );
    }
}
