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
            .then_with(|| b.root.cmp(&a.root))
    });

    items
}
