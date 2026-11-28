use nostr::prelude::*;
use signed_core::inbox::{InboxReadState, ThreadResolver, group};

fn keys(seed: u8) -> Keys {
    let mut hex = "00000000000000000000000000000000000000000000000000000000000000".to_string();
    hex.push_str(&format!("{seed:02x}"));
    Keys::new(SecretKey::from_hex(&hex).expect("valid secret key"))
}

fn signed(author: &Keys, kind: Kind, tags: Vec<Tag>, created_at: u64) -> Event {
    EventBuilder::new(kind)
        .tags(tags)
        .custom_created_at(Timestamp::from_secs(created_at))
        .finalize(author)
        .expect("signed event")
}

fn e_tag(event: &Event) -> Tag {
    Tag::parse(["e", &event.id.to_hex()]).expect("valid e tag")
}

fn marked_e_tag(event: &Event, marker: &str) -> Tag {
    Tag::parse(["e", &event.id.to_hex(), "wss://relay.example.com", marker]).expect("valid e tag")
}

fn uppercase_e_tag(event: &Event) -> Tag {
    Tag::parse(["E", &event.id.to_hex()]).expect("valid E tag")
}

fn lookup(events: &[Event]) -> impl Fn(EventId) -> Option<Event> + '_ {
    move |id| events.iter().find(|event| event.id == id).cloned()
}

fn issue(author: &Keys, at: u64) -> Event {
    signed(author, Kind::GitIssue, Vec::new(), at)
}

fn titled_issue(author: &Keys, title: &str, at: u64) -> Event {
    signed(
        author,
        Kind::GitIssue,
        vec![Tag::parse(["subject", title]).expect("valid subject tag")],
        at,
    )
}

#[test]
fn group_merges_own_events_into_the_matching_thread() {
    let me = keys(1);
    let issue = titled_issue(&me, "Add retry logic", 100);
    let mine = signed(
        &me,
        Kind::Comment,
        vec![
            uppercase_e_tag(&issue),
            Tag::parse(["K", "1621"]).expect("K tag"),
        ],
        150,
    );
    let reply = signed(
        &keys(2),
        Kind::Comment,
        vec![
            uppercase_e_tag(&issue),
            Tag::parse(["K", "1621"]).expect("K tag"),
        ],
        200,
    );

    let context = [issue.clone(), mine.clone(), reply.clone()];
    let items = group(
        [reply.clone()],
        [issue.clone(), mine.clone()],
        me.public_key(),
        &InboxReadState::default(),
        &lookup(&context),
    );

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].root, issue.id);
    assert_eq!(
        items[0].root_event.as_ref().map(|event| event.id),
        Some(issue.id)
    );
    assert_eq!(items[0].kind(), Some(Kind::GitIssue));
    assert_eq!(items[0].title(), "Add retry logic");
    assert_eq!(items[0].events, vec![reply.clone()]);
    assert_eq!(items[0].own_events, vec![mine.clone(), issue.clone()]);
    assert_eq!(
        items[0]
            .timeline(5)
            .iter()
            .map(|event| event.id)
            .collect::<Vec<_>>(),
        vec![issue.id, mine.id, reply.id]
    );
}

#[test]
fn comment_resolves_to_its_uppercase_root() {
    let issue = issue(&keys(1), 100);
    let comment = signed(
        &keys(2),
        Kind::Comment,
        vec![
            uppercase_e_tag(&issue),
            Tag::parse(["K", "1621"]).expect("valid K tag"),
        ],
        200,
    );
    let events = [issue.clone(), comment.clone()];
    assert_eq!(
        ThreadResolver::new(&lookup(&events)).notification_root(&comment),
        Some(issue.id)
    );
}

#[test]
fn child_patch_resolves_to_the_root_patch() {
    let root_patch = signed(&keys(1), Kind::GitPatch, Vec::new(), 100);
    let child_patch = signed(&keys(1), Kind::GitPatch, vec![e_tag(&root_patch)], 200);
    let events = [root_patch.clone(), child_patch.clone()];
    assert_eq!(
        ThreadResolver::new(&lookup(&events)).notification_root(&child_patch),
        Some(root_patch.id)
    );
}

#[test]
fn status_resolves_via_the_root_marker() {
    let issue = issue(&keys(1), 100);
    let status = signed(
        &keys(2),
        Kind::GitStatusClosed,
        vec![marked_e_tag(&issue, "root")],
        200,
    );
    let events = [issue.clone(), status.clone()];
    assert_eq!(
        ThreadResolver::new(&lookup(&events)).notification_root(&status),
        Some(issue.id)
    );
}

#[test]
fn nested_comment_chain_follows_to_the_root() {
    let issue = issue(&keys(1), 100);
    let reply = signed(&keys(2), Kind::Comment, vec![uppercase_e_tag(&issue)], 200);
    let nested = signed(&keys(3), Kind::Comment, vec![uppercase_e_tag(&reply)], 300);
    let events = [issue.clone(), reply, nested.clone()];
    assert_eq!(
        ThreadResolver::new(&lookup(&events)).notification_root(&nested),
        Some(issue.id)
    );
}
