use nostr::prelude::*;

/// Shorten a [`PublicKey`] to `npub1abc...wxyz` form.
pub fn shorten_pubkey(public_key: PublicKey) -> String {
    let encoded = public_key
        .to_bech32()
        .unwrap_or_else(|_| public_key.to_hex());

    truncate_middle(&encoded)
}

fn truncate_middle(value: &str) -> String {
    const HEAD_CHARS: usize = 9;
    const TAIL_CHARS: usize = 4;

    let length = value.chars().count();
    if length <= HEAD_CHARS + TAIL_CHARS + 3 {
        return value.to_owned();
    }

    let head: String = value.chars().take(HEAD_CHARS).collect();
    let tail: String = value.chars().skip(length - TAIL_CHARS).collect();

    format!("{head}...{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const PUBLIC_KEY_HEX: &str = "68d81165918100b7da43fc28f7d1fc12554466e1115886b9e7bb326f65ec4272";

    #[test]
    fn shortens_a_valid_pubkey() {
        let public_key = PublicKey::from_hex(PUBLIC_KEY_HEX).expect("valid pubkey");
        let npub = public_key.to_bech32().expect("valid pubkey encodes");

        assert_eq!(
            shorten_pubkey(public_key),
            format!("{}...{}", &npub[..9], &npub[npub.len() - 4..])
        );
    }

    #[test]
    fn leaves_short_values_intact() {
        assert_eq!(truncate_middle("npub1short"), "npub1short");
        assert_eq!(truncate_middle("thirteenchars"), "thirteenchars");
    }
}
