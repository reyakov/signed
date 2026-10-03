use std::collections::HashSet;

use nostr::prelude::*;

pub struct Deletions {
    ids: HashSet<(EventId, PublicKey)>,
    // All versions of the addressable event up to `cutoff` are deleted.
    coords: Vec<(Coordinate, PublicKey, Timestamp)>,
    vanished: Vec<(PublicKey, Timestamp)>,
}

impl Deletions {
    pub fn from_events(events: impl IntoIterator<Item = Event>) -> Self {
        let mut ids = HashSet::new();
        let mut coords = Vec::new();
        let mut vanished = Vec::new();

        for event in events {
            if event.kind == Kind::EventDeletion {
                ids.extend(event.tags.event_ids().map(|id| (id, event.pubkey)));
                coords.extend(
                    event
                        .tags
                        .coordinates()
                        .map(|c| (c, event.pubkey, event.created_at)),
                );
            } else if event.kind == Kind::RequestToVanish {
                // We can't verify which relay the request targeted, so any
                // vanish request is honored for the author's events.
                vanished.push((event.pubkey, event.created_at));
            }
        }

        Self {
            ids,
            coords,
            vanished,
        }
    }

    // A request is valid when its author matches the deleted event's author,
    // per NIP-09. Addressable events are deleted up to the request's `created_at`.
    pub fn is_deleted(&self, event: &Event) -> bool {
        if self
            .vanished
            .iter()
            .any(|(pk, cutoff)| *pk == event.pubkey && event.created_at <= *cutoff)
        {
            return true;
        }

        if self.ids.contains(&(event.id, event.pubkey)) {
            return true;
        }

        if event.kind.is_addressable()
            && let Some(identifier) = event.tags.identifier()
        {
            let coordinate = Coordinate::new(event.kind, event.pubkey).identifier(identifier);
            return self.coords.iter().any(|(c, pk, cutoff)| {
                *c == coordinate && *pk == event.pubkey && event.created_at <= *cutoff
            });
        }

        false
    }
}
