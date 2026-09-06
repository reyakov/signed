use nostr_sdk::prelude::*;

/// A lightweight change notification for the UI.
#[derive(Debug, Clone)]
pub struct Update {
    pub kind: Kind,
    /// First `a` tag value of the event, if any, for example the repository coordinate.
    pub coordinate: Option<Coordinate>,
    pub author: PublicKey,
}

impl Update {
    /// Build an update from a received event.
    pub fn from_event(event: &Event) -> Self {
        let coordinate = event.tags.coordinates().nth(0);

        Self {
            kind: event.kind,
            coordinate,
            author: event.pubkey,
        }
    }
}
