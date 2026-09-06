use std::collections::HashSet;

use nostr::prelude::*;

/// NIP-09 deletion requests and NIP-62 vanish requests,
/// built from the kind-5 and kind-62 events in the local database.
///
/// Deleted events are hidden before they reach the UI.
///
/// Pass any event through [`Deletions::is_deleted`] before showing it.
pub struct Deletions {
    /// `(deleted event id, expected author)` from `e` tags of kind-5 events.
    ids: HashSet<(EventId, PublicKey)>,
    /// `(coordinate, expected author, cutoff)` from `a` tags of kind-5 events.
    ///
    /// All versions of the addressable event up to `cutoff` are deleted.
    coords: Vec<(Coordinate, PublicKey, Timestamp)>,
    /// `(author, cutoff)` from kind-62 vanish requests.
    vanished: Vec<(PublicKey, Timestamp)>,
}

impl Deletions {
    /// Build the deletion index from raw kind-5 and kind-62 events.
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
                // Client-side we can't verify which relay the request targeted.
                // Any vanish request is then honored for the author's events.
                vanished.push((event.pubkey, event.created_at));
            }
        }

        Self {
            ids,
            coords,
            vanished,
        }
    }

    /// Whether the event is covered by a valid deletion or vanish request.
    /// A request is valid when its author matches the deleted event's author, per NIP-09.
    ///
    /// Addressable events are deleted up to the request's `created_at`.
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
