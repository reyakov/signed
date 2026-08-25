use nostr::prelude::*;

/// Shorten a [`PublicKey`] to `npub1abc...wxyz` form.
pub fn shorten_pubkey(public_key: PublicKey, len: usize) -> String {
    let npub = public_key.to_bech32().unwrap();
    format!("{}...{}", &npub[..(len + 5)], &npub[npub.len() - len..])
}
