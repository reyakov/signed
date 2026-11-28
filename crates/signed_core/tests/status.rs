use nostr::prelude::*;
use signed_core::{GitEvent, RepoStatus};

const ROOT_ID_HEX: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const OTHER_ID_HEX: &str = "2222222222222222222222222222222222222222222222222222222222222222";

fn keys_from_hex(hex: &str) -> Keys {
    Keys::new(SecretKey::from_hex(hex).expect("valid secret key"))
}

fn root_event_id() -> EventId {
    EventId::from_hex(ROOT_ID_HEX).expect("valid event id")
}

fn status_event(author: &Keys, kind: Kind, root: EventId, created_at: u64) -> Event {
    EventBuilder::new(kind)
        .tags([Tag::event(root)])
        .custom_created_at(Timestamp::from(created_at))
        .finalize(author)
        .expect("signed event")
}

#[test]
fn references_root_matches_e_tag() {
    let root = root_event_id();
    let event = EventBuilder::new(Kind::GitStatusOpen)
        .tags([Tag::event(root)])
        .finalize(&keys_from_hex(
            "0000000000000000000000000000000000000000000000000000000000000001",
        ))
        .expect("signed event");

    assert!(event.references_root(&root));
    assert!(!event.references_root(&EventId::from_hex(OTHER_ID_HEX).expect("valid id")));
}

#[test]
fn latest_status_wins() {
    let owner = keys_from_hex("0000000000000000000000000000000000000000000000000000000000000001");
    let maintainer =
        keys_from_hex("0000000000000000000000000000000000000000000000000000000000000002");
    let root = root_event_id();

    let statuses = [
        status_event(&maintainer, Kind::GitStatusClosed, root, 100),
        status_event(&owner, Kind::GitStatusOpen, root, 200),
    ];

    assert_eq!(
        RepoStatus::resolve(
            statuses.iter(),
            &owner.public_key(),
            &[maintainer.public_key()]
        ),
        RepoStatus::Open
    );
}

#[test]
fn ignores_statuses_from_others() {
    let owner = keys_from_hex("0000000000000000000000000000000000000000000000000000000000000001");
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
        RepoStatus::resolve(
            statuses.iter(),
            &owner.public_key(),
            &[maintainer.public_key()]
        ),
        RepoStatus::Draft
    );
}
