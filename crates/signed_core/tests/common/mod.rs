#![allow(dead_code)]

use nostr::prelude::*;

pub fn keys(seed: u8) -> Keys {
    let mut hex = "00000000000000000000000000000000000000000000000000000000000000".to_string();
    hex.push_str(&format!("{seed:02x}"));
    Keys::new(SecretKey::from_hex(&hex).expect("valid secret key"))
}

pub fn signed(author: &Keys, kind: Kind, tags: Vec<Tag>) -> Event {
    EventBuilder::new(kind)
        .tags(tags)
        .finalize(author)
        .expect("signed event")
}

pub fn kind_tag(name: &str, kind: Kind) -> Tag {
    Tag::parse([name, &kind.as_u16().to_string()]).expect("valid kind tag")
}

pub fn uppercase_e_tag(event: &Event) -> Tag {
    Tag::parse(["E", &event.id.to_hex()]).expect("valid E tag")
}
