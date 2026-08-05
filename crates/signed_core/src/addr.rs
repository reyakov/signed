use std::fmt;
use std::str::FromStr;

use nostr::prelude::*;

/// Address of a NIP-34 repository announcement: `30617:<owner-pubkey>:<repo-id>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RepoAddr {
    pub owner: PublicKey,
    pub id: String,
}

impl RepoAddr {
    pub fn new(owner: PublicKey, id: impl Into<String>) -> Self {
        Self {
            owner,
            id: id.into(),
        }
    }

    /// The NIP-33 coordinate for the announcement event (`a` tag value).
    pub fn coordinate(&self) -> Coordinate {
        Coordinate::new(Kind::GitRepoAnnouncement, self.owner).identifier(self.id.clone())
    }
}

impl fmt::Display for RepoAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.coordinate())
    }
}

impl FromStr for RepoAddr {
    type Err = nostr::error::Error;

    /// Parse from `<kind>:<pubkey>:<d-tag>`, `naddr1...` bech32 or `nostr:naddr1...` URI.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let coordinate = Coordinate::parse(s)?;
        Ok(Self {
            owner: coordinate.public_key,
            id: coordinate.identifier,
        })
    }
}
