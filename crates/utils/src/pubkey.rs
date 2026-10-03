use nostr::prelude::*;

use crate::text::middle_truncate;

const HEAD_CHARS: usize = 9;
const TAIL_CHARS: usize = 4;

pub fn shorten_pubkey(public_key: PublicKey) -> String {
    let encoded = public_key
        .to_bech32()
        .unwrap_or_else(|_| public_key.to_hex());

    middle_truncate(&encoded, HEAD_CHARS, TAIL_CHARS)
}
