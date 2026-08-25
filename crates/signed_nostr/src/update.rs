use nostr_sdk::prelude::*;

/// A lightweight "something changed" signal for the UI.
///
/// Heavy data stays in the database; consumers re-query on receipt.
#[derive(Debug, Clone)]
pub struct Update {
    pub kind: Kind,
    /// First `a` tag value of the event, if any (e.g. the repository coordinate).
    pub coordinate: Option<Coordinate>,
    pub author: PublicKey,
    pub event_id: EventId,
}

impl Update {
    /// Build an update from a received event.
    pub fn from_event(event: &Event) -> Self {
        let coordinate = event.tags.coordinates().nth(0);

        Self {
            kind: event.kind,
            coordinate,
            author: event.pubkey,
            event_id: event.id,
        }
    }
}
