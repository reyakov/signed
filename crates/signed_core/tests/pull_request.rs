use nostr::prelude::*;
use signed_core::PullRequest;

fn keys() -> Keys {
    Keys::new(
        SecretKey::from_hex("0000000000000000000000000000000000000000000000000000000000000001")
            .expect("valid secret key"),
    )
}

fn pr_event(content: &str, tags: Vec<Tag>) -> Event {
    EventBuilder::new(Kind::GitPullRequest)
        .content(content)
        .tags(tags)
        .finalize(&keys())
        .expect("signed event")
}

fn patch_event(content: &str, tags: Vec<Tag>, created_at: u64) -> Event {
    EventBuilder::new(Kind::GitPatch)
        .content(content)
        .tags(tags)
        .custom_created_at(Timestamp::from(created_at))
        .finalize(&keys())
        .expect("signed event")
}

#[test]
fn pull_request_patch_joins_the_whole_patch_set() {
    let root = patch_event("patch-one", vec![], 100);
    let second = patch_event("patch-two", vec![Tag::event(root.id)], 200);
    let pr = pr_event("description", vec![Tag::event(root.id)]);

    assert_eq!(
        PullRequest::new(&pr).patch([&root, &second]),
        "patch-one\npatch-two"
    );
    assert_eq!(
        PullRequest::new(&pr).patches([&root, &second]),
        vec![&root, &second]
    );
}

#[test]
fn pull_request_patches_walks_the_reply_chain_in_order() {
    let root = patch_event("patch-one", vec![], 100);
    let second = patch_event("patch-two", vec![Tag::event(root.id)], 200);
    let third = patch_event("patch-three", vec![Tag::event(second.id)], 300);
    let pr = pr_event("description", vec![Tag::event(root.id)]);

    let series = PullRequest::new(&pr).patches([&third, &root, &second]);
    assert_eq!(
        series
            .iter()
            .map(|p| p.content.as_str())
            .collect::<Vec<_>>(),
        vec!["patch-one", "patch-two", "patch-three"]
    );
}

#[test]
fn pull_request_patches_finds_the_set_via_the_tip_commit() {
    let root = patch_event("patch-one", vec![], 100);
    let tip = "1111111111111111111111111111111111111111";
    let last = patch_event(
        "patch-two",
        vec![
            Tag::event(root.id),
            Tag::parse(["r", tip]).expect("valid tag"),
        ],
        200,
    );
    let pr = pr_event(
        "description",
        vec![Tag::parse(["c", tip]).expect("valid tag")],
    );

    let series = PullRequest::new(&pr).patches([&root, &last]);
    assert_eq!(
        series
            .iter()
            .map(|p| p.content.as_str())
            .collect::<Vec<_>>(),
        vec!["patch-one", "patch-two"]
    );
}

const COMMIT_HEX: &str = "1111111111111111111111111111111111111111";
const OTHER_ROOT_HEX: &str = "2222222222222222222222222222222222222222";

fn signed_at(kind: Kind, tags: Vec<Tag>, created_at: u64) -> Event {
    EventBuilder::new(kind)
        .tags(tags)
        .custom_created_at(Timestamp::from(created_at))
        .finalize(&keys())
        .expect("signed event")
}

fn pr_root() -> Event {
    signed_at(
        Kind::GitPullRequest,
        vec![
            Tag::parse(["c", COMMIT_HEX]).expect("valid tag"),
            Tag::parse(["branch-name", "feature/x"]).expect("valid tag"),
        ],
        100,
    )
}

#[test]
fn latest_update_picks_newest_revision_of_the_root() {
    let root = pr_root();
    let root_hex = root.id.to_hex();

    let revision = |created_at: u64| {
        signed_at(
            Kind::GitPullRequestUpdate,
            vec![Tag::parse(["E", &root_hex]).expect("valid tag")],
            created_at,
        )
    };
    let unrelated = signed_at(
        Kind::GitPullRequestUpdate,
        vec![Tag::parse(["E", OTHER_ROOT_HEX]).expect("valid tag")],
        999,
    );

    let events = [unrelated, revision(200), root.clone(), revision(300)];
    let latest = PullRequest::latest_update(events.iter(), &root).expect("an update");

    assert_eq!(latest.created_at.as_secs(), 300);
    assert_eq!(latest.kind, Kind::GitPullRequestUpdate);
}

#[test]
fn latest_update_ignores_other_authors() {
    let root = pr_root();
    let root_hex = root.id.to_hex();
    let other = Keys::new(
        SecretKey::from_hex("0000000000000000000000000000000000000000000000000000000000000002")
            .expect("valid secret key"),
    );
    let stranger = EventBuilder::new(Kind::GitPullRequestUpdate)
        .tags([Tag::parse(["E", &root_hex]).expect("valid tag")])
        .custom_created_at(Timestamp::from(999))
        .finalize(&other)
        .expect("signed event");

    assert!(PullRequest::latest_update([&stranger, &root].into_iter(), &root).is_none());
}
