use nostr::prelude::*;

use crate::RepoAddr;

/// Target of a `nostr://` clone URL, as defined by NIP-34.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloneTarget {
    /// `nostr://<naddr1...>` encodes a direct repository address.
    Addr(RepoAddr),
    /// `nostr://<npub|nip05>/[relay-hint/]<identifier>`
    UserRepo {
        /// `npub1...` or a NIP-05 identifier.
        user: String,
        relay_hint: Option<RelayUrl>,
        /// `d` tag identifier of the repository.
        identifier: String,
    },
}

/// Parse a `nostr://` clone URL. Returns `None` for other URL schemes.
pub fn parse_clone_url(url: &str) -> Option<CloneTarget> {
    let rest = url.strip_prefix("nostr://")?;
    let mut parts = rest.split('/');

    let first = parts.next()?;
    let second = parts.next()?;
    let third = parts.next();

    if first.starts_with("naddr1") {
        let coordinate = Nip19Coordinate::from_bech32(first).ok()?;
        return Some(CloneTarget::Addr(coordinate.coordinate));
    }

    let (relay_hint, identifier) = match third {
        Some(id) => (
            RelayUrl::parse(&percent_decode(second)).ok(),
            percent_decode(id),
        ),
        None => (None, percent_decode(second)),
    };

    Some(CloneTarget::UserRepo {
        user: first.to_owned(),
        relay_hint,
        identifier,
    })
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = &input[i + 1..i + 3];
            if let Ok(v) = u8::from_str_radix(hex, 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_user_repo_without_relay() {
        let target = parse_clone_url(
            "nostr://npub15qydau2hjma6ngxkl2cyar74wzyjshvl65za5k5rl69264ar2exs5cyejr/ngit",
        )
        .unwrap();
        assert_eq!(
            target,
            CloneTarget::UserRepo {
                user: "npub15qydau2hjma6ngxkl2cyar74wzyjshvl65za5k5rl69264ar2exs5cyejr".to_owned(),
                relay_hint: None,
                identifier: "ngit".to_owned(),
            }
        );
    }

    #[test]
    fn parses_user_repo_with_relay_hint() {
        let target = parse_clone_url("nostr://danconwaydev.com/relay.ngit.dev/ngit").unwrap();
        assert_eq!(
            target,
            CloneTarget::UserRepo {
                user: "danconwaydev.com".to_owned(),
                relay_hint: RelayUrl::parse("relay.ngit.dev").ok(),
                identifier: "ngit".to_owned(),
            }
        );
    }

    #[test]
    fn decodes_percent_encoded_parts() {
        let target = parse_clone_url(
            "nostr://danconwaydev.com/ws%3A%2F%2Flocalhost%3A7334/my-local-only-repo",
        )
        .unwrap();
        assert_eq!(
            target,
            CloneTarget::UserRepo {
                user: "danconwaydev.com".to_owned(),
                relay_hint: RelayUrl::parse("ws://localhost:7334").ok(),
                identifier: "my-local-only-repo".to_owned(),
            }
        );
    }
}
