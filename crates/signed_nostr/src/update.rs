use nostr_sdk::prelude::*;

#[derive(Debug, Clone)]
pub struct Update {
    pub kind: Kind,
    pub coordinate: Option<Coordinate>,
    pub author: PublicKey,
}

impl Update {
    pub fn from_event(event: &Event) -> Self {
        let coordinate = event.tags.coordinates().nth(0);

        Self {
            kind: event.kind,
            coordinate,
            author: event.pubkey,
        }
    }
}
