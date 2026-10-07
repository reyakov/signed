use std::collections::HashSet;

use nostr::prelude::*;

pub struct Deletions {
    ids: HashSet<(EventId, PublicKey)>,
    coords: Vec<(Coordinate, PublicKey, Timestamp)>,
    vanished: Vec<(PublicKey, Timestamp)>,
}

impl Deletions {
    /// Builds a [`Deletions`] instance from a collection of events.
    pub fn from_events<E>(events: E) -> Self
    where
        E: IntoIterator<Item = Event>,
    {
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

    /// Returns whether the given event is deleted.
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
