use std::collections::HashMap;

use nostr::prelude::*;
use utils::sort_newest_first;

use super::{InboxItem, InboxReadState, ThreadResolver};
use crate::RepoAddr;

/// Groups events into inbox threads, merging the user's own events.
pub fn group<E, O, L>(
    events: E,
    own: O,
    me: PublicKey,
    state: &InboxReadState,
    lookup: &L,
) -> Vec<InboxItem>
where
    E: IntoIterator<Item = Event>,
    O: IntoIterator<Item = Event>,
    L: Fn(EventId) -> Option<Event>,
{
    let resolver = ThreadResolver::new(lookup);
    let mut groups: HashMap<EventId, Vec<Event>> = HashMap::new();
    for event in events {
        if event.pubkey == me {
            continue;
        }
        let Some(root) = resolver.notification_root(&event) else {
            continue;
        };
        groups.entry(root).or_default().push(event);
    }

    let mut own_groups: HashMap<EventId, Vec<Event>> = HashMap::new();
    for event in own {
        let root = resolver.notification_root(&event).unwrap_or(event.id);
        own_groups.entry(root).or_default().push(event);
    }

    let mut roots: Vec<EventId> = groups.keys().chain(own_groups.keys()).copied().collect();
    roots.sort();
    roots.dedup();

    let mut items: Vec<InboxItem> = roots
        .into_iter()
        .map(|root| {
            let mut events = groups.remove(&root).unwrap_or_default();
            let mut own_events = own_groups.remove(&root).unwrap_or_default();
            sort_newest_first(&mut events);
            sort_newest_first(&mut own_events);

            let root_event = lookup(root);

            let mut item = InboxItem {
                root,
                address: root_event
                    .as_ref()
                    .and_then(|event| event.tags.coordinates().next())
                    .map(RepoAddr::from),
                root_event,
                events,
                own_events,
                unread_ids: Vec::new(),
                archived: false,
            };
            item.apply_state(state);
            item
        })
        .collect();

    items.sort_by(|a, b| {
        b.latest_activity()
            .cmp(&a.latest_activity())
            .then_with(|| b.root.to_hex().cmp(&a.root.to_hex()))
    });

    items
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Derives deterministic signer keys from a seed byte.
    fn keys(seed: u8) -> Keys {
        let mut hex = "00000000000000000000000000000000000000000000000000000000000000".to_string();
        hex.push_str(&format!("{seed:02x}"));
        Keys::new(SecretKey::from_hex(&hex).expect("valid secret key"))
    }

    /// Builds a signed event with a fixed timestamp.
    fn signed(author: &Keys, kind: Kind, tags: Vec<Tag>, created_at: u64) -> Event {
        EventBuilder::new(kind, "")
            .tags(tags)
            .custom_created_at(Timestamp::from_secs(created_at))
            .finalize(author)
            .expect("signed event")
    }

    /// Builds an uppercase `E` tag for the event.
    fn uppercase_e_tag(event: &Event) -> Tag {
        Tag::parse(["E", &event.id.to_hex()]).expect("valid E tag")
    }

    /// Builds an event lookup closure over a slice.
    fn lookup(events: &[Event]) -> impl Fn(EventId) -> Option<Event> + '_ {
        move |id| events.iter().find(|event| event.id == id).cloned()
    }

    /// Builds a git issue event with a subject tag.
    fn titled_issue(author: &Keys, title: &str, at: u64) -> Event {
        signed(
            author,
            Kind::GitIssue,
            vec![Tag::parse(["subject", title]).expect("valid subject tag")],
            at,
        )
    }

    /// Merges the user's own events into the matching thread.
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
}
